//! Markdown -> pen strokes, laid out and drawn block by block.
//!
//! `draw.rs` traces a whole image as one raster: fine for a photo, but text
//! run through that path is what the README's TODO calls out as its known
//! failure — skeletonization reduces every stroke to 1px regardless of the
//! source's weight, and a whole page thrown into one low-resolution raster
//! merges a hanzi's strokes into a blob before tracing ever sees them. A
//! sibling project (`svg2rm`, not part of this repo) hit the same wall from
//! the Latin side first: it started with Hershey vector fonts — each glyph
//! *is* a handful of pre-drawn strokes, no rasterization needed — and found
//! on real hardware that they read badly at small sizes (a "t"'s crossbar is
//! a separate short stroke that renders too faint to read). It settled on
//! rasterize -> skeletonize -> trace instead: run a real font through a
//! rasterizer at a fixed high resolution, then feed the bitmap through the
//! same threshold/skeletonize/trace pipeline `draw.rs` uses for a whole
//! image — just on a canvas sized for one word or one character, where it
//! has the resolution to find a crossbar, instead of on a whole page, where
//! it doesn't.
//!
//! This module is that approach, ported to Rust and composed with an actual
//! markdown parser and a block layout:
//!
//! - **Text** is rasterized per run (a Latin word, laid out with the font's
//!   own metrics and kerning) or per character (every non-ASCII character
//!   gets its own isolated raster — "逐字", one at a time — so a cursive
//!   hand font's inter-character joins never cross into the tracer, whether
//!   or not adjacent characters end up touching on the page). Latin and
//!   non-ASCII runs use different bundled fonts, split at the script
//!   boundary, for the same reason `svg2rm` split them: mixed Chinese and
//!   English text should not render the English half in a Chinese
//!   calligraphy hand.
//! - **Tables** are drawn as computed grid lines — straight two-point
//!   polylines from arithmetic, not traced from anything. They're already
//!   vectors; running them through a raster tracer would only add noise.
//! - **Images** referenced by a local path reuse `imageproc`'s existing
//!   threshold -> skeletonize -> trace pipeline, the same one `draw.rs` runs
//!   on a dropped photo. That pipeline *is* a form of edge extraction for
//!   flat, icon-like art (binarize, then trace the boundary between ink and
//!   paper) — it is not a gradient-based detector like Canny, which would
//!   matter more for photographic tone than for the flat icons markdown
//!   tends to embed. Reusing the validated pipeline was chosen over adding a
//!   second, unvalidated one; see the module-level TODO in `mod.rs` if that
//!   trade needs revisiting.
//!
//! Everything above composes into one `Vec<Poly>` in final page-pixel
//! coordinates — the layout places each block's strokes directly at its
//! on-page position and size, so unlike `draw.rs`'s image tracer there is no
//! separate raster-to-page `placement` step and no band-reordering pass:
//! the blocks are already emitted top to bottom, in reading order, because
//! that's the order the layout walks them in. `draw::plan_from_page_strokes`
//! and `draw::page_from_page_strokes` turn that final list into pen-replay
//! events or a `.rm` page exactly as they do for a traced image, which is
//! the whole reason those two entry points were split out of `draw.rs`.

use crate::rm::device::Calib;
use crate::rm::draw::{self, MAX_SPUR, Page, Plan, Poly};
use crate::rm::imageproc;
use ab_glyph::{point, Font, FontRef, Glyph, OutlinedGlyph, ScaleFont};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::collections::HashMap;
use std::sync::OnceLock;

// ————— fonts —————
//
// Latin text doesn't have a bundled TTF any more — it goes through
// `hershey::layout_word` instead, straight to vector strokes with nothing
// to rasterize. See that module's doc for why. Only CJK still rasterizes a
// real font.

/// Cursive running-script Chinese hand (SIL OFL — see
/// `resources/fonts/ZhiMangXing-OFL.txt`). Its connected joins between
/// characters would be a liability for a font run rasterized as one strip
/// and traced whole, which is exactly why non-ASCII text is rasterized one
/// character at a time instead (see the module doc): each character sits
/// alone on its own raster, so there is nothing for it to join *into*.
/// `MaShanZheng-Regular.ttf`, also in `resources/fonts/`, is a tidier
/// alternative — swap the path below to try it.
const CJK_FONT_BYTES: &[u8] = include_bytes!("../../resources/fonts/ZhiMangXing-Regular.ttf");

