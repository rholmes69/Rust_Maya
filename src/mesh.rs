//! Polygon mesh data, procedural primitives and modeling operations.

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// An n-gon polygon mesh. Faces are wound counter-clockwise when seen from outside.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub faces: Vec<Vec<u32>>,
    /// UV coordinates (map1). Shared between face corners through `face_uvs`.
    #[serde(default)]
    pub uvs: Vec<Vec2>,
    /// Per face, the UV index of each corner (parallel to `faces`). Empty = no UVs.
    #[serde(default)]
    pub face_uvs: Vec<Vec<u32>>,
}

/// Triangulated, render-ready version of a mesh (in object space).
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    /// 3 entries per triangle.
    pub pos: Vec<Vec3>,
    pub nrm: Vec<Vec3>,
    /// Source face of each triangle.
    pub tri_face: Vec<u32>,
    /// Source vertex index of each corner (3 per triangle).
    pub vert: Vec<u32>,
    /// Unique edges as vertex index pairs.
    pub edges: Vec<(u32, u32)>,
    /// UV of each corner (empty when the mesh has no UVs).
    pub uv: Vec<Vec2>,
}

/// Map 0..n through `f`, splitting large jobs across threads.
pub fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, f: &F) -> Vec<T> {
    let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).min(8);
    if n < 30_000 || threads < 2 {
        return (0..n).map(f).collect();
    }
    let per = n.div_ceil(threads);
    let parts: Vec<Vec<T>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads).map(|t| s.spawn(move || (t * per..((t + 1) * per).min(n)).map(f).collect::<Vec<T>>())).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut out = Vec::with_capacity(n);
    for p in parts {
        out.extend(p);
    }
    out
}

/// Newell polygon normal.
pub fn newell(face: &[u32], positions: &[Vec3]) -> Vec3 {
    let mut n = Vec3::ZERO;
    for i in 0..face.len() {
        let a = positions[face[i] as usize];
        let b = positions[face[(i + 1) % face.len()] as usize];
        n.x += (a.y - b.y) * (a.z + b.z);
        n.y += (a.z - b.z) * (a.x + b.x);
        n.z += (a.x - b.x) * (a.y + b.y);
    }
    n.normalize_or_zero()
}

impl Mesh {
    /// True when the mesh has a complete, consistent UV set.
    pub fn has_uvs(&self) -> bool {
        !self.uvs.is_empty()
            && self.face_uvs.len() == self.faces.len()
            && self.faces.iter().zip(&self.face_uvs).all(|(f, u)| f.len() == u.len() && u.iter().all(|i| (*i as usize) < self.uvs.len()))
    }

    pub fn drop_uvs(&mut self) {
        self.uvs.clear();
        self.face_uvs.clear();
    }

    /// Set UVs from per-face corner coordinates, sharing UV indices between corners of the
    /// same vertex with the same coordinate (so unsplit edges stay connected in the UV editor).
    pub fn set_corner_uvs(&mut self, corners: &[Vec<Vec2>]) {
        let mut map: HashMap<(u32, i64, i64), u32> = HashMap::new();
        self.uvs.clear();
        self.face_uvs = self
            .faces
            .iter()
            .zip(corners)
            .map(|(f, cu)| {
                f.iter()
                    .zip(cu)
                    .map(|(&v, &uv)| {
                        let key = (v, (uv.x * 1e5).round() as i64, (uv.y * 1e5).round() as i64);
                        *map.entry(key).or_insert_with(|| {
                            self.uvs.push(uv);
                            self.uvs.len() as u32 - 1
                        })
                    })
                    .collect()
            })
            .collect();
    }

    /// Per-face corner UVs (zeros when the mesh has none).
    pub fn corner_uvs(&self) -> Vec<Vec<Vec2>> {
        if self.has_uvs() {
            self.face_uvs.iter().map(|f| f.iter().map(|i| self.uvs[*i as usize]).collect()).collect()
        } else {
            self.faces.iter().map(|f| vec![Vec2::ZERO; f.len()]).collect()
        }
    }

    /// Remove unreferenced UVs.
    pub fn compact_uvs(&mut self) {
        if !self.has_uvs() {
            self.drop_uvs();
            return;
        }
        let mut map = vec![u32::MAX; self.uvs.len()];
        let mut nu = Vec::new();
        for f in &mut self.face_uvs {
            for i in f.iter_mut() {
                if map[*i as usize] == u32::MAX {
                    map[*i as usize] = nu.len() as u32;
                    nu.push(self.uvs[*i as usize]);
                }
                *i = map[*i as usize];
            }
        }
        self.uvs = nu;
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }
    pub fn tri_count(&self) -> usize {
        self.faces.iter().map(|f| f.len().saturating_sub(2)).sum()
    }

    /// Newell's method — robust for non-planar n-gons.
    pub fn face_normal(&self, f: usize) -> Vec3 {
        newell(&self.faces[f], &self.positions)
    }

