//! Threshold -> skeletonize -> trace, replacing scikit-image's
//! threshold_otsu/skeletonize and rm_capture.py's _trace_skeleton — hand
//! rolled since there's no equivalent crate ecosystem as mature as
//! scikit-image, but all three algorithms are standard and well-understood.

use std::collections::HashSet;

/// Otsu's method: find the grayscale threshold that maximizes between-class
/// variance. Standard single-pass histogram algorithm.
pub fn otsu_threshold(gray: &[u8]) -> u8 {
    let mut hist = [0u32; 256];
    for &p in gray {
        hist[p as usize] += 1;
    }
    otsu_from_hist(&hist, gray.len() as f64)
}

/// The same, from a histogram that has already been gathered — which is how
/// `adaptive_threshold_mask` gets one per tile without walking the image once
/// per tile.
fn otsu_from_hist(hist: &[u32; 256], total: f64) -> u8 {
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();

    let mut sum_b = 0.0;
    let mut w_b = 0.0;
    let mut max_var = -1.0;
    let mut threshold = 0u8;
    for t in 0..256 {
        w_b += hist[t] as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f <= 0.0 {
            break;
        }
        sum_b += t as f64 * hist[t] as f64;
        let m_b = sum_b / w_b;
        let m_f = (sum_all - sum_b) / w_f;
        let var_between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if var_between > max_var {
            max_var = var_between;
            threshold = t as u8;
        }
    }
    threshold
}

#[derive(Clone)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<bool>,
}

impl Mask {
    fn get(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            false
        } else {
            self.data[y as usize * self.w + x as usize]
        }
    }
}

/// dark pixels = ink, matching Python's `arr < threshold_otsu(arr)`.
pub fn threshold_mask(gray: &[u8], w: usize, h: usize, threshold: u8) -> Mask {
    Mask { w, h, data: gray.iter().map(|&p| p < threshold).collect() }
}

/// Side of the square tiles the paper level is sampled on, as a fraction of
/// the image's long edge. Small enough that a lighting gradient is roughly
/// constant across one tile, large enough that a tile of dense text still has
/// bare paper in it.
const TILE_FRACTION: f64 = 0.1;
const MIN_TILE: usize = 48;
/// The percentile of a tile taken to *be* the paper. Ink is the dark minority
/// of a page; the brightest tenth of a tile is paper unless the tile is almost
/// entirely covered, which is the case `PAPER_FLOOR` catches.
const PAPER_PCT: f64 = 0.90;
/// How dark a tile's paper estimate may get, relative to the brightest paper
/// found anywhere, before it stops being believed. A tile inside a large solid
/// mark has no paper in it at all and would otherwise measure its own ink as
/// paper — and then, divided through by itself, come out blank.
const PAPER_FLOOR: f64 = 0.35;

