//! The three steps between a photograph and a set of strokes, each tested on
//! the failure that put it there.
//!
//! All of this used to be one line — a global Otsu, straight into the
//! skeletonizer — and the symptom of every one of these was the same: text
//! that came out of the tablet unreadable. That is a bad signal to debug from,
//! so each step now has the specific thing it is for pinned down here.

use super::imageproc::{
    adaptive_threshold_mask, otsu_threshold, prune_spurs, simplify, skeletonize, threshold_mask,
    trace_skeleton, Mask,
};

/// Bars of ink on paper, under an illumination that falls off to the right —
/// a page photographed with the light behind you, in other words.
///
/// `dark` is how much light reaches the right edge. At 0.2 the paper there is
/// darker than the *ink* on the left, so no single threshold can be right
/// about both halves; that is the whole point of the fixture.
fn lit_page(w: usize, h: usize, dark: f64) -> Vec<u8> {
    const INK: f64 = 60.0;
    const PAPER: f64 = 255.0;
    const PERIOD: usize = 40;
    const BAR: usize = 6;
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let base = if x % PERIOD < BAR { INK } else { PAPER };
            let light = 1.0 - (1.0 - dark) * (x as f64 / (w - 1) as f64);
            out[y * w + x] = (base * light).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn ink_fraction(m: &Mask) -> f64 {
    m.data.iter().filter(|&&b| b).count() as f64 / m.data.len() as f64
}

/// The failure that motivated all of this: under a global threshold, the
/// shadowed end of a photographed page is darker than the ink at the lit end,
/// so a third of the picture turns solid black. What the tracer then hands
/// back is one long line down the edge of the shadow and no text at all.
#[test]
fn a_lighting_gradient_does_not_become_ink() {
    let (w, h) = (400usize, 300usize);
    let gray = lit_page(w, h, 0.2);
    let truth = 6.0 / 40.0;

    let global = threshold_mask(&gray, w, h, otsu_threshold(&gray));
    let adaptive = adaptive_threshold_mask(&gray, w, h);
    let (g, a) = (ink_fraction(&global), ink_fraction(&adaptive));

    // Documented rather than merely asserted around: this is what the old
    // path did, and it is why there is a new one.
    assert!(g > truth * 2.0, "a global threshold used to flood here; it now gives {g:.3}");
    assert!(
        (a - truth).abs() < truth * 0.25,
        "adaptive found {a:.3} ink where the page has {truth:.3}"
    );
}

/// ...and it must not have bought that by becoming a different algorithm on
/// ordinary input. On an evenly lit image there is nothing to correct, and the
/// result has to be the global threshold it replaced.
#[test]
fn even_lighting_still_gets_the_plain_threshold() {
    let (w, h) = (400usize, 300usize);
    let gray = lit_page(w, h, 1.0);
    let global = threshold_mask(&gray, w, h, otsu_threshold(&gray));
    let adaptive = adaptive_threshold_mask(&gray, w, h);

    let differ = global
        .data
        .iter()
        .zip(&adaptive.data)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differ * 1000 < w * h,
        "{differ} of {} pixels differ on an evenly lit image",
        w * h
    );
}

/// A blank page has no two classes to separate, and Otsu will happily cut
/// noise down the middle and report half of it as ink. Nothing should be
/// found here at all.
#[test]
fn a_blank_page_has_no_ink_in_it() {
    let (w, h) = (300usize, 300usize);
    let gray = vec![250u8; w * h];
    assert_eq!(ink_fraction(&adaptive_threshold_mask(&gray, w, h)), 0.0);
}

// ————— simplification —————

