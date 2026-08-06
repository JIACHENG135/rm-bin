//! The pending queue: what has been dropped in but not yet sent.
//!
//! Before this existed, a drop *was* a send — the bin waited out a quiet
//! period and then committed. That made the cost invisible: every drop cost
//! the tablet a 15–20 second blackout, and a five-image afternoon cost five of
//! them. So the drop and the send came apart. Images land here and wait; the
//! user decides when the tablet gets interrupted, once, for all of them.
//!
//! Two consequences shape everything below:
//!
//! * **The queue is a promise.** Anything sitting in it is guaranteed to be
//!   sendable, so a file is decoded at *drop* time, not at send time. A file
//!   that cannot be decoded never enters — there is no "bad entry" state for
//!   the user to discover later.
//! * **It has to survive quitting.** What the user put in is an intention, and
//!   losing someone's intention is worse than keeping a copy of it. So the
//!   queue is on disk, and so are its thumbnails — because "截屏 xxx.png"
//!   is routinely deleted before the next launch, and a queue entry that can
//!   no longer be shown or sent is just a lie.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::settings;

/// Extensions allowed in without opening the file. Anything else is turned
/// away on `dragenter`, before the user has even let go.
const IMAGE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff",
];

/// Long edge of the cached thumbnail. Big enough for the 68x88 cards on a
/// retina display, small enough that a hundred of them cost less than one of
/// the PDFs they stand for.
const THUMB_EDGE: u32 = 240;
const THUMB_QUALITY: u8 = 70;

/// How long a queue has to sit before the stack visibly settles — the one
/// and only nudge, and it never repeats.
const SETTLE_SECS: u64 = 72 * 60 * 60;

/// The queue changed. `{ items, skipped, rejected }`.
const QUEUE_EVENT: &str = "queue-changed";
/// How far into a send we are. `{ stage, frac }` where stage is
/// `render` (this machine is busy) or `upload` (the tablet is busy).
const PROGRESS_EVENT: &str = "batch-progress";
/// The outcome, once per send.
/// `{ ok, route, placed: [{ name, folder }], skipped, error }`.
const RESULT_EVENT: &str = "batch-result";

/// One image waiting to go.
///
/// `thumb` is a path rather than a data URL: the window shows up to a dozen of
/// these and re-renders on every pointer move during a tear gesture, so they
/// go through the asset protocol where the webview can cache them.
/// `serde(default)` is load-bearing, not decoration: a queue written by an
/// older build is missing whatever fields have been added since, and a strict
/// decode would fail the whole file and silently hand back an empty queue —
/// which is exactly the "losing someone's intention" this module exists to
/// prevent. Missing fields degrade to zero; nothing here is unusable without
/// them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PendingItem {
    pub id: String,
    /// Where the original lives. May be gone by the time we send — see
    /// `thumb`, which is why that is not fatal for *showing* the queue.
    pub path: String,
    /// The original file's own name, shown when the pointer settles on a card.
    pub name: String,
    /// Cached thumbnail on disk.
    pub thumb: String,
    /// Average colour, lightened — it fills the card's top bar, so a card is
    /// recognisably *that* picture even before the thumbnail decodes.
    pub tint: String,
    /// Size and mtime of the original when it was admitted — enough to tell
    /// "the file the user queued" from "a different file that has since taken
    /// its name", which is the only thing worth checking before sending.
    pub mtime: u64,
    pub size: u64,
    /// Unix seconds when it was dropped in. Drives "最早 12 分钟前" and the
    /// 72-hour settle.
    pub added_at: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct QueueState {
    pub items: Vec<PendingItem>,
    /// Entries dropped on load because neither the original nor its thumbnail
    /// survived. Reported once, then forgotten.
    pub skipped: usize,
    /// How many files in the last drop were turned away.
    pub rejected: usize,
}

#[derive(Default)]
struct Queue {
    items: Vec<PendingItem>,
    /// True from the moment a send starts until it finishes. A second send
    /// cannot start on top of the first, and drops during one are held.
    sending: bool,
}

fn queue() -> &'static Mutex<Queue> {
    static Q: OnceLock<Mutex<Queue>> = OnceLock::new();
    Q.get_or_init(Mutex::default)
}

/* ————— on-disk home ————— */

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no config dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    Ok(dir)
}

fn pending_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("pending.json"))
}

fn thumbs_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?.join("thumbs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    Ok(dir)
}

/// write-then-rename, so a crash mid-write cannot leave a half-queue behind.
fn persist(app: &AppHandle, items: &[PendingItem]) {
    let Ok(path) = pending_path(app) else { return };
    let Ok(body) = serde_json::to_vec_pretty(items) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(&body)?;
        f.sync_all()
    });
    if written.is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Stable across restarts and across re-dropping the same file, which is what
