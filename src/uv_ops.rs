//! Undoable UV actions on the App (UV Editor, menus, MEL and the AI).

use crate::app::{App, CompMode};
use crate::scene::NodeId;
use crate::uv;
use glam::Vec2;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// 0 = X, 1 = Y, 2 = Z, 3 = best plane.
    Planar(usize),
    Cylindrical,
    Spherical,
    Automatic,
}

impl App {
    /// Meshes affected by a UV operation, with their selected faces in face mode.
    pub fn uv_targets(&self) -> Vec<(NodeId, Option<Vec<usize>>)> {
        let mut out: Vec<(NodeId, Option<Vec<usize>>)> = Vec::new();
        if self.comp_mode == CompMode::Face && !self.comp_sel.is_empty() {
            let mut m: HashMap<NodeId, Vec<usize>> = HashMap::new();
            for (id, f) in &self.comp_sel {
                m.entry(*id).or_default().push(*f as usize);
            }
            out.extend(m.into_iter().map(|(id, f)| (id, Some(f))));
        } else {
            let mut seen = HashSet::new();
            for &s in &self.sel {
                for id in self.scene.subtree(s) {
                    if self.scene.node(id).is_some_and(|n| n.mesh().is_some()) && seen.insert(id) {
                        out.push((id, None));
                    }
                }
            }
        }
        out
    }

    /// Project UVs onto the selected meshes (or selected faces). Returns the number of meshes.
    pub fn uv_project(&mut self, p: Projection) -> Result<usize, String> {
        let targets = self.uv_targets();
        if targets.is_empty() {
            return Err("UV projection: select polygon objects or faces.".into());
        }
        self.checkpoint();
        for (id, faces) in &targets {
            if let Some(m) = self.scene.node_mut(*id).and_then(|n| n.mesh_mut(true)) {
                let sub = faces.as_deref();
                match p {
                    Projection::Planar(a) => uv::planar(m, sub, a),
                    Projection::Cylindrical => uv::cylindrical(m, sub),
                    Projection::Spherical => uv::spherical(m, sub),
                    Projection::Automatic => uv::automatic(m, sub),
                }
            }
        }
        self.uv_sel.clear();
        Ok(targets.len())
    }

    pub fn uv_layout(&mut self) -> Result<usize, String> {
        let targets = self.uv_targets();
        if targets.is_empty() {
            return Err("Layout UVs: select polygon objects.".into());
        }
        self.checkpoint();
        for (id, _) in &targets {
            if let Some(m) = self.scene.node_mut(*id).and_then(|n| n.mesh_mut(true)) {
                uv::layout(m);
            }
        }
        self.uv_sel.clear();
        Ok(targets.len())
    }

    pub fn uv_normalize(&mut self) -> Result<usize, String> {
        let targets = self.uv_targets();
        if targets.is_empty() {
            return Err("Normalize UVs: select polygon objects.".into());
        }
        self.checkpoint();
        for (id, _) in &targets {
            if let Some(m) = self.scene.node_mut(*id).and_then(|n| n.mesh_mut(true)) {
                uv::normalize(m);
            }
        }
        Ok(targets.len())
    }

    /// UV ids to transform per mesh: the UV Editor selection, else selected faces, else all.
    fn uv_transform_sets(&self) -> Vec<(NodeId, Option<HashSet<u32>>)> {
        if !self.uv_sel.is_empty() {
            let mut m: HashMap<NodeId, HashSet<u32>> = HashMap::new();
            for (id, u) in &self.uv_sel {
                m.entry(*id).or_default().insert(*u);
            }
            return m.into_iter().map(|(k, v)| (k, Some(v))).collect();
        }
        self.uv_targets()
            .into_iter()
            .map(|(id, faces)| {
                let set = faces.and_then(|f| self.scene.node(id).and_then(|n| n.mesh()).map(|m| uv::uvs_of_faces(m, &f)));
                (id, set)
            })
            .collect()
    }

