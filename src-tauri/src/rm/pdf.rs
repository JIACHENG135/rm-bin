//! The image as a document: a one-page PDF, handed to the tablet's own
//! importer.
//!
//! This is the only path that gets all three of the things the others each
//! give up. The tracing paths (`draw`, `rmfile`) produce ink, so they can
//! only ever show what a pen could draw — a photograph comes out of them as a
//! thicket, and text loses its weight to the skeletonizer. `screen` shows the
//! real image but it is not a document: nothing is saved and the pen has
//! nothing to write on, because xochitl is stopped. A PDF is the picture, at
//! full tone, in a file xochitl opens and annotates like anything else.
//!
//! It also asks the least of the device. No ssh, no key, no stopped xochitl,
//! no binaries deployed, no restart: the tablet has an HTTP endpoint whose
//! entire job is accepting documents, and this posts one to it.
//!
//! What it needs instead is that endpoint switched on — Settings › General ›
//! Storage › USB web interface — and reachable, which by default means over
//! the USB cable at 10.11.99.1.

use std::io::Write;
use std::process::Command;

/// Long edge of the embedded image, in pixels.
///
/// The panel is 1620 across, so this is about 1.6x native — enough that
/// zooming in on the tablet still finds detail, without carrying a 12
/// megapixel original around in a file that has to cross a USB link.
const MAX_EDGE: u32 = 2560;

/// JPEG quality for the embedded image.
///
/// This path exists for photographs, and at 92 the artefacts are well below
/// what an e-ink panel resolves. It is the one compromise here: PDF's
/// lossless image filters all need a deflate encoder, which would mean a
/// dependency, and an uncompressed page is ~10 MB. For line art, note that
/// even a visibly ringing JPEG is closer to the original than the tracing
/// paths get — that is the comparison that matters when choosing this mode.
const QUALITY: u8 = 92;

/// Long edge of the PDF page, in points. A4's 842 — an ordinary page size, so
/// the document looks unremarkable in the library and on any other reader.
const PAGE_LONG_EDGE: f64 = 842.0;

/// Render `image_path` as a single-page PDF.
pub fn build(image_path: &str) -> Result<Vec<u8>, String> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{GenericImageView, ImageEncoder};

    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (sw, sh) = img.dimensions();
    if sw == 0 || sh == 0 {
        return Err("这张图是空的".into());
    }

    // Only ever downscale: enlarging would add nothing and cost bytes.
    let long = sw.max(sh);
    let img = if long > MAX_EDGE {
        let s = MAX_EDGE as f64 / long as f64;
        img.resize(
            ((sw as f64 * s).round() as u32).max(1),
            ((sh as f64 * s).round() as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };
    let (w, h) = img.dimensions();

    // Greyscale when the source already is, since the tablet renders grey and
    // a third of the bytes is a third of the transfer.
    let grey = matches!(
        img.color(),
        image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8 | image::ColorType::La16
    );
    let (pixels, color_space, components) = if grey {
        (img.to_luma8().into_raw(), "/DeviceGray", image::ExtendedColorType::L8)
    } else {
        (img.to_rgb8().into_raw(), "/DeviceRGB", image::ExtendedColorType::Rgb8)
    };

    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, QUALITY)
        .write_image(&pixels, w, h, components)
        .map_err(|e| format!("无法编码图片：{e}"))?;

    // Page takes the image's aspect exactly, so a photograph fills it with no
    // white bars down the sides.
    let (pw, ph) = if w >= h {
        (PAGE_LONG_EDGE, PAGE_LONG_EDGE * h as f64 / w as f64)
    } else {
        (PAGE_LONG_EDGE * w as f64 / h as f64, PAGE_LONG_EDGE)
    };

    Ok(assemble(&jpeg, w, h, color_space, pw, ph))
}