/// makes "you already put this in" free: the id collides, so the duplicate is
/// simply not added.
fn item_id(path: &str, mtime: u64, size: u64) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    mtime.hash(&mut h);
    size.hash(&mut h);
    format!("q{:016x}", h.finish())
}

fn file_stamp(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (mtime, meta.len())
}

pub fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/* ————— admission: decode once, here, or not at all ————— */

/// Decode `path`, write a thumbnail beside the queue, and report the average
/// colour. Failure here is the *only* place a bad image is discovered, which
/// is the point: it happens while the user's hand is still on the file.
fn make_thumb(dir: &Path, id: &str, path: &str) -> Result<(String, String), String> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{GenericImageView, ImageEncoder};

    let img = image::open(path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("这张图是空的".into());
    }

    let long = w.max(h);
    let small = if long > THUMB_EDGE {
        let s = THUMB_EDGE as f64 / long as f64;
        img.resize(
            ((w as f64 * s).round() as u32).max(1),
            ((h as f64 * s).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };

    let rgb = small.to_rgb8();
    let tint = average_tint(&rgb);

    let out = dir.join(format!("{id}.jpg"));
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, THUMB_QUALITY)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("缩略图生成失败：{e}"))?;
    std::fs::write(&out, &bytes).map_err(|e| format!("缩略图写入失败：{e}"))?;

    Ok((out.to_string_lossy().into_owned(), tint))
}

/// Mean colour, then mixed most of the way to white.
///
/// The raw average of a photograph is mud; what the 40px chip needs is a hue
/// the eye can match against the picture it remembers, at a lightness that
/// still reads as paper. 55% white gets both.
fn average_tint(rgb: &image::RgbImage) -> String {
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    let px = ((rgb.width() as u64) * (rgb.height() as u64)).max(1);
    for p in rgb.pixels() {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
    }
    let mix = |c: u64| -> u8 {
        let avg = (c / px) as f64;
        (avg * 0.45 + 255.0 * 0.55).round().clamp(0.0, 255.0) as u8
    };
    format!("#{:02x}{:02x}{:02x}", mix(r), mix(g), mix(b))
}

/* ————— commands ————— */

fn snapshot(q: &Queue, skipped: usize, rejected: usize) -> QueueState {
    QueueState {
        items: q.items.clone(),
        skipped,
        rejected,
    }
}

fn emit(app: &AppHandle, state: &QueueState) {
    let _ = app.emit(QUEUE_EVENT, state);
}

