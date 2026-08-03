//! Image -> pen strokes -> raw event bytes, ordered so the tablet inks the
//! page top-to-bottom.
//!
//! The tracing half (threshold -> skeletonize -> trace) is rm-agent's
//! pipeline, reused verbatim via `imageproc`. What's new here is *ordering*:
//! rm-agent emits strokes in whatever order the tracer found them, because
//! nothing was watching. rm-bin's window shows a grayscale scan sweeping down
//! the little screen, and that scan is supposed to be the same event as the
//! ink appearing on the tablet.
//!
//! The first attempt made the *drawing* follow the window: strokes were cut
//! into horizontal bands so a top-down wipe would stay honest. That cost
//! visible seams on hardware, and only ever bought an approximation. This
//! version inverts it — the window follows the *drawing*. Strokes go out
//! whole, and `Plan::preview` hands the frontend the same polylines in image
//! coordinates so it can ink them onto its little screen in the order and at
//! the rate the tablet is inking them. Nothing has to be cut, and the
//! correspondence stops being an approximation.
//!
//! Bands survive as a pure ordering heuristic: drawing roughly top-to-bottom
//! looks more deliberate than the order the tracer happened to find things
//! in. Nothing depends on it any more.

use crate::rm::device::{self, Calib};
use crate::rm::imageproc;
use crate::rm::rmfile;
use image::GenericImageView;

/// A representative long edge for the raster the tracer works on.
///
/// Not the tracing resolution itself, which `page_fit` derives per image and
/// per device — it depends on the page, and on whether the picture is portrait
/// or landscape, and lands somewhere between about 1100 and 1700. This is the
/// middle of that range, and it exists because `gemini`'s prompt describes the
/// machine its drawing is about to go through and has to quote a real number
/// when it says how small a detail will survive.
pub(crate) const BASE_WORK: f64 = 1300.0;

/// How far a simplified stroke may sit from the points it replaces, in raster
/// pixels — which, since the raster is now the size the drawing will be, are
/// screen pixels.
///
/// Under one pixel is beneath both what the panel resolves and what the pen's
/// own width covers, so this is invisible in the ink. What it buys is
/// substantial: a traced skeleton is a staircase with a point on every pixel,
/// and `stroke_events` re-samples whatever it is handed at the digitizer's
/// step — some five screen pixels — so the raw trace asks for five events
/// where one will do.
pub(crate) const EPSILON: f64 = 0.9;
/// Where simplifying stops being free. Past about three pixels the corners of
/// small glyphs start to round off, and by then it has stopped helping anyway:
/// once every segment is longer than the digitizer's step, the event count is
/// set by the total length of the ink and no amount of further simplification
/// moves it.
const EPSILON_MAX: f64 = 3.0;
/// Barbs of up to this many pixels hanging off a junction are skeletonizing
/// artefacts rather than marks. Real short strokes — the dot of an "i", a 点 —
/// don't touch a junction, and `prune_spurs` only cuts the ones that do.
pub(crate) const MAX_SPUR: usize = 4;
/// How far above its own resolution a source may be traced. See `page_fit`.
const MAX_UPSCALE: f64 = 2.0;

/// Roughly how many pen samples a drawing may cost.
///
/// This replaces a cap on traced points, which measured the wrong thing: the
/// tablet is handed events at a fixed pace, and the events are produced by
/// re-sampling the strokes at the digitizer's step, so what a drawing costs is
/// the *length of its ink*, not how many points described it. Capping points
/// meant a densely traced image was punished for its detail and then had that
/// detail destroyed to pay the bill.
const MAX_PEN_SAMPLES: usize = 24_000;
/// The `.rm` path writes a file instead of replaying a pen, so nothing is
/// paced and the only cost is the tablet's own rendering. Still finite —
/// xochitl gets sluggish on pages with a huge number of points.
const MAX_FILE_SAMPLES: usize = 120_000;