    /// Faces using each vertex (for fast normal recomputation of deformed meshes).
    pub fn vertex_faces(&self) -> Vec<Vec<u32>> {
        let mut vf: Vec<Vec<u32>> = vec![Vec::new(); self.positions.len()];
        for (fi, f) in self.faces.iter().enumerate() {
            for &v in f {
                vf[v as usize].push(fi as u32);
            }
        }
        vf
    }

    /// Corner positions and normals of the cached triangulation `tri` for deformed vertex
    /// positions `pos` (same topology), using the same 60° smoothing rule as `triangulate`.
    pub fn deformed_corners(&self, tri: &TriMesh, vert_faces: &[Vec<u32>], pos: &[Vec3]) -> (Vec<Vec3>, Vec<Vec3>) {
        let face_n: Vec<Vec3> = par_map(self.faces.len(), &|f| newell(&self.faces[f], pos));
        let cos_thresh = 60f32.to_radians().cos();
        let cn: Vec<Vec3> = par_map(tri.vert.len(), &|i| {
            let v = tri.vert[i];
            let fi = tri.tri_face[i / 3] as usize;
            let n0 = face_n[fi];
            let mut acc = Vec3::ZERO;
            for &of in &vert_faces[v as usize] {
                let n = face_n[of as usize];
                if n.dot(n0) >= cos_thresh {
                    acc += n;
                }
            }
            let r = acc.normalize_or_zero();
            if r == Vec3::ZERO { n0 } else { r }
        });
        let cp: Vec<Vec3> = tri.vert.iter().map(|v| pos[*v as usize]).collect();
        (cp, cn)
    }

    pub fn face_center(&self, f: usize) -> Vec3 {
        let face = &self.faces[f];
        let s: Vec3 = face.iter().map(|&i| self.positions[i as usize]).sum();
        s / face.len().max(1) as f32
    }

    pub fn edges(&self) -> Vec<(u32, u32)> {
        let mut set = HashSet::new();
        let mut out = Vec::new();
        for f in &self.faces {
            for i in 0..f.len() {
                let a = f[i];
                let b = f[(i + 1) % f.len()];
                let key = (a.min(b), a.max(b));
                if set.insert(key) {
                    out.push(key);
                }
            }
        }
        out
    }

    /// Fan-triangulate with angle-based (soft/hard edge, 60°) vertex normals.
    pub fn triangulate(&self) -> TriMesh {
        let face_n: Vec<Vec3> = (0..self.faces.len()).map(|f| self.face_normal(f)).collect();
        let mut vert_faces: Vec<Vec<u32>> = vec![Vec::new(); self.positions.len()];
        for (fi, f) in self.faces.iter().enumerate() {
            for &v in f {
                vert_faces[v as usize].push(fi as u32);
            }
        }
        let cos_thresh = 60f32.to_radians().cos();
        let corner_normal = |fi: usize, v: u32| -> Vec3 {
            let n0 = face_n[fi];
            let mut acc = Vec3::ZERO;
            for &of in &vert_faces[v as usize] {
                let n = face_n[of as usize];
                if n.dot(n0) >= cos_thresh {
                    acc += n;
                }
            }
            let r = acc.normalize_or_zero();
            if r == Vec3::ZERO { n0 } else { r }
        };
        let mut t = TriMesh::default();
        let has_uv = self.has_uvs();
        for (fi, f) in self.faces.iter().enumerate() {
            if f.len() < 3 {
                continue;
            }
            for i in 1..f.len() - 1 {
                for k in [0, i, i + 1] {
                    let v = f[k];
                    t.pos.push(self.positions[v as usize]);
                    t.nrm.push(corner_normal(fi, v));
                    t.vert.push(v);
                    if has_uv {
                        t.uv.push(self.uvs[self.face_uvs[fi][k] as usize]);
                    }
                }
                t.tri_face.push(fi as u32);
            }
        }
        t.edges = self.edges();
        t
    }

    /// Ray / mesh intersection in object space. Returns (distance, face index).
    pub fn raycast(&self, ro: Vec3, rd: Vec3) -> Option<(f32, usize)> {
        let mut best: Option<(f32, usize)> = None;
        for (fi, f) in self.faces.iter().enumerate() {
            if f.len() < 3 {
                continue;
            }
            for i in 1..f.len() - 1 {
                let a = self.positions[f[0] as usize];
                let b = self.positions[f[i] as usize];
                let c = self.positions[f[i + 1] as usize];
                if let Some(t) = ray_triangle(ro, rd, a, b, c)
                    && best.is_none_or(|(bt, _)| t < bt) {
                        best = Some((t, fi));
                    }
            }
        }
        best
    }