/// One threshold for the whole image is only right when the whole image is lit
/// the same way.
///
/// A single Otsu cut is what the tracer used to run on, and it is fine for a
/// screenshot. On a photograph of a page it is not: the cut that separates ink
/// from paper in the bright half sits above the *paper* in the shadowed half,
/// so a whole corner of the image comes out solid black and the skeletonizer
/// returns one long meaningless line down the shadow's edge.
///
/// Rather than vary the threshold, this flattens the thing being thresholded:
/// estimate how bright the paper is in each tile, interpolate that into a
/// smooth illumination surface, divide it out, and then take a single Otsu of
/// the result. It is the same idea as a flat-field correction, and it is
/// preferable to a per-tile threshold because it never has to decide what an
/// empty tile means — an empty tile simply reports the paper it is made of and
/// normalises to white.
///
/// (A per-tile Otsu *was* the first version. It classified each tile as ink or
/// paper by its histogram, and on a photograph with a shadow it duly decided
/// the shadow was ink — reproducing, from the other direction, exactly the
/// artefact it was written to remove.)
///
/// On an evenly lit image the surface is flat and this reduces to the global
/// Otsu it replaced, which is what makes it safe everywhere rather than behind
/// a setting.
pub fn adaptive_threshold_mask(gray: &[u8], w: usize, h: usize) -> Mask {
    let tile = (((w.max(h)) as f64 * TILE_FRACTION) as usize).max(MIN_TILE);
    let (cols, rows) = (w.div_ceil(tile), h.div_ceil(tile));
    // Too few tiles to interpolate between: one threshold is all there is.
    if cols < 2 || rows < 2 {
        return threshold_mask(gray, w, h, otsu_threshold(gray));
    }

    let mut paper = vec![0f64; cols * rows];
    for ty in 0..rows {
        for tx in 0..cols {
            let (x1, y1) = (((tx + 1) * tile).min(w), ((ty + 1) * tile).min(h));
            let mut hist = [0u32; 256];
            let mut n = 0u32;
            for y in ty * tile..y1 {
                for x in tx * tile..x1 {
                    hist[gray[y * w + x] as usize] += 1;
                    n += 1;
                }
            }
            paper[ty * cols + tx] = percentile(&hist, n, PAPER_PCT);
        }
    }
    let floor = (paper.iter().copied().fold(0.0, f64::max) * PAPER_FLOOR).max(1.0);
    for p in &mut paper {
        *p = p.max(floor);
    }

    // Bilinear between tile centres. The x weights repeat on every row, so
    // they're computed once — this is the innermost loop in the pipeline.
    let axis = |v: usize, count: usize| {
        let f = ((v as f64 + 0.5) / tile as f64 - 0.5).clamp(0.0, (count - 1) as f64);
        let i = (f.floor() as usize).min(count - 2);
        (i, f - i as f64)
    };
    let xs: Vec<(usize, f64)> = (0..w).map(|x| axis(x, cols)).collect();

    let mut flat = vec![0u8; w * h];
    for y in 0..h {
        let (j, wy) = axis(y, rows);
        let (top, bot) = (&paper[j * cols..], &paper[(j + 1) * cols..]);
        for x in 0..w {
            let (i, wx) = xs[x];
            let a = top[i] * (1.0 - wx) + top[i + 1] * wx;
            let b = bot[i] * (1.0 - wx) + bot[i + 1] * wx;
            let level = a * (1.0 - wy) + b * wy;
            flat[y * w + x] = (gray[y * w + x] as f64 * 255.0 / level).clamp(0.0, 255.0) as u8;
        }
    }
    threshold_mask(&flat, w, h, otsu_threshold(&flat))
}

/// The grey level at `pct` of a histogram's mass.
fn percentile(hist: &[u32; 256], total: u32, pct: f64) -> f64 {
    let want = pct * total as f64;
    let mut acc = 0f64;
    for (i, &c) in hist.iter().enumerate() {
        acc += c as f64;
        if acc >= want {
            return i as f64;
        }
    }
    255.0
}

/// Zhang-Suen thinning (Zhang, T.Y. & Suen, C.Y., 1984) — the standard
/// two-subiteration skeletonization algorithm; scikit-image's skeletonize
/// uses a related fast implementation of the same idea. Iterates until no
/// pixel is removed.
///
/// The one departure from the textbook loop is that each pass walks a list of
/// the pixels that are still ink rather than the whole grid. It computes the
/// same thing — a pixel that is not ink can never be removed — but the cost
/// becomes proportional to the ink instead of to the image, and ink is a few
/// percent of a page. That is what makes tracing at the panel's own
/// resolution affordable; on the full grid, quadrupling the raster quadrupled
/// every pass whether or not there was anything in it.
pub fn skeletonize(mask: &Mask) -> Mask {
    let (w, h) = (mask.w, mask.h);
    let mut data = mask.data.clone();
    let mut ink: Vec<u32> =
        (0..w * h).filter(|&i| data[i]).map(|i| i as u32).collect();
    let get = |data: &[bool], x: i32, y: i32| -> bool {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            false
        } else {
            data[y as usize * w + x as usize]
        }
    };

    loop {
        let mut changed = false;
        for sub_iter in 0..2 {
            let mut to_remove = Vec::new();
            for &i in &ink {
                let i = i as usize;
                if !data[i] {
                    continue;
                }
                let (xi, yi) = ((i % w) as i32, (i / w) as i32);
                let p2 = get(&data, xi, yi - 1);
                let p3 = get(&data, xi + 1, yi - 1);
                let p4 = get(&data, xi + 1, yi);
                let p5 = get(&data, xi + 1, yi + 1);
                let p6 = get(&data, xi, yi + 1);
                let p7 = get(&data, xi - 1, yi + 1);
                let p8 = get(&data, xi - 1, yi);
                let p9 = get(&data, xi - 1, yi - 1);
                let n = [p2, p3, p4, p5, p6, p7, p8, p9];
                let b: u32 = n.iter().map(|&v| v as u32).sum();
                let mut a = 0;
                for k in 0..8 {
                    if !n[k] && n[(k + 1) % 8] {
                        a += 1;
                    }
                }
                let cond34 = if sub_iter == 0 {
                    !(p2 && p4 && p6) && !(p4 && p6 && p8)
                } else {
                    !(p2 && p4 && p8) && !(p2 && p6 && p8)
                };
                if (2..=6).contains(&b) && a == 1 && cond34 {
                    to_remove.push(i);
                }
            }
            if !to_remove.is_empty() {
                changed = true;
                for i in to_remove {
                    data[i] = false;
                }
            }
        }
        ink.retain(|&i| data[i as usize]);
        if !changed {
            break;
        }
    }
    Mask { w, h, data }
}