/// Blank margin left around the image, as a fraction of the screen.
///
/// This is a safety margin as much as an aesthetic one. xochitl's toolbar is
/// an overlay that eats pen input, so a stroke crossing it is a *button
/// press*, not a line — a full-bleed test card came out fine as ink and also
/// rotated the page to landscape, opened the overflow menu and added a page
/// on its way through. Expanded, the toolbar is ~5.6% of the screen on the
/// edge it's docked to (~7.7% when that's a long edge), and the user can dock
/// it to any of them; 10% clears it wherever it sits. `push`'s settle pause
/// is the other half of this.
pub(crate) const MARGIN: f64 = 0.10;
/// Horizontal slices the drawing order is quantised to. Coarser bands mean
/// longer left-to-right sweeps; finer ones mean the drawing marches down the
/// page more strictly.
const BANDS: usize = 64;
/// Preview coordinates are integers on a 0..N grid in the image's own frame —
/// N is well past what a ~100px window can resolve, and keeps each number to
/// four digits of JSON.
const PREVIEW_UNITS: u16 = 2000;
/// How far a preview stroke may sit from the stroke it stands for, as a
/// fraction of the image height. The window's screen is barely 100 pixels
/// across, so this is a third of a pixel there — invisible, and it takes a
/// long straight stroke down to the two points it deserves.
const PREVIEW_EPSILON: f64 = 1.0 / 250.0;

pub(crate) type Poly = Vec<(f64, f64)>;

/// One stroke as the frontend needs it: points in the image's own frame,
/// quantised to `PREVIEW_UNITS`.
pub type PreviewStroke = Vec<[u16; 2]>;

pub struct Plan {
    /// The stroke events, in drawing order. `device::push` adds the pen-down
    /// and pen-up around them.
    pub bytes: Vec<u8>,
    /// Cumulative byte offset after each stroke, ascending — the bridge
    /// between "bytes handed to the tablet" and "strokes inked".
    stroke_ends: Vec<usize>,
    /// The same strokes, in the same order, for the window to draw along with.
    pub preview: Vec<PreviewStroke>,
}

impl Plan {
    /// How many strokes the tablet has inked, given how many bytes have been
    /// handed to it — fractional, so the stroke currently under the pen is
    /// reported part-drawn and the window can ink it at the same rate rather
    /// than popping it in whole.
    pub fn strokes_done(&self, bytes: usize) -> f64 {
        let i = self.stroke_ends.partition_point(|&end| end <= bytes);
        let Some(&end) = self.stroke_ends.get(i) else {
            return self.stroke_ends.len() as f64;
        };
        let start = if i == 0 { 0 } else { self.stroke_ends[i - 1] };
        let span = end - start;
        let within = if span > 0 { (bytes - start) as f64 / span as f64 } else { 1.0 };
        i as f64 + within
    }

    pub fn stroke_count(&self) -> usize {
        self.stroke_ends.len()
    }
}

/// The raster to trace on: the size the image will actually occupy on the
/// page, in screen pixels.
///
/// This used to be a constant 700 on the long edge, which is roughly half the
/// panel — so every image was thrown away down to half resolution before
/// anything looked at it, and text was the casualty. Small type survives
/// binarization or it doesn't, and at half size it doesn't: adjacent strokes
/// of a hanzi land in the same pixel, merge, and skeletonize to a blob. There
/// is nothing further down the pipeline that can recover a glyph that was
/// already illegible in the mask.
///
/// The page's own size is the natural stopping point in the other direction.
/// Below it, detail is thrown away that the panel could have shown; above it,
/// detail is traced that the panel cannot show and the pen cannot draw, and
/// paid for in drawing time.
///
/// The source is the other ceiling. A small image blown up to page size has no
/// more detail in it than it started with, only more pixels describing the
/// same edges — so past a modest enlargement the extra raster is pure cost.
/// Some enlargement does help: it gives the resampler room to put an edge
/// between two source pixels instead of on one, and a diagonal traced at the
/// source's own resolution arrives as a staircase with steps you can see once
/// it is drawn at four times the size.
fn page_fit(src_w: u32, src_h: u32, calib: &Calib) -> (f64, f64) {
    let aspect = src_w as f64 / src_h as f64;
    let avail_w = calib.screen_w * (1.0 - 2.0 * MARGIN);
    let avail_h = calib.screen_h * (1.0 - 2.0 * MARGIN);
    // Same uniform fit `placement` will do, so the two agree and the scale
    // between raster pixels and screen pixels comes out at one.
    let w = avail_w.min(avail_h * aspect).min(MAX_UPSCALE * src_w as f64);
    (w.max(1.0), (w / aspect).max(1.0))
}

