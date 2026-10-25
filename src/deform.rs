//! Lattice (FFD), Wrap and Wire deformers.

use crate::app::{App, CompMode};
use crate::scene::{NodeId, Scene, Shape};
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

/// A mesh's binding to a lattice node.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LatticeBind {
    pub lattice: NodeId,
    /// Inverse world matrix of the lattice at bind time (the "base lattice").
    pub base_inv: Mat4,
}

/// A mesh's binding to a wrap influence mesh.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WrapBind {
    pub driver: NodeId,
    /// Per vertex: driver triangle (3 driver vertices), barycentric weights, offset along the
    /// triangle normal and the tangent-frame offset (all at bind).
    pub binds: Vec<([u32; 3], [f32; 3], f32)>,
}

/// Regular lattice grid point (local space, -0.5..0.5).
pub fn grid_point(div: [u32; 3], i: u32, j: u32, k: u32) -> Vec3 {
    let f = |a: u32, n: u32| if n <= 1 { 0.0 } else { a as f32 / (n - 1) as f32 - 0.5 };
    Vec3::new(f(i, div[0]), f(j, div[1]), f(k, div[2]))
}

pub fn lattice_index(div: [u32; 3], i: u32, j: u32, k: u32) -> usize {
    ((k * div[1] + j) * div[0] + i) as usize
}

pub fn lattice_grid(div: [u32; 3]) -> Vec<Vec3> {
    let mut v = Vec::with_capacity((div[0] * div[1] * div[2]) as usize);
    for k in 0..div[2] {
        for j in 0..div[1] {
            for i in 0..div[0] {
                v.push(grid_point(div, i, j, k));
            }
        }
    }
    v
}

/// Lattice displacement at local point `u` (-0.5..0.5 cube): trilinear blend of the point
/// offsets of the surrounding cell. None outside the lattice.
pub fn lattice_offset(div: [u32; 3], points: &[Vec3], u: Vec3) -> Option<Vec3> {
    if u.x < -0.5001 || u.y < -0.5001 || u.z < -0.5001 || u.x > 0.5001 || u.y > 0.5001 || u.z > 0.5001 {
        return None;
    }
    let cell = |x: f32, n: u32| -> (u32, f32) {
        if n <= 1 {
            return (0, 0.0);
        }
        let s = ((x + 0.5).clamp(0.0, 1.0)) * (n - 1) as f32;
        let i = (s.floor() as u32).min(n - 2);
        (i, s - i as f32)
    };
    let (i, fx) = cell(u.x, div[0]);
    let (j, fy) = cell(u.y, div[1]);
    let (k, fz) = cell(u.z, div[2]);
    let mut d = Vec3::ZERO;
    for (di, wx) in [(0, 1.0 - fx), (1, fx)] {
        for (dj, wy) in [(0, 1.0 - fy), (1, fy)] {
            for (dk, wz) in [(0, 1.0 - fz), (1, fz)] {
                let (a, b, c) = ((i + di).min(div[0] - 1), (j + dj).min(div[1] - 1), (k + dk).min(div[2] - 1));
                let idx = lattice_index(div, a, b, c);
                let off = points.get(idx).copied().unwrap_or(Vec3::ZERO) - grid_point(div, a, b, c);
                d += off * (wx * wy * wz);
            }
        }
    }
    Some(d)
}

/// Apply lattice bindings to world-space points.
pub fn apply_lattices(scene: &Scene, binds: &[LatticeBind], pts: &mut [Vec3]) {
    for b in binds {
        let Some(Shape::Lattice { div, points }) = scene.node(b.lattice).map(|n| &n.shape) else { continue };
        let lw = scene.world_matrix(b.lattice);
        for p in pts.iter_mut() {
            let u = b.base_inv.transform_point3(*p);
            if let Some(d) = lattice_offset(*div, points, u) {
                *p = lw.transform_point3(u + d);
            }
        }
    }
}

