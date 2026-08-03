//! Image -> pen strokes via contour tracing, instead of via skeleton
//! tracing — `draw.rs`'s counterpart for flat, graphic images rather than
//! photographs or line art.
//!
//! `draw.rs` reduces an image to a 1px centreline and walks that: right for
//! a photograph reduced to line art, or for handwriting, where the *stroke*
//! is the thing that was drawn. It is the wrong tool for a flat, high-
//! contrast image — a logo, an icon, a screenshot of solid UI — because a
//! filled shape has no centreline that means anything; skeletonizing a
//! solid black square finds a meaningless spine down its middle instead of
//! the square's actual edges.
//!
//! Ported from a JavaScript reference (github.com/btk/vectorizer, not part
//! of this repo), which wraps the `potrace` posterize algorithm and then
//! spends most of its own code re-deriving each traced region's original
//! colour via nearest-colour matching and k-means quantisation — real work,
//! but for a *screen full of colour swatches*, not for what this app draws.
//! A reMarkable page is one colour: black ink on white paper. So this module
//! keeps only the part of that pipeline that still means something once
//! colour is off the table — binarize, then trace the boundary between ink
//! and paper — implemented against `vtracer` (a maintained Rust port of the
//! same underlying algorithm, Visioncortex's clustering tracer) rather than
//! reimplemented by hand: contour tracing is a well-covered problem, and
//! there is nothing about it specific enough to this app to be worth
//! re-deriving.
//!
//! The output is a genuinely different *kind* of ink than `draw.rs`
//! produces. A skeleton-traced photo draws its subject's *lines*; this
//! draws the *outline* of every region the threshold found — a silhouette,
//! not a sketch. For the graphic-flat images it suits, that outline is a
//! closer match to the source than a centreline would ever be, because
//! there was no centreline in the source to begin with.

use crate::rm::device::Calib;
use crate::rm::draw::{self, Page, Plan, Poly, EPSILON, MARGIN};
use crate::rm::imageproc;
use visioncortex::{CompoundPathElement, PathSimplifyMode};
use vtracer::{ColorImage, Config, Preset};

/// `vtracer`'s own corner/length/splice thresholds already do the real
/// simplification — they are potrace's tuning knobs, not something this
/// module has a more informed opinion about than the algorithm's own
/// defaults. `Polygon` rather than the preset's default `Spline` is the one
/// deliberate override: a straight-segment polyline is what
/// `device::stroke_events` wants directly, and a curve-fitted spline would
/// only have to be flattened back into one.
fn config() -> Config {
    Config { mode: PathSimplifyMode::Polygon, ..Config::from_preset(Preset::Bw) }
}

/// Regions smaller than this many pixels are treated as noise, not shape —
/// see `vtracer::Config::filter_speckle`, which this feeds. A JPEG's
/// compression artefacts alone would otherwise trace as hundreds of
/// one-pixel islands.
const MIN_REGION_PX: usize = 6;

/// A representative long edge for the raster contours are traced on — the
/// same role `draw::BASE_WORK` plays for skeleton tracing, and the same
/// number, so the two paths cost about the same to draw for a same-sized
/// source.
const WORK_PX: f64 = draw::BASE_WORK;

/// Trace `image_path`'s contours at a size that fits the page, in the
/// image's own pixel frame — mirrors `draw.rs`'s `page_fit`, which is
/// private to that module and tuned for the skeleton tracer's raster-vs-
/// source tradeoffs specifically, not reused directly here for that reason.
fn work_size(src_w: u32, src_h: u32) -> (u32, u32) {
    let long = src_w.max(src_h) as f64;
    let scale = (WORK_PX / long).min(1.0); // never upscale a small source
    (((src_w as f64) * scale).max(1.0) as u32, ((src_h as f64) * scale).max(1.0) as u32)
}

