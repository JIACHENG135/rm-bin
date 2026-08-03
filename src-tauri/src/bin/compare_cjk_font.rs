//! Same idea as `compare_cjk_gap`, sweeping the CJK font instead (at a
//! fixed, already-reasonable point gap) — one push, several labelled rows.
//!
//! Usage: `cargo run --bin compare_cjk_font -- --push host port [gap]`
//! Font candidates are hardcoded below; edit the list to try others.

use rm_bin_lib::rm::device::{self, PAPER_PRO};
use rm_bin_lib::rm::{draw, markdown};

const PHRASE: &str = "# 中文测试笔画清晰度";  // heading level -> 2x body font, easier to judge at a glance

const CANDIDATES: &[(&str, &str, u32)] = &[
    ("STHeiti Light", "/System/Library/Fonts/STHeiti Light.ttc", 0),
    ("STHeiti Medium", "/System/Library/Fonts/STHeiti Medium.ttc", 0),
    ("Hiragino Sans GB W3", "/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
    ("Hiragino Sans GB W6", "/System/Library/Fonts/Hiragino Sans GB.ttc", 1),
    ("Songti (serif)", "/System/Library/Fonts/Supplemental/Songti.ttc", 0),
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let push_idx = args.iter().position(|a| a == "--push");
    let (host, port, gap_idx) = match push_idx {
        Some(i) => (args[i + 1].clone(), args[i + 2].parse().unwrap_or(22u16), i + 3),
        None => ("192.168.3.177".to_string(), 22u16, 1),
    };
    let gap: f64 = args.get(gap_idx).and_then(|s| s.parse().ok()).unwrap_or(4.0);

    let calib = &PAPER_PRO;
    let font_px = calib.screen_h / 46.0;
    let row_h = font_px * 4.0; // headings render ~2x body size, so double the row pitch too
    let mut row_top = calib.screen_h * 0.10;
    let mut all: Vec<Vec<(f64, f64)>> = Vec::new();

    for &(label, path, index) in CANDIDATES {
        let strokes = match markdown::debug_layout_with_gap_and_font(PHRASE, calib, gap, path, index) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{label}: skip — {e}");
                continue;
            }
        };
        let top0 = strokes.iter().flatten().map(|&(_, y)| y).fold(f64::MAX, f64::min);
        let dy = row_top - top0;
        eprintln!("row @ y={row_top:.0}: {label} — {} strokes", strokes.len());
        all.extend(strokes.into_iter().map(|s| s.into_iter().map(|(x, y)| (x, y + dy)).collect::<Vec<_>>()));
        row_top += row_h;
    }

    if row_top > calib.screen_h * 0.92 {
        eprintln!("warning: rows may run off the bottom margin");
    }

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