/// System CJK fonts to prefer over the bundled calligraphy hand, in order,
/// as `(path, TrueType-collection face index)`. Real hardware testing
/// (see the module doc) found that a cursive, connected-stroke hand —
/// which is what makes it look handwritten — skeletonizes into the same
/// kind of fragile, many-short-curve strokes that failed for Latin cursive
/// fonts too, and that the fix which worked there (fewer, straighter
/// strokes) carried over: a plain print typeface reads far more reliably.
///
/// Hiragino Sans GB W6 is first because it won a five-way, same-page,
/// same-hardware comparison against STHeiti Light, STHeiti Medium,
/// Hiragino Sans GB W3 and Songti — a heavier weight's thicker, more
/// deliberate strokes held up better than the lighter/serif alternatives.
/// The others stay as fallbacks, in the order they were runners-up, for
/// whichever of these isn't installed.
const SYSTEM_CJK_CANDIDATES: &[(&str, u32)] = &[
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 1), // W6
    ("/System/Library/Fonts/STHeiti Medium.ttc", 0),
    ("/System/Library/Fonts/STHeiti Light.ttc", 0),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0), // W3
    ("/System/Library/Fonts/Supplemental/Songti.ttc", 0),
];

fn cjk_font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| {
        // System fonts are read from disk at runtime rather than embedded:
        // Apple's system fonts are licensed for on-device rendering, not
        // for a third party to bundle their raw bytes into an app binary
        // and redistribute — reading the copy already on the user's own
        // Mac, the way any application uses a system font, doesn't have
        // that problem. `ZhiMangXing`, below, *is* bundled — it's SIL OFL
        // licensed, which explicitly permits that.
        for &(path, index) in SYSTEM_CJK_CANDIDATES {
            let Ok(data) = std::fs::read(path) else { continue };
            let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
            if let Ok(font) = FontRef::try_from_slice_and_index(leaked, index) {
                return font;
            }
        }
        FontRef::try_from_slice(CJK_FONT_BYTES).expect("bundled fallback font")
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Script {
    Latin,
    Cjk,
}

fn script_of(c: char) -> Script {
    if c.is_ascii() {
        Script::Latin
    } else {
        Script::Cjk
    }
}

// ————— rasterizing a run through a real font (CJK only) —————

/// Working resolution every run is rasterized at, in pixels of font height,
/// before its traced strokes are scaled down to whatever size the layout
/// actually wants on the page. High enough that a crossbar or a dense
/// hanzi's strokes survive binarization as separate ink instead of merging
/// into a blob — matches `svg2rm`'s reference implementation's
/// `render_px=200`, which was tuned against the same failure on real
/// hardware. See `draw::BASE_WORK`'s doc comment for the whole-image version
/// of this same problem.
pub(crate) const RASTER_PX: f32 = 200.0;
/// Barbs this short, hanging off a stroke junction, are skeletonizing
/// artefacts rather than marks — reusing `draw.rs`'s `MAX_SPUR` rather than
/// picking a second number, since a glyph raster and a page raster are
/// pruned by the same rule for the same reason.
const GLYPH_MAX_SPUR: usize = MAX_SPUR;
/// How far a traced stroke may drift from the point it replaces, in raster
/// pixels — well under both what a 200px-tall glyph shows and what the
/// eventual on-page scale-down will show, so this is free simplification,
/// not a visible concession. Runs `imageproc::simplify` in the run's own
/// local raster frame rather than after placement, because a run's raster
/// is a fixed size (`RASTER_PX`) regardless of what final size it lands at,
/// so one epsilon here is correct for every block kind that uses it.
const GLYPH_EPSILON: f64 = 0.9;

/// How far apart, in final page pixels, two consecutive points of a placed
/// stroke may be before `place` inserts more between them.
///
/// `device::stroke_events` already re-samples every stroke at a fixed
/// 40-digitizer-unit step — roughly 5.6 page pixels on Paper Pro — and that
/// is fine for a stroke traced from a whole photo, which is usually long
/// relative to the gap. A glyph stroke is not: a single stroke inside a
/// hanzi, or a digit's stroke, can be only a few dozen page pixels long
/// *in total*, and at native spacing that same 5.6px step is coarse enough
/// relative to the stroke's own size to render on real hardware as
/// visibly separate dabs rather than one line — confirmed by pushing the
/// same text at several point densities to a real Paper Pro and comparing
/// results on the same page. Re-densifying here, before the strokes ever
/// reach `device::stroke_events`, fixes it without changing that shared
/// step for every other drawing mode.
const MAX_POINT_GAP: f64 = 4.0;

/// Insert points so no two consecutive ones in `stroke` are farther apart
/// than `max_gap` — see `MAX_POINT_GAP`.
fn densify(stroke: &[(f64, f64)], max_gap: f64) -> Poly {
    let mut out = Vec::with_capacity(stroke.len());
    for pair in stroke.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        out.push((x0, y0));
        let dist = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let n = (dist / max_gap).ceil().max(1.0) as usize;
        for k in 1..n {
            let t = k as f64 / n as f64;
            out.push((x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
        }
    }
    if let Some(&last) = stroke.last() {
        out.push(last);
    }
    out
}

/// One rasterized, traced run: strokes in the run's own local raster-pixel
/// frame (origin top-left), plus the two numbers layout needs to place it —
/// how far the cursor should advance after it, and where its baseline sits,
/// both in that same local frame.
struct RunArt {
    strokes: Vec<Poly>,
    advance: f64,
    /// Distance from the raster's top edge down to the font's baseline.
    /// Mixed-script text shares a baseline, not a top edge — a CJK
    /// character has no descender the way a Latin "g" does, so aligning by
    /// top edge would float every hanzi above the Latin text around it.
    baseline: f64,
}

/// Rasterize `text` (assumed one script — the caller has already split at
/// script boundaries) through `font` at `RASTER_PX`, using the font's own
/// glyph metrics and kerning table for layout — the real thing `svg2rm`'s
/// Python reference got from PIL's text layout, here from `ab_glyph`'s
/// documented `scaled_glyph`/`kern`/`h_advance` idiom.
fn rasterize_run(font: &FontRef<'static>, text: &str) -> Option<(Vec<u8>, usize, usize, f64, f64)> {
    let scaled = font.as_scaled(RASTER_PX);
    let mut caret = point(0.0, scaled.ascent());
    let mut outlines: Vec<OutlinedGlyph> = Vec::new();
    let mut last: Option<Glyph> = None;
    for c in text.chars() {
        let mut g = scaled.scaled_glyph(c);
        if let Some(prev) = last.take() {
            caret.x += scaled.kern(prev.id, g.id);
        }
        g.position = caret;
        caret.x += scaled.h_advance(g.id);
        last = Some(g.clone());
        if let Some(outlined) = font.outline_glyph(g) {
            outlines.push(outlined);
        }
    }
    if caret.x <= 0.0 {
        return None; // a run of whitespace, or characters the font has no glyph for
    }

    let width = caret.x.ceil().max(1.0) as usize;
    let height = scaled.height().ceil().max(1.0) as usize;
    let mut gray = vec![255u8; width * height];
    for outlined in outlines {
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                let idx = py as usize * width + px as usize;
                let ink = (255.0 - coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                gray[idx] = gray[idx].min(ink); // darkest of any overlapping glyph wins
            }
        });
    }
    Some((gray, width, height, caret.x as f64, scaled.ascent() as f64))
}

