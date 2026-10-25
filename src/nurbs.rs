//! NURBS-style curves (non-rational B-splines) and curve → polygon surface tools.
//!
//! A curve is a list of CVs, a degree (1 = polyline, 3 = Maya's default cubic) and either an
//! explicit knot vector (open curves) or the default one: clamped uniform for open curves,
//! periodic uniform for closed ones (the first `degree` CVs wrap around).
//!
//! Surfaces (revolve, loft, extrude, planar) are generated as polygon meshes with UVs and keep
//! construction history: moving a CV of an input curve rebuilds the mesh.

use crate::mesh::Mesh;
use crate::scene::{NodeId, Scene, Shape};
use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------------------ curve

#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub cvs: Vec<Vec3>,
    pub degree: usize,
    pub closed: bool,
    /// Full knot vector (open curves, `cvs + degree + 1` values); empty = default.
    pub knots: Vec<f32>,
}

pub fn default_degree() -> u32 {
    1
}

impl Curve {
    pub fn new(cvs: Vec<Vec3>, degree: usize, closed: bool) -> Curve {
        Curve { cvs, degree, closed, knots: Vec::new() }.normalized()
    }

    pub fn from_shape(shape: &Shape) -> Option<Curve> {
        match shape {
            Shape::Curve { points, closed, degree, knots } => Some(Curve { cvs: points.clone(), degree: *degree as usize, closed: *closed, knots: knots.clone() }.normalized()),
            _ => None,
        }
    }

    pub fn into_shape(self) -> Shape {
        let c = self.normalized();
        Shape::Curve { points: c.cvs, closed: c.closed, degree: c.degree as u32, knots: c.knots }
    }

    /// Clamp the degree to what the CV count allows and drop knot vectors of the wrong size.
    pub fn normalized(mut self) -> Curve {
        let n = self.cvs.len();
        let max_deg = n.saturating_sub(1);
        self.degree = self.degree.clamp(1, max_deg.max(1));
        if self.closed || self.knots.len() != n + self.degree + 1 || self.knots.windows(2).any(|w| w[1] < w[0]) {
            self.knots.clear();
        }
        self
    }

    pub fn is_valid(&self) -> bool {
        self.cvs.len() >= 2 && (!self.closed || self.cvs.len() >= 3)
    }

    /// Spans (Maya's `spans` attribute): CVs − degree for open curves, CVs for closed.
    pub fn spans(&self) -> usize {
        if self.closed { self.cvs.len() } else { self.cvs.len().saturating_sub(self.degree) }
    }

    /// Control points and knots actually evaluated (periodic curves wrap their CVs).
    fn eval_data(&self) -> (Vec<Vec3>, Vec<f32>) {
        let p = self.degree;
        let n = self.cvs.len();
        if self.closed {
            let mut q = self.cvs.clone();
            q.extend_from_slice(&self.cvs[..p.min(n)]);
            let m = q.len();
            let u = (0..m + p + 1).map(|i| i as f32 - p as f32).collect();
            (q, u)
        } else if self.knots.len() == n + p + 1 {
            (self.cvs.clone(), self.knots.clone())
        } else {
            (self.cvs.clone(), clamped_uniform(n, p))
        }
    }

    pub fn knot_vector(&self) -> Vec<f32> {
        self.eval_data().1
    }

    /// Parameter range (min, max).
    pub fn domain(&self) -> (f32, f32) {
        let (q, u) = self.eval_data();
        (u[self.degree], u[q.len()])
    }

    /// Point at parameter `t` (clamped to the domain).
    pub fn point(&self, t: f32) -> Vec3 {
        if self.cvs.is_empty() {
            return Vec3::ZERO;
        }
        if self.cvs.len() == 1 {
            return self.cvs[0];
        }
        let (q, u) = self.eval_data();
        de_boor(&q, &u, self.degree, t)
    }

    /// First derivative (finite difference, inside the domain).
    pub fn tangent(&self, t: f32) -> Vec3 {
        let (a, b) = self.domain();
        let h = (b - a) * 1e-3;
        let (t0, t1) = if self.closed { (t - h, t + h) } else { ((t - h).max(a), (t + h).min(b)) };
        let d = self.point(t1) - self.point(t0);
        if d.length_squared() < 1e-20 { Vec3::X } else { d / (t1 - t0) }
    }

    /// Sample along the curve: `per_span` segments for each non-empty knot span (degree 1
    /// curves sample their CVs exactly). Closed curves don't repeat the first point.
    pub fn tessellate(&self, per_span: usize) -> Vec<Vec3> {
        if !self.is_valid() {
            return self.cvs.clone();
        }
        if self.degree == 1 && self.knots.is_empty() {
            return self.cvs.clone();
        }
        let (q, u) = self.eval_data();
        let p = self.degree;
        let mut out = Vec::new();
        let per = if p == 1 { 1 } else { per_span.max(1) };
        for k in p..q.len() {
            let (a, b) = (u[k], u[k + 1]);
            if b - a <= 1e-9 {
                continue;
            }
            for s in 0..per {
                out.push(de_boor(&q, &u, p, a + (b - a) * s as f32 / per as f32));
            }
        }
        if !self.closed {
            out.push(*self.cvs.last().unwrap());
        }
        out
    }

