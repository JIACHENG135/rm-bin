//! Latin text as Hershey vector-font strokes, replacing the
//! rasterize→skeletonize→trace pipeline `markdown.rs` still uses for CJK.
//!
//! That pipeline is what this crate uses everywhere else — a real font,
//! rasterized, thresholded, skeletonized, traced — and for CJK it draws
//! cleanly (verified on real Paper Pro hardware). For Latin cursive
//! handwriting fonts it does not: skeletonizing a connected script glyph
//! produces many short, sharply-curved strokes, and on real hardware those
//! came out either visibly dotted (sparse point spacing) or, densified,
//! sometimes merged into spurious connecting lines between letters —
//! traced across a dozen-plus live device pushes comparing fonts, point
//! densities and this crate's own tracer output directly against each
//! other on the same page. The one thing that read cleanly, every time,
//! was a plain Hershey Simplex font (`futural`) sent at its native point
//! spacing — few, long, mostly-straight strokes per glyph, not many short
//! curved ones.
//!
//! Hershey glyphs are pre-drawn pen strokes, not outlines — there's
//! nothing to rasterize or trace, which is exactly why they hold up. They
//! only cover ASCII, though, so CJK still needs the raster path; see
//! `markdown.rs`'s `Layout::art` for the split.
//!
//! The strokes themselves come from `svg2rm`'s reference implementation
//! (not part of this repo) via the Python `hershey-fonts` package, dumped
//! once into `resources/hershey/futural.json` with the same per-character
//! continuity grouping `text2rm.py::text_to_strokes_raw` uses, and bundled
//! here rather than parsed from the original Hershey/JHF format at
//! runtime — there are only 94 ASCII glyphs, so a precomputed table is
//! simpler and more robust than a font-format parser for a one-time
//! extraction. Hershey's font data itself is US federal government work
//! (Dr. A. V. Hershey, US Naval Weapons Laboratory) and is public domain.

use crate::rm::draw::Poly;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const TABLE_BYTES: &[u8] = include_bytes!("../../resources/hershey/futural.json");

#[derive(Deserialize)]
struct RawGlyph {
    width: f64,
    strokes: Vec<Vec<(f64, f64)>>,
}

#[derive(Deserialize)]
struct RawTable {
    glyphs: HashMap<String, RawGlyph>,
}

fn table() -> &'static HashMap<char, RawGlyph> {
    static TABLE: OnceLock<HashMap<char, RawGlyph>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let raw: RawTable = serde_json::from_slice(TABLE_BYTES).expect("bundled hershey table");
        raw.glyphs.into_iter().filter_map(|(k, v)| k.chars().next().map(|c| (c, v))).collect()
    })
}

/// Gap between consecutive characters, and the width of a literal space —
/// both in the table's own raw units. `svg2rm`'s constants, kept as-is so
/// the two implementations stay comparable.
const GAP: f64 = 25.0;
const SPACE: f64 = 60.0;

/// The table's own vertical reference points, in its raw (Y-up, baseline
/// near 25) units — measured across all 94 glyphs: `(` and `)` reach the
/// highest at 114.3, true descenders (`g`, `y`, `p`, …) bottom out at 0,
/// and non-descending glyphs (`H`, `o`, comma, semicolon, …) all bottom out
/// at 25, which is what pins the baseline there. `TOP` is rounded up a
/// touch past the tallest glyph actually seen, as a fixed font-wide
/// metric rather than "whatever the tallest character in this string
/// happens to be" — the same role `ascent()` plays for a real font.
const TOP: f64 = 115.0;
const BASELINE: f64 = 25.0;
/// Puts the table's own "100 units per line" convention (`svg2rm`'s
/// `normalize_rendering(100)`) on the same nominal scale
/// `markdown::RASTER_PX` uses for the CJK raster path, so a line mixing
/// Latin and CJK text doesn't come out with mismatched letter sizes —
/// neither script's code needs to know the other's unit system beyond
/// this one shared constant.
const SCALE: f64 = crate::rm::markdown::RASTER_PX as f64 / 100.0;

/// Lay out `text` — assumed one already-tokenized Latin word, no spaces —
/// as Hershey strokes, left-aligned from x=0. Mirrors `svg2rm`'s
/// `text_to_strokes_raw` glyph positioning exactly: each character's own
/// strokes (already left-edge-aligned per glyph in the bundled table),
/// offset by a running cursor that advances by that glyph's width plus
/// `GAP`.
///
/// Returns `(strokes, advance, baseline)` in `markdown.rs`'s `RunArt`
/// convention — origin top-left, Y increasing downward, on the shared
/// `RASTER_PX`-relative scale — so `Layout::art` can wrap this directly
/// into a `RunArt` with no further transform.
pub fn layout_word(text: &str) -> Option<(Vec<Poly>, f64, f64)> {
    let t = table();
    let mut cursor = 0.0f64;
    let mut strokes: Vec<Poly> = Vec::new();
    let mut any = false;

    for ch in text.chars() {
        let Some(g) = t.get(&ch) else {
            cursor += SPACE; // a character this table has no glyph for
            continue;
        };
        for s in &g.strokes {
            strokes.push(s.iter().map(|&(x, y)| (x + cursor, y)).collect());
            any = true;
        }
        cursor += g.width + GAP;
    }
    if !any {
        return None;
    }

    let out: Vec<Poly> =
        strokes.into_iter().map(|s| s.into_iter().map(|(x, y)| (x * SCALE, (TOP - y) * SCALE)).collect()).collect();
    Some((out, cursor * SCALE, (TOP - BASELINE) * SCALE))
}