/// Rasterize, threshold, skeletonize and trace one run, in its own local
/// raster frame. `None` for a run with no visible ink (whitespace, or a
/// character missing from the font) — the caller advances the cursor by a
/// fixed space width in that case rather than placing nothing.
fn run_art(font: &FontRef<'static>, text: &str) -> Option<RunArt> {
    let (gray, w, h, advance, baseline) = rasterize_run(font, text)?;
    // Plain Otsu, not `imageproc::adaptive_threshold_mask`: that variant
    // exists to flatten a lighting gradient across a *photographed* page.
    // This raster was drawn by us on a flat white background, so there is
    // no gradient to correct, and the adaptive version's tiling pass would
    // be pure overhead on something this small.
    let mask = imageproc::threshold_mask(&gray, w, h, imageproc::otsu_threshold(&gray));
    let skeleton = imageproc::prune_spurs(&imageproc::skeletonize(&mask), GLYPH_MAX_SPUR);
    let strokes: Vec<Poly> = imageproc::trace_skeleton(&skeleton)
        .into_iter()
        .map(|s| {
            imageproc::simplify(
                &s.into_iter().map(|(x, y)| (x as f64, y as f64)).collect::<Vec<_>>(),
                GLYPH_EPSILON,
            )
        })
        .filter(|s: &Poly| s.len() >= 2)
        .collect();
    if strokes.is_empty() {
        return None;
    }
    Some(RunArt { strokes, advance, baseline })
}

// ————— tokenizing text into script-homogeneous units —————

/// One layout unit: a Latin word (rasterized and traced together, so the
/// font's own kerning applies within it), a single non-ASCII character
/// (rasterized alone — see the module doc for why), or a gap with no ink of
/// its own.
enum Token {
    Word(String, Script),
    Space,
}