    /// Points evenly spaced by arc length (`count` points; closed curves skip the duplicate).
    pub fn resample(&self, count: usize) -> Vec<Vec3> {
        let count = count.max(2);
        let mut dense = self.tessellate(32);
        if self.closed && !dense.is_empty() {
            dense.push(dense[0]);
        }
        if dense.len() < 2 {
            return vec![dense.first().copied().unwrap_or(Vec3::ZERO); count];
        }
        let mut acc = vec![0.0f32];
        for w in dense.windows(2) {
            acc.push(acc.last().unwrap() + w[0].distance(w[1]));
        }
        let total = *acc.last().unwrap();
        let steps = if self.closed { count } else { count - 1 };
        let mut out = Vec::with_capacity(count);
        let mut j = 0;
        for i in 0..count {
            let s = total * i as f32 / steps as f32;
            while j + 2 < acc.len() && acc[j + 1] < s {
                j += 1;
            }
            let seg = (acc[j + 1] - acc[j]).max(1e-12);
            out.push(dense[j].lerp(dense[j + 1], ((s - acc[j]) / seg).clamp(0.0, 1.0)));
        }
        out
    }

    pub fn length(&self) -> f32 {
        let mut d = self.tessellate(32);
        if self.closed && !d.is_empty() {
            d.push(d[0]);
        }
        d.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// Parameter of the curve point closest to `p`.
    pub fn closest_param(&self, p: Vec3) -> f32 {
        let (a, b) = self.domain();
        let n = (self.spans().max(1) * 24).max(48);
        let mut best = (f32::MAX, a);
        for i in 0..=n {
            let t = a + (b - a) * i as f32 / n as f32;
            let d = self.point(t).distance_squared(p);
            if d < best.0 {
                best = (d, t);
            }
        }
        // Golden-section refinement in the neighbouring interval.
        let h = (b - a) / n as f32;
        let (mut lo, mut hi) = (best.1 - h, best.1 + h);
        if !self.closed {
            lo = lo.max(a);
            hi = hi.min(b);
        }
        let g = 0.618_034f32;
        for _ in 0..30 {
            let m1 = hi - g * (hi - lo);
            let m2 = lo + g * (hi - lo);
            if self.point(m1).distance_squared(p) < self.point(m2).distance_squared(p) {
                hi = m2;
            } else {
                lo = m1;
            }
        }
        let t = 0.5 * (lo + hi);
        if self.closed { a + (t - a).rem_euclid(b - a) } else { t }
    }

    pub fn transformed(&self, m: Mat4) -> Curve {
        Curve { cvs: self.cvs.iter().map(|p| m.transform_point3(*p)).collect(), ..self.clone() }
    }

    /// Reverse direction (same shape).
    pub fn reversed(&self) -> Curve {
        let mut c = self.clone();
        c.cvs.reverse();
        if !c.knots.is_empty() {
            let (a, b) = (c.knots[0], *c.knots.last().unwrap());
            c.knots = c.knots.iter().rev().map(|k| a + b - k).collect();
        }
        if c.closed {
            // Keep the start point: rotate so the old first CV stays first.
            c.cvs.rotate_right(1);
        }
        c
    }

    /// Insert knot `t` once (open curves; Boehm's algorithm).
    fn insert_knot(&self, t: f32) -> Curve {
        let (q, u) = self.eval_data();
        let p = self.degree;
        let k = find_span(&u, p, q.len(), t);
        let mut nq = Vec::with_capacity(q.len() + 1);
        for i in 0..=q.len() {
            if i <= k.saturating_sub(p) {
                nq.push(q[i]);
            } else if i > k {
                nq.push(q[i - 1]);
            } else {
                let den = u[i + p] - u[i];
                let a = if den.abs() < 1e-12 { 0.0 } else { (t - u[i]) / den };
                nq.push(q[i - 1].lerp(q[i], a));
            }
        }
        let mut nu = u.clone();
        nu.insert(k + 1, t);
        Curve { cvs: nq, degree: p, closed: false, knots: nu }
    }

    /// Split at parameter `t` into two curves (exact for open curves).
    pub fn detach(&self, t: f32) -> Option<(Curve, Curve)> {
        let (a, b) = self.domain();
        if self.closed {
            // Re-open the loop at t: resample from t around to t.
            let n = (self.cvs.len() * 4).max(8);
            let pts: Vec<Vec3> = (0..=n).map(|i| self.point(t + (b - a) * i as f32 / n as f32)).collect();
            let c = interpolate(&pts, self.degree, ParamMode::Uniform)?;
            return Some((c.clone(), Curve { cvs: Vec::new(), ..c }));
        }
        let eps = (b - a) * 1e-4;
        if t <= a + eps || t >= b - eps {
            return None;
        }
        let p = self.degree;
        let mut c = Curve { knots: self.knot_vector(), ..self.clone() };
        loop {
            let mult = c.knots.iter().filter(|k| (**k - t).abs() < 1e-6).count();
            if mult >= p {
                break;
            }
            c = c.insert_knot(t);
        }
        let f = c.knots.iter().position(|k| (*k - t).abs() < 1e-6)?;
        let mut lk: Vec<f32> = c.knots[..f + p].to_vec();
        lk.push(t);
        let left = Curve { cvs: c.cvs[..f].to_vec(), degree: p, closed: false, knots: lk };
        let mut rk = vec![t];
        rk.extend_from_slice(&c.knots[f..]);
        let right = Curve { cvs: c.cvs[f - 1..].to_vec(), degree: p, closed: false, knots: rk };
        Some((left.normalized(), right.normalized()))
    }

    /// Join `other` onto the end of this curve. The junction is the midpoint of the two
    /// touching ends ("blend"); both curves are made the same degree first.
    pub fn attach(&self, other: &Curve) -> Curve {
        let p = self.degree.max(other.degree);
        let up = |c: &Curve| {
            if c.degree == p && !c.closed {
                Curve { knots: c.knot_vector(), ..c.clone() }
            } else {
                interpolate(&c.resample((c.cvs.len() * 3).max(p + 1)), p, ParamMode::Uniform).unwrap_or_else(|| c.clone())
            }
        };
        let (mut a, b) = (up(self), up(other));
        let j = (*a.cvs.last().unwrap() + b.cvs[0]) * 0.5;
        *a.cvs.last_mut().unwrap() = j;
        let ea = *a.knots.last().unwrap();
        let mut knots: Vec<f32> = a.knots[..a.knots.len() - 1].to_vec();
        let shift = ea - b.knots[0];
        knots.extend(b.knots[p + 1..].iter().map(|k| k + shift));
        let mut cvs = a.cvs.clone();
        cvs.extend_from_slice(&b.cvs[1..]);
        Curve { cvs, degree: p, closed: false, knots }.normalized()
    }

    /// Maya's rebuildCurve: `spans` uniform spans of `degree` fitted through points evenly
    /// spaced along the curve.
    pub fn rebuild(&self, spans: usize, degree: usize) -> Option<Curve> {
        let spans = spans.max(1);
        if self.closed {
            let pts = self.resample(spans.max(3));
            interpolate_closed(&pts, degree)
        } else {
            fit_uniform(&self.resample((spans + degree) * 8 + 1), spans, degree)
        }
    }

    /// Open ↔ closed (keeps the CVs; closing makes the curve periodic).
    pub fn toggled_closed(&self) -> Curve {
        Curve { cvs: self.cvs.clone(), degree: self.degree, closed: !self.closed, knots: Vec::new() }.normalized()
    }
}

/// USD NurbsCurves form: (points, order, knots, range). Periodic curves are written with
/// their first `degree` CVs repeated and uniform knots.
pub fn to_usd(c: &Curve) -> (Vec<Vec3>, usize, Vec<f32>, (f32, f32)) {
    let (q, u) = c.eval_data();
    (q, c.degree + 1, u, c.domain())
}

/// From USD NurbsCurves data (one curve). Detects the periodic form written by `to_usd`.
pub fn from_usd(points: &[Vec3], order: usize, knots: &[f32]) -> Curve {
    let p = order.saturating_sub(1).max(1);
    let m = points.len();
    if knots.len() == m + p + 1 && m > 2 * p {
        let uniform = knots.windows(2).all(|w| ((w[1] - w[0]) - (knots[1] - knots[0])).abs() < 1e-4) && knots[1] > knots[0];
        let wraps = (0..p).all(|i| points[i].distance(points[m - p + i]) < 1e-5);
        if uniform && wraps {
            return Curve { cvs: points[..m - p].to_vec(), degree: p, closed: true, knots: Vec::new() };
        }
        return Curve { cvs: points.to_vec(), degree: p, closed: false, knots: knots.to_vec() }.normalized();
    }
    Curve::new(points.to_vec(), p, false)
}

/// Clamped uniform knots for `n` CVs of degree `p`: [0]*(p+1), 1, 2, …, [n-p]*(p+1).
pub fn clamped_uniform(n: usize, p: usize) -> Vec<f32> {
    let inner = n.saturating_sub(p);
    let mut u = vec![0.0; p + 1];
    for i in 1..inner {
        u.push(i as f32);
    }
    u.extend(std::iter::repeat_n(inner as f32, p + 1));
    u
}

fn find_span(u: &[f32], p: usize, n: usize, t: f32) -> usize {
    if t >= u[n] {
        // Last non-empty span.
        let mut k = n - 1;
        while k > p && u[k] >= u[n] {
            k -= 1;
        }
        return k;
    }
    if t <= u[p] {
        return p;
    }
    let (mut lo, mut hi) = (p, n);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if t < u[mid] { hi = mid } else { lo = mid }
    }
    lo
}

