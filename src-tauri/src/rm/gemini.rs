//! Turning a photograph into something a pen can actually draw.
//!
//! The tracer takes an image apart into strokes: threshold, skeletonize,
//! follow the medial axis. That works on line art and falls apart on
//! photographs, because a photograph has no lines in it — it has tone, and
//! thresholding tone produces blobs whose skeletons are a thicket.
//!
//! So ask a model to draw the line art first. What comes back is not the
//! photograph — it is a redrawing of it, which is the honest description and
//! the reason this is its own mode rather than something applied silently to
//! the other two. For an image that is *already* clean line art, this makes
//! things worse, not better.
//!
//! HTTP goes through `curl` rather than a Rust client. The app already talks
//! to the outside world by spawning `ssh`, adding a TLS stack and its
//! transitive dependencies to send one request a minute is a poor trade, and
//! macOS has curl. The key never appears in an argument — `ps` is world
//! readable — it goes in a config file that is written 0600 and deleted.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";

/// Flash rather than Pro: this is a redrawing job with a tightly specified
/// output, not a composition problem, and it sits in the middle of a drag and
/// drop where seconds are felt.
const MODEL: &str = "gemini-3.1-flash-image";

/// Every clause here is aimed at the tracer downstream, not at looking good.
/// Even line weight, because skeletonizing collapses weight anyway and
/// varying it only produces ragged edges; no shading or hatching, because
/// each hatch line becomes a stroke and a thousand strokes is an hour of pen
/// replay; white background, because Otsu's threshold splits the histogram
/// and a grey background moves the split somewhere useless.
const PROMPT: &str = "Redraw this image as clean black-and-white line art for a pen plotter. \
Pure white background. Solid black outlines of even, moderate thickness. \
No shading, no hatching, no cross-hatching, no stippling, no grey, no gradients, no filled areas. \
Keep the composition, subject and proportions of the original. \
Prefer few confident continuous contours over many short strokes. \
Do not add any text, border, frame, signature or watermark.";

/// Long enough for a slow generation, short enough that a drop that is never
/// coming back gives up while the window is still showing the photo.
const TIMEOUT_SECS: u32 = 120;

/// The key comes from the environment, so it is never written to disk by this
/// app at all.
///
/// The catch, and the reason the error says so: a GUI app launched from
/// Finder does not inherit a shell's environment. `launchctl setenv` is what
/// makes a variable visible to it.
pub fn api_key() -> Result<String, String> {
    match std::env::var("GEMINI_API_KEY") {
        Ok(k) if !k.trim().is_empty() => Ok(k.trim().to_string()),
        _ => Err("没有找到 GEMINI_API_KEY。\
                  从终端启动 RM Bin，或先运行 launchctl setenv GEMINI_API_KEY <你的密钥> \
                  再重新打开（从访达启动的应用拿不到 shell 的环境变量）"
            .into()),
    }
}

