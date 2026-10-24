//! Edge-level polygon modeling: edge loops/rings, Insert Edge Loop, Bevel, Bridge, Merge,
//! Mirror and Delete Edge.

use crate::mesh::Mesh;
use glam::Vec3;
use std::collections::{HashMap, HashSet};

pub type Edge = (u32, u32);

pub fn key(a: u32, b: u32) -> Edge {
    (a.min(b), a.max(b))
}

impl Mesh {
    /// Directed edge (a → b) → face index.
    pub fn directed_edges(&self) -> HashMap<(u32, u32), usize> {
        let mut m = HashMap::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 0..f.len() {
                m.insert((f[i], f[(i + 1) % f.len()]), fi);
            }
        }
        m
    }

    /// Faces using an undirected edge.
    pub fn edge_faces(&self, e: Edge) -> Vec<usize> {
        let (a, b) = e;
        self.faces
            .iter()
            .enumerate()
            .filter(|(_, f)| (0..f.len()).any(|i| {
                let (x, y) = (f[i], f[(i + 1) % f.len()]);
                (x == a && y == b) || (x == b && y == a)
            }))
            .map(|(i, _)| i)
            .collect()
    }

    fn vertex_edges(&self) -> HashMap<u32, Vec<Edge>> {
        let mut m: HashMap<u32, Vec<Edge>> = HashMap::new();
        for e in self.edges() {
            m.entry(e.0).or_default().push(e);
            m.entry(e.1).or_default().push(e);
        }
        m
    }

    /// Edge loop through 4-valent vertices (Maya: double-click an edge).
    pub fn edge_loop(&self, start: Edge) -> Vec<Edge> {
        let start = key(start.0, start.1);
        let vedges = self.vertex_edges();
        let mut out = vec![start];
        let mut seen: HashSet<Edge> = [start].into();
        for &(_from, mut at) in &[(start.0, start.1), (start.1, start.0)] {
            let mut cur = start;
            loop {
                let edges = vedges.get(&at).cloned().unwrap_or_default();
                if edges.len() != 4 {
                    break;
                }
                // Exclude the current edge and the edges sharing a face with it at `at`.
                let cur_faces = self.edge_faces(cur);
                if cur_faces.len() != 2 {
                    break;
                }
                let next = edges.into_iter().find(|e| {
                    *e != cur && !cur_faces.iter().any(|&f| {
                        let face = &self.faces[f];
                        face.contains(&e.0) && face.contains(&e.1)
                    })
                });
                let Some(next) = next else { break };
                if !seen.insert(next) {
                    break;
                }
                out.push(next);
                let other = if next.0 == at { next.1 } else { next.0 };
                at = other;
                cur = next;
            }
        }
        out
    }

    /// Edge ring across quads (the edges an Insert Edge Loop would cut).
    pub fn edge_ring(&self, start: Edge) -> Vec<Edge> {
        let start = key(start.0, start.1);
        let mut out = vec![start];
        let mut seen: HashSet<Edge> = [start].into();
        let mut used_faces: HashSet<usize> = HashSet::new();
        let mut stack = vec![start];
        while let Some(e) = stack.pop() {
            for f in self.edge_faces(e) {
                if used_faces.contains(&f) || self.faces[f].len() != 4 {
                    continue;
                }
                used_faces.insert(f);
                let face = &self.faces[f];
                let i = (0..4).find(|&i| key(face[i], face[(i + 1) % 4]) == e).unwrap();
                let opp = key(face[(i + 2) % 4], face[(i + 3) % 4]);
                if seen.insert(opp) {
                    out.push(opp);
                    stack.push(opp);
                }
            }
        }
        out
    }

    /// Insert an edge loop across the ring of `start`, cutting each ring edge at `t`
    /// (measured from `start.0`). Returns the new loop's edges.
    pub fn insert_edge_loop(&mut self, start: Edge, t: f32) -> Vec<Edge> {
        let t = t.clamp(0.01, 0.99);
        // Split points: edge key -> (new vertex, "from" vertex).
        let mut split: HashMap<Edge, u32> = HashMap::new();
        let mut new_point = |m: &mut Mesh, from: u32, to: u32| -> u32 {
            let k = key(from, to);
            if let Some(v) = split.get(&k) {
                return *v;
            }
            let p = m.positions[from as usize].lerp(m.positions[to as usize], t);
            m.positions.push(p);
            let v = m.positions.len() as u32 - 1;
            split.insert(k, v);
            v
        };
        let mut done: HashSet<usize> = HashSet::new();
        let mut queue = vec![(start.0, start.1)];
        let mut new_faces: Vec<(usize, Vec<u32>, Vec<u32>)> = Vec::new();
        let mut loop_edges = Vec::new();
        while let Some((from, to)) = queue.pop() {
            let e = key(from, to);
            for f in self.edge_faces(e) {
                if done.contains(&f) || self.faces[f].len() != 4 {
                    continue;
                }
                done.insert(f);
                let face = self.faces[f].clone();
                let i = (0..4).find(|&i| key(face[i], face[(i + 1) % 4]) == e).unwrap();
                let q = [face[i], face[(i + 1) % 4], face[(i + 2) % 4], face[(i + 3) % 4]];
                let m01 = new_point(self, from, to);
                // The opposite edge's "from" is the vertex joined to `from` by a side edge.
                let (ofrom, oto) = if from == q[0] { (q[3], q[2]) } else { (q[2], q[3]) };
                let m23 = new_point(self, ofrom, oto);
                new_faces.push((f, vec![q[0], m01, m23, q[3]], vec![m01, q[1], q[2], m23]));
                loop_edges.push(key(m01, m23));
                queue.push((ofrom, oto));
            }
        }
        // Apply splits (replace the original face, append the second half).
        for (f, a, b) in new_faces {
            self.faces[f] = a;
            self.faces.push(b);
        }
        // Fix T-junctions on non-quad neighbours at the ends of an open ring.
        for fi in 0..self.faces.len() {
            if done.contains(&fi) {
                continue;
            }
            let f = self.faces[fi].clone();
            let mut nf = Vec::with_capacity(f.len() + 2);
            for i in 0..f.len() {
                let (a, b) = (f[i], f[(i + 1) % f.len()]);
                nf.push(a);
                if let Some(&m) = split.get(&key(a, b)) {
                    nf.push(m);
                }
            }
            self.faces[fi] = nf;
        }
        loop_edges
    }

    /// Chamfer the given edges (Maya Bevel with 1 segment). Each edge shared by two faces
    /// becomes a strip of width ~`width`; small corner triangles close the ends.
    pub fn bevel_edges(&mut self, edges: &[Edge], width: f32) {
        for &e in edges {
            let (a, b) = e;
            let de = self.directed_edges();
            // F1 contains a→b, F2 contains b→a.
            let (f1, f2, a, b) = match (de.get(&(a, b)), de.get(&(b, a))) {
                (Some(&f1), Some(&f2)) => (f1, f2, a, b),
                _ => continue,
            };
            let around = |face: &Vec<u32>, v: u32| -> (u32, u32) {
                let i = face.iter().position(|x| *x == v).unwrap();
                (face[(i + face.len() - 1) % face.len()], face[(i + 1) % face.len()])
            };
            let (p, _) = around(&self.faces[f1], a); // p → a → b
            let (_, n) = around(&self.faces[f1], b); // a → b → n
            let (p2, _) = around(&self.faces[f2], b); // p2 → b → a
            let (_, n2) = around(&self.faces[f2], a); // b → a → n2
            let pa = self.positions[a as usize];
            let pb = self.positions[b as usize];
            let off = |from: Vec3, to: Vec3| -> Vec3 {
                let d = to - from;
                from + d.normalize_or_zero() * width.min(d.length() * 0.45)
            };
            let add = |m: &mut Mesh, p: Vec3| -> u32 {
                m.positions.push(p);
                m.positions.len() as u32 - 1
            };
            let a1 = add(self, off(pa, self.positions[p as usize]));
            let b1 = add(self, off(pb, self.positions[n as usize]));
            let b2 = add(self, off(pb, self.positions[p2 as usize]));
            let a2 = add(self, off(pa, self.positions[n2 as usize]));
            // Side faces: insert the new points on the shared side edges (avoid T-junctions).
            let insert_between = |m: &mut Mesh, x: u32, y: u32, v: u32| {
                // Find the face with directed edge x → y and insert v between them.
                if let Some(fi) = m.faces.iter().position(|f| (0..f.len()).any(|i| f[i] == x && f[(i + 1) % f.len()] == y)) {
                    let f = &mut m.faces[fi];
                    let i = (0..f.len()).find(|&i| f[i] == x && f[(i + 1) % f.len()] == y).unwrap();
                    f.insert(i + 1, v);
                }
            };
            insert_between(self, a, p, a1); // G had a → p
            insert_between(self, n, b, b1); // H had n → b
            insert_between(self, b, p2, b2); // I had b → p2
            insert_between(self, n2, a, a2); // J had n2 → a
            for v in self.faces[f1].iter_mut() {
                if *v == a {
                    *v = a1;
                } else if *v == b {
                    *v = b1;
                }
            }
            for v in self.faces[f2].iter_mut() {
                if *v == b {
                    *v = b2;
                } else if *v == a {
                    *v = a2;
                }
            }
            self.faces.push(vec![b1, a1, a2, b2]);
            self.faces.push(vec![a1, a, a2]);
            self.faces.push(vec![b, b1, b2]);
        }
        self.remove_degenerate();
    }

    /// Bridge two faces with the same number of vertices (removes them and joins the loops).
    pub fn bridge_faces(&mut self, f1: usize, f2: usize) -> Result<(), String> {
        if f1 == f2 || f1 >= self.faces.len() || f2 >= self.faces.len() {
            return Err("Bridge: select two different faces".into());
        }
        let l1 = self.faces[f1].clone();
        let l2r: Vec<u32> = self.faces[f2].iter().rev().copied().collect();
        if l1.len() != l2r.len() {
            return Err(format!("Bridge: the faces need the same vertex count ({} vs {})", l1.len(), l2r.len()));
        }
        let n = l1.len();
        let best = (0..n)
            .min_by(|&x, &y| {
                let cost = |k: usize| (0..n).map(|i| (self.positions[l1[i] as usize] - self.positions[l2r[(i + k) % n] as usize]).length()).sum::<f32>();
                cost(x).total_cmp(&cost(y))
            })
            .unwrap();
        let (hi, lo) = (f1.max(f2), f1.min(f2));
        self.faces.remove(hi);
        self.faces.remove(lo);
        for i in 0..n {
            let j = (i + 1) % n;
            self.faces.push(vec![l1[i], l1[j], l2r[(j + best) % n], l2r[(i + best) % n]]);
        }
        Ok(())
    }

    /// Merge vertices to their centroid.
    pub fn merge_to_center(&mut self, verts: &[u32]) {
        if verts.len() < 2 {
            return;
        }
        let c: Vec3 = verts.iter().map(|v| self.positions[*v as usize]).sum::<Vec3>() / verts.len() as f32;
        let target = verts[0];
        self.positions[target as usize] = c;
        let set: HashSet<u32> = verts.iter().copied().collect();
        for f in &mut self.faces {
            for v in f.iter_mut() {
                if set.contains(v) {
                    *v = target;
                }
            }
        }
        self.remove_degenerate();
    }

    /// Merge vertices closer than `dist` (Maya Merge with a threshold). Returns merged count.
    pub fn merge_by_distance(&mut self, dist: f32) -> usize {
        let n = self.positions.len();
        let mut map: Vec<u32> = (0..n as u32).collect();
        let cell = dist.max(1e-6) * 2.0;
        let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
        let mut merged = 0;
        for i in 0..n {
            let p = self.positions[i];
            let c = ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64);
            let mut found = None;
            'search: for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(list) = grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                            for &j in list {
                                if (self.positions[j as usize] - p).length() <= dist {
                                    found = Some(j);
                                    break 'search;
                                }
                            }
                        }
                    }
                }
            }
            match found {
                Some(j) => {
                    map[i] = j;
                    merged += 1;
                }
                None => grid.entry(c).or_default().push(i as u32),
            }
        }
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = map[*v as usize];
            }
        }
        self.remove_degenerate();
        merged
    }

    /// Drop repeated consecutive vertices and faces with fewer than 3 vertices; compact.
    pub fn remove_degenerate(&mut self) {
        for f in &mut self.faces {
            f.dedup();
            while f.len() > 1 && f.first() == f.last() {
                f.pop();
            }
        }
        self.faces.retain(|f| {
            let u: HashSet<u32> = f.iter().copied().collect();
            u.len() >= 3
        });
        self.compact();
    }

    /// Mirror the mesh across a local axis plane (0 = X, 1 = Y, 2 = Z), welding seam
    /// vertices within `tol` and dropping faces lying on the mirror plane.
    pub fn mirror(&mut self, axis: usize, tol: f32) {
        let on_plane = |p: Vec3| p[axis].abs() <= tol;
        let keep: Vec<bool> = self.faces.iter().map(|f| !f.iter().all(|v| on_plane(self.positions[*v as usize]))).collect();
        self.retain_faces(&keep);
        let n = self.positions.len() as u32;
        let mut map = Vec::with_capacity(n as usize);
        for i in 0..n {
            let mut p = self.positions[i as usize];
            if on_plane(p) {
                // Snap seam vertices exactly onto the plane and reuse them.
                self.positions[i as usize][axis] = 0.0;
                map.push(i);
            } else {
                p[axis] = -p[axis];
                self.positions.push(p);
                map.push(self.positions.len() as u32 - 1);
            }
        }
        let mirrored: Vec<Vec<u32>> = self.faces.iter().map(|f| f.iter().rev().map(|v| map[*v as usize]).collect()).collect();
        if self.has_uvs() {
            // Mirrored faces share the original UVs (overlapping, as in Maya).
            let mu: Vec<Vec<u32>> = self.face_uvs.iter().map(|f| f.iter().rev().copied().collect()).collect();
            self.face_uvs.extend(mu);
        }
        self.faces.extend(mirrored);
        self.compact();
    }

    /// A vertex on edge (a, b) at parameter t: `a`/`b` themselves near the ends, else a new
    /// vertex inserted into every face that uses the edge (UVs are interpolated).
    pub fn split_edge_at(&mut self, a: u32, b: u32, t: f32) -> u32 {
        if t <= 0.03 {
            return a;
        }
        if t >= 0.97 {
            return b;
        }
        let had_uv = self.has_uvs();
        let p = self.positions[a as usize].lerp(self.positions[b as usize], t);
        self.positions.push(p);
        let v = self.positions.len() as u32 - 1;
        for fi in 0..self.faces.len() {
            let n = self.faces[fi].len();
            let mut k = 0;
            while k < self.faces[fi].len() {
                let (x, y) = (self.faces[fi][k], self.faces[fi][(k + 1) % self.faces[fi].len()]);
                if (x == a && y == b) || (x == b && y == a) {
                    let s = if x == a { t } else { 1.0 - t };
                    self.faces[fi].insert(k + 1, v);
                    if had_uv {
                        let fu = &self.face_uvs[fi];
                        let uv = self.uvs[fu[k] as usize].lerp(self.uvs[fu[(k + 1) % fu.len()] as usize], s);
                        self.uvs.push(uv);
                        let ui = self.uvs.len() as u32 - 1;
                        self.face_uvs[fi].insert(k + 1, ui);
                    }
                    break;
                }
                k += 1;
            }
            let _ = n;
        }
        v
    }

    /// Split the face that contains both vertices (not adjacent in it) along u–v.
    pub fn split_face(&mut self, u: u32, v: u32) -> bool {
        let had_uv = self.has_uvs();
        for fi in 0..self.faces.len() {
            let f = &self.faces[fi];
            let (Some(iu), Some(iv)) = (f.iter().position(|x| *x == u), f.iter().position(|x| *x == v)) else { continue };
            let n = f.len();
            if iu == iv || (iu + 1) % n == iv || (iv + 1) % n == iu {
                continue;
            }
            let walk = |from: usize, to: usize, list: &[u32]| -> Vec<u32> {
                let mut out = vec![list[from]];
                let mut k = from;
                while k != to {
                    k = (k + 1) % n;
                    out.push(list[k]);
                }
                out
            };
            let f = f.clone();
            let a = walk(iu, iv, &f);
            let b = walk(iv, iu, &f);
            if had_uv {
                let fu = self.face_uvs[fi].clone();
                self.face_uvs[fi] = walk(iu, iv, &fu);
                self.face_uvs.push(walk(iv, iu, &fu));
            }
            self.faces[fi] = a;
            self.faces.push(b);
            return true;
        }
        false
    }

    /// Multi-Cut: connect a sequence of points on edges (edge, t) with new edges across the
    /// faces between them. Returns the number of faces split.
    pub fn multi_cut(&mut self, pts: &[(Edge, f32)]) -> usize {
        let mut verts: Vec<u32> = Vec::new();
        for &((a, b), t) in pts {
            let v = self.split_edge_at(a, b, t);
            if verts.last() != Some(&v) {
                verts.push(v);
            }
        }
        let mut n = 0;
        for w in verts.windows(2) {
            if self.split_face(w[0], w[1]) {
                n += 1;
            }
        }
        n
    }

    /// Delete (dissolve) an edge shared by two faces, merging them into one.
    pub fn dissolve_edge(&mut self, e: Edge) -> bool {
        let de = self.directed_edges();
        let (a, b) = e;
        let (Some(&f1), Some(&f2)) = (de.get(&(a, b)), de.get(&(b, a))) else { return false };
        if f1 == f2 {
            return false;
        }
        let rot = |f: &Vec<u32>, start: u32| -> Vec<u32> {
            let i = f.iter().position(|x| *x == start).unwrap();
            f[i..].iter().chain(f[..i].iter()).copied().collect()
        };
        let r1 = rot(&self.faces[f1], b); // b … a
        let r2 = rot(&self.faces[f2], a); // a … b
        let mut merged = r1.clone();
        merged.extend(r2[1..r2.len() - 1].iter().copied());
        let (hi, lo) = (f1.max(f2), f1.min(f2));
        self.faces.remove(hi);
        self.faces.remove(lo);
        self.faces.push(merged);
        self.remove_degenerate();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{cube, plane};

    fn euler(m: &Mesh) -> i64 {
        m.vertex_count() as i64 - m.edges().len() as i64 + m.face_count() as i64
    }

    fn consistent(m: &Mesh) -> bool {
        // Every directed edge appears at most once (orientable, consistent winding).
        let mut seen = HashSet::new();
        m.faces.iter().all(|f| (0..f.len()).all(|i| seen.insert((f[i], f[(i + 1) % f.len()]))))
    }

    #[test]
    fn loops_and_rings_on_a_grid() {
        let p = plane(4.0, 4.0, 4, 4); // 5x5 verts
        let e = p.edges()[0];
        let ring = p.edge_ring(e);
        assert_eq!(ring.len(), 5);
        let c = cube(1.0, 1.0, 1.0, 2, 2, 2);
        // A loop around the middle of a subdivided cube has 8 edges.
        let mid = c.edges().into_iter().find(|&(a, b)| {
            let (pa, pb) = (c.positions[a as usize], c.positions[b as usize]);
            pa.y.abs() < 1e-4 && pb.y.abs() < 1e-4
        }).unwrap();
        assert_eq!(c.edge_loop(mid).len(), 8);
    }

    #[test]
    fn insert_loop_on_cube() {
        let mut c = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let e = c.edges()[0];
        let new = c.insert_edge_loop(e, 0.5);
        assert_eq!(new.len(), 4);
        assert_eq!(c.face_count(), 10);
        assert_eq!(c.vertex_count(), 12);
        assert_eq!(euler(&c), 2);
        assert!(consistent(&c));
    }

    #[test]
    fn bevel_cube_edge() {
        let mut c = cube(2.0, 2.0, 2.0, 1, 1, 1);
        let e = c.edges()[0];
        c.bevel_edges(&[e], 0.2);
        assert_eq!(euler(&c), 2, "still a closed surface");
        assert!(consistent(&c));
        assert_eq!(c.face_count(), 9);
    }

    #[test]
    fn bridge_two_cubes() {
        let mut a = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let mut b = cube(1.0, 1.0, 1.0, 1, 1, 1);
        for p in &mut b.positions {
            p.y += 3.0;
        }
        a.append(&b);
        let top = (0..6).find(|&f| a.face_normal(f).y > 0.9).unwrap();
        let bottom = (6..12).find(|&f| a.face_normal(f).y < -0.9).unwrap();
        a.bridge_faces(top, bottom).unwrap();
        assert_eq!(a.face_count(), 14);
        assert_eq!(euler(&a), 2, "one closed genus-0 surface");
        assert!(consistent(&a));
    }

    #[test]
    fn merge_mirror_dissolve() {
        let mut c = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let top: Vec<u32> = (0..8).filter(|&v| c.positions[v as usize].y > 0.0).collect();
        c.merge_to_center(&top);
        assert_eq!(c.vertex_count(), 5, "a pyramid");
        assert_eq!(euler(&c), 2);
        // Half cube (x >= 0) mirrored back into a whole one.
        let mut h = cube(1.0, 1.0, 1.0, 2, 1, 1);
        let left: Vec<usize> = (0..h.face_count()).filter(|&f| h.face_center(f).x < 0.0).collect();
        h.delete_faces(&left);
        let before = h.face_count();
        h.mirror(0, 1e-4);
        assert_eq!(h.face_count(), before * 2);
        assert_eq!(euler(&h), 2);
        assert!(consistent(&h));
        // Dissolving a cube edge leaves 5 faces.
        let mut d = cube(1.0, 1.0, 1.0, 1, 1, 1);
        assert!(d.dissolve_edge(d.edges()[0]));
        assert_eq!(d.face_count(), 5);
        // Merge by distance welds duplicated vertices.
        let mut w = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let extra = w.positions[0];
        w.positions.push(extra);
        let last = w.positions.len() as u32 - 1;
        if let Some(f) = w.faces.iter_mut().find(|f| f.contains(&0)) {
            for v in f.iter_mut() {
                if *v == 0 {
                    *v = last;
                }
            }
        }
        assert_eq!(w.merge_by_distance(1e-4), 1);
        assert_eq!(w.vertex_count(), 8);
    }

    #[test]
    fn multi_cut_splits_faces() {
        let mut m = crate::mesh::Primitive::default_cube().build();
        let f0 = m.face_count();
        let e = m.faces[0].clone();
        // Cut across the first face: midpoints of two opposite edges.
        let n = m.multi_cut(&[((e[0], e[1]), 0.5), ((e[2], e[3]), 0.5)]);
        assert_eq!(n, 1);
        assert_eq!(m.face_count(), f0 + 1);
        assert!(m.has_uvs(), "UVs are kept through the cut");
        assert!(consistent(&m));
        assert_eq!(euler(&m), 2, "still a closed surface");
        // A cut from a vertex to an edge point.
        let f = m.faces[3].clone();
        let n = m.multi_cut(&[((f[0], f[1]), 0.0), ((f[1], f[2]), 0.5), ((f[2], f[3]), 0.5)]);
        assert!(n >= 1);
        assert!(consistent(&m) && euler(&m) == 2);
    }
}