fn de_boor(q: &[Vec3], u: &[f32], p: usize, t: f32) -> Vec3 {
    let n = q.len();
    let t = t.clamp(u[p], u[n]);
    let k = find_span(u, p, n, t);
    let mut d: Vec<Vec3> = (0..=p).map(|j| q[j + k - p]).collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let den = u[i + p - r + 1] - u[i];
            let a = if den.abs() < 1e-12 { 0.0 } else { (t - u[i]) / den };
            d[j] = d[j - 1].lerp(d[j], a);
        }
    }
    d[p]
}

/// Non-zero basis functions N[k-p..=k] at t (Piegl & Tiller A2.2).
fn basis(u: &[f32], p: usize, k: usize, t: f32) -> Vec<f32> {
    let mut n = vec![0.0f32; p + 1];
    let mut left = vec![0.0f32; p + 1];
    let mut right = vec![0.0f32; p + 1];
    n[0] = 1.0;
    for j in 1..=p {
        left[j] = t - u[k + 1 - j];
        right[j] = u[k + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let den = right[r + 1] + left[j - r];
            let tmp = if den.abs() < 1e-12 { 0.0 } else { n[r] / den };
            n[r] = saved + right[r + 1] * tmp;
            saved = left[j - r] * tmp;
        }
        n[j] = saved;
    }
    n
}

/// Solve A·X = B (dense, partial pivoting) for 3-vector right-hand sides.
fn solve(mut a: Vec<Vec<f32>>, mut b: Vec<Vec3>) -> Option<Vec<Vec3>> {
    let n = b.len();
    for c in 0..n {
        let piv = (c..n).max_by(|i, j| a[*i][c].abs().total_cmp(&a[*j][c].abs()))?;
        if a[piv][c].abs() < 1e-10 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            if f == 0.0 {
                continue;
            }
            let (top, bottom) = a.split_at_mut(r);
            for (x, y) in bottom[0][c..].iter_mut().zip(&top[c][c..]) {
                *x -= f * y;
            }
            let bc = b[c];
            b[r] -= bc * f;
        }
    }
    let mut x = vec![Vec3::ZERO; n];
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in r + 1..n {
            s -= x[k] * a[r][k];
        }
        x[r] = s / a[r][r];
    }
    Some(x)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParamMode {
    /// Chord length (EP curves through clicked points).
    Chord,
    /// Uniform (rebuild: gives uniform knots).
    Uniform,
}

