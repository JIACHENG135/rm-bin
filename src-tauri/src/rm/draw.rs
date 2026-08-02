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

/// Longer edge of the raster we trace on. Bigger keeps more detail but costs
/// stroke points, and every point is ~4 input events crawling down an SSH
/// pipe at digitizer pace — this is really a drawing-time dial.
pub(crate) const BASE_WORK: f64 = 700.0;
/// Past this the draw takes minutes, so retrace smaller instead.
const MAX_STROKE_POINTS: usize = 14_000;
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
const MARGIN: f64 = 0.10;
/// Horizontal slices the drawing order is quantised to. Coarser bands mean
/// longer left-to-right sweeps; finer ones mean the drawing marches down the
/// page more strictly.
const BANDS: usize = 64;
/// Preview coordinates are integers on a 0..N grid in the image's own frame —
/// N is well past what a ~100px window can resolve, and keeps each number to
/// four digits of JSON.
const PREVIEW_UNITS: u16 = 2000;
/// Preview points closer together than this (as a fraction of image height)
/// are dropped.
const PREVIEW_EPSILON: f64 = 1.0 / 400.0;

type Poly = Vec<(f64, f64)>;

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

/// Trace an image and put its strokes in drawing order, in work-raster
/// coordinates. Shared by both outputs — the pen replay and the `.rm` writer
/// differ only in what they map these onto, so tracing, the retry-smaller
/// loop and the band ordering all live here.
fn trace_and_order(image_path: &str) -> Result<(Vec<Poly>, f64, f64), String> {
    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (src_w, src_h) = img.dimensions();
    if src_w == 0 || src_h == 0 {
        return Err("这张图是空的".into());
    }

    // Work raster keeps the source aspect ratio, so its pixels stay square in
    // screen space and the placement is a plain uniform scale.
    let aspect = src_w as f64 / src_h as f64;
    let (mut work_w, mut work_h) = if aspect >= 1.0 {
        (BASE_WORK, BASE_WORK / aspect)
    } else {
        (BASE_WORK * aspect, BASE_WORK)
    };

    let strokes = loop {
        let (w, h) = (work_w.max(1.0) as u32, work_h.max(1.0) as u32);
        let resized = img.resize_exact(w, h, image::imageops::FilterType::Triangle);
        let gray = resized.to_luma8();
        let threshold = imageproc::otsu_threshold(gray.as_raw());
        let mask = imageproc::threshold_mask(gray.as_raw(), w as usize, h as usize, threshold);
        let traced = imageproc::trace_skeleton(&imageproc::skeletonize(&mask));
        let points: usize = traced.iter().map(|s| s.len()).sum();
        if points <= MAX_STROKE_POINTS || work_h <= 80.0 {
            work_w = w as f64;
            work_h = h as f64;
            break traced;
        }
        work_w *= 0.75;
        work_h *= 0.75;
    };

    if strokes.is_empty() {
        return Err("这张图里找不到可以描的线条".into());
    }
    Ok((order_by_band(&strokes, work_h, BANDS), work_w, work_h))
}

/// Trace `image_path` and lay it out centered on `calib`'s page, as pen
/// digitizer events to be replayed.
pub fn plan(image_path: &str, calib: &Calib) -> Result<Plan, String> {
    let (ordered, work_w, work_h) = trace_and_order(image_path)?;
    let to_screen = placement(work_w, work_h, calib);

    // Stroke events only — `push` frames the pen session around them, so
    // stroke 0 begins the moment the first ink lands rather than the moment
    // the pen starts hovering.
    let mut bytes = Vec::new();
    let mut stroke_ends = Vec::with_capacity(ordered.len());
    let mut preview = Vec::with_capacity(ordered.len());
    for stroke in &ordered {
        let placed: Poly = stroke
            .iter()
            .map(|&p| {
                let (u, v) = to_screen(p);
                calib.pen_from_screen(u, v)
            })
            .collect();
        bytes.extend_from_slice(&device::stroke_events(calib, std::slice::from_ref(&placed)));
        stroke_ends.push(bytes.len());
        preview.push(to_preview(stroke, work_w, work_h));
    }

    Ok(Plan { bytes, stroke_ends, preview })
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
    let (ordered, work_w, work_h) = trace_and_order(image_path)?;
    let to_screen = placement(work_w, work_h, calib);
    Ok(Page {
        strokes: ordered
            .iter()
            .map(|stroke| {
                stroke
                    .iter()
                    .map(|&p| {
                        let (u, v) = to_screen(p);
                        rmfile::Point {
                            x: ((u - 0.5) * calib.screen_w) as f32,
                            y: (v * calib.screen_h) as f32,
                        }
                    })
                    .collect()
            })
            .collect(),
        preview: ordered
            .iter()
            .map(|s| to_preview(s, work_w, work_h))
            .collect(),
    })
}

/// Quantised, decimated copy of a stroke in the image's own frame, for the
/// window to draw. The window's screen is barely 100px across, so trace
/// resolution is wasted there — dropping points closer together than
/// `PREVIEW_EPSILON` cuts the payload several-fold and changes nothing you
/// can see.
fn to_preview(stroke: &Poly, work_w: f64, work_h: f64) -> PreviewStroke {
    let q = |x: f64, span: f64| {
        ((x / span).clamp(0.0, 1.0) * PREVIEW_UNITS as f64).round() as u16
    };
    let mut out: PreviewStroke = Vec::new();
    let mut last = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (i, &(x, y)) in stroke.iter().enumerate() {
        let far = ((x - last.0).powi(2) + (y - last.1).powi(2)).sqrt()
            > PREVIEW_EPSILON * work_h;
        if i + 1 == stroke.len() || far {
            out.push([q(x, work_w), q(y, work_h)]);
            last = (x, y);
        }
    }
    out
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
fn order_by_band(strokes: &[Vec<(i32, i32)>], work_h: f64, bands: usize) -> Vec<Poly> {
    let band_h = (work_h / bands as f64).max(1.0);
    let band_of = |y: f64| ((y / band_h) as usize).min(bands - 1);

    let mut buckets: Vec<Vec<Poly>> = vec![Vec::new(); bands];
    for stroke in strokes {
        let pts: Poly = stroke.iter().map(|&(x, y)| (x as f64, y as f64)).collect();
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
