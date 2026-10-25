//! Quad Draw (Mesh Tools › Quad Draw): retopology by placing dots on a live surface and
//! filling quads between them.
//!
//! The live object is a reference mesh; dots snap onto it (offset slightly along its normal so
//! the new mesh draws on top). The new mesh is an ordinary polygon object ("quadDraw1") with
//! connected, consistently wound faces.

use crate::app::App;
use crate::mesh::Mesh;
use crate::scene::{NodeId, Shape};
use glam::{Vec2, Vec3};

#[derive(Clone, Debug, Default)]
pub struct QuadDraw {
    /// Reference surface dots snap to ("Make Live").
    pub live: Option<NodeId>,
    /// Mesh being built.
    pub target: Option<NodeId>,
    /// Dot being dragged (vertex index) and whether the drag started an undo step.
    pub drag: Option<u32>,
    /// Boundary edge being extended with Ctrl-drag: (a, b, start mouse world point).
    pub extend: Option<(u32, u32, Vec3)>,
    /// Snap offset along the live normal (fraction of the live object's size).
    pub offset: f32,
    /// Relax brush / button iterations.
    pub relax_iters: u32,
}

/// Live-surface triangles in world space (with normals), for snapping.
pub struct LiveSurface {
    tris: Vec<[Vec3; 3]>,
    size: f32,
}

impl LiveSurface {
    pub fn from_node(app: &App, id: NodeId) -> Option<LiveSurface> {
        let mesh = app.scene.node(id)?.mesh()?;
        let pts = crate::rig::deformed_positions(&app.scene, id).unwrap_or_else(|| {
            let w = app.scene.world_matrix(id);
            mesh.positions.iter().map(|p| w.transform_point3(*p)).collect()
        });
        let mut tris = Vec::new();
        for f in &mesh.faces {
            for i in 1..f.len().saturating_sub(1) {
                tris.push([pts[f[0] as usize], pts[f[i] as usize], pts[f[i + 1] as usize]]);
            }
        }
        let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        Some(LiveSurface { tris, size: (hi - lo).length().max(1e-6) })
    }

    /// Closest surface point and its (face) normal.
    pub fn closest(&self, p: Vec3) -> Option<(Vec3, Vec3)> {
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for t in &self.tris {
            let b = crate::deform::closest_bary(p, t[0], t[1], t[2]);
            let q = t[0] * b[0] + t[1] * b[1] + t[2] * b[2];
            let d = q.distance_squared(p);
            if best.is_none_or(|x| d < x.0) {
                let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or(Vec3::Y);
                best = Some((d, q, n));
            }
        }
        best.map(|b| (b.1, b.2))
    }

    /// Snap a world point onto the surface, lifted by `offset` × size along the normal.
    pub fn snap(&self, p: Vec3, offset: f32) -> (Vec3, Vec3) {
        match self.closest(p) {
            Some((q, n)) => (q + n * offset * self.size, n),
            None => (p, Vec3::Y),
        }
    }
}

/// Directed edges of a mesh's faces.
fn directed(m: &Mesh) -> std::collections::HashSet<(u32, u32)> {
    let mut s = std::collections::HashSet::new();
    for f in &m.faces {
        for i in 0..f.len() {
            s.insert((f[i], f[(i + 1) % f.len()]));
        }
    }
    s
}

/// Boundary edges (used by exactly one face), as directed (a, b) following that face.
pub fn boundary_edges(m: &Mesh) -> Vec<(u32, u32)> {
    let d = directed(m);
    d.iter().filter(|(a, b)| !d.contains(&(*b, *a))).copied().collect()
}