    /// Extrude a set of faces as one region ("Keep Faces Together").
    /// The extruded caps keep their original face indices; side faces are appended.
    pub fn extrude_faces(&mut self, sel: &[usize], offset: f32) {
        let sel: HashSet<usize> = sel.iter().copied().filter(|&f| f < self.faces.len()).collect();
        if sel.is_empty() {
            return;
        }
        // Directed edges belonging to selected faces.
        let mut directed: HashSet<(u32, u32)> = HashSet::new();
        for &f in &sel {
            let face = &self.faces[f];
            for i in 0..face.len() {
                directed.insert((face[i], face[(i + 1) % face.len()]));
            }
        }
        // Region vertex normals (average of adjacent selected face normals).
        let mut vnorm: HashMap<u32, Vec3> = HashMap::new();
        for &f in &sel {
            let n = self.face_normal(f);
            for &v in &self.faces[f] {
                *vnorm.entry(v).or_insert(Vec3::ZERO) += n;
            }
        }
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut keys: Vec<u32> = vnorm.keys().copied().collect();
        keys.sort();
        for v in keys {
            let n = vnorm[&v].normalize_or_zero();
            let p = self.positions[v as usize] + n * offset;
            remap.insert(v, self.positions.len() as u32);
            self.positions.push(p);
        }
        let had_uvs = self.has_uvs();
        let mut sides = Vec::new();
        let mut side_uvs = Vec::new();
        let mut sorted_sel: Vec<usize> = sel.iter().copied().collect();
        sorted_sel.sort();
        for &f in &sorted_sel {
            let face = self.faces[f].clone();
            for i in 0..face.len() {
                let a = face[i];
                let b = face[(i + 1) % face.len()];
                if !directed.contains(&(b, a)) {
                    // Boundary edge of the region -> side quad.
                    sides.push(vec![a, b, remap[&b], remap[&a]]);
                    if had_uvs {
                        // Side faces reuse the edge's UVs (zero-width strip, like a fresh extrude in Maya).
                        let fu = &self.face_uvs[f];
                        let (ua, ub) = (fu[i], fu[(i + 1) % face.len()]);
                        side_uvs.push(vec![ua, ub, ub, ua]);
                    }
                }
            }
            self.faces[f] = face.iter().map(|v| remap[v]).collect();
        }
        self.faces.extend(sides);
        if had_uvs {
            self.face_uvs.extend(side_uvs);
        }
    }

    pub fn delete_faces(&mut self, sel: &[usize]) {
        let sel: HashSet<usize> = sel.iter().copied().collect();
        let keep: Vec<bool> = (0..self.faces.len()).map(|i| !sel.contains(&i)).collect();
        self.retain_faces(&keep);
        self.compact();
    }

    pub fn delete_vertices(&mut self, verts: &HashSet<u32>) {
        let keep: Vec<bool> = self.faces.iter().map(|f| !f.iter().any(|v| verts.contains(v))).collect();
        self.retain_faces(&keep);
        self.compact();
    }

    /// Keep faces (and their UVs) where `keep[i]` is true.
    pub fn retain_faces(&mut self, keep: &[bool]) {
        let had = self.has_uvs();
        let mut i = 0;
        self.faces.retain(|_| {
            i += 1;
            keep[i - 1]
        });
        if had {
            let mut i = 0;
            self.face_uvs.retain(|_| {
                i += 1;
                keep[i - 1]
            });
        } else {
            self.drop_uvs();
        }
    }

    /// Remove vertices not referenced by any face.
    pub fn compact(&mut self) {
        let mut used = vec![false; self.positions.len()];
        for f in &self.faces {
            for &v in f {
                used[v as usize] = true;
            }
        }
        let mut map = vec![u32::MAX; self.positions.len()];
        let mut np = Vec::new();
        for (i, p) in self.positions.iter().enumerate() {
            if used[i] {
                map[i] = np.len() as u32;
                np.push(*p);
            }
        }
        self.positions = np;
        for f in &mut self.faces {
            for v in f.iter_mut() {
                *v = map[*v as usize];
            }
        }
        self.compact_uvs();
    }

    pub fn reverse_normals(&mut self) {
        for f in &mut self.faces {
            f.reverse();
        }
        if self.face_uvs.len() == self.faces.len() {
            for f in &mut self.face_uvs {
                f.reverse();
            }
        }
    }