/// Open curve of `degree` passing through every point (global interpolation).
pub fn interpolate(pts: &[Vec3], degree: usize, mode: ParamMode) -> Option<Curve> {
    let n = pts.len();
    if n < 2 {
        return None;
    }
    let p = degree.clamp(1, n - 1);
    // Parameters.
    let mut t = vec![0.0f32; n];
    if mode == ParamMode::Chord {
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        if total > 1e-9 {
            for k in 1..n {
                t[k] = t[k - 1] + pts[k].distance(pts[k - 1]) / total;
            }
        }
    }
    if mode == ParamMode::Uniform || t[n - 1] <= 0.0 {
        for (k, tk) in t.iter_mut().enumerate() {
            *tk = k as f32 / (n - 1) as f32;
        }
    }
    t[n - 1] = 1.0;
    // Knots by averaging; scaled so uniform parameters give integer knots.
    let mut u = vec![0.0f32; p + 1];
    for j in 1..n - p {
        u.push((j..j + p).map(|i| t[i]).sum::<f32>() / p as f32);
    }
    u.extend(std::iter::repeat_n(1.0, p + 1));
    let a: Vec<Vec<f32>> = t
        .iter()
        .map(|&tk| {
            let k = find_span(&u, p, n, tk);
            let b = basis(&u, p, k, tk);
            let mut row = vec![0.0; n];
            for (j, v) in b.into_iter().enumerate() {
                row[k - p + j] = v;
            }
            row
        })
        .collect();
    let cvs = solve(a, pts.to_vec())?;
    let scale = (n - p) as f32;
    let knots = u.iter().map(|k| k * scale).collect();
    Some(Curve { cvs, degree: p, closed: false, knots }.normalized())
}

/// Least-squares fit of an open curve with `spans` uniform spans (clamped uniform knots) to
/// points evenly spaced along it; the end points are kept exactly.
pub fn fit_uniform(pts: &[Vec3], spans: usize, degree: usize) -> Option<Curve> {
    let p = degree.clamp(1, 7);
    let n = spans.max(1) + p;
    if pts.len() < n {
        return interpolate(pts, p, ParamMode::Uniform);
    }
    let u = clamped_uniform(n, p);
    let dom = (n - p) as f32;
    let m = pts.len();
    // Arc-length parameters first, then a few rounds of parameter correction (project each
    // point on the current fit and refit).
    let mut params: Vec<f32> = (0..m).map(|i| dom * i as f32 / (m - 1) as f32).collect();
    let mut best: Option<Curve> = None;
    for _ in 0..4 {
        let c = fit_params(pts, &params, &u, n, p)?;
        for (i, q) in pts.iter().enumerate().skip(1).take(m - 2) {
            params[i] = c.closest_param(*q);
        }
        best = Some(c);
    }
    best
}

fn fit_params(pts: &[Vec3], params: &[f32], u: &[f32], n: usize, p: usize) -> Option<Curve> {
    let m = pts.len();
    // Normal equations for the interior CVs (first / last fixed to the end points).
    let k = n - 2;
    let mut ata = vec![vec![0.0f32; k]; k];
    let mut atb = vec![Vec3::ZERO; k];
    for (q, &t) in pts.iter().zip(params) {
        let s = find_span(u, p, n, t);
        let b = basis(u, p, s, t);
        let mut row = vec![0.0f32; n];
        for (j, v) in b.into_iter().enumerate() {
            row[s - p + j] = v;
        }
        let rhs = *q - pts[0] * row[0] - pts[m - 1] * row[n - 1];
        for a in 0..k {
            if row[a + 1] == 0.0 {
                continue;
            }
            for c in 0..k {
                ata[a][c] += row[a + 1] * row[c + 1];
            }
            atb[a] += rhs * row[a + 1];
        }
    }
    let inner = if k == 0 { Vec::new() } else { solve(ata, atb)? };
    let mut cvs = vec![pts[0]];
    cvs.extend(inner);
    cvs.push(pts[m - 1]);
    Some(Curve { cvs, degree: p, closed: false, knots: u.to_vec() }.normalized())
}

/// Closed (periodic, uniform) curve through every point.
pub fn interpolate_closed(pts: &[Vec3], degree: usize) -> Option<Curve> {
    let n = pts.len();
    if n < 3 {
        return None;
    }
    let p = degree.clamp(1, n - 1);
    let probe = Curve { cvs: vec![Vec3::ZERO; n], degree: p, closed: true, knots: Vec::new() };
    let (_, u) = probe.eval_data();
    let m = n + p;
    let a: Vec<Vec<f32>> = (0..n)
        .map(|k| {
            let tk = k as f32;
            let s = find_span(&u, p, m, tk);
            let b = basis(&u, p, s, tk);
            let mut row = vec![0.0; n];
            for (j, v) in b.into_iter().enumerate() {
                row[(s - p + j) % n] += v;
            }
            row
        })
        .collect();
    let cvs = solve(a, pts.to_vec())?;
    Some(Curve { cvs, degree: p, closed: true, knots: Vec::new() })
}

/// Maya-style NURBS circle: periodic cubic with `sections` CVs, radius `r`, around `normal`.
pub fn circle(r: f32, sections: usize, normal: Vec3, center: Vec3) -> Curve {
    let s = sections.max(3);
    let n = normal.try_normalize().unwrap_or(Vec3::Z);
    // A uniform periodic cubic through points at R lies at R·(2 + cos θ)/3 at the knots.
    let th = std::f32::consts::TAU / s as f32;
    let rr = r * 3.0 / (2.0 + th.cos());
    let q = Quat::from_rotation_arc(Vec3::Z, n);
    let cvs = (0..s).map(|i| center + q * Vec3::new(rr * (th * i as f32).cos(), rr * (th * i as f32).sin(), 0.0)).collect();
    Curve { cvs, degree: 3, closed: true, knots: Vec::new() }
}