    /// Move / scale / rotate UVs (UV selection, else selected faces, else whole meshes).
    /// `live` skips the undo checkpoint (used while dragging).
    pub fn uv_transform(&mut self, translate: Vec2, scale: Vec2, rotate_deg: f32, pivot: Option<Vec2>, live: bool) -> Result<usize, String> {
        let sets = self.uv_transform_sets();
        if sets.is_empty() {
            return Err("Edit UVs: select UVs in the UV Editor, faces, or objects.".into());
        }
        if !live {
            self.checkpoint();
        }
        let mut n = 0;
        // A shared pivot across meshes (selection centre) unless given.
        let pivot = pivot.or_else(|| self.uv_selection_bounds().map(|(a, b)| (a + b) * 0.5));
        for (id, ids) in sets {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                if !m.has_uvs() {
                    continue;
                }
                uv::transform(m, ids.as_ref(), translate, scale, rotate_deg, pivot);
                n += 1;
            }
        }
        if n == 0 {
            return Err("Edit UVs: the selection has no UVs — apply a projection first.".into());
        }
        Ok(n)
    }

    /// Bounds of the UVs `uv_transform` would affect.
    pub fn uv_selection_bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut lo = Vec2::splat(f32::MAX);
        let mut hi = Vec2::splat(f32::MIN);
        let mut any = false;
        for (id, ids) in self.uv_transform_sets() {
            let Some(m) = self.scene.node(id).and_then(|n| n.mesh()).filter(|m| m.has_uvs()) else { continue };
            let it: Box<dyn Iterator<Item = Vec2>> = match &ids {
                Some(s) => Box::new(s.iter().filter_map(|i| m.uvs.get(*i as usize).copied())),
                None => Box::new(m.uvs.iter().copied()),
            };
            for p in it {
                lo = lo.min(p);
                hi = hi.max(p);
                any = true;
            }
        }
        any.then_some((lo, hi))
    }

    /// Mesh edges for UV cut / sew: selected edges (edge mode), else edges whose two UVs are
    /// selected in the UV Editor.
    fn uv_edge_sets(&self) -> Vec<(NodeId, HashSet<(u32, u32)>)> {
        let key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
        let mut out: HashMap<NodeId, HashSet<(u32, u32)>> = HashMap::new();
        for (id, edges) in self.selected_edges() {
            out.entry(id).or_default().extend(edges.into_iter().map(|(a, b)| key(a, b)));
        }
        if out.is_empty() && !self.uv_sel.is_empty() {
            let ids: HashSet<NodeId> = self.uv_sel.iter().map(|(n, _)| *n).collect();
            for id in ids {
                let Some(m) = self.scene.node(id).and_then(|n| n.mesh()).filter(|m| m.has_uvs()) else { continue };
                for (f, fu) in m.faces.iter().zip(&m.face_uvs) {
                    let n = f.len();
                    for k in 0..n {
                        if self.uv_sel.contains(&(id, fu[k])) && self.uv_sel.contains(&(id, fu[(k + 1) % n])) {
                            out.entry(id).or_default().insert(key(f[k], f[(k + 1) % n]));
                        }
                    }
                }
            }
        }
        out.into_iter().filter(|(_, e)| !e.is_empty()).collect()
    }

    /// Cut UVs along the selected edges. Returns the number of UVs created.
    pub fn uv_cut(&mut self) -> Result<usize, String> {
        let sets = self.uv_edge_sets();
        if sets.is_empty() {
            return Err("Cut UVs: select edges (edge mode, or UVs along an edge in the UV Editor).".into());
        }
        self.checkpoint();
        let mut n = 0;
        for (id, edges) in sets {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                n += uv::cut_edges(m, &edges);
            }
        }
        self.uv_sel.clear();
        Ok(n)
    }

    /// Move and sew UVs along the selected edges. Returns the number of edges sewn.
    pub fn uv_sew(&mut self) -> Result<usize, String> {
        let sets = self.uv_edge_sets();
        if sets.is_empty() {
            return Err("Sew UVs: select seam edges (edge mode, or UVs along an edge in the UV Editor).".into());
        }
        self.checkpoint();
        let mut n = 0;
        for (id, edges) in sets {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                n += uv::sew_edges(m, &edges);
            }
        }
        self.uv_sel.clear();
        Ok(n)
    }

    /// Unfold UV shells (selected faces' shells, shells with selected UVs, or whole meshes).
    pub fn uv_unfold(&mut self) -> Result<usize, String> {
        let mut sets: Vec<(NodeId, Option<Vec<usize>>)> = Vec::new();
        if !self.uv_sel.is_empty() {
            let ids: HashSet<NodeId> = self.uv_sel.iter().map(|(n, _)| *n).collect();
            for id in ids {
                let Some(m) = self.scene.node(id).and_then(|n| n.mesh()).filter(|m| m.has_uvs()) else { continue };
                let faces: Vec<usize> = (0..m.faces.len()).filter(|f| m.face_uvs[*f].iter().any(|u| self.uv_sel.contains(&(id, *u)))).collect();
                sets.push((id, Some(faces)));
            }
        } else {
            sets = self.uv_targets();
        }
        if sets.is_empty() {
            return Err("Unfold: select polygon objects, faces or UVs.".into());
        }
        self.checkpoint();
        let mut n = 0;
        for (id, faces) in sets {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                if !m.has_uvs() {
                    uv::automatic(m, None);
                }
                n += uv::unfold(m, faces.as_deref());
            }
        }
        if n == 0 {
            return Err("Unfold: nothing flattened — closed shells need a seam. Select edges and Cut first (e.g. one vertical edge loop on a cylinder).".into());
        }
        Ok(n)
    }

    /// Select whole UV shells containing the current UV selection.
    pub fn uv_select_shell(&mut self) {
        let mut add = Vec::new();
        let ids: HashSet<NodeId> = self.uv_sel.iter().map(|(n, _)| *n).collect();
        for id in ids {
            let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) else { continue };
            for shell in uv::shells(m) {
                let uvs = uv::uvs_of_faces(m, &shell);
                if uvs.iter().any(|u| self.uv_sel.contains(&(id, *u))) {
                    add.extend(uvs.into_iter().map(|u| (id, u)));
                }
            }
        }
        self.uv_sel.extend(add);
    }

    /// Set the colour texture of a material ("checker" for the UV checker, None to clear).
    pub fn set_color_map(&mut self, mat: u32, path: Option<String>) -> Result<(), String> {
        self.set_material_map(mat, crate::look::MapSlot::Color, path)
    }

    /// Connect (or disconnect with None) a texture to a material slot.
    pub fn set_material_map(&mut self, mat: u32, slot: crate::look::MapSlot, path: Option<String>) -> Result<(), String> {
        if let Some(p) = &path
            && p != crate::texture::CHECKER
        {
            self.textures.get(p)?;
        }
        self.checkpoint();
        if let Some(m) = self.material_mut(mat) {
            *m.map_mut(slot) = path;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;

    #[test]
    fn mel_uv_and_texture() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -n box; polySphere -n ball").unwrap();
        let ball = app.scene.find_by_name("ball").unwrap();
        assert!(app.scene.node(ball).unwrap().mesh().unwrap().has_uvs(), "primitives come with UVs");
        execute(&mut app, "polyAutoProjection box").unwrap();
        let bx = app.scene.find_by_name("box").unwrap();
        assert_eq!(crate::uv::shells(app.scene.node(bx).unwrap().mesh().unwrap()).len(), 6);
        execute(&mut app, "polyPlanarProjection -md y ball; polyCylindricalProjection ball; polySphericalProjection ball; polyLayoutUV ball; polyNormalizeUV ball").unwrap();
        // Move all UVs of the box by 0.5 in U.
        let before = app.scene.node(bx).unwrap().mesh().unwrap().uvs[0];
        execute(&mut app, "polyEditUV -u 0.5 -v 0 box").unwrap();
        let after = app.scene.node(bx).unwrap().mesh().unwrap().uvs[0];
        assert!((after.x - before.x - 0.5).abs() < 1e-5);
        execute(&mut app, "undo").unwrap();
        assert_eq!(app.scene.node(bx).unwrap().mesh().unwrap().uvs[0], before);
        // Textures: checker map on a material, then a PNG file.
        let png = std::env::temp_dir().join("rm_uv_test.png");
        std::fs::write(&png, crate::texture::TexImage::checker().to_png()).unwrap();
        execute(&mut app, "shadingNode -asShader lambert -n tiles; hyperShade -assign tiles box ball").unwrap();
        execute(&mut app, &format!("setAttr tiles.colorMap \"{}\"", png.display())).unwrap();
        assert_eq!(execute(&mut app, "getAttr tiles.colorMap").unwrap(), png.display().to_string());
        execute(&mut app, "setAttr tiles.repeatUV 2 2").unwrap();
        assert!(execute(&mut app, "setAttr tiles.colorMap \"/missing/file.png\"").is_err());
        let g = app.build_geometry();
        let ts = g.textures.as_ref().unwrap();
        assert_eq!(ts.layers.len(), 2, "checker + the png");
        // Vertices carry the texture layer and repeated UVs.
        let v = &g.tris[0].data[0];
        assert_eq!(v.mat[3], 2.0);
        assert_eq!(v.emi[3], 1.0);
        execute(&mut app, "uvChecker on").unwrap();
        assert!(app.views[app.active_view].uv_checker);
    }

    #[test]
    fn mel_cut_sew_unfold() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 4 -h 2 -sx 8 -sy 4 -n p").unwrap();
        let p = app.scene.find_by_name("p").unwrap();
        // Scramble the UVs (a smooth warp), then unfold: the distortion disappears.
        {
            let m = app.scene.node_mut(p).unwrap().mesh_mut(true).unwrap();
            for u in m.uvs.iter_mut() {
                *u = glam::Vec2::new(u.x * u.x + 0.3 * u.y, u.y + 0.4 * u.x * u.y);
            }
        }
        let before = crate::uv::stretch(app.scene.node(p).unwrap().mesh().unwrap());
        let r = execute(&mut app, "u3dUnfold p").unwrap();
        assert!(r.starts_with("1 shells"), "{r}");
        let after = crate::uv::stretch(app.scene.node(p).unwrap().mesh().unwrap());
        assert!(before > 0.1 && after < 0.01, "{before} -> {after}");
        // Cut the middle row of edges: two shells; sew them back: one.
        let m = app.scene.node(p).unwrap().mesh().unwrap();
        let mid: Vec<String> = m
            .edges()
            .iter()
            .enumerate()
            .filter(|(_, (a, b))| m.positions[*a as usize].z.abs() < 1e-4 && m.positions[*b as usize].z.abs() < 1e-4)
            .map(|(i, _)| i.to_string())
            .collect();
        assert_eq!(mid.len(), 8);
        execute(&mut app, &format!("polyMapCut -e {} p", mid.join(" "))).unwrap();
        assert_eq!(crate::uv::shells(app.scene.node(p).unwrap().mesh().unwrap()).len(), 2);
        execute(&mut app, "undo").unwrap();
        assert_eq!(crate::uv::shells(app.scene.node(p).unwrap().mesh().unwrap()).len(), 1);
        execute(&mut app, &format!("polyMapCut -e {0} p; polyMapSewMove -e {0} p", mid.join(" "))).unwrap();
        assert_eq!(crate::uv::shells(app.scene.node(p).unwrap().mesh().unwrap()).len(), 1);
        assert!(app.scene.node(p).unwrap().mesh().unwrap().has_uvs());
    }
}