    pub fn triangulate_faces(&mut self) {
        let had = self.has_uvs();
        let mut nf = Vec::new();
        let mut nu = Vec::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 1..f.len().saturating_sub(1) {
                nf.push(vec![f[0], f[i], f[i + 1]]);
                if had {
                    let u = &self.face_uvs[fi];
                    nu.push(vec![u[0], u[i], u[i + 1]]);
                }
            }
        }
        self.faces = nf;
        self.face_uvs = nu;
        if !had {
            self.drop_uvs();
        }
    }

    /// One level of Catmull-Clark subdivision (Mesh > Smooth).
    pub fn catmull_clark(&self) -> Mesh {
        let nv = self.positions.len();
        let face_pts: Vec<Vec3> = (0..self.faces.len()).map(|f| self.face_center(f)).collect();
        let mut edge_index: HashMap<(u32, u32), usize> = HashMap::new();
        let mut edge_faces: Vec<Vec<usize>> = Vec::new();
        let mut edge_verts: Vec<(u32, u32)> = Vec::new();
        for (fi, f) in self.faces.iter().enumerate() {
            for i in 0..f.len() {
                let a = f[i];
                let b = f[(i + 1) % f.len()];
                let key = (a.min(b), a.max(b));
                let e = *edge_index.entry(key).or_insert_with(|| {
                    edge_faces.push(Vec::new());
                    edge_verts.push(key);
                    edge_verts.len() - 1
                });
                edge_faces[e].push(fi);
            }
        }
        let p = &self.positions;
        let edge_pts: Vec<Vec3> = edge_verts
            .iter()
            .enumerate()
            .map(|(e, &(a, b))| {
                let mid = (p[a as usize] + p[b as usize]) * 0.5;
                if edge_faces[e].len() == 2 {
                    (p[a as usize] + p[b as usize] + face_pts[edge_faces[e][0]] + face_pts[edge_faces[e][1]]) * 0.25
                } else {
                    mid
                }
            })
            .collect();
        // Per-vertex adjacency.
        let mut v_faces: Vec<Vec<usize>> = vec![Vec::new(); nv];
        for (fi, f) in self.faces.iter().enumerate() {
            for &v in f {
                v_faces[v as usize].push(fi);
            }
        }
        let mut v_edges: Vec<Vec<usize>> = vec![Vec::new(); nv];
        for (e, &(a, b)) in edge_verts.iter().enumerate() {
            v_edges[a as usize].push(e);
            v_edges[b as usize].push(e);
        }
        let mut new_pos: Vec<Vec3> = Vec::with_capacity(nv + edge_pts.len() + face_pts.len());
        for v in 0..nv {
            let pv = p[v];
            let boundary: Vec<usize> = v_edges[v].iter().copied().filter(|&e| edge_faces[e].len() < 2).collect();
            if v_edges[v].is_empty() {
                new_pos.push(pv);
            } else if !boundary.is_empty() {
                if boundary.len() >= 2 {
                    let other = |e: usize| {
                        let (a, b) = edge_verts[e];
                        if a as usize == v { p[b as usize] } else { p[a as usize] }
                    };
                    new_pos.push(pv * 0.75 + (other(boundary[0]) + other(boundary[1])) * 0.125);
                } else {
                    new_pos.push(pv);
                }
            } else {
                let n = v_faces[v].len() as f32;
                let f_avg: Vec3 = v_faces[v].iter().map(|&f| face_pts[f]).sum::<Vec3>() / n;
                let r_avg: Vec3 = v_edges[v]
                    .iter()
                    .map(|&e| (p[edge_verts[e].0 as usize] + p[edge_verts[e].1 as usize]) * 0.5)
                    .sum::<Vec3>()
                    / v_edges[v].len() as f32;
                new_pos.push((f_avg + r_avg * 2.0 + pv * (n - 3.0)) / n);
            }
        }
        let e_base = new_pos.len() as u32;
        new_pos.extend(edge_pts.iter().copied());
        let f_base = new_pos.len() as u32;
        new_pos.extend(face_pts.iter().copied());
        let mut faces = Vec::new();
        // UVs are subdivided linearly (corner, edge midpoint, face center).
        let had = self.has_uvs();
        let mut uvs = self.uvs.clone();
        let mut face_uvs = Vec::new();
        let mut uv_edge: HashMap<(u32, u32), u32> = HashMap::new();
        for (fi, f) in self.faces.iter().enumerate() {
            let k = f.len();
            let eidx = |a: u32, b: u32| e_base + edge_index[&(a.min(b), a.max(b))] as u32;
            let (fu, center_uv) = if had {
                let fu = &self.face_uvs[fi];
                let c = fu.iter().map(|i| self.uvs[*i as usize]).sum::<Vec2>() / k as f32;
                uvs.push(c);
                (Some(fu), uvs.len() as u32 - 1)
            } else {
                (None, 0)
            };
            let mut mid = |a: u32, b: u32, uvs: &mut Vec<Vec2>| -> u32 {
                *uv_edge.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    uvs.push((uvs[a as usize] + uvs[b as usize]) * 0.5);
                    uvs.len() as u32 - 1
                })
            };
            for i in 0..k {
                let v = f[i];
                let next = f[(i + 1) % k];
                let prev = f[(i + k - 1) % k];
                faces.push(vec![v, eidx(v, next), f_base + fi as u32, eidx(prev, v)]);
                if let Some(fu) = fu {
                    let (u, un, up) = (fu[i], fu[(i + 1) % k], fu[(i + k - 1) % k]);
                    let m1 = mid(u, un, &mut uvs);
                    let m2 = mid(up, u, &mut uvs);
                    face_uvs.push(vec![u, m1, center_uv, m2]);
                }
            }
        }
        if !had {
            uvs.clear();
        }
        Mesh { positions: new_pos, faces, uvs, face_uvs }
    }

    /// Append another mesh (already in the same space).
    pub fn append(&mut self, other: &Mesh) {
        // Keep UVs if either side has them (the side without gets a single (0,0) UV).
        let keep_uvs = self.has_uvs() || other.has_uvs();
        if keep_uvs && !self.has_uvs() {
            self.uvs = vec![Vec2::ZERO];
            self.face_uvs = self.faces.iter().map(|f| vec![0; f.len()]).collect();
        }
        let base = self.positions.len() as u32;
        let ubase = self.uvs.len() as u32;
        self.positions.extend(other.positions.iter().copied());
        for f in &other.faces {
            self.faces.push(f.iter().map(|v| v + base).collect());
        }
        if keep_uvs {
            if other.has_uvs() {
                self.uvs.extend(other.uvs.iter().copied());
                for f in &other.face_uvs {
                    self.face_uvs.push(f.iter().map(|i| i + ubase).collect());
                }
            } else {
                self.uvs.push(Vec2::ZERO);
                for f in &other.faces {
                    self.face_uvs.push(vec![ubase; f.len()]);
                }
            }
        }
    }

    /// Split into connected shells (faces sharing vertices).
    pub fn shells(&self) -> Vec<Mesh> {
        let n = self.positions.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            let mut c = x;
            while p[c] != r {
                let nx = p[c];
                p[c] = r;
                c = nx;
            }
            r
        }
        for f in &self.faces {
            for w in f.windows(2) {
                let a = find(&mut parent, w[0] as usize);
                let b = find(&mut parent, w[1] as usize);
                if a != b {
                    parent[a] = b;
                }
            }
        }
        let mut groups: Vec<(usize, Mesh)> = Vec::new();
        let had = self.has_uvs();
        for (fi, f) in self.faces.iter().enumerate() {
            if f.is_empty() {
                continue;
            }
            let root = find(&mut parent, f[0] as usize);
            let idx = match groups.iter().position(|(r, _)| *r == root) {
                Some(i) => i,
                None => {
                    groups.push((root, Mesh { positions: self.positions.clone(), uvs: self.uvs.clone(), ..Default::default() }));
                    groups.len() - 1
                }
            };
            groups[idx].1.faces.push(f.clone());
            if had {
                groups[idx].1.face_uvs.push(self.face_uvs[fi].clone());
            }
        }
        groups
            .into_iter()
            .map(|(_, mut m)| {
                m.compact();
                m
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        if self.positions.is_empty() {
            return None;
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for p in &self.positions {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        Some((lo, hi))
    }
}

pub fn ray_triangle(ro: Vec3, rd: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = rd.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ro - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = rd.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 1e-6).then_some(t)
}

// ---------------------------------------------------------------------------
// Primitives ("history" nodes in Maya terms: polyCube1, polySphere1 ...)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Primitive {
    Cube { width: f32, height: f32, depth: f32, sx: u32, sy: u32, sz: u32 },
    Sphere { radius: f32, axis_div: u32, height_div: u32 },
    Cylinder { radius: f32, height: f32, axis_div: u32, height_div: u32, caps: bool },
    Cone { radius: f32, height: f32, axis_div: u32 },
    Plane { width: f32, height: f32, sx: u32, sy: u32 },
    Torus { radius: f32, section_radius: f32, axis_div: u32, height_div: u32 },
}