/// Restore the queue from disk.
///
/// Entries whose original *and* thumbnail are both gone are dropped silently
/// — they can be neither shown nor sent, so there is nothing honest left to
/// do with them. The count comes back once so the panel can say so, and is
/// never mentioned again.
#[tauri::command]
pub fn load_pending(app: AppHandle) -> QueueState {
    let stored: Vec<PendingItem> = pending_path(&app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let before = stored.len();
    let thumbs = thumbs_dir(&app).ok();
    let items: Vec<PendingItem> = stored
        .into_iter()
        .map(|mut it| {
            // older records did not carry the thumbnail path, but it has
            // always been derivable from the id
            if it.thumb.is_empty() {
                if let Some(dir) = &thumbs {
                    it.thumb = dir.join(format!("{}.jpg", it.id)).to_string_lossy().into_owned();
                }
            }
            it
        })
        .filter(|it| Path::new(&it.path).exists() || Path::new(&it.thumb).exists())
        .collect();
    let skipped = before - items.len();

    // Thumbnails whose entry is gone are dead weight; a send or a clear
    // normally takes them, but a crash in between would leave them behind.
    if let Ok(dir) = thumbs_dir(&app) {
        prune_thumbs(&dir, &items);
    }

    let state = {
        let Ok(mut q) = queue().lock() else {
            return QueueState::default();
        };
        q.items = items;
        if skipped > 0 {
            persist(&app, &q.items);
        }
        snapshot(&q, skipped, 0)
    };
    emit(&app, &state);
    state
}

fn prune_thumbs(dir: &Path, items: &[PendingItem]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !items.iter().any(|it| Path::new(&it.thumb) == p) {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Take a drop. Everything that can be shown and sent goes in; everything
/// else is counted and refused, and the window shakes its head once.
#[tauri::command]
pub async fn enqueue_images(app: AppHandle, paths: Vec<String>) -> QueueState {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || enqueue_blocking(handle, paths))
        .await
        .unwrap_or_default()
}

fn enqueue_blocking(app: AppHandle, paths: Vec<String>) -> QueueState {
    let Ok(dir) = thumbs_dir(&app) else {
        return QueueState::default();
    };

    let existing: Vec<String> = queue()
        .lock()
        .map(|q| q.items.iter().map(|i| i.id.clone()).collect())
        .unwrap_or_default();

    let mut added = Vec::new();
    let mut rejected = 0usize;
    let mut seen = existing;

    for path in paths {
        if !is_image_path(&path) {
            rejected += 1;
            continue;
        }
        let (mtime, size) = file_stamp(Path::new(&path));
        let id = item_id(&path, mtime, size);
        if seen.contains(&id) {
            // Same file, same bytes, already waiting. Silently the same item.
            continue;
        }
        match make_thumb(&dir, &id, &path) {
            Ok((thumb, tint)) => {
                let name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                seen.push(id.clone());
                added.push(PendingItem {
                    id,
                    path,
                    name,
                    thumb,
                    tint,
                    mtime,
                    size,
                    added_at: now_secs(),
                });
            }
            Err(e) => {
                eprintln!("[rm-bin] 不收 {path}：{e}");
                rejected += 1;
            }
        }
    }

    let state = {
        let Ok(mut q) = queue().lock() else {
            return QueueState::default();
        };
        q.items.extend(added);
        persist(&app, &q.items);
        snapshot(&q, 0, rejected)
    };
    emit(&app, &state);
    state
}

/// Tear one sheet out. The thumbnail stays on disk for now — undo is four
/// seconds away, and re-generating it would mean re-reading a file that may
/// not be there any more.
#[tauri::command]
pub fn remove_pending(app: AppHandle, id: String) -> QueueState {
    let state = {
        let Ok(mut q) = queue().lock() else {
            return QueueState::default();
        };
        q.items.retain(|it| it.id != id);
        persist(&app, &q.items);
        snapshot(&q, 0, 0)
    };
    emit(&app, &state);
    state
}

/// Put a torn sheet back where it was.
#[tauri::command]
pub fn restore_pending(app: AppHandle, item: PendingItem, index: usize) -> QueueState {
    let state = {
        let Ok(mut q) = queue().lock() else {
            return QueueState::default();
        };
        if !q.items.iter().any(|it| it.id == item.id) {
            let at = index.min(q.items.len());
            q.items.insert(at, item);
            persist(&app, &q.items);
        }
        snapshot(&q, 0, 0)
    };
    emit(&app, &state);
    state
}

#[tauri::command]
pub fn clear_pending(app: AppHandle) -> QueueState {
    let state = {
        let Ok(mut q) = queue().lock() else {
            return QueueState::default();
        };
        q.items.clear();
        persist(&app, &q.items);
        snapshot(&q, 0, 0)
    };
    if let Ok(dir) = thumbs_dir(&app) {
        prune_thumbs(&dir, &[]);
    }
    emit(&app, &state);
    state
}

/// Is the tablet there?
///
/// A TCP handshake against the ssh port, with a short timeout: enough to tell
/// "reachable" from "nothing there", cheap enough to run on a 5-second poll
/// while the panel is open.
#[tauri::command]
pub async fn device_online(app: AppHandle) -> bool {
    let cfg = settings::load_settings(app);
    tauri::async_runtime::spawn_blocking(move || {
        use std::net::{TcpStream, ToSocketAddrs};
        format!("{}:{}", cfg.host.trim(), cfg.port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next())
            .map(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(1200)).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// Send everything, in one ssh session, with one xochitl restart.
///
/// Two phases are reported separately because "who is busy" matters more to
/// the person watching than "how far along": during `render` this machine is
/// working and the tablet is untouched; during `deliver` the tablet is going
/// dark and there is nothing here to look at.
#[tauri::command]
pub async fn flush_queue(app: AppHandle) -> Result<(), String> {
    {
        let mut q = queue().lock().map_err(|_| "队列状态已损坏".to_string())?;
        if q.sending {
            return Err("正在发送".into());
        }
        if q.items.is_empty() {
            return Err("队列是空的".into());
        }
        q.sending = true;
    }

    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || send_blocking(handle));
    Ok(())
}

fn send_blocking(app: AppHandle) {
    let batch: Vec<PendingItem> = queue()
        .lock()
        .map(|q| q.items.clone())
        .unwrap_or_default();
    let cfg = settings::load_settings(app.clone());
    let total = batch.len().max(1);

    // A: build the documents. Anything that fails here failed *since* it was
    // admitted — the original was deleted or truncated under us — so it is a
    // skip, not a refusal.
    let mut items = Vec::with_capacity(batch.len());
    let mut skipped = 0usize;
    for (i, it) in batch.iter().enumerate() {
        let _ = app.emit(
            PROGRESS_EVENT,
            serde_json::json!({ "stage": "render", "frac": (i as f64 / total as f64) * 0.6 }),
        );
        match crate::rm::pdf::build(&it.path) {
            Ok(pdf) => items.push(crate::rm::pdf::Item {
                image_path: it.path.clone(),
                fallback_name: crate::rm::upload::name_from_path(&it.path),
                pdf,
            }),
            Err(e) => {
                eprintln!("[rm-bin] 跳过 {}：{e}", it.path);
                skipped += 1;
            }
        }
    }

    if items.is_empty() {
        finish(&app, false);
        let _ = app.emit(
            RESULT_EVENT,
            serde_json::json!({
                "ok": false, "route": null, "placed": [], "skipped": skipped,
                "error": "这批里没有能用的图片"
            }),
        );
        return;
    }

    // B: the tablet's own time. No sub-progress — there is nothing here that
    // could honestly report on what is happening at the other end.
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({ "stage": "upload", "frac": 0.6 }),
    );

    let result = crate::rm::pdf::deliver(&cfg.host, cfg.port, &items, &cfg.gemini_api_key);

    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({ "stage": "done", "frac": 1.0 }),
    );

    match result {
        Ok(delivery) => {
            let route = match delivery.route {
                crate::rm::pdf::Route::WebInterface => "web",
                crate::rm::pdf::Route::Ssh => "ssh",
            };
            eprintln!(
                "[rm-bin] {} 份文档送达 {} via {route}",
                delivery.placed.len(),
                cfg.host
            );
            let placed: Vec<_> = delivery
                .placed
                .iter()
                .map(|p| serde_json::json!({ "name": p.name, "folder": p.folder }))
                .collect();
            // Only a delivered batch empties the queue. A failure leaves it
            // exactly as it was, so the retry is one long-press away.
            finish(&app, true);
            let _ = app.emit(
                RESULT_EVENT,
                serde_json::json!({
                    "ok": true, "route": route, "placed": placed,
                    "skipped": skipped, "error": null
                }),
            );
        }
        Err(e) => {
            eprintln!("[rm-bin] send failed: {e}");
            finish(&app, false);
            let _ = app.emit(
                RESULT_EVENT,
                serde_json::json!({
                    "ok": false, "route": null, "placed": [],
                    "skipped": skipped, "error": e
                }),
            );
        }
    }
}

/// Release the send lock, and — only on success — drop what went out.
///
/// Anything dropped in *while* the send was running is still in `items` and
/// must survive: it was never part of this batch.
fn finish(app: &AppHandle, delivered: bool) {
    let state = {
        let Ok(mut q) = queue().lock() else { return };
        q.sending = false;
        if delivered {
            q.items.clear();
            persist(app, &q.items);
        }
        snapshot(&q, 0, 0)
    };
    if delivered {
        if let Ok(dir) = thumbs_dir(app) {
            prune_thumbs(&dir, &[]);
        }
    }
    emit(app, &state);
}

/// Seconds since the oldest entry was dropped in, or `None` for an empty
/// queue. The window uses it for "最早 3 天前" and for the settle.
pub fn oldest_age_secs() -> Option<u64> {
    let q = queue().lock().ok()?;
    let oldest = q.items.iter().map(|i| i.added_at).min()?;
    Some(now_secs().saturating_sub(oldest))
}

pub fn is_settled() -> bool {
    oldest_age_secs().map(|s| s > SETTLE_SECS).unwrap_or(false)
}

pub fn len() -> usize {
    queue().lock().map(|q| q.items.len()).unwrap_or(0)
}

/// "12 分钟前" / "3 天前". Coarse on purpose: the only question it answers is
/// "has this been sitting here longer than I thought", and minutes-since is
/// as much precision as that question has.
pub fn age_label() -> Option<String> {
    let s = oldest_age_secs()?;
    Some(if s < 60 {
        "刚刚".into()
    } else if s < 3600 {
        format!("{} 分钟前", s / 60)
    } else if s < 86_400 {
        format!("{} 小时前", s / 3600)
    } else {
        format!("{} 天前", s / 86_400)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_image_extensions_get_in() {
        assert!(is_image_path("/a/b.PNG"));
        assert!(is_image_path("/a/b.jpeg"));
        assert!(!is_image_path("/a/b.zip"));
        assert!(!is_image_path("/a/b"));
        // svg has no decoder in the `image` build, so it must not be
        // whitelisted — admitting it would put an entry in the queue that
        // cannot be turned into a PDF.
        assert!(!is_image_path("/a/b.svg"));
    }

    #[test]
    fn id_is_stable_for_the_same_file_and_differs_otherwise() {
        let a = item_id("/tmp/x.png", 100, 20);
        assert_eq!(a, item_id("/tmp/x.png", 100, 20));
        assert_ne!(a, item_id("/tmp/x.png", 101, 20));
        assert_ne!(a, item_id("/tmp/y.png", 100, 20));
    }

    #[test]
    fn tint_is_a_light_version_of_the_mean() {
        let mut img = image::RgbImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgb([0, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([100, 200, 60]));
        // mean is (50,100,30) -> 0.45*mean + 0.55*255
        assert_eq!(average_tint(&img), "#a3b99a");
    }
}
