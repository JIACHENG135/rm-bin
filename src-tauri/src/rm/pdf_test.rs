//! The PDF is written byte by byte, so the bytes are what gets checked.
//!
//! A cross-reference table with a wrong offset is the classic way to produce
//! a file that looks fine in a hex dump and that every reader refuses, and it
//! is invisible until a tablet says "cannot open". So the offsets here are
//! followed back into the file and the objects they claim to point at are
//! read, rather than merely asserting the table exists.

use super::pdf;

/// Unique per call, not just per process: two tests calling `png` with the
/// same dimensions and colour otherwise collide on the same path — `cargo
/// test` runs tests concurrently in one process, so `process::id()` alone
/// doesn't tell them apart, and one thread's `remove_file`/`save` racing
/// another's `image::open` reads a truncated file (an intermittent
/// "unexpected end of file", not a real bug in what's under test).
fn png(w: u32, h: u32, colour: bool) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "rm-bin-pdf-{w}x{h}-{}-{}-{n}.png",
        colour,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&p);
    if colour {
        let mut img = image::RgbImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
        }
        img.save(&p).unwrap();
    } else {
        let mut img = image::GrayImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Luma([((x + y) % 256) as u8]);
        }
        img.save(&p).unwrap();
    }
    p
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Every offset in the table must land exactly on `N 0 obj`. This is the one
/// error a reader will not forgive and a human will not spot.
#[test]
fn the_cross_reference_table_points_at_the_objects() {
    let src = png(300, 200, false);
    let doc = pdf::build(src.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_file(&src);

    assert!(doc.starts_with(b"%PDF-1.4"), "missing header");
    assert!(doc.ends_with(b"%%EOF\n"), "missing trailer");

    // All of this walks *bytes*. Going through `from_utf8_lossy` would be the
    // obvious thing and would be wrong: the embedded JPEG is not UTF-8, so
    // every invalid byte becomes a three-byte replacement character and the
    // string's indices stop being the file's offsets — which is exactly the
    // kind of drift this test exists to catch.
    let tail = doc.len().saturating_sub(64);
    let startxref = find(&doc[tail..], b"startxref").expect("no startxref") + tail + 9;
    let digits: Vec<u8> = doc[startxref..]
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_digit())
        .collect();
    let xref_at: usize = String::from_utf8(digits).unwrap().parse().unwrap();
    assert_eq!(&doc[xref_at..xref_at + 4], b"xref", "startxref misses the table");

    // Entries are fixed-width — "0000000000 00000 n \n" — after the two
    // header lines, entry 0 being the free-list head.
    let after_header = find(&doc[xref_at..], b"f \n").unwrap() + xref_at + 3;
    for i in 0..5 {
        let entry = &doc[after_header + i * 20..after_header + i * 20 + 10];
        let off: usize = std::str::from_utf8(entry)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("bad entry {i}: {:?}", String::from_utf8_lossy(entry)));
        let expect = format!("{} 0 obj", i + 1);
        assert_eq!(
            &doc[off..off + expect.len()],
            expect.as_bytes(),
            "object {} is not where the table says",
            i + 1
        );
    }
}

