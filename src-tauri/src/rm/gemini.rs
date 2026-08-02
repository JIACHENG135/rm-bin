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

/// The prompt, in rm-agent's manner: describe the machine the drawing is
/// about to go through, then let the rules fall out of it.
///
/// Asking for "clean line art, no shading" — which is what this used to say —
/// gets you a picture that looks like line art and traces terribly, because
/// the failure modes here are not the ones a human illustrator would guess.
/// The tracer's problem is topological: it walks a 1-pixel skeleton and has
/// to choose a branch wherever ink meets ink. Nothing about "clean" tells a
/// model that two contours *touching* is worse than either contour being
/// wrong, or that a filled shape carries no more information than its
/// outline. Explaining the pipeline does.
///
/// `work_px` is `draw::BASE_WORK` rather than a number typed twice: the size
/// floor below which strokes fuse is a consequence of that raster, and a
/// prompt claiming a different one would be quietly lying to the model.
pub(crate) fn prompt(work_px: u32) -> String {
    format!(
        "Redraw this photograph as line art that a pen will physically draw, stroke by \
stroke, on an e-ink tablet.\n\n\
Here is exactly what happens to your image afterwards, so you can reason about what \
will survive it: it is downscaled to about {work_px} pixels on its longest edge, \
converted to black and white by Otsu thresholding, thinned to a 1-pixel-wide \
centreline skeleton (Zhang-Suen), and that skeleton is walked into ordered pen strokes \
— starting from endpoints, following connected pixels, and taking the \
straightest-continuing branch wherever a pixel has more than one unvisited neighbour.\n\n\
Three consequences decide whether a drawing survives this.\n\n\
Touching ink becomes a tangle. Anywhere two pieces of ink touch or cross, they become \
one connected blob with branch points, and the tracer has to guess which branch \
continues which line. It guesses wrong often enough that touching ink reliably comes \
out garbled instead of as the two clean shapes you drew. This applies to any contact at \
all. So leave generous space between every distinct element, and let contours stop \
short of each other rather than meet.\n\n\
Fills and shading carry no information. Nothing grey and nothing coloured survives the \
threshold: a filled or shaded region becomes an undifferentiated black area whose \
skeleton is a meaningless spine down its middle. Use outlines only. Never hatch, \
cross-hatch or stipple — every hatch line is itself a branch point where it meets the \
outline it is filling.\n\n\
Small detail fuses. Below roughly one part in {work_px} of the image's long edge, \
neighbouring strokes merge into a single blob when thinned. Draw few, large, confident \
shapes. It is far better to under-describe the subject with a dozen clean contours than \
to render it faithfully with hundreds of small ones.\n\n\
Given all that: redraw the subject as a small number of long, smooth, well-separated \
outlines on a plain white background, keeping the composition, proportions and \
recognisable shape of the original. Even, moderate line weight throughout — the \
thinning discards weight anyway, and varying it only ragged the edges. Drop background \
scenery that is not the subject rather than outlining it too. No page frame, border, \
margin line, signature or watermark. If the original contains lettering that has to be \
kept, write it as separated print letters, never joined script: connected letters are \
the touching-ink case again, and a whole word merges into one blob."
    )
}

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
            { "type": "text", "text": prompt(crate::rm::draw::BASE_WORK as u32) },
            { "type": "image", "mime_type": mime_of(image_path), "data": base64_encode(&bytes) }
        ]
    })
    .to_string();

    let dir = std::env::temp_dir();
    let stem = format!("rmbin-gemini-{}", std::process::id());
    let body_path = dir.join(format!("{stem}.json"));
    let conf_path = dir.join(format!("{stem}.conf"));
    let out_path = dir.join(format!("{stem}.out"));

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
    let image = match extract_image(&raw) {
        Ok(p) => p,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };
    cleanup();

    // The extension has to match the bytes: everything downstream opens this
    // file with `image::open`, which decides the format from the name. The
    // first version always wrote `.png` and the model answered with a JPEG,
    // so a perfectly good picture came back as "Invalid PNG signature".
    //
    // Sniffed rather than taken from the response's own `mime_type`, because
    // the bytes are the only part of that answer that cannot be wrong.
    let ext = extension_of(&image)?;
    let path = dir.join(format!("{stem}.{ext}"));
    std::fs::write(&path, &image).map_err(|e| format!("无法保存线稿：{e}"))?;
    Ok(path)
}

/// The file extension for some image bytes, by magic number.
pub(crate) fn extension_of(bytes: &[u8]) -> Result<&'static str, String> {
    let head = |sig: &[u8]| bytes.starts_with(sig);
    if head(b"\x89PNG\r\n\x1a\n") {
        Ok("png")
    } else if head(b"\xff\xd8\xff") {
        Ok("jpg")
    } else if bytes.len() > 12 && head(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Ok("webp")
    } else if head(b"GIF87a") || head(b"GIF89a") {
        Ok("gif")
    } else if head(b"BM") {
        Ok("bmp")
    } else {
        // Say what did arrive: at this point the request succeeded and
        // something was decoded, so the interesting question is what.
        let head: String = bytes.iter().take(8).map(|b| format!("{b:02x} ")).collect();
        Err(format!(
            "Gemini 返回了无法识别的图片格式（{} 字节，开头 {}）",
            bytes.len(),
            head.trim_end()
        ))
    }
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
