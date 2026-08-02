//! Decode a `Plan`'s event stream back into points and check the properties
//! the scan-sync depends on. Everything downstream of `plan()` happens on
//! hardware, so these run against the bytes themselves.

use super::device::{self, Calib, Model, PAPER_PRO, RM2};
use super::draw;

/// Walk raw `input_event` records and pull out the (x, y) of every SYN that
/// had the pen down — i.e. the points the tablet will actually ink.
fn decode(c: &Calib, bytes: &[u8]) -> Vec<(i32, i32)> {
    let t = c.event_size - 8;
    let (mut x, mut y, mut down) = (0i32, 0i32, false);
    let mut pts = Vec::new();
    for e in bytes.chunks_exact(c.event_size) {
        let typ = u16::from_le_bytes([e[t], e[t + 1]]);
        let code = u16::from_le_bytes([e[t + 2], e[t + 3]]);
        let val = i32::from_le_bytes([e[t + 4], e[t + 5], e[t + 6], e[t + 7]]);
        match (typ, code) {
            (device::EV_ABS, device::ABS_X) => x = val,
            (device::EV_ABS, device::ABS_Y) => y = val,
            (device::EV_KEY, device::BTN_TOUCH) => down = val == 1,
            (device::EV_SYN, 0) if down => pts.push((x, y)),
            _ => {}
        }
    }
    pts
}

/// A shape with ink at every height, including one full-height vertical bar —
/// the case that motivated band-splitting, since traced whole it would be
/// drawn entirely while the scan was still at the top.
fn test_image() -> String {
    let (w, h) = (400u32, 560u32);
    let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255]));
    for y in 0..h {
        for x in 0..w {
            let bar = (60..70).contains(&x);
            let diag = ((x as i32 * 5 / 4) - y as i32).abs() < 5;
            let ring = {
                let (dx, dy) = (x as f64 - 260.0, y as f64 - 300.0);
                let d = (dx * dx + dy * dy).sqrt();
                (95.0..103.0).contains(&d)
            };
            if bar || diag || ring {
                img.put_pixel(x, y, image::Luma([0]));
            }
        }
    }
    // A distinct file per call. This used to be one fixed name, which is a
    // race: cargo runs these tests in parallel threads of one process, so two
    // of them would write the same path while a third read it half-written.
    // It only started failing when slower tests elsewhere shifted the timing
    // — the usual way a latent race announces itself.
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "rm-bin-draw-test-{}-{}.png",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    img.save(&path).unwrap();
    path.to_string_lossy().into_owned()
}

fn check(calib: Calib) {
    let path = test_image();
    let plan = draw::plan(&path, &calib).unwrap();
    let pts = decode(&calib, &plan.bytes);
    assert!(pts.len() > 500, "too few points: {}", pts.len());

    // 1. Nothing leaves the page.
    for &(x, y) in &pts {
        assert!((0..=calib.max_x as i32).contains(&x), "x out of range: {x}");
        assert!((0..=calib.max_y as i32).contains(&y), "y out of range: {y}");
    }

    // 2. Ink marches down the page. Screen-space v is what the window's scan
    //    tracks; on rM2 that's the *descending* pen_x axis, which is exactly
    //    the sort of flip this asserts against.
    let v_of = |&(x, y): &(i32, i32)| match calib.model {
        Model::Rm2 => 1.0 - x as f64 / calib.max_x,
        Model::PaperPro => y as f64 / calib.max_y,
    };
    // Within a band order is left-to-right, so compare band-to-band rather
    // than point-to-point: the last tenth must sit below the first tenth.
    let n = pts.len();
    let head = pts[..n / 10].iter().map(v_of).fold(f64::MIN, f64::max);
    let tail = pts[n - n / 10..].iter().map(v_of).fold(f64::MAX, f64::min);
    assert!(tail > head, "ink did not progress downward: head {head} tail {tail}");

    // 3. Progress counts strokes, monotonically, from 0 to all of them. The
    //    window indexes `draw-plan` with it, so a value that slipped backwards
    //    or past the end would un-draw or over-draw strokes.
    let total = plan.bytes.len();
    let count = plan.stroke_count();
    assert_eq!(plan.strokes_done(0), 0.0);
    assert_eq!(plan.strokes_done(total), count as f64);
    let mut last = 0.0;
    for i in 0..=400 {
        let d = plan.strokes_done(total * i / 400);
        assert!(d >= last - 1e-9, "progress went backwards: {last} -> {d}");
        assert!((0.0..=count as f64).contains(&d), "progress out of range: {d}");
        last = d;
    }

    // 5. The image fills the page down to the margin, and no further — the
    //    margin is what keeps strokes off xochitl's toolbar overlay, so this
    //    guards both "not squeezed into a corner" and "not full-bleed".
    let (lo, hi) = pts.iter().map(v_of).fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(v), h.max(v)));
    let span = hi - lo;
    assert!((0.74..=0.80).contains(&span), "image covers {span:.2} of the page, want ~0.78");
    assert!(lo > 0.09, "ink starts {lo:.3} from the top edge, inside the margin");
}