/// The image goes in as the JPEG it already is; re-encoding it as something
/// else would be a second lossy pass for nothing.
#[test]
fn the_image_is_embedded_as_jpeg_at_its_real_size() {
    let src = png(300, 200, false);
    let doc = pdf::build(src.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_file(&src);

    let text = String::from_utf8_lossy(&doc);
    assert!(text.contains("/Filter /DCTDecode"));
    assert!(text.contains("/Width 300 /Height 200"), "wrong dimensions");
    // A grey source stays grey — a third of the bytes of RGB.
    assert!(text.contains("/ColorSpace /DeviceGray"));
    // And the stream really is a JPEG.
    let at = find(&doc, b"stream\n").unwrap() + 7;
    assert_eq!(&doc[at..at + 3], b"\xff\xd8\xff", "stream is not JPEG data");
}

#[test]
fn a_colour_source_keeps_its_colour_space() {
    let src = png(120, 90, true);
    let doc = pdf::build(src.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_file(&src);
    assert!(String::from_utf8_lossy(&doc).contains("/ColorSpace /DeviceRGB"));
}

/// The page takes the image's aspect so a photograph fills it without bars.
/// Landscape and portrait have to come out the right way up.
#[test]
fn the_page_matches_the_image_aspect() {
    for (w, h) in [(400u32, 200u32), (200, 400)] {
        let src = png(w, h, false);
        let doc = pdf::build(src.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&src);

        let text = String::from_utf8_lossy(&doc);
        let at = text.find("/MediaBox [0 0 ").unwrap() + 15;
        let rest = &text[at..text[at..].find(']').unwrap() + at];
        let mut it = rest.split_whitespace();
        let pw: f64 = it.next().unwrap().parse().unwrap();
        let ph: f64 = it.next().unwrap().parse().unwrap();

        assert!((pw.max(ph) - 842.0).abs() < 0.51, "long edge should be A4's: {pw}x{ph}");
        let want = w as f64 / h as f64;
        assert!(
            ((pw / ph) - want).abs() < 0.01,
            "page {pw}x{ph} does not match image {w}x{h}"
        );
    }
}

/// A photograph off a phone is far larger than the panel; carrying all of it
/// over a USB link would be pointless.
#[test]
fn oversized_images_are_scaled_down_but_small_ones_are_left_alone() {
    let big = png(6000, 3000, false);
    let doc = pdf::build(big.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_file(&big);
    let text = String::from_utf8_lossy(&doc);
    assert!(text.contains("/Width 2560 /Height 1280"), "not scaled to the cap");

    let small = png(64, 48, false);
    let doc = pdf::build(small.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_file(&small);
    assert!(
        String::from_utf8_lossy(&doc).contains("/Width 64 /Height 48"),
        "a small image should not be enlarged"
    );
}

#[test]
fn an_unreadable_file_is_an_error() {
    assert!(pdf::build("/nonexistent/nope.png").is_err());
}

/// The web interface lives at a different address from ssh — it exists only
/// while the USB gadget is up, and there it is always 10.11.99.1. So a tablet
/// configured over wifi must still try the USB address, or plugging a cable
/// in would not get you the no-restart path without also editing a setting.
#[test]
fn the_usb_address_is_tried_even_when_configured_for_wifi() {
    assert_eq!(pdf::web_hosts("10.0.0.113"), ["10.0.0.113", "10.11.99.1"]);
    // ...and not twice when they are the same.
    assert_eq!(pdf::web_hosts("10.11.99.1"), ["10.11.99.1"]);
}

/// The `.content` beside an imported PDF is what tells xochitl it is a PDF at
/// all; get it wrong and the document appears in the library and opens empty.
#[test]
fn the_pdf_wrapper_declares_itself_a_pdf() {
    let c = pdf::content(123_456);
    let v: serde_json::Value = serde_json::from_str(&c).expect("must be valid JSON");
    assert_eq!(v["fileType"], "pdf");
    assert_eq!(v["pageCount"], 1);
    assert_eq!(v["sizeInBytes"], "123456");
    assert_eq!(v["formatVersion"], 2);
}

/// The endpoint answers 200 whatever happens, so the body is the only signal
/// — and "no reply at all" almost always means something else is on port 80.
#[test]
fn the_upload_reply_is_judged_by_its_body() {
    assert!(pdf::check_reply(r#"{"status":"Upload successful"}"#).is_ok());
    assert!(pdf::check_reply(r#"{"status":"ok","name":"x"}"#).is_ok());

    let err = pdf::check_reply(r#"{"error":"file too large"}"#).unwrap_err();
    assert!(err.contains("file too large"), "{err}");

    assert!(pdf::check_reply(r#"{"success":false}"#).is_err());
    assert!(pdf::check_reply("").is_err());
    assert!(pdf::check_reply("   ").is_err());

    let html = pdf::check_reply("<html><body>404</body></html>").unwrap_err();
    assert!(html.contains("网页接口"), "{html}");
}

/// A batch is several documents decided against *one* listing, so each
/// decision has to be visible to the next. Two screenshots bound for the same
/// not-yet-existing folder must end up in one folder, not two identically
/// named ones — the failure batching was introduced to stop, reappearing
/// inside a single batch.
#[test]
fn a_folder_created_for_one_item_is_reused_by_the_next() {
    let mut snap = Vec::new();

    let a = pdf::place(&mut snap, "doc-a", "算法题解", "分组背包", 0);
    let b = pdf::place(&mut snap, "doc-b", "算法题解", "树形DP", 0);

    assert!(a.new_folder.is_some(), "the first item has to create the folder");
    assert!(b.new_folder.is_none(), "the second must reuse it, not mint a second one");
    assert_eq!(a.parent, b.parent, "both documents belong in the same folder");
    assert!(!a.parent.is_empty());
}

/// Same name twice inside one batch has to number the second, which only
/// works if the first was written back into the listing.
#[test]
fn identical_names_inside_one_batch_are_numbered() {
    let mut snap = Vec::new();

    let a = pdf::place(&mut snap, "doc-a", "算法题解", "分组背包", 0);
    let b = pdf::place(&mut snap, "doc-b", "算法题解", "分组背包", 0);

    assert_eq!(a.visible_name, "分组背包");
    assert_eq!(b.visible_name, "分组背包 (2)");
}

/// A folder that already exists on the tablet is reused rather than shadowed,
/// and nothing new is minted for it.
#[test]
fn an_existing_folder_is_reused_without_minting() {
    let mut snap = crate::rm::upload::parse_snapshot(&format!(
        "\u{1}{}\n{}\n",
        "already-there",
        r#"{"parent":"","visibleName":"算法题解","type":"CollectionType"}"#
    ));

    let p = pdf::place(&mut snap, "doc-a", "算法题解", "分组背包", 0);
    assert_eq!(p.parent, "already-there");
    assert!(p.new_folder.is_none());
}
