//! Sculpt brushes (Sculpt, Smooth, Grab, Flatten, Inflate, Pinch with X symmetry) and soft
//! selection weights.

use crate::app::App;
use crate::scene::NodeId;
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    Sculpt,
    Smooth,
    Grab,
    Flatten,
    Inflate,
    Pinch,
}

impl Brush {
    pub const ALL: [Brush; 6] = [Brush::Sculpt, Brush::Smooth, Brush::Grab, Brush::Flatten, Brush::Inflate, Brush::Pinch];
    pub fn name(self) -> &'static str {
        match self {
            Brush::Sculpt => "Sculpt",
            Brush::Smooth => "Smooth",
            Brush::Grab => "Grab",
            Brush::Flatten => "Flatten",
            Brush::Inflate => "Inflate",
            Brush::Pinch => "Pinch",
        }
    }
    pub fn parse(s: &str) -> Option<Brush> {
        Brush::ALL.into_iter().find(|b| b.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Debug)]
pub struct SculptSettings {
    pub brush: Brush,
    /// Brush radius in screen pixels.
    pub radius_px: f32,
    /// 0..1
    pub strength: f32,
    pub symmetry_x: bool,
    /// Ctrl inverts (dig instead of pull).
    pub invert: bool,
}

impl Default for SculptSettings {
    fn default() -> Self {
        SculptSettings { brush: Brush::Sculpt, radius_px: 60.0, strength: 0.5, symmetry_x: false, invert: false }
    }
}

/// Per-stroke state.
#[derive(Clone, Debug)]
pub struct SculptStroke {
    pub id: NodeId,
    pub neighbours: Vec<Vec<u32>>,
    /// Grab: affected vertices (index, weight) and their object-space start positions.
    pub grab: Vec<(u32, f32, Vec3)>,
    /// Grab: world-space anchor where the stroke started.
    pub anchor: Vec3,
}

/// Smooth falloff: 1 at the centre, 0 at `r`.
pub fn falloff(d: f32, r: f32) -> f32 {
    if d >= r {
        return 0.0;
    }
    let x = 1.0 - (d / r) * (d / r);
    x * x
}

fn vertex_normals(m: &crate::mesh::Mesh) -> Vec<Vec3> {
    let mut n = vec![Vec3::ZERO; m.positions.len()];
    for f in 0..m.faces.len() {
        let fnrm = m.face_normal(f);
        for &v in &m.faces[f] {
            n[v as usize] += fnrm;
        }
    }
    n.into_iter().map(|x| x.normalize_or_zero()).collect()
}

