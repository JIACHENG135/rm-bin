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
    let total = gray.len() as f64;
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

/// Zhang-Suen thinning (Zhang, T.Y. & Suen, C.Y., 1984) — the standard
/// two-subiteration skeletonization algorithm; scikit-image's skeletonize
/// uses a related fast implementation of the same idea. Iterates until no
/// pixel is removed.
pub fn skeletonize(mask: &Mask) -> Mask {
    let (w, h) = (mask.w, mask.h);
    let mut data = mask.data.clone();
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
            for y in 0..h {
                for x in 0..w {
                    if !data[y * w + x] {
                        continue;
                    }
                    let (xi, yi) = (x as i32, y as i32);
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
                    for i in 0..8 {
                        if !n[i] && n[(i + 1) % 8] {
                            a += 1;
                        }
                    }
                    let cond34 = if sub_iter == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    if (2..=6).contains(&b) && a == 1 && cond34 {
                        to_remove.push(y * w + x);
                    }
                }
            }
            if !to_remove.is_empty() {
                changed = true;
                for i in to_remove {
                    data[i] = false;
                }
            }
        }
        if !changed {
            break;
        }
    }
    Mask { w, h, data }
}

const LOOKAHEAD: usize = 4;

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
        let mut v = Vec::with_capacity(8);
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                if self.mask.get(x + dx, y + dy) {
                    v.push((x + dx, y + dy));
                }
            }
        }
        v
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

        *candidates
            .iter()
            .max_by(|&&a, &&b| {
                let mut lv_a: HashSet<(i32, i32)> = path.iter().cloned().collect();
                lv_a.insert((cx, cy));
                let end_a = self.lookahead_point(a, (cx, cy), &mut lv_a);
                let score_a = straightness((cx, cy), pvx, pvy, plen, end_a);

                let mut lv_b: HashSet<(i32, i32)> = path.iter().cloned().collect();
                lv_b.insert((cx, cy));
                let end_b = self.lookahead_point(b, (cx, cy), &mut lv_b);
                let score_b = straightness((cx, cy), pvx, pvy, plen, end_b);

                score_a.partial_cmp(&score_b).unwrap()
            })
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
        all_points.iter().cloned().filter(|&(x, y)| tracer.neighbors(x, y).len() == 1).collect();
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
        loop {
            let candidates: Vec<(i32, i32)> =
                tracer.neighbors(cx, cy).into_iter().filter(|&(x, y)| !visited[idx(x, y)]).collect();
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
