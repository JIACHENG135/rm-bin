//! A/B test: take `text2rm.py`'s raw Hershey strokes (dumped as JSON,
//! *before* its own rM2-specific `fit_and_rotate`/`build`) and place them
//! into this crate's own page layout and pen-replay pipeline — same
//! `device::stroke_events`, same `device::push` pacing, same page margins
//! and font size as `markdown.rs` uses for body text. The only thing that
//! differs from a `draw_markdown` run is where the glyph strokes came from:
//! Hershey's hand-drawn vector strokes here, `markdown::run_art`'s
//! rasterize-skeletonize-trace there. If one reads cleanly on real hardware
//! and the other doesn't, the difference is the answer.
//!
//! Usage: `cargo run --bin draw_hershey -- strokes.json [preview.png] [--push host port]`
//!
//! Where strokes.json is `[[[x,y],[x,y],...], ...]` in the Hershey library's
//! own raw units — Y increasing *upward*, baseline near 0, one line of text
//! normalized to 100 units tall (`HersheyFonts.normalize_rendering(100)`,
//! matching `text2rm.py`'s convention).

use rm_bin_lib::rm::device::{self, PAPER_PRO};
use rm_bin_lib::rm::draw;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(json_path) = args.get(1) else {
        eprintln!("usage: draw_hershey <strokes.json> [preview.png] [--push host port]");
        std::process::exit(2);
    };

    let raw: Vec<Vec<(f64, f64)>> =
        serde_json::from_str(&std::fs::read_to_string(json_path).unwrap()).unwrap();
    eprintln!("{} raw Hershey strokes", raw.len());

    let calib = &PAPER_PRO;
    // Same body-text size markdown.rs uses (BODY_FRAC = 1/46 of screen
    // height) and the same left margin it uses post-fix (PAGE_MARGIN =
    // 0.14) — sizing and position are held constant so this is a fair
    // like-for-like comparison, not an easier or harder one.
    let font_px = calib.screen_h / 46.0;
    let left = calib.screen_w * 0.14;
    let top = calib.screen_h * 0.14;

    let ys: Vec<f64> = raw.iter().flatten().map(|&(_, y)| y).collect();
    let (min_y, max_y) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
    let scale = font_px / (max_y - min_y).max(1.0);

    let mut strokes: Vec<Vec<(f64, f64)>> = raw
        .iter()
        .map(|s| {
            s.iter()
                .map(|&(x, y)| (left + x * scale, top + (max_y - y) * scale)) // flip: Hershey Y-up -> page Y-down
                .collect()
        })
        .collect();

    // Diagnostic-only: `device::stroke_events` re-samples every stroke at a
    // fixed 40-digitizer-unit step, tuned against long strokes traced from a
    // whole page. A single glyph stroke is only a few hundred digitizer
    // units long *in total*, so that same step could plausibly be coarse
    // enough, relative to the glyph's own size, to render as visibly
    // separated dabs rather than one continuous line — hence testing it
    // here, on this one throwaway tool, instead of touching the shared
    // `device::STEP` constant that every drawing mode depends on.
    if let Some(px) = args.iter().position(|a| a == "--densify").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<f64>().ok())
    {
        eprintln!("pre-densifying to <= {px}px between points before handoff to stroke_events");
        strokes = strokes.iter().map(|s| densify(s, px)).collect();
    }

    if let Some(png_path) = args.get(2).filter(|s| !s.starts_with("--")) {
        const SCALE: u32 = 2;
        let (w, h) = ((calib.screen_w as u32) * SCALE, (calib.screen_h as u32) * SCALE);
        let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255]));
        let sx = w as f64 / calib.screen_w;
        let sy = h as f64 / calib.screen_h;
        for s in &strokes {
            for pair in s.windows(2) {
                let (x0, y0) = pair[0];
                let (x1, y1) = pair[1];
                draw_line(&mut img, (x0 * sx) as i64, (y0 * sy) as i64, (x1 * sx) as i64, (y1 * sy) as i64, w, h);
            }
        }
        img.save(png_path).unwrap();
        eprintln!("wrote {png_path}");
    }

    if let Some(push_idx) = args.iter().position(|a| a == "--push") {
        let host = args.get(push_idx + 1).cloned().unwrap_or_else(|| "192.168.3.177".into());
        let port: u16 = args.get(push_idx + 2).and_then(|s| s.parse().ok()).unwrap_or(22);
        let plan = draw::plan_from_page_strokes(&strokes, calib);
        eprintln!("pushing {} strokes ({} bytes) to {host}:{port}...", plan.stroke_count(), plan.bytes.len());
        let mut last = -1i32;
        let result = device::push(&host, port, calib, &plan.bytes, |written| {
            let pct = (written as f64 / plan.bytes.len() as f64 * 100.0) as i32;
            if pct != last {
                eprint!("\r{pct}%  ");
                last = pct;
            }
        });
        eprintln!();
        match result {
            Ok(()) => eprintln!("done."),
            Err(e) => {
                eprintln!("failed: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Insert extra points so no two consecutive ones are farther apart than
/// `max_gap` page pixels — diagnostic tool only, see the call site.
fn densify(stroke: &[(f64, f64)], max_gap: f64) -> Vec<(f64, f64)> {
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

fn draw_line(img: &mut image::GrayImage, x0: i64, y0: i64, x1: i64, y1: i64, w: u32, h: u32) {
    let (mut x0, mut y0) = (x0, y0);
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        if x0 >= 0 && y0 >= 0 && (x0 as u32) < w && (y0 as u32) < h {
            img.put_pixel(x0 as u32, y0 as u32, image::Luma([0]));
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}