impl Primitive {
    pub fn default_cube() -> Self {
        Primitive::Cube { width: 1.0, height: 1.0, depth: 1.0, sx: 1, sy: 1, sz: 1 }
    }
    pub fn default_sphere() -> Self {
        Primitive::Sphere { radius: 1.0, axis_div: 20, height_div: 20 }
    }
    pub fn default_cylinder() -> Self {
        Primitive::Cylinder { radius: 1.0, height: 2.0, axis_div: 20, height_div: 1, caps: true }
    }
    pub fn default_cone() -> Self {
        Primitive::Cone { radius: 1.0, height: 2.0, axis_div: 20 }
    }
    pub fn default_plane() -> Self {
        Primitive::Plane { width: 1.0, height: 1.0, sx: 10, sy: 10 }
    }
    pub fn default_torus() -> Self {
        Primitive::Torus { radius: 1.0, section_radius: 0.5, axis_div: 20, height_div: 20 }
    }

    /// Maya-style base name used for transforms (pCube) and history (polyCube).
    pub fn base_name(&self) -> &'static str {
        match self {
            Primitive::Cube { .. } => "Cube",
            Primitive::Sphere { .. } => "Sphere",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Cone { .. } => "Cone",
            Primitive::Plane { .. } => "Plane",
            Primitive::Torus { .. } => "Torus",
        }
    }

    pub fn build(&self) -> Mesh {
        let mut m = self.build_geometry();
        crate::uv::primitive_uvs(self, &mut m);
        m
    }

    fn build_geometry(&self) -> Mesh {
        match *self {
            Primitive::Cube { width, height, depth, sx, sy, sz } => cube(width, height, depth, sx.max(1), sy.max(1), sz.max(1)),
            Primitive::Sphere { radius, axis_div, height_div } => sphere(radius, axis_div.max(3), height_div.max(2)),
            Primitive::Cylinder { radius, height, axis_div, height_div, caps } => {
                cylinder(radius, radius, height, axis_div.max(3), height_div.max(1), caps)
            }
            Primitive::Cone { radius, height, axis_div } => cone(radius, height, axis_div.max(3)),
            Primitive::Plane { width, height, sx, sy } => plane(width, height, sx.max(1), sy.max(1)),
            Primitive::Torus { radius, section_radius, axis_div, height_div } => {
                torus(radius, section_radius, axis_div.max(3), height_div.max(3))
            }
        }
    }
}