/// A mesh's binding to a wire curve (Maya's wire deformer).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WireBind {
    pub curve: NodeId,
    /// World-space CVs of the curve at bind time (the "base wire").
    pub base_cvs: Vec<Vec3>,
    /// Per influenced vertex: (vertex, curve parameter, falloff weight).
    pub binds: Vec<(u32, f32, f32)>,
    /// Distance from the wire at which the influence reaches zero.
    pub dropoff: f32,
    /// Overall strength (0..1).
    pub envelope: f32,
    /// How much vertices turn with the curve's tangent (0..1).
    pub rotation: f32,
}

/// Smooth falloff: 1 on the wire, 0 at `dropoff`.
pub fn wire_falloff(d: f32, dropoff: f32) -> f32 {
    if dropoff <= 0.0 || d >= dropoff {
        return 0.0;
    }
    let x = d / dropoff;
    (1.0 - x * x) * (1.0 - x * x)
}

/// Bind world-space points to a world-space wire curve.
pub fn wire_bind(curve: &crate::nurbs::Curve, pts: &[Vec3], dropoff: f32) -> Vec<(u32, f32, f32)> {
    let mut out = Vec::new();
    // Cheap reject: points farther than dropoff from every curve sample.
    let samples = curve.tessellate(8);
    let reach = dropoff + samples.windows(2).map(|w| w[0].distance(w[1])).fold(0.0, f32::max);
    for (v, p) in pts.iter().enumerate() {
        if samples.iter().all(|s| s.distance(*p) > reach) {
            continue;
        }
        let t = curve.closest_param(*p);
        let w = wire_falloff(curve.point(t).distance(*p), dropoff);
        if w > 1e-5 {
            out.push((v as u32, t, w));
        }
    }
    out
}

/// Apply wire bindings to world-space points.
pub fn apply_wires(scene: &Scene, binds: &[WireBind], pts: &mut [Vec3]) {
    for b in binds {
        let Some(cur) = crate::nurbs::world_curve(scene, b.curve) else { continue };
        if cur.cvs.len() != b.base_cvs.len() {
            continue;
        }
        let base = crate::nurbs::Curve { cvs: b.base_cvs.clone(), ..cur.clone() };
        for &(v, t, w) in &b.binds {
            let Some(p) = pts.get_mut(v as usize) else { continue };
            let (bp, cp) = (base.point(t), cur.point(t));
            let rot = if b.rotation > 0.0 {
                let (tb, tc) = (base.tangent(t).normalize_or_zero(), cur.tangent(t).normalize_or_zero());
                if tb == Vec3::ZERO || tc == Vec3::ZERO { glam::Quat::IDENTITY } else { glam::Quat::IDENTITY.slerp(glam::Quat::from_rotation_arc(tb, tc), b.rotation) }
            } else {
                glam::Quat::IDENTITY
            };
            let target = cp + rot * (*p - bp);
            *p = p.lerp(target, (w * b.envelope).clamp(0.0, 1.0));
        }
    }
}

/// Closest point on triangle (a, b, c) to p, as barycentric weights.
pub(crate) fn closest_bary(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    // Ericson, Real-Time Collision Detection 5.1.5.
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    [1.0 - v - w, v, w]
}

/// Bind every point to its closest driver triangle.
pub fn wrap_bind(driver_world: &[Vec3], tris: &[[u32; 3]], pts: &[Vec3]) -> Vec<([u32; 3], [f32; 3], f32)> {
    pts.iter()
        .map(|p| {
            let mut best = ([0u32; 3], [1.0, 0.0, 0.0], f32::MAX, 0.0f32);
            for t in tris {
                let (a, b, c) = (driver_world[t[0] as usize], driver_world[t[1] as usize], driver_world[t[2] as usize]);
                let bc = closest_bary(*p, a, b, c);
                let q = a * bc[0] + b * bc[1] + c * bc[2];
                let d = q.distance_squared(*p);
                if d < best.2 {
                    let n = (b - a).cross(c - a).normalize_or_zero();
                    best = (*t, bc, d, (*p - q).dot(n));
                }
            }
            (best.0, best.1, best.3)
        })
        .collect()
}

