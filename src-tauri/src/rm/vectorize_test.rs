use super::vectorize;
use crate::rm::device::PAPER_PRO;
use image::Rgba;

/// A synthetic PNG so the test doesn't depend on any file existing on disk —
/// a black square on a white field is the simplest shape with a real,
/// non-trivial contour (four corners, not a degenerate line).
fn square_png(path: &std::path::Path) {
    let mut img = image::RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
    for y in 60..140 {
        for x in 60..140 {
            img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }
    img.save(path).unwrap();
}

fn tmp_png(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("rm-bin-vectorize-{name}-{}.png", std::process::id()));
    square_png(&p);
    p
}

#[test]
fn traces_a_simple_shape_into_a_closed_contour() {
    let p = tmp_png("square");
    let plan = vectorize::plan(p.to_str().unwrap(), &PAPER_PRO).unwrap();
    let _ = std::fs::remove_file(&p);

    assert!(!plan.bytes.is_empty());
    // A square's contour is one closed loop — expect a small, non-zero
    // number of strokes, not the hundreds a skeleton trace of a filled
    // region would produce (a filled square skeletonizes to a meaningless
    // spine, which is the whole reason this module exists instead).
    assert!(plan.stroke_count() >= 1 && plan.stroke_count() < 10, "got {} strokes", plan.stroke_count());
}

#[test]
fn blank_image_is_an_error_not_a_blank_plan() {
    let p = std::env::temp_dir().join(format!("rm-bin-vectorize-blank-{}.png", std::process::id()));
    image::RgbaImage::from_pixel(100, 100, Rgba([255, 255, 255, 255])).save(&p).unwrap();
    let result = vectorize::plan(p.to_str().unwrap(), &PAPER_PRO);
    let _ = std::fs::remove_file(&p);
    assert!(result.is_err());
}

#[test]
fn missing_file_is_a_readable_error() {
    let result = vectorize::plan("/no/such/file.png", &PAPER_PRO);
    assert!(result.is_err());
}

#[test]
fn strokes_stay_within_the_page() {
    let p = tmp_png("bounds");
    let plan = vectorize::plan(p.to_str().unwrap(), &PAPER_PRO).unwrap();
    let _ = std::fs::remove_file(&p);
    for stroke in &plan.preview {
        for &[x, y] in stroke {
            assert!(x <= 2000 && y <= 2000, "point outside the preview's own 0..2000 grid: {x},{y}");
        }
    }
}

#[test]
fn page_and_plan_agree_on_stroke_count() {
    let p = tmp_png("agree");
    let plan = vectorize::plan(p.to_str().unwrap(), &PAPER_PRO).unwrap();
    let page = vectorize::page(p.to_str().unwrap(), &PAPER_PRO).unwrap();
    let _ = std::fs::remove_file(&p);
    assert_eq!(plan.stroke_count(), page.preview.len());
}

#[test]
fn a_wide_source_image_still_fits_the_page() {
    let p = std::env::temp_dir().join(format!("rm-bin-vectorize-wide-{}.png", std::process::id()));
    let mut img = image::RgbaImage::from_pixel(400, 100, Rgba([255, 255, 255, 255]));
    for y in 20..80 {
        for x in 20..380 {
            img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }
    img.save(&p).unwrap();
    let plan = vectorize::plan(p.to_str().unwrap(), &PAPER_PRO);
    let _ = std::fs::remove_file(&p);
    assert!(plan.is_ok());
}