impl App {
    /// Start a sculpt stroke on a mesh (records undo, builds adjacency).
    pub fn sculpt_begin(&mut self, id: NodeId) -> bool {
        let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) else { return false };
        let mut nb = vec![Vec::new(); m.positions.len()];
        for (a, b) in m.edges() {
            nb[a as usize].push(b);
            nb[b as usize].push(a);
        }
        self.checkpoint();
        self.sculpt_stroke = Some(SculptStroke { id, neighbours: nb, grab: Vec::new(), anchor: Vec3::ZERO });
        true
    }

    pub fn sculpt_end(&mut self) {
        self.sculpt_stroke = None;
    }

    /// One brush dab at a world-space hit point (with surface normal) and world radius.
    pub fn sculpt_dab(&mut self, hit: Vec3, normal: Vec3, radius: f32) {
        let Some(stroke) = self.sculpt_stroke.clone() else { return };
        let s = self.sculpt.clone();
        let w = self.scene.world_matrix(stroke.id);
        let inv = w.inverse();
        let mut centres = vec![(hit, normal)];
        if s.symmetry_x {
            centres.push((Vec3::new(-hit.x, hit.y, hit.z), Vec3::new(-normal.x, normal.y, normal.z)));
        }
        let Some(mesh) = self.scene.node_mut(stroke.id).and_then(|n| n.mesh_mut(true)) else { return };
        let world: Vec<Vec3> = mesh.positions.iter().map(|p| w.transform_point3(*p)).collect();
        let vn: Vec<Vec3> = vertex_normals(mesh).into_iter().map(|n| w.transform_vector3(n).normalize_or_zero()).collect();
        let sign = if s.invert { -1.0 } else { 1.0 };
        let k = s.strength.clamp(0.0, 1.0);
        let mut out = world.clone();
        for (c, n) in centres {
            let n = n.normalize_or(Vec3::Y);
            for (v, p) in world.iter().enumerate() {
                let f = falloff(p.distance(c), radius);
                if f <= 0.0 {
                    continue;
                }
                let delta = match s.brush {
                    Brush::Sculpt => n * (sign * k * radius * 0.08 * f),
                    Brush::Inflate => vn[v] * (sign * k * radius * 0.06 * f),
                    Brush::Smooth => {
                        let nb = &stroke.neighbours[v];
                        if nb.is_empty() {
                            Vec3::ZERO
                        } else {
                            let avg = nb.iter().map(|i| world[*i as usize]).sum::<Vec3>() / nb.len() as f32;
                            (avg - *p) * (k * f).min(1.0)
                        }
                    }
                    Brush::Flatten => -n * ((*p - c).dot(n) * k * f * 0.5),
                    Brush::Pinch => {
                        let t = (c - *p) - n * (c - *p).dot(n);
                        t * (sign * k * f * 0.15)
                    }
                    Brush::Grab => Vec3::ZERO,
                };
                out[v] += delta;
            }
        }
        for (p, o) in mesh.positions.iter_mut().zip(&out) {
            *p = inv.transform_point3(*o);
        }
    }

    /// Grab brush: capture the vertices under the brush at the stroke start.
    pub fn sculpt_grab_start(&mut self, hit: Vec3, radius: f32) {
        let Some(st) = self.sculpt_stroke.as_mut() else { return };
        let id = st.id;
        let w = self.scene.world_matrix(id);
        let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) else { return };
        let sym = self.sculpt.symmetry_x;
        let grab: Vec<(u32, f32, Vec3)> = m
            .positions
            .iter()
            .enumerate()
            .filter_map(|(v, p)| {
                let wp = w.transform_point3(*p);
                let mut f = falloff(wp.distance(hit), radius);
                if sym {
                    f = f.max(falloff(wp.distance(Vec3::new(-hit.x, hit.y, hit.z)), radius));
                }
                (f > 0.0).then_some((v as u32, f, *p))
            })
            .collect();
        let st = self.sculpt_stroke.as_mut().unwrap();
        st.grab = grab;
        st.anchor = hit;
    }

    /// Grab brush: move the captured vertices by a world-space offset from the anchor.
    pub fn sculpt_grab_to(&mut self, world_offset: Vec3) {
        let Some(st) = self.sculpt_stroke.clone() else { return };
        let w = self.scene.world_matrix(st.id);
        let inv: Mat4 = w.inverse();
        let local = inv.transform_vector3(world_offset);
        let sym = self.sculpt.symmetry_x;
        let Some(mesh) = self.scene.node_mut(st.id).and_then(|n| n.mesh_mut(true)) else { return };
        for (v, f, start) in &st.grab {
            let wp = w.transform_point3(*start);
            // With symmetry, vertices on the -X side move mirrored.
            let mirrored = sym && wp.x * st.anchor.x < 0.0;
            let d = if mirrored { Vec3::new(-local.x, local.y, local.z) } else { local };
            mesh.positions[*v as usize] = *start + d * *f;
        }
    }

    /// Soft-selection weights: (node, vertex, weight) for vertices within `soft_radius`
    /// (world distance) of the selected components, falloff to 0 at the radius.
    pub fn soft_weights(&self) -> Vec<(NodeId, u32, f32)> {
        let mut out = Vec::new();
        for (id, verts) in self.comp_vertices() {
            let Some(pts) = self.scene.node(id).and_then(|n| n.points()) else { continue };
            let w = self.scene.world_matrix(id);
            let world: Vec<Vec3> = pts.iter().map(|p| w.transform_point3(*p)).collect();
            let sel: std::collections::HashSet<u32> = verts.iter().copied().collect();
            for (v, p) in world.iter().enumerate() {
                let wgt = if sel.contains(&(v as u32)) {
                    1.0
                } else if self.soft_select {
                    let d = verts.iter().map(|s| world[*s as usize].distance(*p)).fold(f32::MAX, f32::min);
                    falloff(d, self.soft_radius)
                } else {
                    0.0
                };
                if wgt > 0.0 {
                    out.push((id, v as u32, wgt));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn brushes_change_the_mesh() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 4 -h 4 -sx 20 -sy 20 -n ground").unwrap();
        let g = app.scene.find_by_name("ground").unwrap();
        let ys = |app: &App| app.scene.node(g).unwrap().mesh().unwrap().positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!(app.sculpt_begin(g));
        app.sculpt.symmetry_x = true;
        for _ in 0..5 {
            app.sculpt_dab(Vec3::new(1.0, 0.0, 0.0), Vec3::Y, 0.8);
        }
        app.sculpt_end();
        let peak = ys(&app);
        assert!(peak > 0.1, "sculpt pulls the surface up: {peak}");
        let m = app.scene.node(g).unwrap().mesh().unwrap();
        let at = |x: f32| m.positions.iter().filter(|p| (p.x - x).abs() < 0.11 && p.z.abs() < 0.11).map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((at(1.0) - at(-1.0)).abs() < 1e-4, "X symmetry");
        assert!(m.has_uvs(), "topology and UVs unchanged");
        // Smooth reduces the peak; undo restores.
        app.sculpt.brush = Brush::Smooth;
        app.sculpt_begin(g);
        for _ in 0..10 {
            app.sculpt_dab(Vec3::new(1.0, peak, 0.0), Vec3::Y, 1.0);
        }
        assert!(ys(&app) < peak);
        execute(&mut app, "undo; undo").unwrap();
        assert!(ys(&app).abs() < 1e-6);
        // Grab.
        app.sculpt.brush = Brush::Grab;
        app.sculpt.symmetry_x = false;
        app.sculpt_begin(g);
        app.sculpt_grab_start(Vec3::ZERO, 0.5);
        app.sculpt_grab_to(Vec3::new(0.0, 1.0, 0.0));
        assert!((ys(&app) - 1.0).abs() < 1e-3);
        // MEL brush command.
        execute(&mut app, "sculpt -b inflate -s 0.3 -r 40 -sym on").unwrap();
        assert_eq!(app.sculpt.brush, Brush::Inflate);
        assert!(app.sculpt.symmetry_x);
    }

    #[test]
    fn soft_select_weights() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 4 -h 4 -sx 8 -sy 8 -n p; select p; selectMode -v").unwrap();
        let p = app.scene.find_by_name("p").unwrap();
        app.comp_sel.insert((p, 40)); // centre vertex
        app.soft_select = true;
        app.soft_radius = 1.2;
        let w = app.soft_weights();
        assert!(w.len() > 5);
        assert!(w.iter().any(|x| x.1 == 40 && x.2 == 1.0));
        assert!(w.iter().all(|x| x.2 <= 1.0 && x.2 > 0.0));
        execute(&mut app, "softSelect -sse off").unwrap();
        assert_eq!(app.soft_weights().len(), 1);
    }
}