/// Split `text` into words and gaps, splitting at every script boundary as
/// well as at whitespace. A run of non-ASCII characters becomes one `Word`
/// token *per character* — not one token for the whole run — which is what
/// makes rasterization "逐字": the tokenizer, not the rasterizer, is what
/// enforces one-character-at-a-time, so nothing downstream needs to know
/// that CJK is special.
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let flush = |word: &mut String, tokens: &mut Vec<Token>| {
        if !word.is_empty() {
            tokens.push(Token::Word(std::mem::take(word), Script::Latin));
        }
    };
    for c in text.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut tokens);
            if !matches!(tokens.last(), Some(Token::Space) | None) {
                tokens.push(Token::Space);
            }
        } else if script_of(c) == Script::Cjk {
            flush(&mut word, &mut tokens);
            tokens.push(Token::Word(c.to_string(), Script::Cjk));
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut tokens);
    tokens
}

// ————— page layout —————

/// Font size, as a fraction of the page's own height, for body text — so
/// the same markdown looks the same relative size on an rM2 and a Paper
/// Pro even though their panels are different physical resolutions. Chosen
/// to fit roughly 40 lines of body text top to bottom, which is a normal
/// page density for a document meant to be read rather than glanced at.
const BODY_FRAC: f64 = 1.0 / 46.0;
/// Line pitch as a multiple of font size — standard single-spaced prose.
const LINE_SPACING: f64 = 1.55;
/// Font size multiplier per heading level, h1 first. Chosen by eye against
/// a real page rather than any typesetting scale: big enough that h1 reads
/// as a title from across a desk, tight enough by h4 that "just emphasized
/// paragraph text" is what it actually looks like.
const HEADING_SCALE: [f64; 6] = [2.0, 1.7, 1.4, 1.2, 1.1, 1.0];
/// Extra vertical gap above a block, as a multiple of *its own* font size —
/// paragraphs need less air between them than a heading needs above it.
const BLOCK_GAP: f64 = 0.5;
const HEADING_GAP: f64 = 0.9;
/// Table cell text, relative to `BODY_FRAC` — slightly smaller so a row of
/// three or four cells still fits the page width without wrapping every
/// cell.
const CELL_SCALE: f64 = 0.82;
/// Width of an inter-word gap, as a fraction of the line's font size.
const SPACE_FRAC: f64 = 0.32;
/// Left indent for list items and code blocks, as a fraction of content
/// width.
const INDENT_FRAC: f64 = 0.06;

/// Blank margin kept clear on every edge, as a fraction of the page —
/// bigger than `draw::MARGIN`'s 10%, and deliberately so. `draw::MARGIN`'s
/// safety number was measured against a *traced photograph*: organic,
/// curved strokes scattered across the page, none of which run dead
/// straight along the very edge of the safe zone. This module can produce
/// exactly that shape — a heading's underline or a table's border is a
/// single straight stroke spanning nearly the whole content width, right
/// next to the margin boundary — and a straight edge-hugging stroke has a
/// categorically higher chance of clipping a still-expanded or
/// oddly-docked toolbar than a curve ever does. Confirmed the hard way: an
/// early version of this margin, borrowed straight from `draw::MARGIN`,
/// produced a page that visibly rotated to landscape mid-draw on real
/// hardware — the exact failure `draw.rs`'s own `MARGIN` doc comment
/// describes from a full-bleed image test. This number is the fix, not a
/// tuned optimum; it has not been pushed back down to find the real edge.
const PAGE_MARGIN: f64 = 0.14;

/// Everything the layout needs about the page it's writing onto, plus the
/// strokes it has produced so far.
struct Layout<'a> {
    calib: &'a Calib,
    left: f64,
    right: f64,
    /// Vertical write position — always the *top* of the next line, not a
    /// baseline; each block computes its own baseline from its own font
    /// size when it places text.
    y: f64,
    strokes: Vec<Poly>,
    cache: HashMap<(Script, String), Option<RunArt>>,
    /// See `MAX_POINT_GAP` — a field rather than only a constant so
    /// `debug_layout_with_gap` can sweep it for comparison without
    /// rebuilding, which is how it was tuned against real hardware.
    max_point_gap: f64,
    /// The CJK font, likewise a field rather than only ever `cjk_font()` —
    /// `debug_layout_with_gap_and_font` swept *this* against real hardware
    /// too, which is how `SYSTEM_CJK_CANDIDATES`'s current order was
    /// chosen.
    cjk_font: &'a FontRef<'static>,
}