/// What a set of strokes will cost to draw, in pen samples.
///
/// Mirrors `device::stroke_events` exactly: one sample to start a stroke, then
/// one for each digitizer step along each segment. It is the honest predictor
/// of drawing time, and — unlike a count of traced points — it is what
/// simplifying the strokes actually reduces.
fn cost(strokes: &[Poly], work_w: f64, work_h: f64, calib: &Calib) -> usize {
    let to_screen = placement(work_w, work_h, calib);
    strokes
        .iter()
        .map(|s| {
            let mut n = 0usize;
            let mut prev: Option<(f64, f64)> = None;
            for &p in s {
                let (u, v) = to_screen(p);
                let q = calib.pen_from_screen(u, v);
                n += match prev {
                    None => 1,
                    Some((x0, y0)) => {
                        let d = (q.0 - x0).abs().max((q.1 - y0).abs());
                        ((d / device::STEP as f64) as usize).max(1)
                    }
                };
                prev = Some(q);
            }
            n
        })
        .sum()
}

/// Trace an image and put its strokes in drawing order, in work-raster
/// coordinates. Shared by both outputs — the pen replay and the `.rm` writer
/// differ only in what they map these onto and in what they can afford, so
/// tracing, simplification and the band ordering all live here.
fn trace_and_order(
    image_path: &str,
    calib: &Calib,
    budget: usize,
) -> Result<(Vec<Poly>, f64, f64), String> {
    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (src_w, src_h) = img.dimensions();
    if src_w == 0 || src_h == 0 {
        return Err("这张图是空的".into());
    }

    let (mut work_w, mut work_h) = page_fit(src_w, src_h, calib);

    loop {
        let (w, h) = (work_w as u32, work_h as u32);
        // Lanczos rather than a triangle filter: it is downscaling by a large
        // factor and what has to survive is edge contrast, since the very next
        // step is a threshold. A triangle filter's soft edges are what puts a
        // glyph's strokes into the same grey band as the paper between them.
        let gray = img
            .resize_exact(w, h, image::imageops::FilterType::Lanczos3)
            .to_luma8();
        let mask = imageproc::adaptive_threshold_mask(gray.as_raw(), w as usize, h as usize);
        let skeleton = imageproc::prune_spurs(&imageproc::skeletonize(&mask), MAX_SPUR);
        let raw: Vec<Poly> = imageproc::trace_skeleton(&skeleton)
            .iter()
            .map(|s| s.iter().map(|&(x, y)| (x as f64, y as f64)).collect())
            .collect();
        (work_w, work_h) = (w as f64, h as f64);

        if raw.is_empty() {
            return Err("这张图里找不到可以描的线条".into());
        }

        // Simplify as little as the budget allows: `EPSILON` is already below
        // what the ink can show, so anything beyond it is a concession.
        let mut epsilon = EPSILON;
        let mut strokes = simplify_all(&raw, epsilon);
        while cost(&strokes, work_w, work_h, calib) > budget && epsilon < EPSILON_MAX {
            epsilon = (epsilon * 1.5).min(EPSILON_MAX);
            strokes = simplify_all(&raw, epsilon);
        }

        // Only once simplification has run out does the raster give way. It
        // costs real detail, so it is the last resort rather than the first:
        // a smaller raster finds fewer separate marks, which is the only way
        // left to shorten a drawing whose ink is simply very long.
        if cost(&strokes, work_w, work_h, calib) <= budget || work_h <= 80.0 {
            return Ok((order_by_band(strokes, work_h, BANDS), work_w, work_h));
        }
        work_w = (work_w * 0.75).max(1.0);
        work_h = (work_h * 0.75).max(1.0);
    }
}