#[test]
fn paper_pro_plan_is_in_bounds_and_top_down() {
    check(PAPER_PRO);
}

#[test]
fn rm2_plan_is_in_bounds_and_top_down() {
    check(RM2);
}

/// The push is paced, so the byte count *is* the drawing time — and that
/// time is also how long the window's scan animation runs. The dials are the
/// raster size `page_fit` picks, `EPSILON` and `MAX_PEN_SAMPLES`; this is the
/// guard that keeps a careless nudge to any of them from turning a drop into
/// a ten-minute wait.
#[test]
fn draw_time_stays_bounded() {
    for calib in [PAPER_PRO, RM2] {
        let plan = draw::plan(&test_image(), &calib).unwrap();
        let events = plan.bytes.len() as f64 / calib.event_size as f64;
        let secs = events / 20.0 * 0.004;
        println!(
            "{:?}: {} strokes, {events:.0} events, ~{secs:.1}s to draw",
            calib.model,
            plan.stroke_count()
        );
        assert!(secs < 120.0, "{:?} would take {secs:.0}s", calib.model);
    }
}

/// Simplifying the strokes is what pays for tracing at the panel's own
/// resolution: the raw trace puts a point on every pixel, and the event
/// encoder re-samples at the digitizer's step, which is several pixels wide.
/// If simplification ever stopped happening the drawing would still be
/// correct and would take some five times as long — a regression with no
/// symptom other than tedium, so it gets a number.
#[test]
fn simplification_pays_for_the_resolution() {
    for calib in [PAPER_PRO, RM2] {
        let plan = draw::plan(&test_image(), &calib).unwrap();
        let emitted = decode(&calib, &plan.bytes).len();
        // The test card is ~2900px of ink once placed on the page; at one
        // sample per digitizer step that is a few thousand points, where the
        // unsimplified trace would be a few tens of thousands.
        assert!(
            emitted < 12_000,
            "{:?}: {emitted} samples — simplification is not running",
            calib.model
        );
    }
}