impl<'a> Layout<'a> {
    fn with_gap_and_font(calib: &'a Calib, max_point_gap: f64, cjk_font: &'a FontRef<'static>) -> Self {
        let left = calib.screen_w * PAGE_MARGIN;
        let right = calib.screen_w * (1.0 - PAGE_MARGIN);
        Layout {
            calib,
            left,
            right,
            y: calib.screen_h * PAGE_MARGIN,
            strokes: Vec::new(),
            cache: HashMap::new(),
            max_point_gap,
            cjk_font,
        }
    }

    fn content_width(&self) -> f64 {
        self.right - self.left
    }

    /// Whether anything more will fit before the bottom margin — callers
    /// stop adding blocks once this goes false rather than spill text off
    /// the page, which the pen can't draw anyway (see `draw.rs`'s `MARGIN`
    /// doc comment on why the toolbar's dock strip has to stay clear).
    fn has_room(&self, needed: f64) -> bool {
        self.y + needed <= self.calib.screen_h * (1.0 - PAGE_MARGIN)
    }

    fn art(&mut self, script: Script, text: &str) -> Option<&RunArt> {
        self.cache
            .entry((script, text.to_string()))
            .or_insert_with(|| match script {
                // Vector strokes straight from the table — see hershey.rs's
                // module doc for why Latin text no longer rasterizes at all.
                Script::Latin => {
                    crate::rm::hershey::layout_word(text).map(|(strokes, advance, baseline)| RunArt {
                        strokes,
                        advance,
                        baseline,
                    })
                }
                Script::Cjk => run_art(self.cjk_font, text),
            })
            .as_ref()
    }

    /// Place one run's strokes at `(x, y)` in page pixels, scaled from its
    /// `RASTER_PX` raster down to `font_px`, aligned so `art.baseline`
    /// lands at `baseline_y`.
    fn place(&mut self, art_strokes: &[Poly], baseline_raster: f64, x: f64, baseline_y: f64, font_px: f64) {
        let scale = font_px / RASTER_PX as f64;
        for stroke in art_strokes {
            let placed: Poly = stroke
                .iter()
                .map(|&(rx, ry)| (x + rx * scale, baseline_y + (ry - baseline_raster) * scale))
                .collect();
            self.strokes.push(densify(&placed, self.max_point_gap));
        }
    }

    /// Lay out `text` as word-wrapped lines at `font_px`, starting at an
    /// explicit `(left, top)` in page pixels. Pure with respect to
    /// `self.y` — it only ever reads `self`'s cache and writes `self`'s
    /// stroke list — and returns how tall the wrapped block came out, so a
    /// caller with its own idea of the vertical cursor (a table cell,
    /// which shares one row with its neighbours rather than owning the
    /// flow) can place several of these at the *same* top without each
    /// call pushing the next one down a line.
    fn flow_text_at(&mut self, text: &str, font_px: f64, left: f64, top: f64) -> f64 {
        let right = self.right;
        let line_h = font_px * LINE_SPACING;
        let space_w = font_px as f64 * SPACE_FRAC;

        let mut cursor_x = left;
        let mut baseline_y = top + font_px; // first line's baseline
        let mut wrote_on_line = false;

        for token in tokenize(text) {
            match token {
                Token::Space => {
                    if wrote_on_line {
                        cursor_x += space_w;
                    }
                }
                Token::Word(word, script) => {
                    let Some((strokes, baseline, advance)) = self.art(script, &word).map(|a| {
                        (a.strokes.clone(), a.baseline, a.advance)
                    }) else {
                        continue; // no ink for this word (e.g. unsupported glyph) — skip, don't stall the line
                    };
                    let scale = font_px / RASTER_PX as f64;
                    let width = advance * scale;
                    if wrote_on_line && cursor_x + width > right {
                        cursor_x = left;
                        baseline_y += line_h;
                        wrote_on_line = false;
                    }
                    self.place(&strokes, baseline, cursor_x, baseline_y, font_px);
                    cursor_x += width;
                    wrote_on_line = true;
                }
            }
        }
        if wrote_on_line { baseline_y - font_px + line_h - top } else { 0.0 }
    }

    /// Lay out `text` at the current cursor, indented from the left
    /// margin, and advance the cursor past it — the ordinary case, used by
    /// every block except a table cell (`layout_table` calls
    /// `flow_text_at` directly so several cells can share one row's top).
    fn flow_text(&mut self, text: &str, font_px: f64, indent: f64) {
        let h = self.flow_text_at(text, font_px, self.left + indent, self.y);
        self.y += h;
    }

