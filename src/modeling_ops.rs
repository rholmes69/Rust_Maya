//! Undoable edge-level modeling actions on the App (menus, tools, MEL, AI).

use crate::app::{App, CompMode};
use crate::mesh_ops::{key, Edge};
use crate::scene::NodeId;
use std::collections::{BTreeSet, HashMap};

impl App {
    /// Selected components of one kind grouped by node.
    fn comp_by_node(&self) -> HashMap<NodeId, Vec<u32>> {
        let mut m: HashMap<NodeId, Vec<u32>> = HashMap::new();
        for &(id, i) in &self.comp_sel {
            m.entry(id).or_default().push(i);
        }
        m
    }

    fn selected_edges(&self) -> HashMap<NodeId, Vec<Edge>> {
        let mut out = HashMap::new();
        if self.comp_mode != CompMode::Edge {
            return out;
        }
        for (id, idx) in self.comp_by_node() {
            if let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) {
                let all = m.edges();
                out.insert(id, idx.iter().filter_map(|i| all.get(*i as usize).copied()).collect());
            }
        }
        out
    }

    /// Select edges of `id` given as vertex pairs (switches to edge mode).
    pub fn select_edges(&mut self, id: NodeId, edges: &[Edge], add: bool) {
        let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) else { return };
        let all = m.edges();
        let idx: Vec<u32> = edges.iter().filter_map(|e| all.iter().position(|x| *x == key(e.0, e.1)).map(|i| i as u32)).collect();
        if self.comp_mode != CompMode::Edge {
            self.comp_mode = CompMode::Edge;
            self.comp_sel.clear();
        }
        if !self.sel.contains(&id) {
            self.sel.push(id);
        }
        if !add {
            self.comp_sel.clear();
        }
        for i in idx {
            self.comp_sel.insert((id, i));
        }
    }

    pub fn select_edge_loop(&mut self, ring: bool) {
        let sel = self.selected_edges();
        let mut out: Vec<(NodeId, Vec<Edge>)> = Vec::new();
        for (id, edges) in sel {
            let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) else { continue };
            let mut all = BTreeSet::new();
            for e in edges {
                for x in if ring { m.edge_ring(e) } else { m.edge_loop(e) } {
                    all.insert(x);
                }
            }
            out.push((id, all.into_iter().collect()));
        }
        if out.is_empty() {
            self.warn("Select an edge first (edge mode, F10).");
            return;
        }
        self.comp_sel.clear();
        for (id, e) in out {
            self.select_edges(id, &e, true);
        }
    }

    /// Insert an edge loop through the ring of `edge` at fraction `t` (from edge.0).
    pub fn insert_edge_loop(&mut self, id: NodeId, edge: Edge, t: f32) -> usize {
        self.checkpoint();
        let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) else { return 0 };
        let new = m.insert_edge_loop(edge, t);
        let n = new.len();
        self.select_edges(id, &new, false);
        n
    }

    /// Insert loops on every selected edge's ring (Maya polySplitRing).
    pub fn split_ring_selected(&mut self, t: f32) -> Result<String, String> {
        let sel = self.selected_edges();
        if sel.is_empty() {
            return Err("Insert Edge Loop: select an edge (edge mode) or use the Insert Edge Loop tool.".into());
        }
        self.checkpoint();
        let mut total = 0;
        for (id, edges) in sel {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                // Edges are vertex pairs, so they stay valid while the mesh grows.
                let mut done = BTreeSet::new();
                for e in edges {
                    if done.contains(&e) {
                        continue;
                    }
                    for r in m.edge_ring(e) {
                        done.insert(r);
                    }
                    total += m.insert_edge_loop(e, t).len();
                }
            }
        }
        self.comp_sel.clear();
        Ok(format!("{total} edge(s) inserted"))
    }

    pub fn bevel_selected(&mut self, width: f32) -> Result<String, String> {
        let mut sel = self.selected_edges();
        if sel.is_empty() && self.comp_mode == CompMode::Object {
            // Object mode: bevel every edge of the selected meshes.
            for id in self.comp_targets() {
                if let Some(m) = self.scene.node(id).and_then(|n| n.mesh()) {
                    sel.insert(id, m.edges());
                }
            }
        }
        if sel.is_empty() {
            return Err("Bevel: select edges (edge mode) or polygon objects.".into());
        }
        self.checkpoint();
        let mut n = 0;
        for (id, edges) in sel {
            if let Some(m) = self.scene.node_mut(id).and_then(|nd| nd.mesh_mut(true)) {
                n += edges.len();
                m.bevel_edges(&edges, width);
            }
        }
        self.comp_sel.clear();
        Ok(format!("Beveled {n} edge(s)"))
    }

    pub fn bridge_selected(&mut self) -> Result<String, String> {
        if self.comp_mode != CompMode::Face {
            return Err("Bridge: select two faces on the same mesh (face mode). Combine separate objects first.".into());
        }
        let by = self.comp_by_node();
        let Some((id, faces)) = by.into_iter().find(|(_, f)| f.len() == 2) else {
            return Err("Bridge: select exactly two faces on one mesh.".into());
        };
        self.checkpoint();
        let r = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)).map(|m| m.bridge_faces(faces[0] as usize, faces[1] as usize));
        self.comp_sel.clear();
        match r {
            Some(Ok(())) => Ok("Bridged".into()),
            Some(Err(e)) => Err(e),
            None => Err("Bridge: not a mesh".into()),
        }
    }

    /// Merge: selected components to their center, or (object mode) weld by distance.
    pub fn merge_selected(&mut self, dist: Option<f32>) -> Result<String, String> {
        let cv = self.comp_vertices();
        if self.comp_mode != CompMode::Object && !cv.is_empty() && dist.is_none() {
            self.checkpoint();
            for (id, verts) in cv {
                if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                    m.merge_to_center(&verts);
                }
            }
            self.comp_sel.clear();
            return Ok("Merged to center".into());
        }
        let targets = self.comp_targets();
        if targets.is_empty() {
            return Err("Merge: select vertices/edges/faces or a polygon object.".into());
        }
        self.checkpoint();
        let mut n = 0;
        for id in targets {
            if let Some(m) = self.scene.node_mut(id).and_then(|nd| nd.mesh_mut(true)) {
                n += m.merge_by_distance(dist.unwrap_or(0.001));
            }
        }
        self.comp_sel.clear();
        Ok(format!("Merged {n} vertex(es)"))
    }

    pub fn mirror_selected(&mut self, axis: usize, tol: f32) -> Result<String, String> {
        let targets = self.comp_targets();
        if targets.is_empty() {
            return Err("Mirror: select a polygon object.".into());
        }
        self.checkpoint();
        for id in targets {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                m.mirror(axis, tol);
            }
        }
        self.comp_sel.clear();
        Ok(format!("Mirrored across {}", ["X", "Y", "Z"][axis.min(2)]))
    }

    pub fn delete_edges(&mut self) {
        let sel = self.selected_edges();
        if sel.is_empty() {
            return;
        }
        self.checkpoint();
        for (id, edges) in sel {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) {
                for e in edges {
                    m.dissolve_edge(e);
                }
            }
        }
        self.comp_sel.clear();
    }
}