/// Rebuild wrapped points from the driver's current world positions.
pub fn wrap_apply(driver_world: &[Vec3], binds: &[([u32; 3], [f32; 3], f32)], pts: &mut [Vec3]) {
    for (p, (t, bc, off)) in pts.iter_mut().zip(binds) {
        let (Some(a), Some(b), Some(c)) = (driver_world.get(t[0] as usize), driver_world.get(t[1] as usize), driver_world.get(t[2] as usize)) else { continue };
        let n = (*b - *a).cross(*c - *a).normalize_or_zero();
        *p = *a * bc[0] + *b * bc[1] + *c * bc[2] + n * *off;
    }
}

/// Driver triangles (fan triangulation of its faces).
pub fn driver_tris(m: &crate::mesh::Mesh) -> Vec<[u32; 3]> {
    let mut t = Vec::new();
    for f in &m.faces {
        for i in 1..f.len().saturating_sub(1) {
            t.push([f[0], f[i], f[i + 1]]);
        }
    }
    t
}

impl App {
    /// Create a lattice around the selected meshes (or selected vertices' meshes) and bind them.
    pub fn create_lattice(&mut self, div: [u32; 3], name: Option<&str>) -> Result<NodeId, String> {
        let div = [div[0].clamp(2, 20), div[1].clamp(2, 20), div[2].clamp(2, 20)];
        let meshes: Vec<NodeId> = self.sel.iter().flat_map(|s| self.scene.subtree(*s)).filter(|id| self.scene.node(*id).is_some_and(|n| n.mesh().is_some())).collect();
        if meshes.is_empty() {
            return Err("Lattice: select the meshes to deform.".into());
        }
        let (lo, hi) = self.world_bounds(&meshes).ok_or("Lattice: empty selection")?;
        let size = (hi - lo).max(Vec3::splat(1e-3)) * 1.02;
        self.checkpoint();
        let id = self.scene.add_node("ffd1Lattice", Shape::Lattice { div, points: lattice_grid(div) }, None);
        self.apply_name(id, name);
        if let Some(n) = self.scene.node_mut(id) {
            n.translate = (lo + hi) * 0.5;
            n.scale = size;
            n.color = [0.75, 0.55, 0.95];
        }
        let base_inv = self.scene.world_matrix(id).inverse();
        for m in meshes {
            if let Some(n) = self.scene.node_mut(m) {
                n.lattices.push(LatticeBind { lattice: id, base_inv });
            }
        }
        self.sel = vec![id];
        if self.comp_mode != CompMode::Object {
            self.comp_mode = CompMode::Object;
            self.comp_sel.clear();
        }
        Ok(id)
    }

    /// Set a lattice point (local coordinates, -0.5..0.5 at rest).
    pub fn set_lattice_point(&mut self, id: NodeId, ijk: [u32; 3], p: Vec3) -> Result<(), String> {
        let Some(Shape::Lattice { div, .. }) = self.scene.node(id).map(|n| n.shape.clone()) else { return Err("not a lattice".into()) };
        if ijk[0] >= div[0] || ijk[1] >= div[1] || ijk[2] >= div[2] {
            return Err(format!("lattice point out of range (divisions {} {} {})", div[0], div[1], div[2]));
        }
        self.checkpoint();
        if let Some(Shape::Lattice { points, .. }) = self.scene.node_mut(id).map(|n| &mut n.shape) {
            points[lattice_index(div, ijk[0], ijk[1], ijk[2])] = p;
        }
        Ok(())
    }

    pub fn reset_lattice(&mut self, id: NodeId) {
        self.checkpoint();
        if let Some(Shape::Lattice { div, points }) = self.scene.node_mut(id).map(|n| &mut n.shape) {
            *points = lattice_grid(*div);
        }
    }