/// Distance from a point to a polyline, for checking that simplifying moved
/// the line by less than it promised.
fn distance_to(poly: &[(f64, f64)], p: (f64, f64)) -> f64 {
    poly.windows(2)
        .map(|s| {
            let ((ax, ay), (bx, by)) = (s[0], s[1]);
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 {
                (((p.0 - ax) * dx + (p.1 - ay) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (cx, cy) = (ax + t * dx, ay + t * dy);
            ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
        })
        .fold(f64::MAX, f64::min)
}

/// A traced skeleton puts a point on every pixel, and the event encoder
/// re-samples at the digitizer's step — several pixels — so an unsimplified
/// straight line costs five or six events for every one it needs. This is
/// what makes tracing at the panel's resolution affordable, so the saving is
/// worth a number.
#[test]
fn a_straight_run_collapses_to_its_ends() {
    let line: Vec<(f64, f64)> = (0..200).map(|i| (i as f64, 0.0)).collect();
    assert_eq!(simplify(&line, 0.9), vec![(0.0, 0.0), (199.0, 0.0)]);

    // An 8-connected diagonal is a staircase, every step half a pixel off the
    // line it describes. Left alone it is both expensive and visibly wobbly
    // in the ink.
    let stair: Vec<(f64, f64)> = (0..200).map(|i| (i as f64, i as f64)).collect();
    assert_eq!(simplify(&stair, 0.9).len(), 2);
}

/// ...but a curve is not a straight run, and the whole value of doing this by
/// deviation rather than by spacing is that the shape survives.
#[test]
fn a_curve_keeps_its_shape_within_the_tolerance() {
    let circle: Vec<(f64, f64)> = (0..=400)
        .map(|i| {
            let a = i as f64 / 400.0 * std::f64::consts::TAU;
            (100.0 + 80.0 * a.cos(), 100.0 + 80.0 * a.sin())
        })
        .collect();
    for eps in [0.5, 0.9, 3.0] {
        let simple = simplify(&circle, eps);
        assert!(simple.len() < circle.len() / 3, "eps {eps} kept {}", simple.len());
        let worst = circle
            .iter()
            .map(|&p| distance_to(&simple, p))
            .fold(0.0, f64::max);
        assert!(worst <= eps * 1.01, "eps {eps} moved the line by {worst:.2}");
    }
    // Endpoints are never dropped: a stroke that lost one would start or end
    // somewhere other than where it was traced.
    let simple = simplify(&circle, 0.9);
    assert_eq!(simple[0], circle[0]);
    assert_eq!(*simple.last().unwrap(), *circle.last().unwrap());
}

#[test]
fn simplifying_a_two_point_stroke_is_a_no_op() {
    let two = vec![(0.0, 0.0), (5.0, 5.0)];
    assert_eq!(simplify(&two, 10.0), two);
}

// ————— spurs —————

fn mask_of(w: usize, h: usize, on: &[(usize, usize)]) -> Mask {
    let mut data = vec![false; w * h];
    for &(x, y) in on {
        data[y * w + x] = true;
    }
    Mask { w, h, data }
}

/// Zhang-Suen leaves a barb wherever a stroke ends in anything but a point,
/// and a junction is such a place — so a page of text comes out furred with
/// two-pixel ticks that the pen draws as smudges around every letter.
///
/// The qualifier that keeps this from eating real marks is that a barb has to
/// run into a junction. A short piece that stands on its own is a mark: the
/// dot of an "i", a 点, a comma.
#[test]
fn spurs_are_cut_but_freestanding_marks_are_not() {
    let (w, h) = (60usize, 40usize);
    let mut on: Vec<(usize, usize)> = (2..50).map(|x| (x, 20)).collect();
    on.extend([(20, 17), (20, 18), (20, 19)]); // a three-pixel barb
    on.extend([(30, 5), (31, 5), (32, 5)]); // a mark of its own, touching nothing

    let pruned = prune_spurs(&mask_of(w, h, &on), 4);

    for x in 2..50 {
        assert!(pruned.data[20 * w + x], "the stroke lost ({x}, 20)");
    }
    for y in 17..=18 {
        assert!(!pruned.data[y * w + 20], "the barb kept (20, {y})");
    }
    for x in 30..=32 {
        assert!(pruned.data[5 * w + x], "a freestanding mark was cut at ({x}, 5)");
    }
}

/// A barb longer than the limit is a real stroke and stays whole.
#[test]
fn a_long_branch_is_a_stroke_not_a_spur() {
    let (w, h) = (60usize, 40usize);
    let mut on: Vec<(usize, usize)> = (2..50).map(|x| (x, 30)).collect();
    on.extend((10..30).map(|y| (20, y)));

    let pruned = prune_spurs(&mask_of(w, h, &on), 4);
    for y in 10..30 {
        assert!(pruned.data[y * w + 20], "cut into a real branch at (20, {y})");
    }
}

// ————— the pipeline, end to end on a shape —————

/// The skeleton of a thick bar is a line down the middle of it. Away from the
/// ends — where thinning always flares a little, and where the drawing does
/// not care — it should be exactly one pixel wide and exactly in the middle,
/// and it should come back out of the tracer as a stroke rather than as
/// fragments.
#[test]
fn a_thick_bar_thins_to_a_line_down_its_middle() {
    let (w, h) = (80usize, 80usize);
    let (x0, x1, y0, y1) = (34usize, 46usize, 10usize, 70usize);
    let on: Vec<(usize, usize)> =
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).collect();
    let skeleton = prune_spurs(&skeletonize(&mask_of(w, h, &on)), 4);

    let middle = (x0 + x1) / 2;
    for y in y0 + 8..y1 - 8 {
        let row: Vec<usize> = (0..w).filter(|&x| skeleton.data[y * w + x]).collect();
        assert_eq!(row.len(), 1, "row {y} is {} pixels wide: {row:?}", row.len());
        assert!(
            row[0].abs_diff(middle) <= 1,
            "row {y} skeletonized to x={} rather than the middle at {middle}",
            row[0]
        );
    }

    let strokes = trace_skeleton(&skeleton);
    let longest = strokes.iter().map(|s| s.len()).max().unwrap_or(0);
    assert!(
        longest > (y1 - y0) - 20,
        "the bar came back as fragments: {} strokes, longest {longest}",
        strokes.len()
    );
}
