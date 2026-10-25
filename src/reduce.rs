//! Mesh › Reduce (quadric-error decimation) and Mesh Display › Soften / Harden Edge.
//!
//! Reduce triangulates and collapses edges one end onto the other (half-edge collapses), so
//! the kept vertices are a subset of the original ones: skin weights, blend-shape deltas and
//! UVs carry over unchanged. Costs use Garland–Heckbert quadrics; collapses that would flip
//! faces, break the surface's topology or tear a UV seam are skipped. Border vertices can be
//! kept in place.

use crate::app::{App, CompMode};
use crate::mesh::Mesh;
use crate::scene::NodeId;
use glam::{DMat4, DVec3, DVec4};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(PartialEq)]
struct Cand {
    cost: f64,
    from: u32,
    to: u32,
    stamp: (u32, u32),
}

impl Eq for Cand {}

impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on cost.
        o.cost.total_cmp(&self.cost)
    }
}

fn plane_quadric(a: DVec3, b: DVec3, c: DVec3) -> DMat4 {
    let n = (b - a).cross(c - a);
    let area = n.length() * 0.5;
    let n = n.normalize_or_zero();
    let d = -n.dot(a);
    let p = DVec4::new(n.x, n.y, n.z, d);
    let m = DMat4::from_cols(p * p.x, p * p.y, p * p.z, p * p.w);
    m * area.max(1e-12)
}

fn qerr(q: &DMat4, p: DVec3) -> f64 {
    let v = DVec4::new(p.x, p.y, p.z, 1.0);
    v.dot(*q * v).max(0.0)
}

/// Result of a reduction: the new mesh and, per new vertex, its index in the source mesh.
pub struct Reduced {
    pub mesh: Mesh,
    pub kept: Vec<u32>,
}