/// Order 3–4 vertices into a simple polygon around their centroid (seen along `normal`).
fn order_ring(m: &Mesh, verts: &[u32], normal: Vec3) -> Vec<u32> {
    let pts: Vec<Vec3> = verts.iter().map(|v| m.positions[*v as usize]).collect();
    let c = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
    let n = normal.normalize_or(Vec3::Y);
    let a = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let bx = n.cross(a).normalize();
    let by = n.cross(bx);
    let mut idx: Vec<(f32, u32)> = verts.iter().zip(&pts).map(|(v, p)| ((*p - c).dot(by).atan2((*p - c).dot(bx)), *v)).collect();
    idx.sort_by(|x, y| x.0.total_cmp(&y.0));
    idx.into_iter().map(|x| x.1).collect()
}

impl App {
    pub fn qd_live_surface(&self) -> Option<LiveSurface> {
        self.qd.live.and_then(|l| LiveSurface::from_node(self, l))
    }

    /// Make an object live (or clear with None).
    pub fn qd_make_live(&mut self, id: Option<NodeId>) -> Result<(), String> {
        if let Some(i) = id
            && !self.scene.node(i).is_some_and(|n| n.mesh().is_some())
        {
            return Err("Make Live: pick a polygon mesh.".into());
        }
        self.qd.live = id;
        if self.qd.offset <= 0.0 {
            self.qd.offset = 0.002;
        }
        self.info(match id.and_then(|i| self.scene.node(i)) {
            Some(n) => format!("{} is live", n.name),
            None => "No live object".into(),
        });
        Ok(())
    }

    /// The mesh being drawn (created on first use).
    pub fn qd_target(&mut self) -> NodeId {
        if let Some(t) = self.qd.target.filter(|t| self.scene.node(*t).is_some_and(|n| n.mesh().is_some())) {
            return t;
        }
        let id = self.scene.add_node("quadDraw1", Shape::Poly { history: None, mesh: Mesh::default(), rev: crate::scene::next_rev() }, None);
        self.apply_name(id, Some("quadDraw1"));
        if let Some(n) = self.scene.node_mut(id) {
            n.color = [0.45, 0.75, 1.0];
        }
        self.qd.target = Some(id);
        id
    }

    fn qd_mesh(&self) -> Option<&Mesh> {
        self.qd.target.and_then(|t| self.scene.node(t)).and_then(|n| n.mesh())
    }

    /// World positions of the target's vertices.
    pub fn qd_points(&self) -> Vec<Vec3> {
        let Some(t) = self.qd.target else { return Vec::new() };
        let Some(m) = self.qd_mesh() else { return Vec::new() };
        let w = self.scene.world_matrix(t);
        m.positions.iter().map(|p| w.transform_point3(*p)).collect()
    }

    fn qd_edit(&mut self, f: impl FnOnce(&mut Mesh, glam::Mat4)) {
        let t = self.qd_target();
        let inv = self.scene.world_matrix(t).inverse();
        if let Some(m) = self.scene.node_mut(t).and_then(|n| n.mesh_mut(true)) {
            f(m, inv);
            m.drop_uvs();
        }
    }

    /// Add a dot at a world point (snapped to the live surface). Returns its vertex index.
    pub fn qd_add_dot(&mut self, p: Vec3) -> u32 {
        self.checkpoint();
        let (sp, _) = self.qd_live_surface().map(|s| s.snap(p, self.qd.offset)).unwrap_or((p, Vec3::Y));
        let mut idx = 0;
        self.qd_edit(|m, inv| {
            m.positions.push(inv.transform_point3(sp));
            idx = (m.positions.len() - 1) as u32;
        });
        idx
    }

    /// Move a dot (no undo step: callers checkpoint at drag start).
    pub fn qd_move_dot(&mut self, v: u32, p: Vec3) {
        let (sp, _) = self.qd_live_surface().map(|s| s.snap(p, self.qd.offset)).unwrap_or((p, Vec3::Y));
        self.qd_edit(|m, inv| {
            if let Some(q) = m.positions.get_mut(v as usize) {
                *q = inv.transform_point3(sp);
            }
        });
    }