/// Aspect ratio must survive the trip to pen space — on Paper Pro the two
/// axes are ~4% anisotropic, and on rM2 the whole frame is rotated, so a
/// naive uniform scale in pen units would visibly squash the image.
#[test]
fn aspect_ratio_is_preserved_in_screen_space() {
    for calib in [PAPER_PRO, RM2] {
        let plan = draw::plan(&test_image(), &calib).unwrap();
        let pts = decode(&calib, &plan.bytes);
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &(x, y) in &pts {
            let (u, v) = match calib.model {
                Model::Rm2 => (y as f64 / calib.max_y, 1.0 - x as f64 / calib.max_x),
                Model::PaperPro => (x as f64 / calib.max_x, y as f64 / calib.max_y),
            };
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let drawn = ((u1 - u0) * calib.screen_w) / ((v1 - v0) * calib.screen_h);
        let source = 400.0 / 560.0;
        assert!(
            (drawn / source - 1.0).abs() < 0.06,
            "{:?}: aspect {drawn:.3} vs source {source:.3}",
            calib.model
        );
    }
}

/// Every write to an evdev node must be a whole number of `struct
/// input_event`s or the kernel rejects it with EINVAL — and the tablet's
/// `input_event` is 16 bytes on the rM2 and 24 on Paper Pro. This is what
/// makes `cat > /dev/input/eventN` (a 4096-byte writer) work on the rM2 and
/// fail on Paper Pro, which cost a debugging session: 4096 divides by 16 but
/// not by 24. `push` sends `event_size * 20` chunks into a remote
/// `dd iflag=fullblock` of the same block size, so alignment holds end to
/// end — provided both the chunk and the total stay multiples of the event.
#[test]
fn event_stream_is_write_aligned() {
    for calib in [PAPER_PRO, RM2] {
        let plan = draw::plan(&test_image(), &calib).unwrap();
        assert_eq!(
            plan.bytes.len() % calib.event_size,
            0,
            "{:?}: {} bytes is not a whole number of {}-byte events",
            calib.model,
            plan.bytes.len(),
            calib.event_size
        );
        // ...and so is the tail dd is left holding after the last full block.
        let chunk = calib.event_size * 20;
        assert_eq!(plan.bytes.len() % chunk % calib.event_size, 0);
    }
}

/// Hardware check, run by hand: draws an orientation test card on a real
/// tablet, so a flipped axis or a swapped pen frame shows up as a visibly
/// different picture rather than a passing test.
///
///     RM_HOST=10.0.0.113 cargo test --release --lib e2e -- --ignored --nocapture
#[test]
#[ignore]
fn e2e() {
    let (w, h) = (600u32, 800u32);
    let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255]));
    let mut ink = |x: i32, y: i32| {
        if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
            img.put_pixel(x as u32, y as u32, image::Luma([0]));
        }
    };
    // frame, a diagonal from the top-left, a circle in the upper-left
    // quadrant, a bar near the bottom — every flip and swap reads differently
    for t in 0..2 {
        for x in 0..w as i32 {
            ink(x, 8 + t);
            ink(x, h as i32 - 9 - t);
        }
        for y in 0..h as i32 {
            ink(8 + t, y);
            ink(w as i32 - 9 - t, y);
        }
    }
    for i in 0..1000 {
        let t = i as f64 / 1000.0;
        ink((30.0 + t * 540.0) as i32, (30.0 + t * 740.0) as i32);
    }
    for i in 0..2000 {
        let a = i as f64 / 2000.0 * std::f64::consts::TAU;
        ink((180.0 + 90.0 * a.cos()) as i32, (200.0 + 90.0 * a.sin()) as i32);
    }
    for y in 700..716 {
        for x in 60..300 {
            ink(x, y);
        }
    }
    let path = std::env::temp_dir().join("rm-bin-e2e.png");
    img.save(&path).unwrap();

    let host = std::env::var("RM_HOST").expect("set RM_HOST to the tablet's address");
    let calib = device::detect(&host, 22).unwrap();
    let plan = draw::plan(path.to_str().unwrap(), &calib).unwrap();
    println!("{:?}: {} strokes, {} bytes", calib.model, plan.stroke_count(), plan.bytes.len());

    let t = std::time::Instant::now();
    let mut tick = 0;
    let count = plan.stroke_count() as f64;
    device::push(&host, 22, &calib, &plan.bytes, |n| {
        let p = plan.strokes_done(n) / count;
        if (p * 10.0) as i32 > tick {
            tick = (p * 10.0) as i32;
            println!("  {:>5.1}s  {:>3.0}%", t.elapsed().as_secs_f64(), p * 100.0);
        }
    })
    .unwrap();
    println!("done in {:.1}s", t.elapsed().as_secs_f64());
}


