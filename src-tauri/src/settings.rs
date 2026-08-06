//! Persisted user settings + the settings window.
//!
//! Stored as JSON in the app config dir, e.g.
//! ~/Library/Application Support/com.zoe.rmbin/settings.json

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// reMarkable over USB always answers here.
const DEFAULT_HOST: &str = "10.11.99.1";
const DEFAULT_PORT: u16 = 22;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// IP address or hostname of the reMarkable.
    pub host: String,
    /// SSH port — 22 unless the user tunnels.
    pub port: u16,
    /// Google AI Studio key. Empty means the feature is off: the document
    /// keeps the dropped file's own name and lands unfiled, exactly as
    /// before this existed.
    ///
    /// Only the ssh fallback path can use it — the web interface has no
    /// folder concept to place a document into, so it keeps sending the
    /// plain filename regardless of this setting.
    pub gemini_api_key: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.into(),
            port: DEFAULT_PORT,
            gemini_api_key: String::new(),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("no config dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    Ok(dir.join("settings.json"))
}

fn read_settings(path: &std::path::Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_settings(path: &std::path::Path, settings: &Settings) -> Result<Settings, String> {
    let host = settings.host.trim().to_string();
    if host.is_empty() {
        return Err("地址不能为空".into());
    }
    let clean = Settings {
        host,
        port: if settings.port == 0 {
            DEFAULT_PORT
        } else {
            settings.port
        },
        gemini_api_key: settings.gemini_api_key.trim().to_string(),
    };

    let body = serde_json::to_vec_pretty(&clean).map_err(|e| e.to_string())?;
    // write-then-rename so a crash mid-write can't leave a truncated file
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(&body).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;

    Ok(clean)
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Settings {
    settings_path(&app)
        .map(|p| read_settings(&p))
        .unwrap_or_default()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, String> {
    write_settings(&settings_path(&app)?, &settings)
}

#[derive(Debug, Serialize)]
pub struct Probe {
    pub ok: bool,
    /// Round-trip of the TCP handshake, in milliseconds.
    pub latency_ms: u128,
    /// Short, user-facing reason when `ok` is false.
    pub detail: String,
}

fn probe_host(host: &str, port: u16) -> Probe {
    let target = format!(
        "{}:{}",
        host.trim(),
        if port == 0 { DEFAULT_PORT } else { port }
    );
    let started = Instant::now();

    let addr = match target.to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => a,
            None => {
                return Probe {
                    ok: false,
                    latency_ms: 0,
                    detail: "无法解析该地址".into(),
                }
            }
        },
        Err(_) => {
            return Probe {
                ok: false,
                latency_ms: 0,
                detail: "地址格式无效".into(),
            }
        }
    };

    match TcpStream::connect_timeout(&addr, Duration::from_millis(2500)) {
        Ok(_) => Probe {
            ok: true,
            latency_ms: started.elapsed().as_millis(),
            detail: String::new(),
        },
        Err(e) => Probe {
            ok: false,
            latency_ms: 0,
            detail: match e.kind() {
                std::io::ErrorKind::TimedOut => "连接超时，设备可能未开机或不在同一网络".into(),
                std::io::ErrorKind::ConnectionRefused => {
                    "设备拒绝连接，请检查是否已开启 SSH".into()
                }
                _ => "无法连接到设备".into(),
            },
        },
    }
}

/// Opens a TCP connection to the device's SSH port — enough to tell
/// "reachable" from "nothing there" without asking for credentials.
#[tauri::command]
pub async fn test_connection(host: String, port: u16) -> Probe {
    tauri::async_runtime::spawn_blocking(move || probe_host(&host, port))
        .await
        .unwrap_or(Probe {
            ok: false,
            latency_ms: 0,
            detail: "检测中断".into(),
        })
}

/// Lays a real Liquid Glass view under the webview.
///
/// This is the material macOS 26 draws menus and popovers with — it is far more
/// transparent than any classic `NSVisualEffectView` material, which is why a
/// vibrancy panel reads as a gray slab next to a menu. `NSGlassEffectView` is
/// macOS 26+, so it is looked up by name and simply skipped on older systems,
/// where `window-vibrancy` provides the closest equivalent.
#[cfg(target_os = "macos")]
fn apply_glass(window: &tauri::WebviewWindow) {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, sel};
    use objc2_foundation::NSRect;

    const NS_VIEW_WIDTH_SIZABLE: usize = 2;
    const NS_VIEW_HEIGHT_SIZABLE: usize = 16;
    const NS_WINDOW_BELOW: isize = -1;
    const GLASS_STYLE_REGULAR: isize = 0;

    let Ok(ns_window) = window.ns_window() else {
        return;
    };

    unsafe {
        let ns_window = ns_window as *mut AnyObject;
        let content: *mut AnyObject = msg_send![ns_window, contentView];

        let Some(glass_class) = AnyClass::get(c"NSGlassEffectView") else {
            // pre-macOS 26: fall back to the thinnest classic material
            use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
            if let Err(e) = apply_vibrancy(
                window,
                NSVisualEffectMaterial::Menu,
                Some(NSVisualEffectState::Active),
                None,
            ) {
                eprintln!("vibrancy unavailable: {e}");
            }
            return;
        };

        let bounds: NSRect = msg_send![content, bounds];
        let glass: *mut AnyObject = msg_send![glass_class, alloc];
        let glass: *mut AnyObject = msg_send![glass, initWithFrame: bounds];

        // `.regular` is what AppKit draws menus with: see-through, but the
        // backdrop is blurred so it never competes with the panel's own text.
        // (`.clear` passes the backdrop through nearly sharp — unreadable.)
        if glass_class.responds_to(sel!(setStyle:)) {
            let _: () = msg_send![glass, setStyle: GLASS_STYLE_REGULAR];
        }
        // the window frame already rounds the corners; rounding again would
        // leave a dark crescent in each one
        let _: () = msg_send![glass, setCornerRadius: 0.0f64];
        let _: () = msg_send![
            glass,
            setAutoresizingMask: NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE
        ];

        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () =
            msg_send![content, addSubview: glass, positioned: NS_WINDOW_BELOW, relativeTo: nil];
    }
}