    /// Fill a quad (4 dots) or triangle (3 dots). The winding follows neighbouring faces, or
    /// else faces out of the live surface (or toward `view_dir`'s opposite).
    pub fn qd_fill(&mut self, verts: &[u32], view_dir: Option<Vec3>) -> Result<usize, String> {
        let m = self.qd_mesh().cloned().ok_or("Quad Draw: place some dots first.")?;
        let mut vs: Vec<u32> = verts.to_vec();
        vs.sort();
        vs.dedup();
        if !(3..=4).contains(&vs.len()) || vs.iter().any(|v| *v as usize >= m.positions.len()) {
            return Err("Quad Draw: a face needs 3 or 4 distinct dots.".into());
        }
        if m.faces.iter().any(|f| {
            let mut s = f.clone();
            s.sort();
            s == vs
        }) {
            return Err("Quad Draw: that face already exists.".into());
        }
        let t = self.qd.target.unwrap();
        let w = self.scene.world_matrix(t);
        let wp: Vec<Vec3> = vs.iter().map(|v| w.transform_point3(m.positions[*v as usize])).collect();
        let c = wp.iter().copied().sum::<Vec3>() / wp.len() as f32;
        let surf_n = self.qd_live_surface().and_then(|s| s.closest(c)).map(|x| x.1);
        let normal = surf_n.or(view_dir.map(|d| -d)).unwrap_or_else(|| crate::nurbs::plane_normal(&wp));
        let mut ring = order_ring(&m, &vs, glam::Mat3::from_mat4(w).transpose() * normal);
        // Winding: agree with neighbours (shared edges run opposite), else with the normal.
        let d = directed(&m);
        let n = ring.len();
        let (same, opp) = (0..n).fold((0, 0), |(s, o), i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            (s + d.contains(&(a, b)) as u32, o + d.contains(&(b, a)) as u32)
        });
        if same > opp {
            ring.reverse();
        } else if same == 0 && opp == 0 {
            let pts: Vec<Vec3> = ring.iter().map(|v| m.positions[*v as usize]).collect();
            let fnrm = (pts[1] - pts[0]).cross(pts[2] - pts[0]) + if n == 4 { (pts[2] - pts[0]).cross(pts[3] - pts[0]) } else { Vec3::ZERO };
            let wn = glam::Mat3::from_mat4(w).inverse().transpose() * fnrm;
            if wn.dot(normal) < 0.0 {
                ring.reverse();
            }
        }
        self.checkpoint();
        let mut fi = 0;
        self.qd_edit(|m, _| {
            m.faces.push(ring);
            fi = m.faces.len() - 1;
        });
        Ok(fi)
    }

    pub fn qd_delete_face(&mut self, f: usize) -> Result<(), String> {
        if self.qd_mesh().is_none_or(|m| f >= m.faces.len()) {
            return Err("Quad Draw: no such face.".into());
        }
        self.checkpoint();
        self.qd_edit(|m, _| {
            m.faces.remove(f);
        });
        Ok(())
    }

    /// Delete a dot and the faces that use it.
    pub fn qd_delete_dot(&mut self, v: u32) -> Result<(), String> {
        if self.qd_mesh().is_none_or(|m| v as usize >= m.positions.len()) {
            return Err("Quad Draw: no such dot.".into());
        }
        self.checkpoint();
        self.qd_edit(|m, _| {
            m.faces.retain(|f| !f.contains(&v));
            m.positions.remove(v as usize);
            for f in &mut m.faces {
                for i in f.iter_mut() {
                    if *i > v {
                        *i -= 1;
                    }
                }
            }
        });
        Ok(())
    }

    /// Extend a boundary edge (a, b) by `offset` (world): adds two dots and a quad.
    pub fn qd_extend(&mut self, a: u32, b: u32, offset: Vec3) -> Result<usize, String> {
        let m = self.qd_mesh().cloned().ok_or("Quad Draw: nothing to extend.")?;
        let bd = boundary_edges(&m);
        let Some(&(ea, eb)) = bd.iter().find(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a)) else {
            return Err("Quad Draw: extend needs a border edge.".into());
        };
        let pts = self.qd_points();
        let (pa, pb) = (pts[ea as usize] + offset, pts[eb as usize] + offset);
        self.checkpoint();
        let snap = self.qd_live_surface();
        let off = self.qd.offset;
        let s = |p: Vec3| snap.as_ref().map(|s| s.snap(p, off).0).unwrap_or(p);
        let (sa, sb) = (s(pa), s(pb));
        let mut fi = 0;
        self.qd_edit(|m, inv| {
            m.positions.push(inv.transform_point3(sa));
            m.positions.push(inv.transform_point3(sb));
            let (na, nb) = ((m.positions.len() - 2) as u32, (m.positions.len() - 1) as u32);
            // The boundary edge runs ea→eb in its face, so the new face uses eb→ea.
            m.faces.push(vec![eb, ea, na, nb]);
            fi = m.faces.len() - 1;
        });
        Ok(fi)
    }

    /// Relax: move interior dots toward the average of their neighbours, then re-snap. Border
    /// dots stay put. Returns the number of dots moved.
    pub fn qd_relax(&mut self, iters: u32) -> Result<usize, String> {
        let m = self.qd_mesh().cloned().ok_or("Quad Draw: nothing to relax.")?;
        let n = m.positions.len();
        let mut nbr: Vec<Vec<u32>> = vec![Vec::new(); n];
        for f in &m.faces {
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                if !nbr[a as usize].contains(&b) {
                    nbr[a as usize].push(b);
                }
                if !nbr[b as usize].contains(&a) {
                    nbr[b as usize].push(a);
                }
            }
        }
        let border: std::collections::HashSet<u32> = boundary_edges(&m).into_iter().flat_map(|(a, b)| [a, b]).collect();
        let movable: Vec<usize> = (0..n).filter(|v| !nbr[*v].is_empty() && !border.contains(&(*v as u32))).collect();
        if movable.is_empty() {
            return Ok(0);
        }
        let snap = self.qd_live_surface();
        let off = self.qd.offset;
        let mut pts = self.qd_points();
        for _ in 0..iters.max(1) {
            let prev = pts.clone();
            for &v in &movable {
                let avg = nbr[v].iter().map(|u| prev[*u as usize]).sum::<Vec3>() / nbr[v].len() as f32;
                let p = prev[v].lerp(avg, 0.5);
                pts[v] = snap.as_ref().map(|s| s.snap(p, off).0).unwrap_or(p);
            }
        }
        self.checkpoint();
        self.qd_edit(|m, inv| {
            for &v in &movable {
                m.positions[v] = inv.transform_point3(pts[v]);
            }
        });
        Ok(movable.len())
    }

    /// Nearest `k` dots to a screen point (`project` maps world → screen), unused-in-a-face
    /// check left to the caller.
    pub fn qd_nearest_dots(&self, screen: Vec2, k: usize, project: &dyn Fn(Vec3) -> Option<Vec2>) -> Vec<u32> {
        let mut d: Vec<(f32, u32)> = self.qd_points().iter().enumerate().filter_map(|(i, p)| project(*p).map(|s| (s.distance(screen), i as u32))).collect();
        d.sort_by(|a, b| a.0.total_cmp(&b.0));
        d.into_iter().take(k).map(|x| x.1).collect()
    }

    /// Finish: leave the tool, keep the new mesh selected.
    pub fn qd_finish(&mut self) {
        if let Some(t) = self.qd.target.take() {
            // Drop unused dots.
            if let Some(m) = self.scene.node_mut(t).and_then(|n| n.mesh_mut(true)) {
                let used: std::collections::BTreeSet<u32> = m.faces.iter().flatten().copied().collect();
                if used.len() < m.positions.len() {
                    let map: std::collections::HashMap<u32, u32> = used.iter().enumerate().map(|(i, v)| (*v, i as u32)).collect();
                    m.positions = used.iter().map(|v| m.positions[*v as usize]).collect();
                    for f in &mut m.faces {
                        for i in f.iter_mut() {
                            *i = map[i];
                        }
                    }
                }
            }
            self.sel = vec![t];
        }
    }
}