/// The eight neighbours of a pixel that are ink, into a fixed buffer.
///
/// A `Vec` here would be an allocation per pixel per visit, and this is called
/// several times for every skeleton pixel in the image.
fn neighbors8(mask: &Mask, x: i32, y: i32) -> ([(i32, i32); 8], usize) {
    let mut out = [(0i32, 0i32); 8];
    let mut n = 0;
    for dy in -1..=1 {
        for dx in -1..=1 {
            if (dx != 0 || dy != 0) && mask.get(x + dx, y + dy) {
                out[n] = (x + dx, y + dy);
                n += 1;
            }
        }
    }
    (out, n)
}

/// Cut the hairs off a skeleton.
///
/// Zhang-Suen leaves a short barb wherever a stroke ends in anything but a
/// clean point, and a stroke junction — every crossing in a Latin "k", every
/// one of the several in a dense hanzi — is exactly such a place. Each barb
/// then becomes its own stroke in the tracer's output, so a page of text comes
/// out furred with two-pixel ticks that the pen dutifully draws and that read,
/// at pen width, as smudges around every letter.
///
/// A barb is a run of length `max_len` or less that starts at a free end and
/// walks into a fork. The two qualifiers are what keep this from eating real
/// marks: a run that never reaches a fork is a free-standing piece — the dot
/// of an "i", a 点, a comma — and is left exactly as it is, however short.
pub fn prune_spurs(mask: &Mask, max_len: usize) -> Mask {
    let mut m = mask.clone();
    // Removing a barb can leave the pixel it hung off with a free end of its
    // own, which is a barb one layer down. Two rounds settles the cases that
    // occur in practice without the cost of iterating to a fixed point.
    for _ in 0..2 {
        let mut kill: Vec<(i32, i32)> = Vec::new();
        for idx in 0..m.w * m.h {
            if !m.data[idx] {
                continue;
            }
            let (x, y) = ((idx % m.w) as i32, (idx / m.w) as i32);
            let (_, deg) = neighbors8(&m, x, y);
            if deg != 1 {
                continue;
            }
            let mut path = vec![(x, y)];
            let mut prev: Option<(i32, i32)> = None;
            // How much of `path` is barb, once a fork has been found — the
            // fork pixel itself belongs to the stroke that carries it.
            let mut barb = 0usize;
            loop {
                let &(cx, cy) = path.last().unwrap();
                let (nb, n) = neighbors8(&m, cx, cy);
                let mut ahead = nb[..n].iter().filter(|&&p| Some(p) != prev);
                let Some(&next) = ahead.next() else { break };
                if ahead.next().is_some() {
                    // Two ways on from here: this pixel is the fork.
                    barb = path.len() - 1;
                    break;
                }
                if neighbors8(&m, next.0, next.1).1 >= 3 {
                    // The next one is. Checking it from here as well as from
                    // there matters because the skeleton is 8-connected: a
                    // barb's last pixel often touches two pixels of the stroke
                    // it hangs off, and only one of the two tests sees that as
                    // a single way forward.
                    barb = path.len();
                    break;
                }
                if path.len() > max_len {
                    break;
                }
                prev = Some((cx, cy));
                path.push(next);
            }
            if barb > 0 && barb <= max_len {
                kill.extend_from_slice(&path[..barb]);
            }
        }
        if kill.is_empty() {
            break;
        }
        for (x, y) in kill {
            m.data[y as usize * m.w + x as usize] = false;
        }
    }
    m
}

