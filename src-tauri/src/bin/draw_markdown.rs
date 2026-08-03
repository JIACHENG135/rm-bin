//! Draw a markdown file on a real reMarkable, over the pen-replay path —
//! the same `debug_*` role rm-agent's own `src/bin/` tools play: exercise
//! one piece of the pipeline against real hardware without going through
//! the whole Tauri app.
//!
//! Usage: `cargo run --bin draw_markdown -- input.md [host] [port]`

use rm_bin_lib::rm::{device, markdown};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(md_path) = args.get(1) else {
        eprintln!("usage: draw_markdown <input.md> [host] [port]");
        std::process::exit(2);
    };
    let host = args.get(2).cloned().unwrap_or_else(|| "192.168.3.177".into());
    let port: u16 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(22);

    let text = std::fs::read_to_string(md_path).unwrap_or_else(|e| {
        eprintln!("can't read {md_path}: {e}");
        std::process::exit(1);
    });

    eprintln!("detecting device at {host}:{port}...");
    let calib = device::detect(&host, port).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    eprintln!("found {:?}", calib.model);

    let plan = markdown::plan(&text, &calib).unwrap_or_else(|e| {
        eprintln!("layout failed: {e}");
        std::process::exit(1);
    });
    eprintln!(
        "{} strokes, {} bytes of pen events — pushing...",
        plan.stroke_count(),
        plan.bytes.len()
    );

    let mut last_pct = -1i32;
    let result = device::push(&host, port, &calib, &plan.bytes, |written| {
        let pct = (written as f64 / plan.bytes.len() as f64 * 100.0) as i32;
        if pct != last_pct {
            eprint!("\r{pct}%  ");
            last_pct = pct;
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
