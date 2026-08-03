//! Dump one word's raw raster (before threshold/skeletonize/trace) and its
//! traced-skeleton overlay side by side, so a rasterization bug and a
//! tracing bug don't get confused with each other.
//!
//! Usage: `cargo run --bin debug_raster -- "Hello" out.png`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = args.get(1).cloned().unwrap_or_else(|| "Hello".into());
    let out = args.get(2).cloned().unwrap_or_else(|| "raster.png".into());

    let font = ab_glyph::FontRef::try_from_slice(include_bytes!("../../resources/fonts/Caveat-Regular.ttf")).unwrap();
    use ab_glyph::{point, Font, ScaleFont};
    let px = 200.0f32;
    let scaled = font.as_scaled(px);
    let mut caret = point(0.0, scaled.ascent());
    let mut outlines = Vec::new();
    let mut last: Option<ab_glyph::Glyph> = None;
    for c in text.chars() {
        let mut g = scaled.scaled_glyph(c);
        if let Some(prev) = last.take() {
            caret.x += scaled.kern(prev.id, g.id);
        }
        g.position = caret;
        caret.x += scaled.h_advance(g.id);
        last = Some(g.clone());
        if let Some(o) = font.outline_glyph(g) {
            outlines.push(o);
        }
    }
    let w = caret.x.ceil().max(1.0) as usize;
    let h = scaled.height().ceil().max(1.0) as usize;
    eprintln!("raster {w}x{h}, advance {}", caret.x);
    let mut gray = vec![255u8; w * h];
    for o in outlines {
        let b = o.px_bounds();
        o.draw(|gx, gy, c| {
            let px = b.min.x as i32 + gx as i32;
            let py = b.min.y as i32 + gy as i32;
            if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                let idx = py as usize * w + px as usize;
                let ink = (255.0 - c.clamp(0.0, 1.0) * 255.0).round() as u8;
                gray[idx] = gray[idx].min(ink);
            }
        });
    }

    let raw_img = image::GrayImage::from_raw(w as u32, h as u32, gray.clone()).unwrap();

    let otsu = rm_bin_lib::rm::imageproc::otsu_threshold(&gray);
    eprintln!("otsu threshold: {otsu}");
    let mask = rm_bin_lib::rm::imageproc::threshold_mask(&gray, w, h, otsu);
    let skeleton = rm_bin_lib::rm::imageproc::prune_spurs(&rm_bin_lib::rm::imageproc::skeletonize(&mask), 4);
    let strokes = rm_bin_lib::rm::imageproc::trace_skeleton(&skeleton);
    eprintln!("{} strokes after trace", strokes.len());

    // side by side: raw raster | binarized | skeleton+traced overlay
    let gap = 20u32;
    let out_w = w as u32 * 3 + gap * 2;
    let mut canvas = image::RgbImage::from_pixel(out_w, h as u32, image::Rgb([255, 255, 255]));
    for y in 0..h {
        for x in 0..w {
            let v = raw_img.get_pixel(x as u32, y as u32).0[0];
            canvas.put_pixel(x as u32, y as u32, image::Rgb([v, v, v]));
            let bin = if mask.data[y * w + x] { 0 } else { 255 };
            canvas.put_pixel(x as u32 + w as u32 + gap, y as u32, image::Rgb([bin, bin, bin]));
        }
    }
    let ox = (w as u32 + gap) * 2;
    for y in 0..h {
        for x in 0..w {
            if skeleton.data[y * w + x] {
                canvas.put_pixel(ox + x as u32, y as u32, image::Rgb([255, 0, 0]));
            }
        }
    }
    // overlay traced polylines in blue, offset slightly so they're visible over red skeleton
    for stroke in &strokes {
        for &(x, y) in stroke {
            if x >= 0 && y >= 0 {
                let (x, y) = (ox + x as u32, y as u32);
                if x < out_w && y < h as u32 {
                    canvas.put_pixel(x, y, image::Rgb([0, 0, 255]));
                }
            }
        }
    }

    canvas.save(&out).unwrap();
    eprintln!("wrote {out}");
}
