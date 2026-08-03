//! Check whether a macOS system CJK font can be loaded from disk (not
//! embedded — see markdown.rs's CJK font doc for why) and covers the
//! characters we need, before wiring it into the real pipeline.
//!
//! Usage: `cargo run --bin probe_system_font -- /path/to/font.ttc [face_index]`

use ab_glyph::{Font, FontRef};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let index: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

    let data = std::fs::read(path).unwrap();
    eprintln!("read {} bytes", data.len());
    let font = FontRef::try_from_slice_and_index(&data, index).unwrap_or_else(|e| {
        eprintln!("failed to parse face {index}: {e:?}");
        std::process::exit(1);
    });
    eprintln!("parsed face {index} ok, {} glyphs, units_per_em={:?}", font.glyph_count(), font.units_per_em());

    let test = "中文两世界你好人大小水火木金土的一是不了在";
    let mut missing = Vec::new();
    for ch in test.chars() {
        let id = font.glyph_id(ch);
        if id.0 == 0 {
            missing.push(ch);
        }
    }
    if missing.is_empty() {
        eprintln!("all {} test characters have glyphs", test.chars().count());
    } else {
        eprintln!("missing glyphs for: {:?}", missing);
    }
}