    /// A straight ruled line — used for table grids and heading underlines.
    /// Vector from the start; no rasterization or tracing involved.
    fn rule(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.strokes.push(vec![(x0, y0), (x1, y1)]);
    }

    /// A small hollow circle, for a bullet — geometry, not a font glyph, so
    /// it can't come out blank because a handwriting font happens not to
    /// include "•" (several of the bundled Google Fonts hand styles don't).
    fn bullet(&mut self, cx: f64, cy: f64, r: f64) {
        const SEGMENTS: usize = 14;
        let ring: Poly = (0..=SEGMENTS)
            .map(|i| {
                let a = i as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.strokes.push(ring);
    }
}

// ————— markdown -> layout —————

#[derive(Default)]
struct ListState {
    ordered: Option<u64>,
    depth: usize,
}

/// Walk a stream of inline events (`Text`/`Code`, with formatting tags
/// ignored) and concatenate their text — v1 renders emphasis and inline
/// code in the same hand as everything else rather than carrying a second
/// font or a distinguishing mark, which is a real simplification, not an
/// oversight: nothing downstream distinguishes bold from plain.
fn collect_inline_text<'e>(events: &mut std::iter::Peekable<impl Iterator<Item = Event<'e>>>, end_at: TagEnd) -> String {
    let mut out = String::new();
    for event in events.by_ref() {
        match event {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(t) if t == end_at => break,
            _ => {}
        }
    }
    out
}

/// Parse `markdown` and lay it out onto `calib`'s page, producing one
/// ordered list of strokes in page-pixel coordinates — top to bottom,
/// because that's the order the block walk below emits them in.
fn layout(markdown: &str, calib: &Calib) -> Result<Vec<Poly>, String> {
    layout_with_gap(markdown, calib, MAX_POINT_GAP)
}

fn layout_with_gap(markdown: &str, calib: &Calib, max_point_gap: f64) -> Result<Vec<Poly>, String> {
    layout_with_gap_and_font(markdown, calib, max_point_gap, cjk_font())
}

fn layout_with_gap_and_font(
    markdown: &str,
    calib: &Calib,
    max_point_gap: f64,
    cjk: &FontRef<'static>,
) -> Result<Vec<Poly>, String> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let mut events = Parser::new_ext(markdown, opts).peekable();

    let mut lo = Layout::with_gap_and_font(calib, max_point_gap, cjk);
    let body_px = calib.screen_h * BODY_FRAC;
    let mut list_stack: Vec<ListState> = Vec::new();

    while let Some(event) = events.next() {
        if !lo.has_room(body_px * LINE_SPACING) {
            break; // out of page — draw what fits rather than fail the whole document
        }
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let text = collect_inline_text(&mut events, TagEnd::Heading(level));
                let scale = HEADING_SCALE[heading_index(level)];
                let font_px = body_px * scale;
                lo.y += font_px * HEADING_GAP;
                lo.flow_text(&text, font_px, 0.0);
                // No underline rule here on purpose, even for h1/h2, even
                // though it would read well: a heading is the first thing on
                // the page, so its underline would be a dead-straight stroke
                // spanning nearly the full content width very close to the
                // top margin — exactly the shape that clipped a toolbar and
                // rotated the page on real hardware during development (see
                // `PAGE_MARGIN`'s doc comment). `layout_table`'s grid lines
                // are the same *shape* of risk but appear lower on the page,
                // clear of the toolbar's usual docks, which is why they stay.
                lo.y += font_px * HEADING_GAP;
            }
            Event::Start(Tag::Paragraph) => {
                let text = collect_inline_text(&mut events, TagEnd::Paragraph);
                lo.y += body_px * BLOCK_GAP;
                lo.flow_text(&text, body_px, 0.0);
                lo.y += body_px * BLOCK_GAP;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                let text = collect_inline_text(&mut events, TagEnd::CodeBlock);
                lo.y += body_px * BLOCK_GAP;
                let indent = lo.content_width() * INDENT_FRAC;
                let bar_x = lo.left + indent * 0.35;
                let top = lo.y;
                for line in text.lines() {
                    if !lo.has_room(body_px * LINE_SPACING) {
                        break;
                    }
                    lo.flow_text(line, body_px * 0.92, indent);
                }
                lo.rule(bar_x, top, bar_x, lo.y.max(top + body_px));
                lo.y += body_px * BLOCK_GAP;
            }
            Event::Start(Tag::List(first)) => {
                list_stack.push(ListState { ordered: first, depth: list_stack.len() });
            }
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
            }
            Event::Start(Tag::Item) => {
                let text = collect_inline_text(&mut events, TagEnd::Item);
                let depth = list_stack.last().map(|s| s.depth).unwrap_or(0) as f64;
                let indent = lo.content_width() * INDENT_FRAC * (depth + 1.0);
                let marker_x = lo.left + indent - lo.content_width() * INDENT_FRAC * 0.55;
                let baseline_y = lo.y + body_px;
                if let Some(state) = list_stack.last_mut() {
                    match &mut state.ordered {
                        Some(n) => {
                            let label = format!("{n}.");
                            *n += 1;
                            if let Some(art) = lo.art(Script::Latin, &label).map(|a| (a.strokes.clone(), a.baseline)) {
                                lo.place(&art.0, art.1, marker_x, baseline_y, body_px);
                            }
                        }
                        None => lo.bullet(marker_x + body_px * 0.15, baseline_y - body_px * 0.32, body_px * 0.11),
                    }
                }
                lo.flow_text(&text, body_px, indent);
                lo.y += body_px * (BLOCK_GAP * 0.4);
            }
            Event::Start(Tag::Table(_)) => layout_table(&mut events, &mut lo, body_px),
            Event::Start(Tag::Image { dest_url, .. }) => {
                // Alt text lives inside the Image span as Text events;
                // consumed so it doesn't leak into the surrounding flow if
                // the image itself fails to load.
                let _alt = collect_inline_text(&mut events, TagEnd::Image);
                if let Err(e) = layout_image(&mut lo, &dest_url, body_px) {
                    eprintln!("[rm-bin] markdown: skipping image {dest_url:?}: {e}");
                }
            }
            _ => {}
        }
    }

    Ok(lo.strokes)
}