/// Binarize `image_path` and trace the boundary of every region found, as
/// polylines in that raster's own pixel frame (origin top-left).
fn trace(image_path: &str) -> Result<Vec<Poly>, String> {
    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (src_w, src_h) = (img.width(), img.height());
    if src_w == 0 || src_h == 0 {
        return Err("这张图是空的".into());
    }
    let (w, h) = work_size(src_w, src_h);
    let rgba = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3).to_rgba8();

    let color_img = ColorImage { pixels: rgba.into_raw(), width: w as usize, height: h as usize };
    let mut cfg = config();
    cfg.filter_speckle = MIN_REGION_PX;
    let svg = vtracer::convert(color_img, cfg)?;

    let mut strokes = Vec::new();
    for path in &svg.paths {
        for element in path.path.iter() {
            let pts: Vec<(f64, f64)> = match element {
                CompoundPathElement::PathI32(p) => p.path.iter().map(|pt| (pt.x as f64, pt.y as f64)).collect(),
                CompoundPathElement::PathF64(p) => p.path.iter().map(|pt| (pt.x, pt.y)).collect(),
                // Not produced by `PathSimplifyMode::Polygon`, but handled
                // rather than dropped in case a future config change makes
                // it possible — the spline's own control points are a
                // reasonable-enough polyline stand-in.
                CompoundPathElement::Spline(s) => s.points.iter().map(|pt| (pt.x, pt.y)).collect(),
            };
            if pts.len() >= 2 {
                strokes.push(imageproc::simplify(&pts, EPSILON));
            }
        }
    }
    if strokes.is_empty() {
        return Err("这张图里描不出任何轮廓——试试调高对比度，或者它本来就是空白的".into());
    }
    Ok(strokes)
}

/// Fit `(src_w, src_h)` uniformly into `calib`'s page with `MARGIN`,
/// centered — same shape as `draw.rs`'s private `placement`, kept as a
/// separate small copy rather than exposing that one, since the two
/// modules' notions of "the raster" differ (a cost-budgeted trace size
/// there, a fixed contour-tracing size here).
fn place(strokes: &[Poly], src_w: f64, src_h: f64, calib: &Calib) -> Vec<Poly> {
    let avail_w = calib.screen_w * (1.0 - 2.0 * MARGIN);
    let avail_h = calib.screen_h * (1.0 - 2.0 * MARGIN);
    let s = (avail_w / src_w).min(avail_h / src_h);
    let x0 = (calib.screen_w - src_w * s) / 2.0;
    let y0 = (calib.screen_h - src_h * s) / 2.0;
    strokes.iter().map(|stroke| stroke.iter().map(|&(x, y)| (x0 + x * s, y0 + y * s)).collect()).collect()
}

/// Trace `image_path`'s contours and lay them out centered on `calib`'s
/// page, as pen digitizer events to be replayed.
pub fn plan(image_path: &str, calib: &Calib) -> Result<Plan, String> {
    let raw = trace(image_path)?;
    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (w, h) = work_size(img.width(), img.height());
    let placed = place(&raw, w as f64, h as f64, calib);
    Ok(draw::plan_from_page_strokes(&placed, calib))
}

/// Trace `image_path`'s contours and lay them out as a `.rm` page.
pub fn page(image_path: &str, calib: &Calib) -> Result<Page, String> {
    let raw = trace(image_path)?;
    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (w, h) = work_size(img.width(), img.height());
    let placed = place(&raw, w as f64, h as f64, calib);
    Ok(draw::page_from_page_strokes(&placed, calib))
}

/// The final page-pixel strokes, before `draw::to_preview`'s window-sized
/// simplification — see `markdown::debug_layout`'s doc for why a tool that
/// wants to inspect what actually gets drawn reads this instead of
/// `Plan::preview`.
#[doc(hidden)]
pub fn debug_trace(image_path: &str, calib: &Calib) -> Result<Vec<Poly>, String> {
    let raw = trace(image_path)?;
    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (w, h) = work_size(img.width(), img.height());
    Ok(place(&raw, w as f64, h as f64, calib))
}