/// World-space curve of a curve node.
pub fn world_curve(scene: &Scene, id: NodeId) -> Option<Curve> {
    let c = Curve::from_shape(&scene.node(id)?.shape)?;
    Some(c.transformed(scene.world_matrix(id)))
}

// ------------------------------------------------------------------------------ surfaces

/// How a surface node is built from its input curves (construction history).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SurfaceOp {
    /// Revolve a profile around an axis through `pivot` (world space).
    Revolve {
        axis: Vec3,
        pivot: Vec3,
        sections: u32,
        sweep: f32,
        /// Start angle (degrees).
        #[serde(default)]
        start: f32,
    },
    /// Skin a surface through two or more curves.
    Loft { sections: u32, close: bool },
    /// Sweep a profile along a path curve (tube), or straight along `dir` for `length`.
    Extrude { length: f32, dir: Vec3 },
    /// Fill a closed curve.
    Planar,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SurfaceHistory {
    pub op: SurfaceOp,
    pub inputs: Vec<NodeId>,
    /// Profile samples along each curve.
    pub samples: u32,
    /// Hash of the inputs at the last rebuild.
    #[serde(default)]
    pub key: u64,
}

impl SurfaceHistory {
    pub fn op_name(&self) -> &'static str {
        match self.op {
            SurfaceOp::Revolve { .. } => "revolve",
            SurfaceOp::Loft { .. } => "loft",
            SurfaceOp::Extrude { .. } => "extrude",
            SurfaceOp::Planar => "planarSrf",
        }
    }
}

/// Sample count along a curve for surfaces: degree-1 curves keep their corners.
fn curve_samples(c: &Curve, samples: usize) -> Vec<Vec3> {
    if c.degree == 1 && c.knots.is_empty() {
        c.cvs.clone()
    } else {
        c.resample(samples.max(3))
    }
}

/// Grid of rows (each the same length) → quads with UVs. `wrap_u` closes rows, `wrap_v`
/// closes the row list. Coincident points along a row (e.g. on a revolve axis) are welded.
fn grid_mesh(rows: &[Vec<Vec3>], wrap_u: bool, wrap_v: bool) -> Mesh {
    let nv = rows.len();
    let nu = rows.first().map(|r| r.len()).unwrap_or(0);
    let mut m = Mesh::default();
    if nv < 2 || nu < 2 {
        return m;
    }
    let mut idx = vec![vec![0u32; nu]; nv];
    // Weld points that coincide across rows (poles / axis points).
    let scale = rows.iter().flatten().fold(0.0f32, |a, p| a.max(p.abs().max_element())).max(1.0);
    let eps = scale * 1e-5;
    for (r, row) in rows.iter().enumerate() {
        for (c, p) in row.iter().enumerate() {
            let reuse = (0..r).find(|&r0| rows[r0][c].distance(*p) < eps).map(|r0| idx[r0][c]);
            idx[r][c] = match reuse {
                Some(i) => i,
                None => {
                    m.positions.push(*p);
                    (m.positions.len() - 1) as u32
                }
            };
        }
    }
    let (cu, cv) = (if wrap_u { nu } else { nu - 1 }, if wrap_v { nv } else { nv - 1 });
    let mut uvs: Vec<Vec<Vec2>> = Vec::new();
    for r in 0..cv {
        for c in 0..cu {
            let (r1, c1) = ((r + 1) % nv, (c + 1) % nu);
            // (row, col, u, v): the wrapped column/row maps to 1, not 0.
            let corner = |rr: usize, cc: usize, next_r: bool, next_c: bool| {
                let u = if next_c { (c + 1) as f32 / cu as f32 } else { c as f32 / cu as f32 };
                let v = if next_r { (r + 1) as f32 / cv as f32 } else { r as f32 / cv as f32 };
                (idx[rr][cc], Vec2::new(u, v))
            };
            let quad = [corner(r, c, false, false), corner(r, c1, false, true), corner(r1, c1, true, true), corner(r1, c, true, false)];
            let mut f: Vec<u32> = Vec::new();
            let mut fu: Vec<Vec2> = Vec::new();
            for (i, uv) in quad {
                if !f.contains(&i) {
                    f.push(i);
                    fu.push(uv);
                }
            }
            if f.len() >= 3 {
                m.faces.push(f);
                uvs.push(fu);
            }
        }
    }
    m.set_corner_uvs(&uvs);
    m
}

/// Make the faces of an open surface point toward `toward` on average (consistent winding is
/// already guaranteed by the grid); used so lofts/revolves face outward.
fn orient_outward(m: &mut Mesh) {
    if m.faces.is_empty() {
        return;
    }
    let c = m.positions.iter().copied().sum::<Vec3>() / m.positions.len().max(1) as f32;
    let mut score = 0.0;
    for f in 0..m.faces.len() {
        score += m.face_normal(f).dot(m.face_center(f) - c) * face_area(m, f);
    }
    if score < 0.0 {
        flip(m);
    }
}

fn face_area(m: &Mesh, f: usize) -> f32 {
    let p: Vec<Vec3> = m.faces[f].iter().map(|v| m.positions[*v as usize]).collect();
    let mut a = Vec3::ZERO;
    for i in 1..p.len().saturating_sub(1) {
        a += (p[i] - p[0]).cross(p[i + 1] - p[0]);
    }
    a.length() * 0.5
}

fn flip(m: &mut Mesh) {
    let uvs = m.corner_uvs();
    for f in &mut m.faces {
        f.reverse();
    }
    if m.has_uvs() {
        let r: Vec<Vec<Vec2>> = uvs.into_iter().map(|mut u| {
            u.reverse();
            u
        }).collect();
        m.set_corner_uvs(&r);
    }
}

