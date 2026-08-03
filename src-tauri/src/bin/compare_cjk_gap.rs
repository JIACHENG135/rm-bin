//! Lay out the same short Chinese phrase through the real markdown
//! pipeline at several `MAX_POINT_GAP` values and stack them as rows on
//! one page, so a density sweep costs one device push instead of one per
//! value.
//!
//! Usage: `cargo run --bin compare_cjk_gap -- --push host port [gap1 gap2 ...]`
//! (gaps default to 2 3 4 5 6 8 if none given)

use rm_bin_lib::rm::device::{self, PAPER_PRO};
use rm_bin_lib::rm::{draw, markdown};

const PHRASE: &str = "# 中文测试笔画清晰度";  // heading level -> 2x body font, easier to judge at a glance

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let push_idx = args.iter().position(|a| a == "--push");
    let (host, port, gap_start) = match push_idx {
        Some(i) => (args[i + 1].clone(), args[i + 2].parse().unwrap_or(22u16), i + 3),
        None => ("192.168.3.177".to_string(), 22u16, 1),
    };
    let gaps: Vec<f64> = if args.len() > gap_start {
        args[gap_start..].iter().filter_map(|s| s.parse().ok()).collect()
    } else {
        vec![2.0, 3.0, 4.0, 5.0, 6.0, 8.0]
    };

    let calib = &PAPER_PRO;
    let font_px = calib.screen_h / 46.0;
    let row_h = font_px * 4.0; // headings render ~2x body size, so double the row pitch too
    let mut row_top = calib.screen_h * 0.10;
    let mut all: Vec<Vec<(f64, f64)>> = Vec::new();

    for &gap in &gaps {
        let strokes = markdown::debug_layout_with_gap(PHRASE, calib, gap).unwrap_or_else(|e| {
            eprintln!("gap {gap}: layout failed: {e}");
            std::process::exit(1);
        });
        // debug_layout_with_gap always starts near PAGE_MARGIN*screen_h —
        // shift the whole block down to this row's slot instead.
        let top0 = strokes.iter().flatten().map(|&(_, y)| y).fold(f64::MAX, f64::min);
        let dy = row_top - top0;
        eprintln!("row @ y={row_top:.0}: gap={gap}px — {} strokes", strokes.len());
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
