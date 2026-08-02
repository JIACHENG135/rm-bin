//! The PDF is written byte by byte, so the bytes are what gets checked.
//!
//! A cross-reference table with a wrong offset is the classic way to produce
//! a file that looks fine in a hex dump and that every reader refuses, and it
//! is invisible until a tablet says "cannot open". So the offsets here are
//! followed back into the file and the objects they claim to point at are
//! read, rather than merely asserting the table exists.

use super::pdf;

fn png(w: u32, h: u32, colour: bool) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "rm-bin-pdf-{w}x{h}-{}-{}.png",
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