/// Simplify every stroke, dropping any that no longer describes a line.
fn simplify_all(raw: &[Poly], epsilon: f64) -> Vec<Poly> {
    raw.iter()
        .map(|s| imageproc::simplify(s, epsilon))
        .filter(|s| s.len() >= 2)
        .collect()
}

/// Trace `image_path` and lay it out centered on `calib`'s page, as pen
/// digitizer events to be replayed.
pub fn plan(image_path: &str, calib: &Calib) -> Result<Plan, String> {
    let (ordered, work_w, work_h) = trace_and_order(image_path, calib, MAX_PEN_SAMPLES)?;
    Ok(plan_from_page_strokes(&to_page_px(&ordered, work_w, work_h, calib), calib))
}

/// Build a `Plan` from strokes already placed at final size, in page
/// pixels — origin top-left, x in `0..calib.screen_w`, y in
/// `0..calib.screen_h`. `plan` gets here via `placement`, which maps its
/// traced raster into this frame; `markdown` lays text and table lines out
/// here directly and has no raster to place, so it calls this straight.
pub fn plan_from_page_strokes(ordered: &[Poly], calib: &Calib) -> Plan {
    // Stroke events only — `push` frames the pen session around them, so
    // stroke 0 begins the moment the first ink lands rather than the moment
    // the pen starts hovering.
    let mut bytes = Vec::new();
    let mut stroke_ends = Vec::with_capacity(ordered.len());
    let mut preview = Vec::with_capacity(ordered.len());
    for stroke in ordered {
        let placed: Poly = stroke
            .iter()
            .map(|&(x, y)| calib.pen_from_screen(x / calib.screen_w, y / calib.screen_h))
            .collect();
        bytes.extend_from_slice(&device::stroke_events(calib, std::slice::from_ref(&placed)));
        stroke_ends.push(bytes.len());
        preview.push(to_preview(stroke, calib.screen_w, calib.screen_h));
    }
    Plan { bytes, stroke_ends, preview }
}

/// Map traced-raster strokes into page-pixel coordinates via `placement`,
/// the step `plan` and `page` used to do inline before each went on to
/// build its own output shape. Pulled out so both — and `markdown`, which
/// skips tracing but wants the same frame — start from the same place.
fn to_page_px(ordered: &[Poly], work_w: f64, work_h: f64, calib: &Calib) -> Vec<Poly> {
    let to_screen = placement(work_w, work_h, calib);
    ordered
        .iter()
        .map(|s| {
            s.iter()
                .map(|&p| {
                    let (u, v) = to_screen(p);
                    (u * calib.screen_w, v * calib.screen_h)
                })
                .collect()
        })
        .collect()
}

/// A traced image as a `.rm` page, plus the same strokes for the window.
pub struct Page {
    /// Strokes in page coordinates, in drawing order — the order matters as
    /// much here as in `Plan`, because a `.rm` page chains its lines and the
    /// tablet renders them along that chain.
    pub strokes: Vec<Vec<rmfile::Point>>,
    /// The same strokes for the window, exactly as `Plan::preview`, so the two
    /// paths hand the frontend the identical thing and it needs to know
    /// nothing about which one ran.
    pub preview: Vec<PreviewStroke>,
}

/// Trace `image_path` and lay it out as a `.rm` page: same tracing, ordering
/// and placement as `plan`, but in page coordinates (x from the centre, y
/// from the top, both in screen pixels) instead of pen digitizer units.
pub fn page(image_path: &str, calib: &Calib) -> Result<Page, String> {
    let (ordered, work_w, work_h) = trace_and_order(image_path, calib, MAX_FILE_SAMPLES)?;
    Ok(page_from_page_strokes(&to_page_px(&ordered, work_w, work_h, calib), calib))
}