/// Right-clicking the bin pops a real NSMenu — the system draws it, so the
/// floating sheet itself needs no chrome of its own.
///
/// It is also where the queue keeps the two things the window itself will not
/// say: how long the oldest sheet has been waiting — stated only to someone
/// who came looking for it — and "clear everything", which is the one
/// irreversible act here and so is the one that never gets a gesture.
#[tauri::command]
pub fn show_context_menu(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    use tauri::menu::{ContextMenu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem};

    let pending = crate::pending::len();

    let mut builder = MenuBuilder::new(&app);

    if pending > 0 {
        if let Some(age) = crate::pending::age_label() {
            // Not a command — the only way the queue ever mentions how long it
            // has been waiting, and only to someone who came looking.
            let stamp = MenuItemBuilder::new(format!("最早：{age}"))
                .id("queue-age")
                .enabled(false)
                .build(&app)
                .map_err(|e| e.to_string())?;
            builder = builder.item(&stamp);
        }

        // Tearing one sheet out is a gesture and is undoable; tearing them all
        // out is neither, so it gets the second step. A submenu is that step —
        // system-drawn, dismissable by moving the mouse away, and it costs no
        // dialog.
        let confirm = MenuItemBuilder::new(format!("确认清空 {pending} 张"))
            .id("queue-clear")
            .build(&app)
            .map_err(|e| e.to_string())?;
        let clear = tauri::menu::SubmenuBuilder::new(&app, "全部清空…")
            .item(&confirm)
            .build()
            .map_err(|e| e.to_string())?;
        builder = builder.separator().item(&clear);
    }

    let settings = MenuItemBuilder::new("设置…")
        .id("open-settings")
        .accelerator("CmdOrCtrl+,")
        .build(&app)
        .map_err(|e| e.to_string())?;
    let quit = PredefinedMenuItem::quit(&app, Some("退出 RM Bin")).map_err(|e| e.to_string())?;
    let menu = builder
        .separator()
        .item(&settings)
        .separator()
        .item(&quit)
        .build()
        .map_err(|e| e.to_string())?;

    menu.popup(window).map_err(|e| e.to_string())
}

/// Opens the settings window, or brings it forward if it already exists.
#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }

    let window =
        WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("settings.html".into()))
            .title("RM Bin 设置")
            .inner_size(460.0, 480.0)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .always_on_top(false)
            .transparent(true) // let the NSVisualEffectView below show through
            .center()
            .build()
            .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    apply_glass(&window);
    crate::clear_native_background(&window);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rm-bin-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let s = read_settings(&tmp("absent"));
        assert_eq!(s.host, DEFAULT_HOST);
        assert_eq!(s.port, DEFAULT_PORT);
    }

    #[test]
    fn roundtrips_and_normalizes() {
        let p = tmp("roundtrip");
        let saved = write_settings(
            &p,
            &Settings {
                host: "  192.168.1.42 ".into(),
                port: 0,
                gemini_api_key: "  abc123  ".into(),
            },
        )
        .unwrap();
        assert_eq!(saved.host, "192.168.1.42");
        assert_eq!(saved.port, DEFAULT_PORT); // 0 means "unset"
        assert_eq!(saved.gemini_api_key, "abc123");

        let loaded = read_settings(&p);
        assert_eq!(loaded.host, "192.168.1.42");
        assert!(!p.with_extension("json.tmp").exists()); // temp file was renamed away
    }

    #[test]
    fn rejects_empty_host() {
        assert!(write_settings(
            &tmp("empty"),
            &Settings {
                host: "   ".into(),
                port: 22,
                gemini_api_key: String::new(),
            }
        )
        .is_err());
    }

    /// Settings files written before `port` existed, or with extra unknown
    /// fields from an older build, must still load rather than refuse to
    /// parse.
    #[test]
    fn settings_without_port_still_load() {
        let p = tmp("legacy");
        std::fs::write(&p, r#"{"host":"1.2.3.4"}"#).unwrap();
        let s = read_settings(&p);
        assert_eq!(s.host, "1.2.3.4");
        assert_eq!(s.port, DEFAULT_PORT);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let p = tmp("corrupt");
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(read_settings(&p).host, DEFAULT_HOST);
    }

    #[test]
    fn probe_reports_reachable_and_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(probe_host("127.0.0.1", port).ok);

        drop(listener);
        let closed = probe_host("127.0.0.1", port);
        assert!(!closed.ok && !closed.detail.is_empty());

        let bogus = probe_host("no such host.invalid", 22);
        assert!(!bogus.ok);
    }
}