/// `makeLive [mesh]` and `quadDraw` MEL commands. None = not one of these.
pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let r = match cmd.as_str() {
        "makeLive" => (|| {
            if rest.iter().any(|t| t == "-n" || t == "-none") {
                app.qd_make_live(None)?;
                return Ok(String::new());
            }
            let names: Vec<String> = rest.iter().filter(|t| !t.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &names)?;
            let id = *ids.first().ok_or("makeLive mesh")?;
            app.qd_make_live(Some(id))?;
            Ok(app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default())
        })(),
        "quadDraw" => (|| {
            let nums = |from: usize| -> Vec<f32> { rest.iter().skip(from).map_while(|t| t.parse::<f32>().ok()).collect() };
            let ints = |from: usize| -> Vec<u32> { rest.iter().skip(from).map_while(|t| t.parse::<u32>().ok()).collect() };
            let Some(flag) = rest.first() else {
                app.tool = crate::app::Tool::QuadDraw;
                return Ok("Quad Draw tool".into());
            };
            match flag.as_str() {
                "-live" | "-l" => {
                    let n = rest.get(1).ok_or("quadDraw -live mesh")?;
                    let id = app.scene.find_by_name(n).ok_or_else(|| format!("No object matches name: {n}"))?;
                    app.qd_make_live(Some(id))?;
                    Ok(n.clone())
                }
                "-dot" | "-d" => {
                    let v = nums(1);
                    if v.len() < 3 {
                        return Err("quadDraw -dot x y z".into());
                    }
                    Ok(app.qd_add_dot(Vec3::new(v[0], v[1], v[2])).to_string())
                }
                "-moveDot" => {
                    let i = ints(1);
                    let v = nums(2);
                    if i.is_empty() || v.len() < 3 {
                        return Err("quadDraw -moveDot index x y z".into());
                    }
                    app.checkpoint();
                    app.qd_move_dot(i[0], Vec3::new(v[0], v[1], v[2]));
                    Ok(String::new())
                }
                "-quad" | "-q" | "-tri" | "-t" => {
                    let v = ints(1);
                    let want = if flag.starts_with("-q") { 4 } else { 3 };
                    if v.len() != want {
                        return Err(format!("quadDraw {flag} needs {want} dot indices"));
                    }
                    app.qd_fill(&v, None).map(|f| f.to_string())
                }
                "-extend" | "-e" => {
                    let i = ints(1);
                    let v = nums(3);
                    if i.len() < 2 || v.len() < 3 {
                        return Err("quadDraw -extend a b dx dy dz".into());
                    }
                    app.qd_extend(i[0], i[1], Vec3::new(v[0], v[1], v[2])).map(|f| f.to_string())
                }
                "-relax" | "-r" => {
                    let n = ints(1).first().copied().unwrap_or(5);
                    app.qd_relax(n).map(|n| format!("{n} relaxed"))
                }
                "-deleteFace" => {
                    let f = ints(1).first().copied().ok_or("quadDraw -deleteFace index")?;
                    app.qd_delete_face(f as usize).map(|_| String::new())
                }
                "-deleteDot" => {
                    let v = ints(1).first().copied().ok_or("quadDraw -deleteDot index")?;
                    app.qd_delete_dot(v).map(|_| String::new())
                }
                "-new" => {
                    app.qd.target = None;
                    Ok(String::new())
                }
                "-finish" | "-f" => {
                    app.qd_finish();
                    if app.tool == crate::app::Tool::QuadDraw {
                        app.tool = crate::app::Tool::Select;
                    }
                    Ok(app.sel.first().and_then(|i| app.scene.node(*i)).map(|n| n.name.clone()).unwrap_or_default())
                }
                f => Err(format!("quadDraw: unknown flag {f} (-live -dot -quad -tri -extend -relax -deleteFace -deleteDot -new -finish)")),
            }
        })(),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;
    use glam::Vec3;

    #[test]
    fn quad_draw_on_live_sphere() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -sa 24 -sh 16 -n ref; makeLive ref").unwrap();
        // Four dots near the +Z pole region, slightly off the surface → snapped onto it.
        let pts = [Vec3::new(-0.3, -0.3, 1.2), Vec3::new(0.3, -0.3, 1.2), Vec3::new(0.3, 0.3, 1.2), Vec3::new(-0.3, 0.3, 1.2)];
        for p in pts {
            execute(&mut app, &format!("quadDraw -dot {} {} {}", p.x, p.y, p.z)).unwrap();
        }
        let qd = app.qd_points();
        assert_eq!(qd.len(), 4);
        for p in &qd {
            assert!((p.length() - 1.0).abs() < 0.02, "snapped to the sphere: {}", p.length());
        }
        // Fill in a scrambled order: the quad is ordered and faces outward (+Z).
        execute(&mut app, "quadDraw -quad 0 2 1 3").unwrap();
        let t = app.qd.target.unwrap();
        let m = app.scene.node(t).unwrap().mesh().unwrap().clone();
        assert_eq!(m.faces.len(), 1);
        assert!(m.face_normal(0).z > 0.9, "{}", m.face_normal(0));
        // Extend the right border edge sideways: new quad shares the edge, same winding.
        execute(&mut app, "quadDraw -extend 1 2 0.6 0 0").unwrap();
        let m = app.scene.node(t).unwrap().mesh().unwrap().clone();
        assert_eq!((m.faces.len(), m.positions.len()), (2, 6));
        assert!(m.face_normal(1).dot(Vec3::new(0.7, 0.0, 0.7)) > 0.5);
        assert!(crate::quad_draw::boundary_edges(&m).len() == 6, "shared edge is interior");
        // A triangle from three dots, then delete it.
        execute(&mut app, "quadDraw -dot 0 -0.9 0.6").unwrap();
        let f = execute(&mut app, "quadDraw -tri 0 1 6").unwrap();
        assert_eq!(f, "2");
        execute(&mut app, "quadDraw -deleteFace 2").unwrap();
        // Relax a 3x3 patch: the centre dot moves to the middle and stays on the sphere.
        execute(&mut app, "quadDraw -new").unwrap();
        let mut ids = Vec::new();
        for j in 0..3 {
            for i in 0..3 {
                let jitter = if i == 1 && j == 1 { 0.15 } else { 0.0 };
                let p = Vec3::new(-0.4 + 0.4 * i as f32 + jitter, -0.4 + 0.4 * j as f32 + jitter, 1.0);
                ids.push(execute(&mut app, &format!("quadDraw -dot {} {} {}", p.x, p.y, p.z)).unwrap());
            }
        }
        for (a, b, c, d) in [(0, 1, 4, 3), (1, 2, 5, 4), (3, 4, 7, 6), (4, 5, 8, 7)] {
            execute(&mut app, &format!("quadDraw -quad {} {} {} {}", ids[a], ids[b], ids[c], ids[d])).unwrap();
        }
        let before = app.qd_points()[4];
        execute(&mut app, "quadDraw -relax 20").unwrap();
        let after = app.qd_points()[4];
        assert!(Vec3::new(after.x, after.y, 0.0).length() < Vec3::new(before.x, before.y, 0.0).length() * 0.3, "{before} -> {after}");
        assert!((after.length() - 1.0).abs() < 0.02);
        let m = app.scene.node(app.qd.target.unwrap()).unwrap().mesh().unwrap().clone();
        assert!((0..4).all(|f| m.face_normal(f).z > 0.5), "consistent outward winding");
        // Undo removes the relax; finishing selects the mesh.
        app.undo();
        assert!(app.qd_points()[4].distance(before) < 1e-5);
        execute(&mut app, "quadDraw -finish").unwrap();
        assert!(app.qd.target.is_none() && app.sel.len() == 1);
    }
}
