//! The parts of the screen path that can be checked without a tablet.
//!
//! Two things here are wire contracts with C code that lives in another
//! language in another directory and only runs on hardware — the header
//! layout and the panel handshake. Those are exactly the sort of thing that
//! goes wrong silently and is then debugged over ssh with the screen taken
//! over, so they are pinned here instead.

use super::screen::{self, Panel};

fn panel() -> Panel {
    Panel { width: 1620, height: 2160, format: 4 }
}

/// Byte-for-byte against what `agent/main.c` reads: magic, op, mode, refresh,
/// then four little-endian u16s at offsets 8, 10, 12, 14. A field that moves
/// on one side and not the other paints somewhere unexpected on a device we
/// cannot see.
#[test]
fn the_header_matches_what_the_agent_parses() {
    let h = screen::header(1, 4, 1, 0x0102, 0x0304, 0x0506, 0x0708);
    assert_eq!(&h[..4], b"RMFB");
    assert_eq!([h[4], h[5], h[6]], [1, 4, 1]);
    assert_eq!(&h[8..10], &[0x02, 0x01]);
    assert_eq!(&h[10..12], &[0x04, 0x03]);
    assert_eq!(&h[12..14], &[0x06, 0x05]);
    assert_eq!(&h[14..16], &[0x08, 0x07]);
    assert_eq!(h.len(), 16);
}

/// The agent's first line is how the app learns the panel's size; a tablet
/// that says something unexpected has to be an error rather than a guess.
#[test]
fn the_handshake_is_parsed_or_refused() {
    let p = screen::parse_hello("RMFB 1620 2160 4\n").expect("valid hello");
    assert_eq!((p.width, p.height, p.format), (1620, 2160, 4));

    for bad in ["", "\n", "hello\n", "RMFB\n", "RMFB 1620\n", "RMFB x y 4\n"] {
        assert!(screen::parse_hello(bad).is_none(), "{bad:?} should be refused");
    }
}

/// The stamp decides whether the tablet gets the binary you just built. It
/// used to be the two file sizes, and two consecutive builds of the agent
/// came out byte-identical in length while differing in content — so the
/// device quietly kept running the old one. Same-length, different-content is
/// therefore the case this pins.
#[test]
fn the_deploy_stamp_notices_a_rebuild_of_the_same_size() {
    let a = vec![1u8; 4096];
    let mut b = a.clone();
    b[2048] = 2;
    let shim = vec![9u8; 128];

    assert_eq!(a.len(), b.len());
    assert_ne!(screen::stamp_of(&a, &shim), screen::stamp_of(&b, &shim));
    assert_eq!(screen::stamp_of(&a, &shim), screen::stamp_of(&a, &shim));
    // The two halves must not be interchangeable either.
    assert_ne!(screen::stamp_of(&a, &shim), screen::stamp_of(&shim, &a));
}

fn write_png(path: &std::path::Path, w: u32, h: u32) {
    let mut img = image::GrayImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Luma([((x + y) % 256) as u8]);
    }
    img.save(path).unwrap();
}

fn tmp(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("rm-bin-screen-{name}-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// Whatever comes in, what goes out is exactly one panel's worth of pixels —
/// the agent blits it row by row against a stride it was told, so a buffer of
/// the wrong length would run off the end of the framebuffer.
#[test]
fn fit_always_produces_exactly_one_panel() {
    let p = panel();
    for (name, w, h) in [("wide", 4000u32, 300u32), ("tall", 200, 5000), ("tiny", 3, 3)] {
        let path = tmp(name);
        write_png(&path, w, h);
        let out = screen::fit(path.to_str().unwrap(), &p).unwrap();
        assert_eq!(out.len(), (p.width * p.height) as usize, "{name}");
        let _ = std::fs::remove_file(&path);
    }
}

/// Fit, not fill: a panoramic image keeps its shape and gets paper above and
/// below rather than being cropped to the panel's aspect ratio.
#[test]
fn a_wide_image_is_letterboxed_not_cropped() {
    let p = panel();
    let path = tmp("letterbox");
    write_png(&path, 3240, 1080); // 3:1, far wider than the 3:4 panel
    let out = screen::fit(path.to_str().unwrap(), &p).unwrap();

    let row_is_blank = |y: u32| {
        let s = (y * p.width) as usize;
        out[s..s + p.width as usize].iter().all(|&v| v == 0xff)
    };
    // scaled height is 1620/3 = 540, centered in 2160: rows 810..1350
    assert!(row_is_blank(10), "top margin should be paper");
    assert!(row_is_blank(p.height - 10), "bottom margin should be paper");
    assert!(!row_is_blank(p.height / 2), "the image should be in the middle");
    let _ = std::fs::remove_file(&path);
}

/// An image the same shape as the panel should reach both edges — otherwise
/// the fit is quietly shrinking everything.
#[test]
fn an_exactly_shaped_image_fills_the_panel() {
    let p = panel();
    let path = tmp("exact");
    write_png(&path, 810, 1080); // same 3:4 ratio, half size
    let out = screen::fit(path.to_str().unwrap(), &p).unwrap();
    let blank_rows = (0..p.height)
        .filter(|&y| {
            let s = (y * p.width) as usize;
            out[s..s + p.width as usize].iter().all(|&v| v == 0xff)
        })
        .count();
    // The synthetic gradient does contain white pixels, but never a whole row
    // of them once it covers the panel.
    assert_eq!(blank_rows, 0, "{blank_rows} rows left as margin");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_empty_or_unreadable_file_is_an_error() {
    let p = panel();
    assert!(screen::fit("/nonexistent/nope.png", &p).is_err());
}
