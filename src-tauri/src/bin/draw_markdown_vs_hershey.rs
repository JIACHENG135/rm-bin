//! Put `markdown.rs`'s own raster-skeleton-traced strokes for "Hello World
//! 123" directly above a Hershey `futural` reference row for the same
//! text, on the same page, in the same push — the question that actually
//! matters for shipping markdown text isn't "which Hershey font is most
//! robust", it's "does my own tracer's output behave like the robust one
//! or the fragile ones".
//!
//! Usage: `cargo run --bin draw_markdown_vs_hershey -- hershey_futural.json --push host port`

use rm_bin_lib::rm::device::{self, PAPER_PRO};
use rm_bin_lib::rm::{draw, markdown};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(hershey_path) = args.get(1) else {
        eprintln!("usage: draw_markdown_vs_hershey <hershey.json> --push host port");
        std::process::exit(2);
    };
    let push_idx = args.iter().position(|a| a == "--push");
    let (host, port) = match push_idx {
        Some(i) => (args[i + 1].clone(), args[i + 2].parse().unwrap_or(22u16)),
        None => ("192.168.3.177".to_string(), 22u16),
    };

    let calib = &PAPER_PRO;

    // Row 1: markdown.rs's own pipeline, unmodified — the actual code path
    // `send_to_remarkable` uses in Mode::Pen, laid out as a plain paragraph.
    let md_strokes = markdown::debug_layout("Hello World 123", calib).unwrap_or_else(|e| {
        eprintln!("markdown layout failed: {e}");
        std::process::exit(1);
    });
    eprintln!("row 1 (markdown.rs tracer): {} strokes", md_strokes.len());
    let md_bottom = md_strokes.iter().flatten().map(|&(_, y)| y).fold(f64::MIN, f64::max);

    // Row 2: the Hershey row that read cleanly in the grid test, placed
    // clear of row 1's own bottom edge instead of a fixed offset, so
    // however tall markdown.rs's line wrapping made row 1, row 2 never
    // overlaps it.
    let font_px = calib.screen_h / 46.0;
    let left = calib.screen_w * 0.14;
    let row2_top = md_bottom + font_px * 1.2;

    let raw: Vec<Vec<(f64, f64)>> = serde_json::from_str(&std::fs::read_to_string(hershey_path).unwrap()).unwrap();
    let ys: Vec<f64> = raw.iter().flatten().map(|&(_, y)| y).collect();
    let max_y = ys.iter().cloned().fold(f64::MIN, f64::max);
    let min_y = ys.iter().cloned().fold(f64::MAX, f64::min);
    let scale = font_px / (max_y - min_y).max(1.0);
    let hershey_strokes: Vec<Vec<(f64, f64)>> = raw
        .iter()
        .map(|s| s.iter().map(|&(x, y)| (left + x * scale, row2_top + (max_y - y) * scale)).collect())
        .collect();
    eprintln!("row 2 (Hershey {hershey_path}): {} strokes", hershey_strokes.len());

    let mut all = md_strokes;
    all.extend(hershey_strokes);

    let plan = draw::plan_from_page_strokes(&all, calib);
    eprintln!("pushing {} total strokes ({} bytes) to {host}:{port}...", plan.stroke_count(), plan.bytes.len());
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