/// Reduce to about `keep` (0..1) of the triangles.
pub fn reduce(src: &Mesh, keep: f32, keep_border: bool) -> Reduced {
    let mut m = src.clone();
    m.triangulate_faces();
    let has_uv = m.has_uvs();
    let pos: Vec<DVec3> = m.positions.iter().map(|p| p.as_dvec3()).collect();
    let nv = pos.len();
    let mut tris: Vec<[u32; 3]> = m.faces.iter().filter(|f| f.len() == 3).map(|f| [f[0], f[1], f[2]]).collect();
    let mut tuv: Vec<[u32; 3]> = if has_uv { m.face_uvs.iter().filter(|f| f.len() == 3).map(|f| [f[0], f[1], f[2]]).collect() } else { Vec::new() };
    let mut alive_t = vec![true; tris.len()];
    let orig_n: Vec<DVec3> = tris.iter().map(|t| (pos[t[1] as usize] - pos[t[0] as usize]).cross(pos[t[2] as usize] - pos[t[0] as usize]).normalize_or_zero()).collect();
    let mut alive_v = vec![true; nv];
    let mut vt: Vec<Vec<usize>> = vec![Vec::new(); nv];
    for (i, t) in tris.iter().enumerate() {
        for &v in t {
            vt[v as usize].push(i);
        }
    }
    // Quadrics, borders, seams.
    let mut q = vec![DMat4::ZERO; nv];
    let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &tris {
        let k = plane_quadric(pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        for &v in t {
            q[v as usize] += k;
        }
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            *edge_count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut locked = vec![false; nv];
    for (&(a, b), &c) in &edge_count {
        if c != 2 {
            if keep_border {
                locked[a as usize] = true;
                locked[b as usize] = true;
            } else {
                // Penalise moving off the border with a plane perpendicular to it.
                for (x, y) in [(a, b), (b, a)] {
                    let tri = vt[x as usize].iter().find(|ti| tris[**ti].contains(&y)).copied();
                    if let Some(ti) = tri {
                        let t = tris[ti];
                        let n = (pos[t[1] as usize] - pos[t[0] as usize]).cross(pos[t[2] as usize] - pos[t[0] as usize]);
                        let e = pos[y as usize] - pos[x as usize];
                        let side = e.cross(n).normalize_or_zero();
                        let d = -side.dot(pos[x as usize]);
                        let p = DVec4::new(side.x, side.y, side.z, d);
                        let k = DMat4::from_cols(p * p.x, p * p.y, p * p.z, p * p.w) * (e.length_squared() * 10.0);
                        q[x as usize] += k;
                    }
                }
            }
        }
    }
    if has_uv {
        for v in 0..nv {
            let mut set = HashSet::new();
            for &ti in &vt[v] {
                let k = tris[ti].iter().position(|x| *x as usize == v).unwrap();
                set.insert(tuv[ti][k]);
            }
            if set.len() > 1 {
                locked[v] = true;
            }
        }
    }
    let mut stamp = vec![0u32; nv];
    let mut heap = BinaryHeap::new();
    let push = |heap: &mut BinaryHeap<Cand>, q: &[DMat4], stamp: &[u32], locked: &[bool], a: u32, b: u32| {
        for (f, t) in [(a, b), (b, a)] {
            if !locked[f as usize] {
                let qq = q[f as usize] + q[t as usize];
                heap.push(Cand { cost: qerr(&qq, pos[t as usize]), from: f, to: t, stamp: (stamp[f as usize], stamp[t as usize]) });
            }
        }
    };
    for &(a, b) in edge_count.keys() {
        push(&mut heap, &q, &stamp, &locked, a, b);
    }
    let target = ((tris.len() as f32) * keep.clamp(0.0, 1.0)).round() as usize;
    let mut alive_count = tris.len();
    let neighbors = |vt: &[Vec<usize>], tris: &[[u32; 3]], alive_t: &[bool], v: u32| -> HashSet<u32> {
        let mut s = HashSet::new();
        for &ti in &vt[v as usize] {
            if alive_t[ti] {
                for &x in &tris[ti] {
                    if x != v {
                        s.insert(x);
                    }
                }
            }
        }
        s
    };
    while alive_count > target {
        let Some(c) = heap.pop() else { break };
        let (a, b) = (c.from, c.to);
        if !alive_v[a as usize] || !alive_v[b as usize] || c.stamp != (stamp[a as usize], stamp[b as usize]) {
            continue;
        }
        let shared: Vec<usize> = vt[a as usize].iter().copied().filter(|t| alive_t[*t] && tris[*t].contains(&b)).collect();
        if shared.is_empty() {
            continue;
        }
        // Link condition: common neighbours are exactly the shared triangles' third vertices.
        let na = neighbors(&vt, &tris, &alive_t, a);
        let nb = neighbors(&vt, &tris, &alive_t, b);
        if na.intersection(&nb).count() != shared.len() {
            continue;
        }
        // UVs: a single UV for `a`, and `b`'s UV the same in the shared triangles.
        let mut uv_map: Option<(u32, u32)> = None;
        if has_uv {
            let ua: HashSet<u32> = vt[a as usize].iter().filter(|t| alive_t[**t]).map(|t| tuv[*t][tris[*t].iter().position(|x| *x == a).unwrap()]).collect();
            let ub: HashSet<u32> = shared.iter().map(|t| tuv[*t][tris[*t].iter().position(|x| *x == b).unwrap()]).collect();
            if ua.len() != 1 || ub.len() != 1 {
                continue;
            }
            uv_map = Some((*ua.iter().next().unwrap(), *ub.iter().next().unwrap()));
        }
        // No flipped or degenerate triangles.
        let pb = pos[b as usize];
        let ok = vt[a as usize].iter().filter(|t| alive_t[**t] && !shared.contains(t)).all(|&t| {
            let tr = tris[t];
            let p: Vec<DVec3> = tr.iter().map(|x| pos[*x as usize]).collect();
            let n0 = (p[1] - p[0]).cross(p[2] - p[0]);
            let p2: Vec<DVec3> = tr.iter().map(|x| if *x == a { pb } else { pos[*x as usize] }).collect();
            let n1 = (p2[1] - p2[0]).cross(p2[2] - p2[0]);
            let n1n = n1.normalize_or_zero();
            n1.length() > 1e-14 && n0.normalize_or_zero().dot(n1n) > 0.2 && orig_n[t].dot(n1n) > 0.2
        });
        if !ok {
            continue;
        }
        // Collapse a → b.
        for &t in &shared {
            alive_t[t] = false;
            alive_count -= 1;
        }
        let moved: Vec<usize> = vt[a as usize].iter().copied().filter(|t| alive_t[*t]).collect();
        for &t in &moved {
            let k = tris[t].iter().position(|x| *x == a).unwrap();
            tris[t][k] = b;
            if let Some((ua, ub)) = uv_map
                && tuv[t][k] == ua
            {
                tuv[t][k] = ub;
            }
            vt[b as usize].push(t);
        }
        alive_v[a as usize] = false;
        let qa = q[a as usize];
        q[b as usize] += qa;
        stamp[b as usize] += 1;
        for x in neighbors(&vt, &tris, &alive_t, b) {
            stamp[x as usize] += 1;
        }
        for x in neighbors(&vt, &tris, &alive_t, b) {
            push(&mut heap, &q, &stamp, &locked, b, x);
            // Edges around the neighbour changed cost too (its stamp moved).
            for y in neighbors(&vt, &tris, &alive_t, x) {
                if y != b {
                    push(&mut heap, &q, &stamp, &locked, x, y);
                }
            }
        }
    }
    // Compact.
    let mut remap = vec![u32::MAX; nv];
    let mut kept = Vec::new();
    let mut out = Mesh::default();
    let mut faces = Vec::new();
    let mut fuv = Vec::new();
    for (t, tr) in tris.iter().enumerate() {
        if !alive_t[t] {
            continue;
        }
        let f: Vec<u32> = tr
            .iter()
            .map(|&v| {
                if remap[v as usize] == u32::MAX {
                    remap[v as usize] = kept.len() as u32;
                    kept.push(v);
                    out.positions.push(m.positions[v as usize]);
                }
                remap[v as usize]
            })
            .collect();
        faces.push(f);
        if has_uv {
            fuv.push(tuv[t].to_vec());
        }
    }
    out.faces = faces;
    if has_uv {
        // Keep only the used UVs.
        let mut uremap: HashMap<u32, u32> = HashMap::new();
        for f in &mut fuv {
            for u in f.iter_mut() {
                let n = uremap.len() as u32;
                let k = *uremap.entry(*u).or_insert_with(|| {
                    out.uvs.push(m.uvs[*u as usize]);
                    n
                });
                *u = k;
            }
        }
        out.face_uvs = fuv;
    }
    if let Some(h) = &src.hard_edges {
        out.hard_edges = Some(h.iter().filter_map(|(a, b)| {
            let (x, y) = (*remap.get(*a as usize)?, *remap.get(*b as usize)?);
            (x != u32::MAX && y != u32::MAX).then_some((x.min(y), x.max(y)))
        }).collect());
    }
    Reduced { mesh: out, kept }
}

impl App {
    /// Mesh › Reduce: keep about `percent` % of the triangles.
    pub fn reduce_meshes(&mut self, ids: &[NodeId], percent: f32, keep_border: bool) -> Result<String, String> {
        let targets: Vec<NodeId> = ids.iter().copied().filter(|i| self.scene.node(*i).is_some_and(|n| n.mesh().is_some())).collect();
        if targets.is_empty() {
            return Err("polyReduce: select polygon meshes.".into());
        }
        self.checkpoint();
        let mut msg = Vec::new();
        for id in targets {
            let Some(src) = self.scene.node(id).and_then(|n| n.mesh()).cloned() else { continue };
            let before: usize = src.faces.iter().map(|f| f.len().saturating_sub(2)).sum();
            let r = reduce(&src, percent / 100.0, keep_border);
            let after = r.mesh.faces.len();
            let n = self.scene.node_mut(id).unwrap();
            if let Some(sk) = n.skin.as_mut() {
                sk.weights = r.kept.iter().map(|v| sk.weights.get(*v as usize).cloned().unwrap_or_default()).collect();
            }
            if let Some(bs) = n.blend.as_mut() {
                for t in &mut bs.targets {
                    t.deltas = r.kept.iter().map(|v| t.deltas.get(*v as usize).copied().unwrap_or_default()).collect();
                }
            }
            n.cloth = None;
            n.wires.clear();
            n.wrap = None;
            n.clusters.clear();
            *n.mesh_mut(true).unwrap() = r.mesh;
            msg.push(format!("{}: {before} → {after} triangles", n.name));
        }
        Ok(msg.join(", "))
    }

    /// Soften / harden edges: selected edges in edge mode, else whole meshes. `angle` like
    /// Maya's polySoftEdge: edges whose faces meet at more than `angle` degrees become hard
    /// (0 = harden all, 180 = soften all).
    pub fn soft_edge(&mut self, ids: &[NodeId], angle: f32) -> Result<usize, String> {
        let mut per: HashMap<NodeId, Vec<(u32, u32)>> = HashMap::new();
        if self.comp_mode == CompMode::Edge && !self.comp_sel.is_empty() {
            for &(id, e) in &self.comp_sel {
                if let Some(edge) = self.scene.node(id).and_then(|n| n.mesh()).and_then(|m| m.edges().get(e as usize).copied()) {
                    per.entry(id).or_default().push(edge);
                }
            }
        }
        let whole: Vec<NodeId> = if per.is_empty() { ids.iter().copied().filter(|i| self.scene.node(*i).is_some_and(|n| n.mesh().is_some())).collect() } else { Vec::new() };
        if per.is_empty() && whole.is_empty() {
            return Err("polySoftEdge: select meshes or edges.".into());
        }
        self.checkpoint();
        let mut count = 0;
        for id in whole {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(false)) {
                m.soften_by_angle(angle);
                count += m.edges().len();
            }
        }
        for (id, edges) in per {
            if let Some(m) = self.scene.node_mut(id).and_then(|n| n.mesh_mut(false)) {
                let mut by_angle = m.clone();
                by_angle.soften_by_angle(angle);
                let rule = by_angle.hard_edges.unwrap_or_default();
                let (hard, soft): (Vec<(u32, u32)>, Vec<(u32, u32)>) = edges.iter().map(|(a, b)| ((*a).min(*b), (*a).max(*b))).partition(|e| rule.contains(e));
                m.set_edges_hard(Some(&hard), true);
                m.set_edges_hard(Some(&soft), false);
                count += edges.len();
            }
        }
        Ok(count)
    }
}

pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let mut t: Vec<String> = rest.to_vec();
    let take = |t: &mut Vec<String>, flags: &[&str]| -> Option<String> {
        let i = t.iter().position(|x| flags.iter().any(|f| *x == format!("-{f}")))?;
        let v = t.get(i + 1).cloned()?;
        t.drain(i..i + 2);
        Some(v)
    };
    let r = match cmd.as_str() {
        "polyReduce" => (|| {
            let pct = take(&mut t, &["p", "percentage", "percent"]).and_then(|v| v.parse::<f32>().ok()).unwrap_or(50.0);
            // -p is the percentage to remove (Maya), so keep 100 - p.
            let keep_border = take(&mut t, &["kb", "keepBorder"]).is_none_or(|v| v != "0" && v != "off" && v != "false");
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            app.reduce_meshes(&ids, (100.0 - pct).clamp(0.0, 100.0), keep_border)
        })(),
        "polySoftEdge" => (|| {
            let angle = take(&mut t, &["a", "angle"]).and_then(|v| v.parse::<f32>().ok()).unwrap_or(180.0);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            if objs.iter().any(|o| o.contains(".e[")) {
                crate::commands::execute(app, &format!("select {}", objs.join(" ")))?;
                let sel = app.sel.clone();
                return app.soft_edge(&sel, angle).map(|n| format!("{n} edges"));
            }
            let ids = crate::commands::targets(app, &objs)?;
            if !objs.is_empty() {
                // Explicit objects: whole meshes even in edge mode.
                app.comp_sel.clear();
            }
            app.soft_edge(&ids, angle).map(|n| format!("{n} edges"))
        })(),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    #[test]
    fn reduce_keeps_shape_uvs_and_skin() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -sa 48 -sh 32 -n ball").unwrap();
        let id = app.scene.find_by_name("ball").unwrap();
        let src = app.scene.node(id).unwrap().mesh().unwrap().clone();
        let before: usize = src.faces.iter().map(|f| f.len() - 2).sum();
        let r = reduce(&src, 0.25, true);
        let after = r.mesh.faces.len();
        assert!(after <= before / 4 + 2 && after > before / 6, "{before} → {after}");
        // Still a sphere: every kept vertex on the surface, faces not flipped (outward normals).
        for p in &r.mesh.positions {
            assert!((p.length() - 1.0).abs() < 1e-4);
        }
        let outward = (0..r.mesh.faces.len()).filter(|f| r.mesh.face_normal(*f).dot(r.mesh.face_center(*f)) > 0.0).count();
        assert_eq!(outward, r.mesh.faces.len(), "no flipped faces");
        // Watertight: every edge has two faces.
        let mut ec: HashMap<(u32, u32), u32> = HashMap::new();
        for f in &r.mesh.faces {
            for i in 0..3 {
                let (a, b) = (f[i], f[(i + 1) % 3]);
                *ec.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(ec.values().all(|c| *c == 2), "closed manifold");
        // UVs kept and in range; kept vertices are original vertices.
        assert!(r.mesh.has_uvs() && r.mesh.uvs.iter().all(|u| u.x >= -1e-4 && u.x <= 1.0001 && u.y >= -1e-4 && u.y <= 1.0001));
        for (k, v) in r.kept.iter().enumerate() {
            assert_eq!(r.mesh.positions[k], src.positions[*v as usize]);
        }
        // A skinned plane keeps its border and its weights.
        execute(&mut app, "select -cl; joint -p 0 0 0 -n j0; joint -p 0 2 0 -n j1; polyPlane -w 2 -h 2 -sx 20 -sy 20 -n sheet; skinCluster j0 j1 sheet").unwrap();
        let msg = execute(&mut app, "polyReduce -p 80 sheet").unwrap();
        assert!(msg.contains("sheet: 800 →"), "{msg}");
        let sid = app.scene.find_by_name("sheet").unwrap();
        let n = app.scene.node(sid).unwrap();
        let m = n.mesh().unwrap();
        assert!(m.faces.len() < 200);
        assert_eq!(n.skin.as_ref().unwrap().weights.len(), m.positions.len());
        let corners = m.positions.iter().filter(|p| (p.x.abs() - 1.0).abs() < 1e-5 && (p.z.abs() - 1.0).abs() < 1e-5).count();
        assert_eq!(corners, 4, "border corners kept");
    }

    #[test]
    fn soften_and_harden_edges() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCylinder -r 1 -h 2 -sa 24 -n can").unwrap();
        let id = app.scene.find_by_name("can").unwrap();
        let normals = |app: &App| app.scene.node(id).unwrap().mesh().unwrap().triangulate().nrm;
        let distinct = |ns: &[glam::Vec3]| {
            let mut v: Vec<[i32; 3]> = ns.iter().map(|n| [(n.x * 1000.0) as i32, (n.y * 1000.0) as i32, (n.z * 1000.0) as i32]).collect();
            v.sort();
            v.dedup();
            v.len()
        };
        let auto = distinct(&normals(&app));
        execute(&mut app, "polySoftEdge -a 0 can").unwrap();
        let hard = distinct(&normals(&app));
        assert_eq!(hard, 24 + 2, "fully hard: one normal per face");
        execute(&mut app, "polySoftEdge -a 180 can").unwrap();
        let soft = distinct(&normals(&app));
        assert!(soft > hard && soft > auto, "soft everywhere: {soft} vs auto {auto}");
        execute(&mut app, "polySoftEdge -a 30 can").unwrap();
        assert_eq!(distinct(&normals(&app)), auto, "30° behaves like the automatic rule here");
        // Selected edges only.
        execute(&mut app, "polySoftEdge -a 0 can; polySoftEdge -a 180 can.e[0]").unwrap();
        let m = app.scene.node(id).unwrap().mesh().unwrap().clone();
        assert_eq!(m.hard_edges.as_ref().unwrap().len(), m.edges().len() - 1);
        // Hard edges survive save / load.
        let f = std::env::temp_dir().join("rm_soft.rma");
        app.save_to(f.clone());
        let mut b = App::new();
        b.open_from(f);
        let bid = b.scene.find_by_name("can").unwrap();
        assert_eq!(b.scene.node(bid).unwrap().mesh().unwrap().hard_edges.as_ref().unwrap().len(), m.edges().len() - 1);
    }
}