    /// Wrap: the last selected mesh (or `driver`) deforms the other selected meshes.
    pub fn create_wrap(&mut self, driver: NodeId, targets: &[NodeId]) -> Result<usize, String> {
        let dm = self.scene.node(driver).and_then(|n| n.mesh()).cloned().ok_or("Wrap: the influence object must be a polygon mesh (select it last).")?;
        let targets: Vec<NodeId> = targets.iter().copied().filter(|t| *t != driver && self.scene.node(*t).is_some_and(|n| n.mesh().is_some())).collect();
        if targets.is_empty() {
            return Err("Wrap: select the meshes to deform, then the influence mesh.".into());
        }
        let dw = crate::rig::deformed_positions(&self.scene, driver).unwrap_or_else(|| {
            let w = self.scene.world_matrix(driver);
            dm.positions.iter().map(|p| w.transform_point3(*p)).collect()
        });
        let tris = driver_tris(&dm);
        self.checkpoint();
        for t in &targets {
            let pts = crate::rig::deformed_positions(&self.scene, *t).unwrap_or_else(|| {
                let w = self.scene.world_matrix(*t);
                self.scene.node(*t).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
            });
            let binds = wrap_bind(&dw, &tris, &pts);
            if let Some(n) = self.scene.node_mut(*t) {
                n.wrap = Some(WrapBind { driver, binds });
            }
        }
        Ok(targets.len())
    }

    pub fn remove_lattice_and_wrap(&mut self, ids: &[NodeId]) {
        self.checkpoint();
        for id in ids {
            if let Some(n) = self.scene.node_mut(*id) {
                n.lattices.clear();
                n.wrap = None;
                n.wires.clear();
            }
        }
    }

    /// Wire deformer: `meshes` follow `curve` within `dropoff` of it. Returns the number of
    /// vertices bound.
    pub fn create_wire(&mut self, curve: NodeId, meshes: &[NodeId], dropoff: f32, envelope: f32, rotation: f32) -> Result<usize, String> {
        let c = crate::nurbs::world_curve(&self.scene, curve).ok_or("Wire: the wire must be a curve (select the meshes, then the curve last).")?;
        if !c.is_valid() {
            return Err("Wire: the curve needs at least 2 CVs.".into());
        }
        let meshes: Vec<NodeId> = meshes.iter().copied().filter(|m| *m != curve && self.scene.node(*m).is_some_and(|n| n.mesh().is_some())).collect();
        if meshes.is_empty() {
            return Err("Wire: select the meshes to deform, then the curve.".into());
        }
        let dropoff = if dropoff > 0.0 {
            dropoff
        } else {
            // Default: a fifth of the mesh size, like a sensible Maya dropoffDistance.
            let (lo, hi) = self.world_bounds(&meshes).unwrap_or((Vec3::ZERO, Vec3::ONE));
            ((hi - lo).max_element() * 0.2).max(1e-3)
        };
        self.checkpoint();
        let mut total = 0;
        for m in meshes {
            let pts = crate::rig::deformed_positions(&self.scene, m).unwrap_or_else(|| {
                let w = self.scene.world_matrix(m);
                self.scene.node(m).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
            });
            let binds = wire_bind(&c, &pts, dropoff);
            total += binds.len();
            if let Some(n) = self.scene.node_mut(m) {
                n.wires.retain(|w| w.curve != curve);
                n.wires.push(WireBind { curve, base_cvs: c.cvs.clone(), binds, dropoff, envelope: envelope.clamp(0.0, 1.0), rotation: rotation.clamp(0.0, 1.0) });
            }
        }
        Ok(total)
    }