/// Adds a subdivided grid patch: origin + u*s + v*t, (s,t in [0,1]), CCW when u x v faces outward.
fn grid_patch(m: &mut Mesh, origin: Vec3, u: Vec3, v: Vec3, nu: u32, nv: u32, welded: &mut HashMap<[i64; 3], u32>) {
    let mut idx = vec![0u32; ((nu + 1) * (nv + 1)) as usize];
    for j in 0..=nv {
        for i in 0..=nu {
            let p = origin + u * (i as f32 / nu as f32) + v * (j as f32 / nv as f32);
            let key = [(p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64, (p.z * 1e4).round() as i64];
            let id = *welded.entry(key).or_insert_with(|| {
                m.positions.push(p);
                m.positions.len() as u32 - 1
            });
            idx[(j * (nu + 1) + i) as usize] = id;
        }
    }
    for j in 0..nv {
        for i in 0..nu {
            let a = idx[(j * (nu + 1) + i) as usize];
            let b = idx[(j * (nu + 1) + i + 1) as usize];
            let c = idx[((j + 1) * (nu + 1) + i + 1) as usize];
            let d = idx[((j + 1) * (nu + 1) + i) as usize];
            m.faces.push(vec![a, b, c, d]);
        }
    }
}

pub fn cube(w: f32, h: f32, d: f32, sx: u32, sy: u32, sz: u32) -> Mesh {
    let mut m = Mesh::default();
    let mut weld = HashMap::new();
    let (hx, hy, hz) = (w / 2.0, h / 2.0, d / 2.0);
    let x = Vec3::X * w;
    let y = Vec3::Y * h;
    let z = Vec3::Z * d;
    // front (+Z)
    grid_patch(&mut m, Vec3::new(-hx, -hy, hz), x, y, sx, sy, &mut weld);
    // back (-Z)
    grid_patch(&mut m, Vec3::new(hx, -hy, -hz), -x, y, sx, sy, &mut weld);
    // right (+X)
    grid_patch(&mut m, Vec3::new(hx, -hy, hz), -z, y, sz, sy, &mut weld);
    // left (-X)
    grid_patch(&mut m, Vec3::new(-hx, -hy, -hz), z, y, sz, sy, &mut weld);
    // top (+Y)
    grid_patch(&mut m, Vec3::new(-hx, hy, hz), x, -z, sx, sz, &mut weld);
    // bottom (-Y)
    grid_patch(&mut m, Vec3::new(-hx, -hy, -hz), x, z, sx, sz, &mut weld);
    m
}

pub fn plane(w: f32, h: f32, sx: u32, sy: u32) -> Mesh {
    let mut m = Mesh::default();
    let mut weld = HashMap::new();
    // Lies in XZ, facing +Y.
    grid_patch(&mut m, Vec3::new(-w / 2.0, 0.0, h / 2.0), Vec3::X * w, -Vec3::Z * h, sx, sy, &mut weld);
    m
}

pub fn sphere(r: f32, axis: u32, height: u32) -> Mesh {
    let mut m = Mesh::default();
    m.positions.push(Vec3::new(0.0, -r, 0.0)); // south pole 0
    for j in 1..height {
        let phi = std::f32::consts::PI * j as f32 / height as f32; // from bottom
        let y = -r * phi.cos();
        let rr = r * phi.sin();
        for i in 0..axis {
            let th = std::f32::consts::TAU * i as f32 / axis as f32;
            m.positions.push(Vec3::new(rr * th.cos(), y, -rr * th.sin()));
        }
    }
    let north = m.positions.len() as u32;
    m.positions.push(Vec3::new(0.0, r, 0.0));
    let ring = |j: u32, i: u32| 1 + (j - 1) * axis + (i % axis);
    for i in 0..axis {
        m.faces.push(vec![0, ring(1, i + 1), ring(1, i)]);
    }
    for j in 1..height - 1 {
        for i in 0..axis {
            m.faces.push(vec![ring(j, i), ring(j, i + 1), ring(j + 1, i + 1), ring(j + 1, i)]);
        }
    }
    for i in 0..axis {
        m.faces.push(vec![north, ring(height - 1, i), ring(height - 1, i + 1)]);
    }
    fix_orientation(&mut m, None);
    m
}

pub fn cylinder(r0: f32, r1: f32, h: f32, axis: u32, hdiv: u32, caps: bool) -> Mesh {
    let mut m = Mesh::default();
    for j in 0..=hdiv {
        let t = j as f32 / hdiv as f32;
        let y = -h / 2.0 + h * t;
        let r = r0 + (r1 - r0) * t;
        for i in 0..axis {
            let th = std::f32::consts::TAU * i as f32 / axis as f32;
            m.positions.push(Vec3::new(r * th.cos(), y, -r * th.sin()));
        }
    }
    let idx = |j: u32, i: u32| j * axis + (i % axis);
    for j in 0..hdiv {
        for i in 0..axis {
            m.faces.push(vec![idx(j, i), idx(j, i + 1), idx(j + 1, i + 1), idx(j + 1, i)]);
        }
    }
    if caps {
        m.faces.push((0..axis).rev().map(|i| idx(0, i)).collect());
        m.faces.push((0..axis).map(|i| idx(hdiv, i)).collect());
    }
    fix_orientation(&mut m, None);
    m
}

pub fn cone(r: f32, h: f32, axis: u32) -> Mesh {
    let mut m = Mesh::default();
    for i in 0..axis {
        let th = std::f32::consts::TAU * i as f32 / axis as f32;
        m.positions.push(Vec3::new(r * th.cos(), -h / 2.0, -r * th.sin()));
    }
    let apex = m.positions.len() as u32;
    m.positions.push(Vec3::new(0.0, h / 2.0, 0.0));
    for i in 0..axis {
        m.faces.push(vec![i, (i + 1) % axis, apex]);
    }
    m.faces.push((0..axis).rev().collect());
    fix_orientation(&mut m, None);
    m
}

pub fn torus(r: f32, sr: f32, axis: u32, hdiv: u32) -> Mesh {
    let mut m = Mesh::default();
    for i in 0..axis {
        let th = std::f32::consts::TAU * i as f32 / axis as f32;
        let dir = Vec3::new(th.cos(), 0.0, -th.sin());
        for j in 0..hdiv {
            let ph = std::f32::consts::TAU * j as f32 / hdiv as f32;
            m.positions.push(dir * (r + sr * ph.cos()) + Vec3::Y * sr * ph.sin());
        }
    }
    let idx = |i: u32, j: u32| (i % axis) * hdiv + (j % hdiv);
    for i in 0..axis {
        for j in 0..hdiv {
            m.faces.push(vec![idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1)]);
        }
    }
    fix_orientation(&mut m, Some(r));
    m
}

