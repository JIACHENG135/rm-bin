//! Asking Gemini what a dropped screenshot is, so the document that lands on
//! the tablet can be named and filed like a person would file it rather than
//! carrying the screenshot's own filename into the library forever.
//!
//! This only ever runs ahead of the ssh fallback path — see `pdf::deliver` —
//! because the web interface has no folder to place a document into, so
//! there is nothing for a suggested folder name to do there.

use std::process::Command;

use base64::Engine;

/// Long edge of the thumbnail sent to Gemini, in pixels. Naming and rough
/// subject classification need far less detail than the page image does, and
/// a small JPEG keeps the request fast and cheap.
const THUMB_EDGE: u32 = 768;

const MODEL: &str = "gemini-2.5-flash";

#[derive(Debug)]
pub struct Suggestion {
    pub folder: String,
    pub name: String,
}

/// Suggest a folder and a filename for `image_path`. Never fails outward:
/// on any error (no key, no network, a malformed reply) it falls back to
/// `fallback_name` unfiled, which is exactly today's behaviour, and logs why.
///
/// `existing_folders` are the top-level folder names already in the document
/// library. Without them every upload was a fresh question with no memory,
/// and the answers drifted across synonyms — a library that had collected
/// 编程题目, 编程题解, 编程习题, 编程刷题, 编程学习, 编程代码, 算法题 and
/// 算法题解 as eight separate folders holding fourteen problems between them.
/// Since `upload::resolve_folder` reuses a folder only on an *exact* name
/// match, the prompt's job is to make reuse the default and verbatim copying
/// the only way to express it.
pub fn suggest(
    image_path: &str,
    api_key: &str,
    fallback_name: &str,
    existing_folders: &[String],
) -> Suggestion {
    match try_suggest(image_path, api_key, existing_folders) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[rm-bin] gemini naming skipped, keeping the file's own name: {e}");
            Suggestion { folder: String::new(), name: fallback_name.to_string() }
        }
    }
}

fn try_suggest(
    image_path: &str,
    api_key: &str,
    existing_folders: &[String],
) -> Result<Suggestion, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("no api key configured".into());
    }

    let jpeg = thumbnail(image_path)?;
    let body = request_body(&jpeg, existing_folders);

    let tmp = std::env::temp_dir().join(format!(
        "rm-bin-gemini-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&tmp, &body).map_err(|e| format!("无法写入临时文件：{e}"))?;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{MODEL}:generateContent?key={key}"
    );
    let out = Command::new("curl")
        .args(["--silent", "--show-error", "--max-time", "20", "--connect-timeout", "5"])
        .args(["-H", "Content-Type: application/json"])
        .arg("--data-binary")
        .arg(format!("@{}", tmp.display()))
        .arg(url)
        .output();
    let _ = std::fs::remove_file(&tmp);

    let out = out.map_err(|e| format!("无法启动 curl：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        return Err(format!("Gemini 请求失败：{err}"));
    }

    parse_reply(&String::from_utf8_lossy(&out.stdout))
}

/// A small, JPEG-encoded copy of the image — Gemini needs only enough detail
/// to say what the thing is, not the full-resolution page image.
fn thumbnail(image_path: &str) -> Result<Vec<u8>, String> {
    use image::{GenericImageView, ImageEncoder};

    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (w, h) = img.dimensions();
    let long = w.max(h);
    let img = if long > THUMB_EDGE {
        let s = THUMB_EDGE as f64 / long as f64;
        img.resize(
            ((w as f64 * s).round() as u32).max(1),
            ((h as f64 * s).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let (w, h) = img.dimensions();

    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
        .write_image(&img.to_rgb8().into_raw(), w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("无法编码缩略图：{e}"))?;
    Ok(jpeg)
}

/// The folder half of the prompt.
///
/// With a library to look at, the instruction is "reuse, and copy the name
/// character for character"; a paraphrase is as good as a new folder to
/// `resolve_folder`, so the rule has to be about the string, not the meaning.
/// The empty case is the original prompt — a first upload has nothing to
/// reuse and inventing a name is the whole job.
fn folder_rule(existing: &[String]) -> String {
    if existing.is_empty() {
        return "1) folder，一个简短的中文分类文件夹名（2-8 个字，例如“工作笔记”“网购订单”\
                “聊天记录”“文档扫描”），用于把相似的截图归到一起；"
            .to_string();
    }

    let list = existing
        .iter()
        .map(|f| format!("  - {f}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "1) folder，这张图应该放进哪个文件夹。文档库里现有这些文件夹：\n{list}\n\
         规则：只要上面任何一个装得下这张图，就必须**一字不差地照抄**那个名字，\
         不要改写、不要加字、不要用近义词。近义的名字算同一个类别——\
         比如已经有“编程题目”，就不要再造“编程题解”“编程习题”“算法题”；\
         已经有“面试面经”，就不要再造“技术面经”“求职面试”。\
         只有当现有文件夹**全都明显不合适**时，才新起一个简短的中文文件夹名（2-8 个字）；"
    )
}

fn request_body(jpeg: &[u8], existing_folders: &[String]) -> Vec<u8> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
    let prompt = format!(
        "这是用户刚截的一张图，将被存成 PDF 放进 reMarkable 平板的文档库。\
         请给出：{}2) name，一个简短的中文文件名（不超过 20 个字，不带扩展名），\
         概括这张图的内容，方便日后在文档库里一眼认出。只输出这两个字段，不要输出多余内容。",
        folder_rule(existing_folders)
    );

    serde_json::json!({
        "contents": [{
            "parts": [
                { "text": prompt },
                { "inline_data": { "mime_type": "image/jpeg", "data": b64 } }
            ]
        }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": {
                    "folder": { "type": "STRING" },
                    "name": { "type": "STRING" }
                },
                "required": ["folder", "name"]
            }
        }
    })
    .to_string()
    .into_bytes()
}

fn parse_reply(body: &str) -> Result<Suggestion, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Gemini 应答不是合法 JSON：{e}"))?;

    if let Some(msg) = v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Err(format!("Gemini 拒绝了请求：{msg}"));
    }

    let text = v
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.get(0))
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| "Gemini 应答里没有找到内容".to_string())?;

    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("Gemini 返回的内容不是合法 JSON：{e}"))?;

    let folder = sanitize(parsed.get("folder").and_then(|f| f.as_str()).unwrap_or(""), 20);
    let name = sanitize(parsed.get("name").and_then(|n| n.as_str()).unwrap_or(""), 40);

    if name.is_empty() {
        return Err("Gemini 没有给出可用的文件名".into());
    }
    Ok(Suggestion { folder, name })
}