pub fn revolve(profile: &Curve, axis: Vec3, pivot: Vec3, sections: usize, start_deg: f32, sweep_deg: f32, samples: usize) -> Mesh {
    let axis = axis.try_normalize().unwrap_or(Vec3::Y);
    let pts = curve_samples(profile, samples);
    let full = (sweep_deg.abs() - 360.0).abs() < 1e-3;
    let s = sections.max(3);
    let rings = if full { s } else { s + 1 };
    let mut rows = Vec::with_capacity(rings);
    for i in 0..rings {
        let a = start_deg.to_radians() + sweep_deg.to_radians() * i as f32 / s as f32;
        let q = Quat::from_axis_angle(axis, a);
        rows.push(pts.iter().map(|p| pivot + q * (*p - pivot)).collect::<Vec<_>>());
    }
    // rows = rings around the axis; columns = profile points.
    let mut m = grid_mesh(&rows, profile.closed, full);
    orient_outward(&mut m);
    m
}

pub fn loft(curves: &[Curve], sections: usize, close: bool, samples: usize) -> Mesh {
    if curves.len() < 2 {
        return Mesh::default();
    }
    let closed_u = curves.iter().all(|c| c.closed);
    let n = samples.max(3);
    let profiles: Vec<Vec<Vec3>> = curves.iter().map(|c| c.resample(n)).collect();
    // Align closed profiles: rotate each so its first point is nearest the previous first.
    let mut profiles = profiles;
    if closed_u {
        for k in 1..profiles.len() {
            let target = profiles[k - 1][0];
            let best = (0..n).min_by(|a, b| profiles[k][*a].distance_squared(target).total_cmp(&profiles[k][*b].distance_squared(target))).unwrap_or(0);
            profiles[k].rotate_left(best);
        }
    }
    // Catmull-Rom between sections (cubic through the curves).
    let k = profiles.len();
    let sec = sections.max(1);
    let get = |i: isize| -> &Vec<Vec3> {
        if close { &profiles[i.rem_euclid(k as isize) as usize] } else { &profiles[i.clamp(0, k as isize - 1) as usize] }
    };
    let mut rows = Vec::new();
    let spans = if close { k } else { k - 1 };
    for s in 0..spans {
        for j in 0..sec {
            let t = j as f32 / sec as f32;
            let (p0, p1, p2, p3) = (get(s as isize - 1), get(s as isize), get(s as isize + 1), get(s as isize + 2));
            rows.push((0..n).map(|i| catmull(p0[i], p1[i], p2[i], p3[i], t)).collect::<Vec<_>>());
        }
    }
    if !close {
        rows.push(profiles[k - 1].clone());
    }
    let mut m = grid_mesh(&rows, closed_u, close);
    orient_outward(&mut m);
    m
}

fn catmull(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1) + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3)
}

/// Best-fit plane normal of a point loop (Newell's method).
pub fn plane_normal(pts: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    n.try_normalize().unwrap_or(Vec3::Y)
}

/// Extrude a profile along a path (tube, rotation-minimising frames) or, without a path,
/// straight along `dir` (default: the profile's plane normal) for `length`.
pub fn extrude(profile: &Curve, path: Option<&Curve>, length: f32, dir: Vec3, samples: usize, path_samples: usize) -> Mesh {
    let pts = curve_samples(profile, samples);
    let rows: Vec<Vec<Vec3>> = match path {
        Some(path) => {
            let pp = curve_samples(path, path_samples);
            if pp.len() < 2 {
                return Mesh::default();
            }
            let tan = |i: usize| -> Vec3 {
                let a = pp[i.saturating_sub(1)];
                let b = pp[(i + 1).min(pp.len() - 1)];
                (b - a).try_normalize().unwrap_or(Vec3::Y)
            };
            // Profile is expressed relative to the path start and carried along.
            let mut rot = Quat::IDENTITY;
            let mut prev_t = tan(0);
            let mut rows = Vec::with_capacity(pp.len());
            for (i, p) in pp.iter().enumerate() {
                let t = tan(i);
                rot = Quat::from_rotation_arc(prev_t, t) * rot;
                prev_t = t;
                rows.push(pts.iter().map(|q| *p + rot * (*q - pp[0])).collect());
            }
            rows
        }
        None => {
            let d = dir.try_normalize().unwrap_or_else(|| plane_normal(&pts)) * length;
            vec![pts.clone(), pts.iter().map(|p| *p + d).collect()]
        }
    };
    let mut m = grid_mesh(&rows, profile.closed, path.is_some_and(|p| p.closed));
    orient_outward(&mut m);
    m
}