/// Make sure a centered primitive has outward-facing normals: flips every face whose
/// normal points toward the center (origin, or the nearest point on a torus' ring).
fn fix_orientation(m: &mut Mesh, ring: Option<f32>) {
    let mut flips = Vec::new();
    for f in 0..m.faces.len() {
        let c = m.face_center(f);
        let n = m.face_normal(f);
        let reference = match ring {
            Some(r) => {
                let flat = Vec2::new(c.x, c.z).normalize_or_zero() * r;
                Vec3::new(flat.x, 0.0, flat.y)
            }
            None => Vec3::ZERO,
        };
        if n.dot(c - reference) < 0.0 {
            flips.push(f);
        }
    }
    for f in flips {
        m.faces[f].reverse();
    }
}

// ---------------------------------------------------------------------------
// Wavefront OBJ
// ---------------------------------------------------------------------------

pub fn to_obj(objects: &[(String, Mesh)]) -> String {
    let mut s = String::from("# Exported from Rust Maya\n");
    let mut base = 1u32;
    let mut tbase = 1u32;
    for (name, m) in objects {
        s.push_str(&format!("o {name}\n"));
        for p in &m.positions {
            s.push_str(&format!("v {:.6} {:.6} {:.6}\n", p.x, p.y, p.z));
        }
        let uv = m.has_uvs();
        if uv {
            for t in &m.uvs {
                s.push_str(&format!("vt {:.6} {:.6}\n", t.x, t.y));
            }
        }
        for (fi, f) in m.faces.iter().enumerate() {
            s.push('f');
            for (k, v) in f.iter().enumerate() {
                if uv {
                    s.push_str(&format!(" {}/{}", v + base, m.face_uvs[fi][k] + tbase));
                } else {
                    s.push_str(&format!(" {}", v + base));
                }
            }
            s.push('\n');
        }
        base += m.positions.len() as u32;
        if uv {
            tbase += m.uvs.len() as u32;
        }
    }
    s
}