/// Both names end up interpolated into `.metadata`'s JSON and shown verbatim
/// in xochitl's file list, and a folder name additionally has to survive as
/// a plain visible name with no path meaning of its own — so anything that
/// could break out of the JSON string or read as a path separator is
/// stripped, the same rule `upload::name_from_path` applies to filenames.
fn sanitize(s: &str, max_chars: usize) -> String {
    s.trim()
        .chars()
        .filter(|c| !c.is_control() && !['"', '\\', '/'].contains(c))
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_reply() {
        let body = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": r#"{"folder":"网购订单","name":"跨境电商发货单"}"#
                    }]
                }
            }]
        })
        .to_string();
        let s = parse_reply(&body).unwrap();
        assert_eq!(s.folder, "网购订单");
        assert_eq!(s.name, "跨境电商发货单");
    }

    #[test]
    fn an_error_reply_is_reported_not_silently_swallowed() {
        let body = serde_json::json!({ "error": { "message": "API key not valid" } }).to_string();
        let err = parse_reply(&body).unwrap_err();
        assert!(err.contains("API key not valid"), "{err}");
    }

    #[test]
    fn sanitize_strips_quotes_backslashes_and_slashes() {
        assert_eq!(sanitize("a\"b\\c/d", 10), "abcd");
    }

    #[test]
    fn sanitize_truncates_to_the_limit() {
        assert_eq!(sanitize(&"x".repeat(100), 5).chars().count(), 5);
    }

    #[test]
    fn missing_key_is_an_error_with_no_network_call() {
        let s = suggest("/nonexistent/nope.png", "", "fallback", &[]);
        assert_eq!(s.name, "fallback");
        assert!(s.folder.is_empty());
    }

    #[test]
    fn an_empty_library_asks_for_a_new_name() {
        let rule = folder_rule(&[]);
        assert!(rule.contains("一个简短的中文分类文件夹名"), "{rule}");
        assert!(!rule.contains("照抄"), "{rule}");
    }

    #[test]
    fn existing_folders_are_listed_and_reuse_is_demanded() {
        let rule = folder_rule(&["算法题解".into(), "面试面经".into(), "移民材料".into()]);
        for f in ["算法题解", "面试面经", "移民材料"] {
            assert!(rule.contains(f), "{f} missing from: {rule}");
        }
        assert!(rule.contains("一字不差地照抄"), "{rule}");
    }

    #[test]
    fn the_folder_list_reaches_the_request_body() {
        let body = request_body(b"not-a-real-jpeg", &["移民材料".into()]);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let text = v["contents"][0]["parts"][0]["text"].as_str().unwrap();
        assert!(text.contains("移民材料"), "{text}");
        // The name half must survive being spliced together with the folder half.
        assert!(text.contains("不带扩展名"), "{text}");
    }
}