fn heading_index(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 0,
        HeadingLevel::H2 => 1,
        HeadingLevel::H3 => 2,
        HeadingLevel::H4 => 3,
        HeadingLevel::H5 => 4,
        HeadingLevel::H6 => 5,
    }
}

/// Cell text, plain — table cells don't get inline-formatting handling
/// beyond what `collect_inline_text` already gives every other block.
fn layout_table<'e>(events: &mut std::iter::Peekable<impl Iterator<Item = Event<'e>>>, lo: &mut Layout, body_px: f64) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut in_row = false;
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::TableRow) | Event::Start(Tag::TableHead) => in_row = true,
            Event::End(TagEnd::TableRow) | Event::End(TagEnd::TableHead) => {
                in_row = false;
                rows.push(std::mem::take(&mut row));
            }
            Event::Start(Tag::TableCell) if in_row => {
                row.push(collect_inline_text(events, TagEnd::TableCell));
            }
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    if rows.is_empty() {
        return;
    }

    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let cell_px = body_px * CELL_SCALE;
    let row_h = cell_px * LINE_SPACING * 1.15;
    let col_w = lo.content_width() / cols as f64;
    let pad = col_w * 0.06;

    lo.y += body_px * BLOCK_GAP;
    let top = lo.y;
    let mut row_top = top;
    let mut header = true;
    for r in &rows {
        if !lo.has_room(row_h) {
            break;
        }
        // Every cell in this row starts from the *same* `row_top` — unlike
        // `flow_text`, `flow_text_at` doesn't advance any shared cursor, so
        // laying out several cells side by side doesn't push each one down
        // a line past the last, which `flow_text` would (see its doc
        // comment). A cell that wraps past one line will still overlap the
        // row below — v1 keeps a fixed `row_h` rather than growing rows to
        // fit, which is a real limitation for a long cell, not an oversight.
        for (c, text) in r.iter().enumerate() {
            let x = lo.left + col_w * c as f64 + pad;
            lo.flow_text_at(text, cell_px, x, row_top);
        }
        row_top += row_h;
        if header {
            lo.rule(lo.left, row_top - row_h * 0.08, lo.right, row_top - row_h * 0.08);
            header = false;
        }
    }
    lo.y = row_top;
    let bottom = row_top;
    for c in 0..=cols {
        let x = lo.left + col_w * c as f64;
        lo.rule(x, top, x, bottom);
    }
    lo.rule(lo.left, top, lo.right, top);
    lo.rule(lo.left, bottom, lo.right, bottom);
    lo.y += body_px * BLOCK_GAP;
}

