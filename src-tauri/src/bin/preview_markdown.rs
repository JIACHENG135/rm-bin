//! Render a markdown file's traced strokes to a PNG, without touching a
//! device — the same "look at the trace before it goes anywhere near
//! hardware" step rm-agent's `debug_trace.rs` used, because a mistraced
//! glyph is obvious to a human at a glance and expensive to diagnose from
//! an SSH round trip.
//!
//! Usage: `cargo run --bin preview_markdown -- input.md output.png [rm2]`

use rm_bin_lib::rm::device::{PAPER_PRO, RM2};
use rm_bin_lib::rm::markdown;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(md_path), Some(out_path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: preview_markdown <input.md> <output.png> [rm2]");
        std::process::exit(2);
    };
    let calib = if args.get(3).map(String::as_str) == Some("rm2") { &RM2 } else { &PAPER_PRO };

    let text = std::fs::read_to_string(md_path).unwrap_or_else(|e| {
        eprintln!("can't read {md_path}: {e}");
        std::process::exit(1);
    });

    // The *raw* final strokes, not `Plan::preview` — the preview simplifies
    // against the window's ~100px width, and a small glyph's detail is
    // finer than that simplification tolerance, so the window (and this
    // tool, if it drew the preview) shows something coarser than what
    // `plan.bytes` actually sends the device. This draws what the pen
    // actually gets.
    let strokes = markdown::debug_layout(&text, calib).unwrap_or_else(|e| {
        eprintln!("layout failed: {e}");
        std::process::exit(1);
    });
    eprintln!("{} strokes", strokes.len());

    // Render at 2x the device's own pixel dimensions — same reasoning as
    // rm-agent's debug_trace.rs "up 3x for visibility": a mistraced glyph
    // should be obvious at a glance, not something you have to zoom into a
    // 1x screenshot to see.
    const SCALE: u32 = 2;
    let (w, h) = ((calib.screen_w as u32) * SCALE, (calib.screen_h as u32) * SCALE);
    let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255]));

    let sx = w as f64 / calib.screen_w;
    let sy = h as f64 / calib.screen_h;
    for stroke in &strokes {
        for pair in stroke.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            draw_line(
                &mut img,
                (x0 * sx) as i64,
                (y0 * sy) as i64,
                (x1 * sx) as i64,
                (y1 * sy) as i64,
                w,
                h,
            );
        }
    }

    img.save(out_path).unwrap_or_else(|e| {
        eprintln!("can't write {out_path}: {e}");
        std::process::exit(1);
    });
    eprintln!("wrote {out_path} ({w}x{h})");
}

/// Bresenham, because this is a debug tool and the `image` crate doesn't
/// carry a line-drawing primitive of its own — no need for a drawing crate
/// dependency just to look at strokes once.
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
