//! Lay out several labelled Hershey-stroke JSON files as separate rows on
//! one page and push them all in one draw — so an A/B/C/D/... comparison
//! costs one device round trip instead of one per candidate.
//!
//! Usage: `cargo run --bin draw_hershey_grid -- --push host port row1.json[:densify_px] row2.json[:densify_px] ...`
//!
//! Each row file is the same raw-Hershey JSON `draw_hershey` reads (Y-up,
//! baseline near 0, one line normalized to 100 units tall). An optional
//! `:N` suffix pre-densifies that row to <= N page pixels between points,
//! matching `draw_hershey --densify`; omit it to send the row at its
//! native point spacing (`device::stroke_events`' own 40-digitizer-unit
//! resampling, untouched).

use rm_bin_lib::rm::device::{self, PAPER_PRO};
use rm_bin_lib::rm::draw;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let push_idx = args.iter().position(|a| a == "--push");
    let (host, port, row_start) = match push_idx {
        Some(i) => (args[i + 1].clone(), args[i + 2].parse().unwrap_or(22u16), i + 3),
        None => ("192.168.3.177".to_string(), 22u16, 1),
    };

    let calib = &PAPER_PRO;
    let font_px = calib.screen_h / 46.0;
    let left = calib.screen_w * 0.14;
    let row_h = font_px * 2.3; // room for the row's own label above it

    let mut all_strokes: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut row_top = calib.screen_h * 0.10;

    for spec in &args[row_start..] {
        if spec == "--push" || spec.parse::<u16>().is_ok() || spec == &host {
            continue; // crude guard in case argv ordering surprises us
        }
        let (path, densify_px) = match spec.split_once(':') {
            Some((p, n)) => (p, n.parse::<f64>().ok()),
            None => (spec.as_str(), None),
        };
        let raw: Vec<Vec<(f64, f64)>> = match std::fs::read_to_string(path).and_then(|s| {
            serde_json::from_str(&s).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        }) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip {path}: {e}");
                continue;
            }
        };

        let ys: Vec<f64> = raw.iter().flatten().map(|&(_, y)| y).collect();
        if ys.is_empty() {
            continue;
        }
        let max_y = ys.iter().cloned().fold(f64::MIN, f64::max);
        let min_y = ys.iter().cloned().fold(f64::MAX, f64::min);
        let scale = font_px / (max_y - min_y).max(1.0);

        let mut placed: Vec<Vec<(f64, f64)>> = raw
            .iter()
            .map(|s| s.iter().map(|&(x, y)| (left + x * scale, row_top + (max_y - y) * scale)).collect())
            .collect();
        if let Some(px) = densify_px {
            placed = placed.iter().map(|s| densify(s, px)).collect();
        }

        let label = format!("{path} {}", densify_px.map(|p| format!("(<= {p}px)")).unwrap_or("(native)".into()));
        eprintln!("row @ y={row_top:.0}: {label} — {} strokes", placed.len());
        all_strokes.extend(placed);
        row_top += row_h;
    }

    if row_top > calib.screen_h * 0.92 {
        eprintln!("warning: rows may run off the bottom margin");
    }

    let plan = draw::plan_from_page_strokes(&all_strokes, calib);
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