/// Trace a locally-referenced image and place it inline, reusing the exact
/// pipeline `draw.rs` runs on a whole dropped image — see the module doc
/// for why that counts as this block's "grayscale edge detection".
/// `dest_url` is treated as a filesystem path, with an optional `file://`
/// prefix stripped; markdown pointing at a remote URL is reported and
/// skipped rather than fetched, since this module draws what's on disk and
/// doesn't reach onto the network to do it.
fn layout_image(lo: &mut Layout, dest_url: &str, body_px: f64) -> Result<(), String> {
    let path = dest_url.strip_prefix("file://").unwrap_or(dest_url);
    if path.contains("://") {
        return Err(format!("not a local path: {dest_url}"));
    }

    let img = image::open(path).map_err(|e| e.to_string())?;
    let (src_w, src_h) = (img.width() as f64, img.height() as f64);
    if src_w == 0.0 || src_h == 0.0 {
        return Err("empty image".into());
    }
    let max_w = lo.content_width();
    let max_h = lo.calib.screen_h * 0.4; // an image shouldn't eat the whole page
    let scale = (max_w / src_w).min(max_h / src_h).min(1.0);
    let (w, h) = ((src_w * scale).max(1.0) as u32, (src_h * scale).max(1.0) as u32);

    lo.y += body_px * BLOCK_GAP;
    if !lo.has_room(h as f64) {
        return Err("no room left on the page".into());
    }

    let gray = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3).to_luma8();
    let mask = imageproc::adaptive_threshold_mask(gray.as_raw(), w as usize, h as usize);
    let skeleton = imageproc::prune_spurs(&imageproc::skeletonize(&mask), MAX_SPUR);
    let ox = lo.left + (max_w - w as f64) / 2.0;
    let oy = lo.y;
    for stroke in imageproc::trace_skeleton(&skeleton) {
        let placed: Poly = imageproc::simplify(
            &stroke.into_iter().map(|(x, y)| (x as f64, y as f64)).collect::<Vec<_>>(),
            draw::EPSILON,
        )
        .into_iter()
        .map(|(x, y)| (ox + x, oy + y))
        .collect();
        if placed.len() >= 2 {
            lo.strokes.push(placed);
        }
    }
    lo.y += h as f64 + body_px * BLOCK_GAP;
    Ok(())
}

/// The raw final-page-pixel strokes, before `draw::to_preview`'s
/// window-sized simplification — for tooling (`preview_markdown`) that
/// wants to inspect what actually gets drawn rather than what the little
/// floating window's preview simplifies it down to.
#[doc(hidden)]
pub fn debug_layout(markdown: &str, calib: &Calib) -> Result<Vec<Poly>, String> {
    layout(markdown, calib)
}

/// Same as `debug_layout`, with `MAX_POINT_GAP` overridden — for sweeping
/// that constant against real hardware without a rebuild per value.
#[doc(hidden)]
pub fn debug_layout_with_gap(markdown: &str, calib: &Calib, max_point_gap: f64) -> Result<Vec<Poly>, String> {
    layout_with_gap(markdown, calib, max_point_gap)
}

/// Same, with the CJK font also overridden — loaded fresh from `path` each
/// call (no caching; this is a comparison tool, not a hot path) at
/// TrueType-collection face `index`. See `SYSTEM_CJK_CANDIDATES`.
#[doc(hidden)]
pub fn debug_layout_with_gap_and_font(
    markdown: &str,
    calib: &Calib,
    max_point_gap: f64,
    path: &str,
    index: u32,
) -> Result<Vec<Poly>, String> {
    let data = std::fs::read(path).map_err(|e| format!("can't read {path}: {e}"))?;
    let font = FontRef::try_from_slice_and_index(Box::leak(data.into_boxed_slice()), index)
        .map_err(|e| format!("can't parse {path} face {index}: {e:?}"))?;
    layout_with_gap_and_font(markdown, calib, max_point_gap, &font)
}

/// Lay `markdown` out on `calib`'s page and turn it into pen-replay events,
/// exactly as `draw::plan` does for a traced image.
pub fn plan(markdown: &str, calib: &Calib) -> Result<Plan, String> {
    let strokes = layout(markdown, calib)?;
    if strokes.is_empty() {
        return Err("这份 markdown 里没有可以画的内容".into());
    }
    Ok(draw::plan_from_page_strokes(&strokes, calib))
}

/// Lay `markdown` out on `calib`'s page and turn it into a `.rm` page,
/// exactly as `draw::page` does for a traced image.
pub fn page(markdown: &str, calib: &Calib) -> Result<Page, String> {
    let strokes = layout(markdown, calib)?;
    if strokes.is_empty() {
        return Err("这份 markdown 里没有可以画的内容".into());
    }
    Ok(draw::page_from_page_strokes(&strokes, calib))
}