/// Fill a closed curve: one n-gon if convex, otherwise ear-clipped triangles. Faces point
/// along the curve's winding (right-hand rule).
pub fn planar(profile: &Curve, samples: usize) -> Mesh {
    let mut pts = curve_samples(profile, samples);
    if pts.len() > 3 && pts[0].distance(*pts.last().unwrap()) < 1e-6 {
        pts.pop();
    }
    let mut m = Mesh { positions: pts.clone(), ..Default::default() };
    if pts.len() < 3 {
        return m;
    }
    let n = plane_normal(&pts);
    let (bx, by) = {
        let a = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let bx = n.cross(a).normalize();
        (bx, n.cross(bx))
    };
    let p2: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.dot(bx), p.dot(by))).collect();
    let cross = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let k = p2.len();
    let convex = (0..k).all(|i| cross(p2[i], p2[(i + 1) % k], p2[(i + 2) % k]) >= -1e-9);
    // UVs from the plane projection, normalised to the bounds.
    let (lo, hi) = p2.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let span = (hi - lo).max(Vec2::splat(1e-6));
    let uv = |i: u32| (p2[i as usize] - lo) / span;
    if convex {
        let f: Vec<u32> = (0..k as u32).collect();
        m.set_corner_uvs(&[f.iter().map(|i| uv(*i)).collect()]);
        m.faces.push(f);
        return m;
    }
    let mut rem: Vec<u32> = (0..k as u32).collect();
    let mut uvs = Vec::new();
    let mut guard = 0;
    while rem.len() > 3 && guard < 10_000 {
        guard += 1;
        let r = rem.len();
        let mut clipped = false;
        for i in 0..r {
            let (a, b, c) = (rem[(i + r - 1) % r], rem[i], rem[(i + 1) % r]);
            let (pa, pb, pc) = (p2[a as usize], p2[b as usize], p2[c as usize]);
            if cross(pa, pb, pc) <= 1e-12 {
                continue;
            }
            let inside = rem.iter().any(|&o| {
                if o == a || o == b || o == c {
                    return false;
                }
                let po = p2[o as usize];
                // Inclusive: a vertex on the new diagonal also blocks the ear.
                if po.distance(pa) < 1e-9 || po.distance(pb) < 1e-9 || po.distance(pc) < 1e-9 {
                    return false;
                }
                cross(pa, pb, po) >= -1e-9 && cross(pb, pc, po) >= -1e-9 && cross(pc, pa, po) >= -1e-9
            });
            if inside {
                continue;
            }
            m.faces.push(vec![a, b, c]);
            uvs.push(vec![uv(a), uv(b), uv(c)]);
            rem.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if rem.len() >= 3 {
        uvs.push(rem.iter().map(|i| uv(*i)).collect());
        m.faces.push(rem);
    }
    m.set_corner_uvs(&uvs);
    m
}

/// Hash of everything a surface's history depends on.
fn history_key(scene: &Scene, h: &SurfaceHistory) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut s = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}{}", h.op, h.samples).hash(&mut s);
    for id in &h.inputs {
        match world_curve(scene, *id) {
            Some(c) => {
                for p in &c.cvs {
                    (p.x.to_bits(), p.y.to_bits(), p.z.to_bits()).hash(&mut s);
                }
                (c.degree, c.closed).hash(&mut s);
                for k in &c.knots {
                    k.to_bits().hash(&mut s);
                }
            }
            None => 0u8.hash(&mut s),
        }
    }
    s.finish()
}

/// Build a surface mesh from its history (geometry from the input curves' world positions).
pub fn build_surface(scene: &Scene, node: NodeId, h: &SurfaceHistory) -> Option<Mesh> {
    let curves: Vec<Curve> = h.inputs.iter().map(|i| world_curve(scene, *i)).collect::<Option<Vec<_>>>()?;
    let samples = h.samples as usize;
    let m = match &h.op {
        SurfaceOp::Revolve { axis, pivot, sections, sweep, start } => revolve(curves.first()?, *axis, *pivot, *sections as usize, *start, *sweep, samples),
        SurfaceOp::Loft { sections, close } => loft(&curves, *sections as usize, *close, samples),
        SurfaceOp::Extrude { length, dir } => extrude(curves.first()?, curves.get(1), *length, *dir, samples, samples * 2),
        SurfaceOp::Planar => planar(curves.first()?, samples),
    };
    if m.faces.is_empty() {
        return None;
    }
    // Built from the curves' world positions; the surface node's own transform applies on top
    // (like Maya: moving the surface moves it, editing the curves reshapes it).
    let _ = node;
    Some(m)
}