/// Build a `Page` from strokes already placed at final size, in page
/// pixels — the `.rm`-file counterpart to `plan_from_page_strokes`. See
/// there for why this exists as its own entry point.
pub fn page_from_page_strokes(ordered: &[Poly], calib: &Calib) -> Page {
    Page {
        strokes: ordered
            .iter()
            .map(|stroke| {
                stroke
                    .iter()
                    .map(|&(x, y)| rmfile::Point {
                        x: (x - calib.screen_w / 2.0) as f32,
                        y: y as f32,
                    })
                    .collect()
            })
            .collect(),
        preview: ordered.iter().map(|s| to_preview(s, calib.screen_w, calib.screen_h)).collect(),
    }
}

/// Quantised, simplified copy of a stroke in the image's own frame, for the
/// window to draw.
///
/// The whole preview crosses the IPC boundary in one message, and the window's
/// screen is barely 100px across, so trace resolution is doubly wasted here.
/// Simplified by deviation rather than thinned by spacing: dropping every
/// point within a fixed distance of the last one kept would cut the corner off
/// a tight curve, whereas this leaves a curve exactly as bent as it was and
/// still takes a straight stroke down to its two ends.
fn to_preview(stroke: &Poly, work_w: f64, work_h: f64) -> PreviewStroke {
    let q = |x: f64, span: f64| {
        ((x / span).clamp(0.0, 1.0) * PREVIEW_UNITS as f64).round() as u16
    };
    imageproc::simplify(stroke, PREVIEW_EPSILON * work_h)
        .into_iter()
        .map(|(x, y)| [q(x, work_w), q(y, work_h)])
        .collect()
}

/// Uniform fit of the work raster into the page, centered, with a margin —
/// returned as a closure from raster pixels to screen-normalised (u, v), both
/// 0..1 with the origin at the top left.
///
/// Screen space rather than pen space because there are two consumers now:
/// the pen replay wants raw digitizer units, and the `.rm` writer wants page
/// pixels. Both are one step from here and neither owns the layout.
fn placement(work_w: f64, work_h: f64, calib: &Calib) -> impl Fn((f64, f64)) -> (f64, f64) + '_ {
    let avail_w = calib.screen_w * (1.0 - 2.0 * MARGIN);
    let avail_h = calib.screen_h * (1.0 - 2.0 * MARGIN);
    let s = (avail_w / work_w).min(avail_h / work_h);
    let x0 = (calib.screen_w - work_w * s) / 2.0;
    let y0 = (calib.screen_h - work_h * s) / 2.0;
    move |(x, y)| {
        let u = (x0 + x * s) / calib.screen_w;
        let v = (y0 + y * s) / calib.screen_h;
        (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
    }
}

/// Bucket strokes by their topmost band and flatten back out, each band left
/// to right — a pure ordering pass, no stroke is cut.
///
/// Topmost, not first: the tracer walks a stroke from whichever end it found
/// an endpoint at, so its first point is as often its bottom as its top.
///
/// An earlier version cut strokes at band boundaries so a top-down wipe in
/// the window would stay honest. On hardware that was a mistake — xochitl
/// tapers every stroke's ends, so a long vertical line arriving as many short
/// pieces came out visibly scalloped. Now that the window follows the strokes
/// instead of the other way round, nothing needs cutting and this is only
/// about the drawing looking deliberate.
fn order_by_band(strokes: Vec<Poly>, work_h: f64, bands: usize) -> Vec<Poly> {
    let band_h = (work_h / bands as f64).max(1.0);
    let band_of = |y: f64| ((y / band_h) as usize).min(bands - 1);

    let mut buckets: Vec<Vec<Poly>> = vec![Vec::new(); bands];
    for pts in strokes {
        let top = pts.iter().map(|&(_, y)| band_of(y)).min().unwrap_or(0);
        buckets[top].push(pts);
    }
    for band in &mut buckets {
        band.sort_by(|a, b| {
            let key = |p: &Poly| p.iter().map(|q| q.0).fold(f64::INFINITY, f64::min);
            key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    buckets.into_iter().flatten().collect()
}