/// Ask the model to redraw `image_path` as line art. Returns a PNG on disk.
pub fn to_line_art(image_path: &str) -> Result<PathBuf, String> {
    let key = api_key()?;
    let bytes = std::fs::read(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    if bytes.is_empty() {
        return Err("这张图是空的".into());
    }

    let body = serde_json::json!({
        "model": MODEL,
        "input": [
            { "type": "text", "text": PROMPT },
            { "type": "image", "mime_type": mime_of(image_path), "data": base64_encode(&bytes) }
        ]
    })
    .to_string();

    let dir = std::env::temp_dir();
    let stem = format!("rmbin-gemini-{}", std::process::id());
    let body_path = dir.join(format!("{stem}.json"));
    let conf_path = dir.join(format!("{stem}.conf"));
    let out_path = dir.join(format!("{stem}.out"));
    let png_path = dir.join(format!("{stem}.png"));

    // The key goes in the config file, never in argv.
    let conf = format!(
        "header = \"x-goog-api-key: {key}\"\n\
         header = \"Content-Type: application/json\"\n\
         url = \"{ENDPOINT}\"\n\
         request = \"POST\"\n\
         data = \"@{}\"\n\
         output = \"{}\"\n\
         silent\n\
         show-error\n\
         max-time = \"{TIMEOUT_SECS}\"\n",
        body_path.display(),
        out_path.display()
    );

    // The config file holds the API key, so it is removed on every path out
    // of here, not just the successful one.
    let cleanup = || {
        for p in [&body_path, &conf_path, &out_path] {
            let _ = std::fs::remove_file(p);
        }
    };

    write_private(&body_path, body.as_bytes()).map_err(|e| {
        cleanup();
        format!("无法写入请求：{e}")
    })?;
    if let Err(e) = write_private(&conf_path, conf.as_bytes()) {
        cleanup();
        return Err(format!("无法写入请求：{e}"));
    }

    let run = Command::new("curl").arg("--config").arg(&conf_path).output();
    let out = match run {
        Ok(o) => o,
        Err(e) => {
            cleanup();
            return Err(format!("无法启动 curl：{e}"));
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        cleanup();
        return Err(format!("Gemini 请求失败：{}", first_line(&err, "网络错误")));
    }

    let raw = std::fs::read(&out_path).unwrap_or_default();
    let png = match extract_image(&raw) {
        Ok(p) => p,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    cleanup();

    std::fs::write(&png_path, &png).map_err(|e| format!("无法保存线稿：{e}"))?;
    Ok(png_path)
}

/// Write with an owner-only mode, since one of these holds the API key.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "image/png",
    }
}

fn first_line<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or(fallback)
}

/// Find the generated image in the response.
///
/// Written as a search rather than a path because the response shape is not
/// something to depend on: the SDKs expose `output_image` as a convenience
/// over a structure that has already moved once (this endpoint replaced
/// `:generateContent`). Any object carrying base64 `data` alongside an
/// `image/*` mime type is the thing we came for, wherever it is nested.
pub(crate) fn extract_image(raw: &[u8]) -> Result<Vec<u8>, String> {
    let json: serde_json::Value = serde_json::from_slice(raw).map_err(|_| {
        let text = String::from_utf8_lossy(raw);
        format!("Gemini 返回了无法解析的内容：{}", first_line(&text, "空响应"))
    })?;

    // An API error is a normal JSON body; say what it said rather than
    // "no image found".
    if let Some(msg) = json.pointer("/error/message").and_then(|v| v.as_str()) {
        return Err(format!("Gemini 拒绝了请求：{msg}"));
    }

    let mut found = None;
    find_image(&json, &mut found);
    let Some(b64) = found else {
        return Err("Gemini 没有返回图片（可能是提示词被拒绝，或配额用尽）".into());
    };
    base64_decode(&b64).ok_or_else(|| "Gemini 返回的图片数据无法解码".into())
}

fn find_image(v: &serde_json::Value, out: &mut Option<String>) {
    if out.is_some() {
        return;
    }
    match v {
        serde_json::Value::Object(map) => {
            let mime = ["mime_type", "mimeType"]
                .iter()
                .find_map(|k| map.get(*k).and_then(|m| m.as_str()));
            if mime.map(|m| m.starts_with("image/")).unwrap_or(false) {
                if let Some(d) = map.get("data").and_then(|d| d.as_str()) {
                    *out = Some(d.to_string());
                    return;
                }
            }
            for child in map.values() {
                find_image(child, out);
                if out.is_some() {
                    return;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                find_image(child, out);
                if out.is_some() {
                    return;
                }
            }
        }
        _ => {}
    }
}

// ————— base64 —————
//
// Twenty lines against a dependency and its own transitive tree, for
// something with a fixed definition that will never need updating.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - i * 6)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Tolerant of whitespace and of the URL-safe alphabet, because what comes
/// back is somebody else's encoder.
pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}