/// Lay out the PDF file itself.
///
/// Written by hand rather than with a crate: this is five objects and a
/// cross-reference table, all of it fixed, and the only thing that has to be
/// computed is where each object starts. A PDF writer dependency would be
/// several thousand lines to avoid counting bytes.
fn assemble(jpeg: &[u8], w: u32, h: u32, color_space: &str, pw: f64, ph: f64) -> Vec<u8> {
    // The content stream: scale the unit image to the page and paint it.
    let content = format!("q\n{pw:.2} 0 0 {ph:.2} 0 0 cm\n/Im0 Do\nQ\n");

    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] \
             /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>"
        )
        .into_bytes(),
        {
            // The image, carried as the JPEG it already is: /DCTDecode means
            // the bytes go in untouched, so nothing is re-encoded twice.
            let mut o = format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} \
                 /ColorSpace {color_space} /BitsPerComponent 8 /Filter /DCTDecode \
                 /Length {} >>\nstream\n",
                jpeg.len()
            )
            .into_bytes();
            o.extend_from_slice(jpeg);
            o.extend_from_slice(b"\nendstream");
            o
        },
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()).into_bytes(),
    ];

    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj", i + 1);
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }

    // The cross-reference table is the part that has to be exact: every entry
    // is a byte offset, in a fixed 20-character format, and a reader that
    // finds the wrong byte there rejects the file rather than recovering.
    let xref_at = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for off in &offsets {
        // Exactly 20 bytes per entry, trailing space included — the format is
        // fixed-width and readers index into it arithmetically.
        let _ = writeln!(out, "{off:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    );
    out
}

/// How the document got there — worth reporting, because the two differ in
/// whether the tablet's interface restarted underneath the user.
pub enum Delivered {
    WebInterface,
    Ssh,
}

/// Get the PDF onto the tablet, by whichever route is open.
///
/// The web interface is the better one — it is the tablet's own importer, so
/// nothing restarts and the document simply appears — but it is off by
/// default and, when on, listens only on the USB address. On a tablet that
/// lives on wifi with no cable there is no port 80 at all, which is exactly
/// what the first version of this ran into.
///
/// So: try it, and fall back to placing the file in the document store over
/// ssh, which works anywhere ssh does and costs an xochitl restart. The web
/// interface is tried first rather than configured, because "is it reachable"
/// is a question with a fast, definitive answer and no setting can be as
/// accurate as asking.
pub fn deliver(host: &str, port: u16, name: &str, pdf: &[u8]) -> Result<Delivered, String> {
    match upload(host, name, pdf) {
        Ok(()) => Ok(Delivered::WebInterface),
        Err(web_err) => match install_over_ssh(host, port, name, pdf) {
            Ok(()) => Ok(Delivered::Ssh),
            // Report both: one of them is the reason, and which one depends
            // on a setup detail only the person in front of the tablet knows.
            Err(ssh_err) => Err(format!("{ssh_err}\n（网页接口也不通：{web_err}）")),
        },
    }
}

/// Place the PDF in xochitl's document store and restart it.
///
/// The wrapper xochitl needs around an imported PDF is the same shape as a
/// notebook's, minus the page: a `.metadata` naming it and a `.content`
/// saying it is a PDF. Everything else — page ids, thumbnails — xochitl
/// generates for itself on the next start.
fn install_over_ssh(host: &str, port: u16, name: &str, pdf: &[u8]) -> Result<(), String> {
    use crate::rm::upload::{install_files, Entry};

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let doc = crate::rm::upload::uuid();

    install_files(
        host,
        port,
        &[
            Entry {
                name: format!("{doc}.metadata"),
                bytes: crate::rm::rmfile::metadata(name, now_ms).into_bytes(),
            },
            Entry {
                name: format!("{doc}.content"),
                bytes: content(pdf.len()).into_bytes(),
            },
            Entry { name: format!("{doc}.pdf"), bytes: pdf.to_vec() },
        ],
        &[],
    )
}

/// The `.content` for an imported PDF.
pub(crate) fn content(size: usize) -> String {
    format!(
        r#"{{
    "coverPageNumber": -1,
    "documentMetadata": {{}},
    "extraMetadata": {{}},
    "fileType": "pdf",
    "fontName": "",
    "formatVersion": 2,
    "lineHeight": -1,
    "margins": 125,
    "orientation": "portrait",
    "pageCount": 1,
    "sizeInBytes": "{size}",
    "tags": [],
    "textAlignment": "justify",
    "textScale": 1
}}
"#
    )
}

/// Post the PDF to the tablet's USB web interface.
///
/// `curl` again, for multipart: hand-rolling a form body to save a
/// subprocess, in an app that already spawns `ssh` for everything else, would
/// be effort spent in the wrong place.
pub fn upload(host: &str, name: &str, pdf: &[u8]) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("{name}.pdf"));
    std::fs::write(&path, pdf).map_err(|e| format!("无法写入临时文件：{e}"))?;

    let url = format!("http://{host}/upload");
    let out = Command::new("curl")
        .args(["--silent", "--show-error", "--max-time", "120", "-X", "POST"])
        .arg("-F")
        .arg(format!("file=@{};type=application/pdf", path.display()))
        .arg(&url)
        .output();
    let _ = std::fs::remove_file(&path);

    let out = out.map_err(|e| format!("无法启动 curl：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        return Err(format!(
            "连不上设备的网页接口（{url}）：{err}。\
             请在设备上打开「设置 › 通用 › 存储 › USB 网页界面」，并用 USB 线连接（地址 10.11.99.1）"
        ));
    }
    check_reply(&String::from_utf8_lossy(&out.stdout))
}

/// The endpoint answers 200 with a JSON body either way, so the status code
/// says nothing and the body has to be read.
pub(crate) fn check_reply(body: &str) -> Result<(), String> {
    let text = body.trim();
    if text.is_empty() {
        // Nothing at all usually means something other than the web interface
        // answered on port 80.
        return Err("设备没有回应上传请求，请确认「USB 网页界面」已打开".into());
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if v.get("status").and_then(|s| s.as_str()) == Some("Upload successful") {
            return Ok(());
        }
        if let Some(msg) = v.get("error").and_then(|s| s.as_str()) {
            return Err(format!("设备拒绝了这份文件：{msg}"));
        }
        // Unknown JSON: treat an explicit failure flag as failure, otherwise
        // accept — the shape of a success reply is not worth being strict
        // about when the file either arrives or doesn't.
        if v.get("success").and_then(|s| s.as_bool()) == Some(false) {
            return Err("设备拒绝了这份文件".into());
        }
        return Ok(());
    }
    // HTML back means we reached a web server that is not this endpoint.
    if text.starts_with('<') {
        return Err("设备的网页接口没有接受上传，请确认它已在设置里打开".into());
    }
    Err(format!("设备返回了意外的应答：{}", text.chars().take(120).collect::<String>()))
}