/// Ramer-Douglas-Peucker: drop the points of a polyline that sit within
/// `epsilon` of the line their neighbours already describe.
///
/// This matters more than it looks. A skeleton is an 8-connected staircase, so
/// a straight line arrives as one point per pixel, each a fraction of a pixel
/// off the true line — and `device::stroke_events` re-samples whatever it is
/// given at the digitizer's own step, which is several pixels. So the raw
/// trace spends five or six events where one would do, and the drawing takes
/// five or six times as long as the picture needs. Simplifying first is what
/// decouples how long a drawing takes from what resolution it was traced at,
/// and that in turn is what makes tracing at the panel's own resolution
/// possible at all. It also removes the staircase itself, which is visible in
/// the ink as a wobble along every straight edge.
///
/// Iterative rather than recursive: a traced stroke can be tens of thousands
/// of points long and the recursion depth is unbounded in the input.
pub fn simplify(pts: &[(f64, f64)], epsilon: f64) -> Vec<(f64, f64)> {
    if pts.len() <= 2 || epsilon <= 0.0 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;

    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let ((ax, ay), (bx, by)) = (pts[a], pts[b]);
        let (dx, dy) = (bx - ax, by - ay);
        let span = (dx * dx + dy * dy).sqrt();
        let (mut worst, mut at) = (0.0f64, a);
        for (i, &(px, py)) in pts.iter().enumerate().take(b).skip(a + 1) {
            // Distance to the segment's line — or to the shared endpoint when
            // the "segment" is a closed loop's start and end, which is the
            // one case where the line is undefined.
            let d = if span > 0.0 {
                ((px - ax) * dy - (py - ay) * dx).abs() / span
            } else {
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
            };
            if d > worst {
                worst = d;
                at = i;
            }
        }
        if worst > epsilon {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

const LOOKAHEAD: usize = 4;
/// How much of the path already walked the branch scorer keeps in view.
///
/// The scorer needs to know which pixels are spoken for so its trial walk
/// doesn't double back along the stroke it came from — but the trial walk is
/// only `LOOKAHEAD` steps, so it cannot reach a pixel more than that many away,
/// and anything further back is unreachable by construction. Carrying the
/// whole path instead, which is what this used to do, made every branch cost a
/// copy of the stroke so far: quadratic in stroke length, and stroke length
/// grows with the raster.
const LOCAL_TAIL: usize = 4 * LOOKAHEAD;

struct Tracer<'a> {
    mask: &'a Mask,
}

fn straightness(cur: (i32, i32), dvx: f64, dvy: f64, dlen: f64, p: (i32, i32)) -> f64 {
    let vx = (p.0 - cur.0) as f64;
    let vy = (p.1 - cur.1) as f64;
    let vlen = (vx * vx + vy * vy).sqrt().max(1.0);
    (dvx * vx + dvy * vy) / (dlen * vlen)
}

impl<'a> Tracer<'a> {
    fn neighbors(&self, x: i32, y: i32) -> Vec<(i32, i32)> {
        let (n, k) = neighbors8(self.mask, x, y);
        n[..k].to_vec()
    }

    /// Simulate walking a few more steps past a candidate, always taking
    /// the locally-straightest continuation, and return where that lands.
    /// Judges a branch candidate by where it actually leads rather than
    /// its immediate 1-pixel direction, which is too noisy right at a
    /// crossing (a handful of pixels can't reliably distinguish two arms
    /// only a few degrees apart).
    fn lookahead_point(
        &self,
        mut cur: (i32, i32),
        mut last: (i32, i32),
        local_visited: &mut HashSet<(i32, i32)>,
    ) -> (i32, i32) {
        for _ in 0..LOOKAHEAD {
            local_visited.insert(cur);
            let nbrs: Vec<(i32, i32)> =
                self.neighbors(cur.0, cur.1).into_iter().filter(|p| !local_visited.contains(p)).collect();
            let Some(&first) = nbrs.first() else { break };
            let nxt = if nbrs.len() == 1 {
                first
            } else {
                let dvx = (cur.0 - last.0) as f64;
                let dvy = (cur.1 - last.1) as f64;
                let dlen = (dvx * dvx + dvy * dvy).sqrt().max(1.0);
                *nbrs
                    .iter()
                    .max_by(|&&a, &&b| {
                        straightness(cur, dvx, dvy, dlen, a)
                            .partial_cmp(&straightness(cur, dvx, dvy, dlen, b))
                            .unwrap()
                    })
                    .unwrap()
            };
            last = cur;
            cur = nxt;
        }
        cur
    }

    /// At a branch point there's more than one unvisited neighbor — e.g.
    /// the digit "2"'s T-junction, or the self-crossing of an "x". Prefer
    /// whichever candidate leads most nearly straight from the incoming
    /// direction, both ends measured over a short run of pixels (not just
    /// one) so single-pixel quantization noise right at the crossing
    /// doesn't flip the decision.
    fn pick_next(&self, cx: i32, cy: i32, path: &[(i32, i32)], candidates: &[(i32, i32)]) -> (i32, i32) {
        if candidates.len() == 1 {
            return candidates[0];
        }
        let k = path.len().min(LOOKAHEAD);
        if k == 0 {
            return candidates[0];
        }
        let (px, py) = path[path.len() - k];
        let pvx = (cx - px) as f64;
        let pvy = (cy - py) as f64;
        let plen = (pvx * pvx + pvy * pvy).sqrt().max(1.0);

        let tail = &path[path.len().saturating_sub(LOCAL_TAIL)..];
        let score = |c: (i32, i32)| {
            let mut seen: HashSet<(i32, i32)> = tail.iter().cloned().collect();
            seen.insert((cx, cy));
            let end = self.lookahead_point(c, (cx, cy), &mut seen);
            straightness((cx, cy), pvx, pvy, plen, end)
        };
        *candidates
            .iter()
            .max_by(|&&a, &&b| score(a).partial_cmp(&score(b)).unwrap())
            .unwrap()
    }
}

/// Trace a 1px-wide skeleton mask into ordered polyline strokes: find
/// endpoints (degree-1 pixels) as stroke starts, walk 8-connected unvisited
/// neighbors until a dead end, then mop up any leftover pixels (closed
/// loops). Naturally splits at branch points instead of forcing one
/// continuous path through a sharp corner — matches how a real hand draws,
/// and avoids corner-overshoot distortion. Direct port of rm_capture.py's
/// _trace_skeleton (already validated on real hardware there).
pub fn trace_skeleton(mask: &Mask) -> Vec<Vec<(i32, i32)>> {
    let tracer = Tracer { mask };
    let (w, h) = (mask.w, mask.h);

    let mut all_points = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if mask.data[y * w + x] {
                all_points.push((x as i32, y as i32));
            }
        }
    }
    let mut starts: Vec<(i32, i32)> =
        all_points.iter().cloned().filter(|&(x, y)| neighbors8(mask, x, y).1 == 1).collect();
    starts.extend(all_points.iter().cloned());

    let mut visited = vec![false; w * h];
    let idx = |x: i32, y: i32| y as usize * w + x as usize;

    let mut strokes = Vec::new();
    for (sx, sy) in starts {
        if visited[idx(sx, sy)] {
            continue;
        }
        let mut path = vec![(sx, sy)];
        visited[idx(sx, sy)] = true;
        let (mut cx, mut cy) = (sx, sy);
        let mut candidates: Vec<(i32, i32)> = Vec::with_capacity(8);
        loop {
            let (nb, n) = neighbors8(mask, cx, cy);
            candidates.clear();
            candidates.extend(nb[..n].iter().copied().filter(|&(x, y)| !visited[idx(x, y)]));
            if candidates.is_empty() {
                break;
            }
            let nxt = tracer.pick_next(cx, cy, &path, &candidates);
            visited[idx(nxt.0, nxt.1)] = true;
            path.push(nxt);
            cx = nxt.0;
            cy = nxt.1;
        }
        if path.len() >= 3 {
            strokes.push(path);
        }
    }
    strokes
}