pub fn from_obj(text: &str) -> Vec<(String, Mesh)> {
    let mut all_pos: Vec<Vec3> = Vec::new();
    let mut all_uv: Vec<Vec2> = Vec::new();
    // (name, faces, face uv indices or None when any face lacks UVs)
    let mut objects: Vec<(String, Vec<Vec<u32>>, Option<Vec<Vec<u32>>>)> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).filter_map(|x| x.parse().ok()).collect();
                if c.len() == 3 {
                    all_pos.push(Vec3::new(c[0], c[1], c[2]));
                }
            }
            Some("vt") => {
                let c: Vec<f32> = it.take(2).filter_map(|x| x.parse().ok()).collect();
                if c.len() == 2 {
                    all_uv.push(Vec2::new(c[0], c[1]));
                }
            }
            Some("o") | Some("g") => {
                let name = it.collect::<Vec<_>>().join("_");
                objects.push((if name.is_empty() { "imported".into() } else { name }, Vec::new(), Some(Vec::new())));
            }
            Some("f") => {
                if objects.is_empty() {
                    objects.push(("imported".into(), Vec::new(), Some(Vec::new())));
                }
                let n = all_pos.len() as i64;
                let nt = all_uv.len() as i64;
                let toks: Vec<&str> = it.collect();
                let face: Vec<u32> = toks
                    .iter()
                    .filter_map(|tok| tok.split('/').next()?.parse::<i64>().ok())
                    .map(|i| if i < 0 { (n + i) as u32 } else { (i - 1) as u32 })
                    .collect();
                let fuv: Vec<u32> = toks
                    .iter()
                    .filter_map(|tok| tok.split('/').nth(1).filter(|s| !s.is_empty())?.parse::<i64>().ok())
                    .map(|i| if i < 0 { (nt + i) as u32 } else { (i - 1) as u32 })
                    .collect();
                if face.len() >= 3 {
                    let o = objects.last_mut().unwrap();
                    o.1.push(face.clone());
                    match (&mut o.2, fuv.len() == face.len()) {
                        (Some(u), true) => u.push(fuv),
                        _ => o.2 = None,
                    }
                }
            }
            _ => {}
        }
    }
    objects
        .into_iter()
        .filter(|(_, f, _)| !f.is_empty())
        .map(|(name, faces, fuv)| {
            let (uvs, face_uvs) = match fuv {
                Some(u) if !all_uv.is_empty() => (all_uv.clone(), u),
                _ => (Vec::new(), Vec::new()),
            };
            let mut m = Mesh { positions: all_pos.clone(), faces, uvs, face_uvs };
            m.compact();
            (name, m)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outward(m: &Mesh) -> bool {
        (0..m.faces.len()).all(|f| m.face_normal(f).dot(m.face_center(f)) > 0.0)
    }

    #[test]
    fn cube_topology() {
        let m = cube(1.0, 1.0, 1.0, 1, 1, 1);
        assert_eq!(m.vertex_count(), 8);
        assert_eq!(m.face_count(), 6);
        assert_eq!(m.edges().len(), 12);
        assert!(outward(&m));
        let m = cube(2.0, 2.0, 2.0, 2, 2, 2);
        assert_eq!(m.face_count(), 24);
        assert_eq!(m.vertex_count(), 26);
    }

    #[test]
    fn sphere_and_cylinder_outward() {
        let s = sphere(1.0, 20, 20);
        assert_eq!(s.vertex_count(), 2 + 19 * 20);
        assert_eq!(s.face_count(), 20 * 20);
        assert!(outward(&s));
        assert!(outward(&cylinder(1.0, 1.0, 2.0, 20, 1, true)));
        assert!(outward(&cone(1.0, 2.0, 20)));
    }

    #[test]
    fn torus_counts() {
        let t = torus(1.0, 0.5, 20, 20);
        assert_eq!(t.vertex_count(), 400);
        assert_eq!(t.face_count(), 400);
        // Euler characteristic of a torus is 0.
        assert_eq!(t.vertex_count() as i64 - t.edges().len() as i64 + t.face_count() as i64, 0);
    }

    #[test]
    fn extrude_top_face() {
        let mut m = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let top = (0..6).find(|&f| m.face_normal(f).y > 0.9).unwrap();
        m.extrude_faces(&[top], 0.5);
        assert_eq!(m.face_count(), 10);
        assert_eq!(m.vertex_count(), 12);
        assert!((m.face_center(top).y - 1.0).abs() < 1e-5);
        // Closed manifold: V - E + F = 2
        assert_eq!(m.vertex_count() as i64 - m.edges().len() as i64 + m.face_count() as i64, 2);
    }

    #[test]
    fn smooth_cube() {
        let m = cube(1.0, 1.0, 1.0, 1, 1, 1).catmull_clark();
        assert_eq!(m.face_count(), 24);
        assert_eq!(m.vertex_count(), 26);
        assert!(outward(&m));
    }

    #[test]
    fn raycast_hits_cube() {
        let m = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let (t, f) = m.raycast(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -1.0)).unwrap();
        assert!((t - 4.5).abs() < 1e-4);
        assert!(m.face_normal(f).z > 0.9);
    }

    #[test]
    fn obj_roundtrip_and_shells() {
        let mut a = cube(1.0, 1.0, 1.0, 1, 1, 1);
        let mut b = cube(1.0, 1.0, 1.0, 1, 1, 1);
        for p in &mut b.positions {
            p.x += 3.0;
        }
        a.append(&b);
        assert_eq!(a.shells().len(), 2);
        let txt = to_obj(&[("x".into(), a.clone())]);
        let back = from_obj(&txt);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].1, a);
    }
}