/// Rebuild surfaces whose input curves changed (called during scene evaluation).
pub fn update_history(scene: &mut Scene) {
    let todo: Vec<(NodeId, SurfaceHistory)> = scene.nodes.iter().filter_map(|n| n.surf.clone().map(|h| (n.id, h))).collect();
    for (id, h) in todo {
        let key = history_key(scene, &h);
        if key == h.key {
            continue;
        }
        let mesh = build_surface(scene, id, &h);
        if let Some(n) = scene.node_mut(id) {
            if let Some(m) = mesh
                && let Shape::Poly { mesh, rev, .. } = &mut n.shape
            {
                *mesh = m;
                *rev = crate::scene::next_rev();
            }
            if let Some(s) = n.surf.as_mut() {
                s.key = key;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3, e: f32) -> bool {
        a.distance(b) < e
    }

    #[test]
    fn clamped_cubic_interpolates_ends_and_matches_polyline_for_degree1() {
        let cvs = vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 0.0), Vec3::new(3.0, 2.0, 0.0), Vec3::new(4.0, 0.0, 0.0)];
        let c = Curve::new(cvs.clone(), 3, false);
        let (a, b) = c.domain();
        assert!(approx(c.point(a), cvs[0], 1e-6) && approx(c.point(b), cvs[3], 1e-6));
        // Bezier case: midpoint is (P0 + 3P1 + 3P2 + P3)/8.
        let mid = (cvs[0] + 3.0 * cvs[1] + 3.0 * cvs[2] + cvs[3]) / 8.0;
        assert!(approx(c.point((a + b) / 2.0), mid, 1e-5));
        let l = Curve::new(cvs.clone(), 1, false);
        assert_eq!(l.tessellate(8), cvs);
        assert_eq!(c.spans(), 1);
    }

    #[test]
    fn circle_is_round_and_closed() {
        let c = circle(2.0, 8, Vec3::Y, Vec3::ZERO);
        let pts = c.tessellate(16);
        let (lo, hi) = pts.iter().fold((f32::MAX, 0.0f32), |(a, b), p| (a.min(p.length()), b.max(p.length())));
        assert!((lo - 2.0).abs() < 0.01 && (hi - 2.0).abs() < 0.01, "{lo} {hi}");
        assert!(pts.iter().all(|p| p.y.abs() < 1e-5));
        let (a, b) = c.domain();
        assert!(approx(c.point(a), c.point(b), 1e-5));
        assert!((c.length() - std::f32::consts::TAU * 2.0).abs() < 0.05);
    }

    #[test]
    fn ep_interpolation_passes_through_points() {
        let pts = vec![Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0), Vec3::new(2.0, 0.0, 1.0), Vec3::new(4.0, 1.0, 0.0), Vec3::new(5.0, 0.0, 0.0)];
        let c = interpolate(&pts, 3, ParamMode::Chord).unwrap();
        assert_eq!(c.cvs.len(), 5);
        for p in &pts {
            let t = c.closest_param(*p);
            assert!(approx(c.point(t), *p, 1e-3), "{p} vs {}", c.point(t));
        }
        let cc = interpolate_closed(&pts, 3).unwrap();
        for p in &pts {
            assert!(approx(cc.point(cc.closest_param(*p)), *p, 1e-3));
        }
    }

    #[test]
    fn detach_attach_reverse_rebuild_preserve_shape() {
        let c = Curve::new(vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 0.0), Vec3::new(2.0, -1.0, 0.0), Vec3::new(3.0, 1.0, 0.0), Vec3::new(4.0, 0.0, 1.0), Vec3::new(5.0, 2.0, 0.0)], 3, false);
        let (a, b) = c.domain();
        let t = a + (b - a) * 0.4;
        let (l, r) = c.detach(t).unwrap();
        let samples = |cv: &Curve| -> Vec<Vec3> {
            let (x, y) = cv.domain();
            (0..=20).map(|i| cv.point(x + (y - x) * i as f32 / 20.0)).collect()
        };
        for p in samples(&l).into_iter().chain(samples(&r)) {
            assert!(approx(c.point(c.closest_param(p)), p, 1e-4), "detached point off the original");
        }
        assert!(approx(l.point(l.domain().1), c.point(t), 1e-4) && approx(r.point(r.domain().0), c.point(t), 1e-4));
        // Attaching the halves gives back the original shape.
        let j = l.attach(&r);
        for p in samples(&c) {
            assert!(approx(j.point(j.closest_param(p)), p, 1e-3));
        }
        // Reverse: same points, opposite direction.
        let rv = c.reversed();
        assert!(approx(rv.point(rv.domain().0), c.point(b), 1e-5));
        for p in samples(&c) {
            assert!(approx(rv.point(rv.closest_param(p)), p, 1e-4));
        }
        // Rebuild to 8 uniform spans stays close.
        let rb = c.rebuild(8, 3).unwrap();
        assert_eq!(rb.spans(), 8);
        let k = rb.knot_vector();
        let inner: Vec<f32> = k[3..k.len() - 3].to_vec();
        assert!(inner.windows(2).all(|w| ((w[1] - w[0]) - 1.0).abs() < 1e-4), "uniform knots: {k:?}");
        for p in samples(&c) {
            assert!(approx(rb.point(rb.closest_param(p)), p, 0.05), "rebuild off by {}", rb.point(rb.closest_param(p)).distance(p));
        }
    }

    #[test]
    fn surfaces_revolve_loft_extrude_planar() {
        // Revolve a vertical line at x=1 → a cylinder wall of radius 1.
        let prof = Curve::new(vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 2.0, 0.0)], 1, false);
        let m = revolve(&prof, Vec3::Y, Vec3::ZERO, 12, 0.0, 360.0, 8);
        assert_eq!(m.faces.len(), 12);
        assert!(m.positions.iter().all(|p| (Vec2::new(p.x, p.z).length() - 1.0).abs() < 1e-5));
        assert!(m.has_uvs());
        // Outward: face normals point away from the axis.
        for f in 0..m.faces.len() {
            let c = m.face_center(f);
            assert!(m.face_normal(f).dot(Vec3::new(c.x, 0.0, c.z)) > 0.0);
        }
        // Revolving a curve that touches the axis welds the pole.
        let cup = Curve::new(vec![Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)], 1, false);
        let m = revolve(&cup, Vec3::Y, Vec3::ZERO, 8, 0.0, 360.0, 8);
        assert_eq!(m.positions.len(), 1 + 8 * 2);
        assert!(m.faces.iter().filter(|f| f.len() == 3).count() == 8);
        // Loft two circles → a tube.
        let a = circle(1.0, 8, Vec3::Y, Vec3::ZERO);
        let b = circle(1.0, 8, Vec3::Y, Vec3::new(0.0, 3.0, 0.0));
        let m = loft(&[a.clone(), b], 2, false, 16);
        assert_eq!(m.faces.len(), 16 * 2);
        let (lo, hi) = m.positions.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p.y), h.max(p.y)));
        assert!(lo.abs() < 1e-5 && (hi - 3.0).abs() < 1e-5);
        // Extrude a circle along a straight path = loft result; without a path → straight.
        let path = Curve::new(vec![Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0)], 1, false);
        let m = extrude(&a, Some(&path), 0.0, Vec3::ZERO, 16, 4);
        assert!(m.positions.iter().all(|p| (Vec2::new(p.x, p.z).length() - 1.0).abs() < 0.01));
        let m = extrude(&a, None, 2.0, Vec3::Y, 16, 0);
        assert_eq!(m.faces.len(), 16);
        // Planar fill of a concave L shape (6 corners) → 4 triangles covering its area.
        let l = Curve::new(
            vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -2.0), Vec3::new(0.0, 0.0, -2.0)],
            1,
            true,
        );
        let m = planar(&l, 8);
        let area: f32 = (0..m.faces.len()).map(|f| face_area(&m, f)).sum();
        assert!((area - 3.0).abs() < 1e-4, "{area} {:?} {:?}", m.faces, m.positions);
        assert!(m.faces.iter().all(|f| f.len() == 3));
        // Counter-clockwise seen from +Y → faces point up.
        assert!((0..m.faces.len()).all(|f| m.face_normal(f).y > 0.99));
    }
}