/// Split the event stream back into individual strokes on pen-up.
fn decode_strokes(c: &Calib, bytes: &[u8]) -> Vec<Vec<(i32, i32)>> {
    let t = c.event_size - 8;
    let (mut x, mut y, mut down) = (0i32, 0i32, false);
    let (mut out, mut cur) = (Vec::new(), Vec::new());
    for e in bytes.chunks_exact(c.event_size) {
        let typ = u16::from_le_bytes([e[t], e[t + 1]]);
        let code = u16::from_le_bytes([e[t + 2], e[t + 3]]);
        let val = i32::from_le_bytes([e[t + 4], e[t + 5], e[t + 6], e[t + 7]]);
        match (typ, code) {
            (device::EV_ABS, device::ABS_X) => x = val,
            (device::EV_ABS, device::ABS_Y) => y = val,
            (device::EV_KEY, device::BTN_TOUCH) => {
                down = val == 1;
                if !down && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (device::EV_SYN, 0) if down => cur.push((x, y)),
            _ => {}
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The window draws `Plan::preview` while the tablet draws `Plan::bytes`, and
/// the whole point of the redesign is that those are the same strokes in the
/// same order. Nothing in the type system says so, so check it: one preview
/// stroke per emitted stroke, and stroke *i* in the same place in both.
#[test]
fn preview_tracks_the_strokes_being_drawn() {
    for calib in [PAPER_PRO, RM2] {
        let plan = draw::plan(&test_image(), &calib).unwrap();
        let drawn = decode_strokes(&calib, &plan.bytes);
        assert_eq!(drawn.len(), plan.preview.len(), "{:?}: preview/stroke count", calib.model);
        assert_eq!(drawn.len(), plan.stroke_count());

        // Pen space differs per model and preview space is the image's own
        // frame, so compare where each stroke *starts and ends*, normalised
        // into the whole drawing's bounding box — that cancels both frames out
        // and still catches a reordering, an axis swap or a flip.
        //
        // Endpoints rather than centroids, which is what this used to compare.
        // A centroid is only comparable between two samplings of a stroke when
        // both are evenly spaced, and neither of these is any more: the preview
        // is simplified by deviation, so its points cluster where the stroke
        // bends, while the drawn one is re-sampled evenly at the digitizer's
        // step. The first and last point, by contrast, are exact in both.
        let screen = |&(x, y): &(i32, i32)| match calib.model {
            Model::Rm2 => (y as f64 / calib.max_y, 1.0 - x as f64 / calib.max_x),
            Model::PaperPro => (x as f64 / calib.max_x, y as f64 / calib.max_y),
        };
        let norm = |cs: Vec<(f64, f64)>| {
            let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for &(x, y) in &cs {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
            cs.iter().map(|&(x, y)| ((x - x0) / (x1 - x0), (y - y0) / (y1 - y0))).collect::<Vec<_>>()
        };

        let a = norm(
            drawn
                .iter()
                .flat_map(|s| [screen(&s[0]), screen(s.last().unwrap())])
                .collect(),
        );
        let b = norm(
            plan.preview
                .iter()
                .flat_map(|s| {
                    let end = *s.last().unwrap();
                    [s[0], end].map(|[x, y]| (x as f64, y as f64))
                })
                .collect(),
        );
        for (i, (p, q)) in a.iter().zip(&b).enumerate() {
            let d = ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
            assert!(
                d < 0.01,
                "{:?}: stroke {}'s {} end is at {p:?} but previews at {q:?}",
                calib.model,
                i / 2,
                if i % 2 == 0 { "start" } else { "finish" }
            );
        }

        // Simplifying must not empty a stroke out — a preview stroke of one
        // point draws nothing — and must stay well under what is drawn, since
        // the whole preview crosses the IPC boundary in one message and the
        // window it lands in is a hundred pixels across.
        assert!(plan.preview.iter().all(|s| s.len() >= 2), "{:?}: simplified a stroke away", calib.model);
        let (kept, traced): (usize, usize) =
            (plan.preview.iter().map(|s| s.len()).sum(), drawn.iter().map(|s| s.len()).sum());
        assert!(kept * 2 < traced, "{:?}: preview kept {kept} of {traced} points", calib.model);
    }
}