    /// Change the envelope / dropoff of wires driven by `curve` (rebinds for a new dropoff).
    pub fn edit_wire(&mut self, curve: NodeId, dropoff: Option<f32>, envelope: Option<f32>, rotation: Option<f32>) -> Result<usize, String> {
        let users: Vec<NodeId> = self.scene.nodes.iter().filter(|n| n.wires.iter().any(|w| w.curve == curve)).map(|n| n.id).collect();
        if users.is_empty() {
            return Err("No wire deformer uses that curve.".into());
        }
        self.checkpoint();
        for id in &users {
            let n = self.scene.node_mut(*id).unwrap();
            for w in n.wires.iter_mut().filter(|w| w.curve == curve) {
                if let Some(e) = envelope {
                    w.envelope = e.clamp(0.0, 1.0);
                }
                if let Some(r) = rotation {
                    w.rotation = r.clamp(0.0, 1.0);
                }
            }
        }
        if let Some(d) = dropoff.filter(|d| *d > 0.0) {
            for id in &users {
                let n = self.scene.node(*id).unwrap();
                let Some(wb) = n.wires.iter().find(|w| w.curve == curve).cloned() else { continue };
                let Some(cur) = crate::nurbs::world_curve(&self.scene, curve) else { continue };
                let base = crate::nurbs::Curve { cvs: wb.base_cvs.clone(), ..cur };
                // Rest positions: evaluate without this wire.
                let mut tmp = self.scene.clone();
                if let Some(t) = tmp.node_mut(*id) {
                    t.wires.retain(|w| w.curve != curve);
                }
                let pts = crate::rig::deformed_positions(&tmp, *id).unwrap_or_else(|| {
                    let w = tmp.world_matrix(*id);
                    tmp.node(*id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
                });
                let binds = wire_bind(&base, &pts, d);
                for w in self.scene.node_mut(*id).unwrap().wires.iter_mut().filter(|w| w.curve == curve) {
                    w.binds = binds.clone();
                    w.dropoff = d;
                }
            }
        }
        Ok(users.len())
    }
}

/// Lattice cage line segments in local space.
pub fn lattice_lines(div: [u32; 3], points: &[Vec3]) -> Vec<(Vec3, Vec3)> {
    let mut l = Vec::new();
    let p = |i, j, k| points.get(lattice_index(div, i, j, k)).copied().unwrap_or(Vec3::ZERO);
    for k in 0..div[2] {
        for j in 0..div[1] {
            for i in 0..div[0] {
                if i + 1 < div[0] {
                    l.push((p(i, j, k), p(i + 1, j, k)));
                }
                if j + 1 < div[1] {
                    l.push((p(i, j, k), p(i, j + 1, k)));
                }
                if k + 1 < div[2] {
                    l.push((p(i, j, k), p(i, j, k + 1)));
                }
            }
        }
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn lattice_and_wrap() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -w 2 -h 2 -d 2 -sx 4 -sy 4 -sz 4 -n box; select box").unwrap();
        let bx = app.scene.find_by_name("box").unwrap();
        let lat = execute(&mut app, "lattice -dv 2 3 2 -n ffd").unwrap();
        assert_eq!(lat, "ffd");
        let l = app.scene.find_by_name("ffd").unwrap();
        // Untouched lattice: no change.
        let rest: Vec<Vec3> = {
            let w = app.scene.world_matrix(bx);
            app.scene.node(bx).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        };
        let d = crate::rig::deformed_positions(&app.scene, bx).unwrap();
        assert!(rest.iter().zip(&d).all(|(a, b)| a.distance(*b) < 1e-4));
        // Pull the top-middle points up: the top of the box rises.
        for i in 0..2 {
            for k in 0..2 {
                let g = grid_point([2, 3, 2], i, 2, k);
                app.set_lattice_point(l, [i, 2, k], g + Vec3::Y * 0.25).unwrap();
            }
        }
        let d = crate::rig::deformed_positions(&app.scene, bx).unwrap();
        let top = d.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((top - (1.0 + 0.25 * 2.04)).abs() < 0.05, "top {top}");
        // Moving the lattice moves the mesh.
        execute(&mut app, "move -r 3 0 0 ffd").unwrap();
        let d2 = crate::rig::deformed_positions(&app.scene, bx).unwrap();
        assert!((d2[0].x - d[0].x - 3.0).abs() < 1e-3);
        execute(&mut app, "latticePoint ffd 0 0 0 -0.5 -0.5 -0.5").unwrap();

        // Wrap: a dense sphere follows a coarse cage.
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -n hi; polyCube -w 2.4 -h 2.4 -d 2.4 -n cage; wrap -d cage hi").unwrap();
        let hi = app.scene.find_by_name("hi").unwrap();
        let before = crate::rig::deformed_positions(&app.scene, hi).unwrap();
        let w = app.scene.world_matrix(hi);
        let rest: Vec<Vec3> = app.scene.node(hi).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect();
        assert!(rest.iter().zip(&before).all(|(a, b)| a.distance(*b) < 1e-3), "binding reproduces the rest shape");
        execute(&mut app, "move 0 2 0 cage").unwrap();
        let after = crate::rig::deformed_positions(&app.scene, hi).unwrap();
        assert!(after.iter().zip(&before).all(|(a, b)| (a.y - b.y - 2.0).abs() < 1e-3), "wrapped mesh follows the cage");
    }
}