impl App {
    /// Apply the Multi-Cut points collected by the tool (or given directly).
    pub fn finish_multi_cut(&mut self) -> Result<usize, String> {
        let pts = std::mem::take(&mut self.cut_points);
        if pts.len() < 2 {
            return Err("Multi-Cut: click at least two edges (Enter cuts, Esc cancels).".into());
        }
        let id = pts[0].0;
        let cut: Vec<((u32, u32), f32)> = pts.iter().filter(|p| p.0 == id).map(|p| (p.1, p.2)).collect();
        self.checkpoint();
        let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(true)) else { return Err("Multi-Cut: mesh not found".into()) };
        let n = m.multi_cut(&cut);
        if n == 0 {
            return Err("Multi-Cut: the points must lie on edges of the same faces.".into());
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{App, CompMode};
    use crate::commands::execute;

    #[test]
    fn mel_edge_modeling() {
        let mut app = App::new();
        execute(&mut app, "polyCylinder -r 1 -h 4 -sa 8 -n tube").unwrap();
        let id = app.scene.find_by_name("tube").unwrap();
        let faces = |app: &App| app.scene.node(id).unwrap().mesh().unwrap().face_count();
        let f0 = faces(&app);
        execute(&mut app, "polySplitRing -dir 0 1 0 -wt 0.25 tube").unwrap();
        assert_eq!(faces(&app), f0 + 8, "a horizontal loop splits the 8 side quads");
        assert_eq!(app.comp_mode, CompMode::Edge);
        assert_eq!(app.comp_sel.len(), 8, "the new loop is selected");
        execute(&mut app, "polyBevel -o 0.05").unwrap();
        assert!(faces(&app) > f0 + 8);
        execute(&mut app, "undo; undo").unwrap();
        assert_eq!(faces(&app), f0);
        // Mirror a half box, bridge two faces, merge.
        execute(&mut app, "selectMode -o; polyCube -sx 2 -n half").unwrap();
        let h = app.scene.find_by_name("half").unwrap();
        execute(&mut app, "selectFaces -1 0 0 half; delete").unwrap();
        execute(&mut app, "selectMode -o; select half; polyMirrorFace -ax x").unwrap();
        let m = app.scene.node(h).unwrap().mesh().unwrap();
        assert_eq!(m.vertex_count() as i64 - m.edges().len() as i64 + m.face_count() as i64, 2);
        execute(&mut app, "selectFaces 0 1 0 half; polyExtrudeFacet -ltz 0").unwrap();
        // Bridge needs exactly two faces.
        assert!(execute(&mut app, "polyBridge").is_err());
    }
}

#[cfg(test)]
mod multicut_mel_tests {
    use crate::app::App;
    use crate::commands::execute;

    #[test]
    fn mel_poly_split() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyPlane -w 2 -h 2 -sx 1 -sy 1 -n p").unwrap();
        let p = app.scene.find_by_name("p").unwrap();
        let f0 = app.scene.node(p).unwrap().mesh().unwrap().faces.len();
        let edges = app.scene.node(p).unwrap().mesh().unwrap().edges();
        // Find two opposite edges (sharing no vertex).
        let (mut a, mut b) = (0, 0);
        'o: for i in 0..edges.len() {
            for j in i + 1..edges.len() {
                let (e, f) = (edges[i], edges[j]);
                if e.0 != f.0 && e.0 != f.1 && e.1 != f.0 && e.1 != f.1 {
                    a = i;
                    b = j;
                    break 'o;
                }
            }
        }
        let r = execute(&mut app, &format!("polySplit -ep {a} 0.5 -ep {b} 0.5 p")).unwrap();
        assert!(r.starts_with('1'), "{r}");
        assert_eq!(app.scene.node(p).unwrap().mesh().unwrap().faces.len(), f0 + 1);
        execute(&mut app, "undo").unwrap();
        assert_eq!(app.scene.node(p).unwrap().mesh().unwrap().faces.len(), f0);
        execute(&mut app, "setToolTo multiCut").unwrap();
        assert_eq!(app.tool, crate::app::Tool::MultiCut);
    }
}
