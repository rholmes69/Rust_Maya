//! UV mapping: default primitive UVs, planar / cylindrical / spherical / automatic
//! projections, shell detection, layout (packing), normalize and UV transforms.

use crate::mesh::{Mesh, Primitive};
use glam::{Vec2, Vec3};
use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;

/// Angle around +Y as a fraction 0..1 (matching the primitives: x = cos θ, z = −sin θ).
fn turn(p: Vec3) -> f32 {
    let a = (-p.z).atan2(p.x);
    (if a < 0.0 { a + TAU } else { a }) / TAU
}

/// Fix the wrap-around seam of angular U inside one face and give pole corners (on the
/// axis) the average U of the others.
fn fix_seam(us: &mut [f32], on_axis: &[bool]) {
    let valid: Vec<f32> = us.iter().zip(on_axis).filter(|(_, a)| !**a).map(|(u, _)| *u).collect();
    if valid.is_empty() {
        return;
    }
    let (lo, hi) = valid.iter().fold((f32::MAX, f32::MIN), |(a, b), u| (a.min(*u), b.max(*u)));
    if hi - lo > 0.5 {
        for (u, a) in us.iter_mut().zip(on_axis) {
            if !*a && *u < 0.5 {
                *u += 1.0;
            }
        }
    }
    let valid: Vec<f32> = us.iter().zip(on_axis).filter(|(_, a)| !**a).map(|(u, _)| *u).collect();
    let avg = valid.iter().sum::<f32>() / valid.len() as f32;
    for (u, a) in us.iter_mut().zip(on_axis) {
        if *a {
            *u = avg;
        }
    }
}

// ------------------------------------------------------------------ primitive UVs

/// Maya-like default UVs for a freshly built primitive.
pub fn primitive_uvs(prim: &Primitive, m: &mut Mesh) {
    let corners: Vec<Vec<Vec2>> = match *prim {
        Primitive::Plane { width, height, .. } => m
            .faces
            .iter()
            .map(|f| f.iter().map(|v| {
                let p = m.positions[*v as usize];
                Vec2::new(p.x / width + 0.5, -p.z / height + 0.5)
            }).collect())
            .collect(),
        Primitive::Cube { width, height, depth, .. } => {
            let (hx, hy, hz) = (width / 2.0, height / 2.0, depth / 2.0);
            (0..m.faces.len())
                .map(|fi| {
                    let n = m.face_normal(fi);
                    let a = n.abs();
                    m.faces[fi]
                        .iter()
                        .map(|v| {
                            let p = m.positions[*v as usize];
                            // (tile origin, s, t) in Maya's cross layout.
                            let (o, s, t) = if a.z >= a.x && a.z >= a.y {
                                if n.z > 0.0 { (Vec2::new(0.375, 0.0), (p.x + hx) / width, (p.y + hy) / height) } else { (Vec2::new(0.375, 0.5), (p.x + hx) / width, (hy - p.y) / height) }
                            } else if a.y >= a.x {
                                if n.y > 0.0 { (Vec2::new(0.375, 0.25), (p.x + hx) / width, (hz - p.z) / depth) } else { (Vec2::new(0.375, 0.75), (p.x + hx) / width, (p.z + hz) / depth) }
                            } else if n.x > 0.0 {
                                (Vec2::new(0.625, 0.0), (hz - p.z) / depth, (p.y + hy) / height)
                            } else {
                                (Vec2::new(0.125, 0.0), (p.z + hz) / depth, (p.y + hy) / height)
                            };
                            o + Vec2::new(s, t) * 0.25
                        })
                        .collect()
                })
                .collect()
        }
        Primitive::Sphere { radius, .. } => (0..m.faces.len())
            .map(|fi| {
                let pts: Vec<Vec3> = m.faces[fi].iter().map(|v| m.positions[*v as usize]).collect();
                let pole: Vec<bool> = pts.iter().map(|p| Vec2::new(p.x, p.z).length() < radius * 1e-4).collect();
                let mut us: Vec<f32> = pts.iter().map(|p| turn(*p)).collect();
                fix_seam(&mut us, &pole);
                pts.iter().zip(us).map(|(p, u)| Vec2::new(u, (p.y / radius).clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5)).collect()
            })
            .collect(),
        Primitive::Cylinder { radius, height, .. } | Primitive::Cone { radius, height, .. } => {
            let is_cone = matches!(prim, Primitive::Cone { .. });
            (0..m.faces.len())
                .map(|fi| {
                    let n = m.face_normal(fi);
                    let pts: Vec<Vec3> = m.faces[fi].iter().map(|v| m.positions[*v as usize]).collect();
                    if n.y.abs() > 0.99 {
                        // Cap: a disc. Bottom cap left, top cap right.
                        let c = if n.y < 0.0 { Vec2::new(0.25, 0.15625) } else { Vec2::new(0.75, 0.15625) };
                        let flip = if n.y < 0.0 { -1.0 } else { 1.0 };
                        pts.iter().map(|p| c + Vec2::new(p.x * flip, -p.z) / radius.max(1e-6) * 0.15).collect()
                    } else {
                        let axis: Vec<bool> = pts.iter().map(|p| Vec2::new(p.x, p.z).length() < radius * 1e-4).collect();
                        let mut us: Vec<f32> = pts.iter().map(|p| turn(*p)).collect();
                        fix_seam(&mut us, &axis);
                        let (v0, v1) = if is_cone { (0.3125, 1.0) } else { (0.3125, 0.6875) };
                        pts.iter().zip(us).map(|(p, u)| Vec2::new(u, v0 + (p.y / height + 0.5) * (v1 - v0))).collect()
                    }
                })
                .collect()
        }
        Primitive::Torus { radius, .. } => (0..m.faces.len())
            .map(|fi| {
                let pts: Vec<Vec3> = m.faces[fi].iter().map(|v| m.positions[*v as usize]).collect();
                let none = vec![false; pts.len()];
                let mut us: Vec<f32> = pts.iter().map(|p| turn(*p)).collect();
                fix_seam(&mut us, &none);
                // Minor angle around the tube.
                let mut vs: Vec<f32> = pts
                    .iter()
                    .map(|p| {
                        let ring = Vec2::new(p.x, p.z).length() - radius;
                        let a = p.y.atan2(ring);
                        (if a < 0.0 { a + TAU } else { a }) / TAU
                    })
                    .collect();
                fix_seam(&mut vs, &none);
                us.into_iter().zip(vs).map(|(u, v)| Vec2::new(u, v)).collect()
            })
            .collect(),
    };
    m.set_corner_uvs(&corners);
}

// ------------------------------------------------------------------- projections

/// Which faces a projection touches: all, or a subset (component mode).
fn face_list(m: &Mesh, subset: Option<&[usize]>) -> Vec<usize> {
    match subset {
        Some(s) if !s.is_empty() => s.iter().copied().filter(|f| *f < m.faces.len()).collect(),
        _ => (0..m.faces.len()).collect(),
    }
}

/// Write new corner UVs for `faces`, keeping the others (when the mesh already has UVs).
fn apply(m: &mut Mesh, faces: &[usize], new: HashMap<usize, Vec<Vec2>>) {
    // Without existing UVs the untouched faces get (0,0).
    let _ = faces;
    let mut corners = m.corner_uvs();
    for (f, c) in new {
        corners[f] = c;
    }
    m.set_corner_uvs(&corners);
}

/// Planar projection along `axis` (0 = X, 1 = Y, 2 = Z, 3 = best plane), fitted to 0..1.
pub fn planar(m: &mut Mesh, subset: Option<&[usize]>, axis: usize) {
    let faces = face_list(m, subset);
    if faces.is_empty() {
        return;
    }
    let axis = if axis > 2 {
        let n: Vec3 = faces.iter().map(|f| m.face_normal(*f)).sum::<Vec3>().abs();
        if n.x >= n.y && n.x >= n.z { 0 } else if n.y >= n.z { 1 } else { 2 }
    } else {
        axis
    };
    let proj = |p: Vec3| match axis {
        0 => Vec2::new(-p.z, p.y),
        1 => Vec2::new(p.x, -p.z),
        _ => Vec2::new(p.x, p.y),
    };
    let raw: HashMap<usize, Vec<Vec2>> = faces.iter().map(|&f| (f, m.faces[f].iter().map(|v| proj(m.positions[*v as usize])).collect())).collect();
    apply(m, &faces, fit_unit(raw));
}

/// Scale/translate a set of corner UVs uniformly into 0..1.
fn fit_unit(mut raw: HashMap<usize, Vec<Vec2>>) -> HashMap<usize, Vec<Vec2>> {
    let (lo, hi) = raw.values().flatten().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let size = (hi - lo).max_element().max(1e-9);
    for c in raw.values_mut() {
        for p in c.iter_mut() {
            *p = (*p - lo) / size;
        }
    }
    raw
}

/// Cylindrical projection around a vertical axis through the selection's centre.
pub fn cylindrical(m: &mut Mesh, subset: Option<&[usize]>) {
    let faces = face_list(m, subset);
    let Some((c, lo, hi)) = bounds_of(m, &faces) else { return };
    let r = Vec2::new(hi.x - lo.x, hi.z - lo.z).length() * 0.5;
    let h = (hi.y - lo.y).max(1e-6);
    let mut out = HashMap::new();
    for &f in &faces {
        let pts: Vec<Vec3> = m.faces[f].iter().map(|v| m.positions[*v as usize] - c).collect();
        let axis: Vec<bool> = pts.iter().map(|p| Vec2::new(p.x, p.z).length() < r.max(1e-6) * 1e-3).collect();
        let mut us: Vec<f32> = pts.iter().map(|p| turn(*p)).collect();
        fix_seam(&mut us, &axis);
        out.insert(f, pts.iter().zip(us).map(|(p, u)| Vec2::new(u, (p.y + c.y - lo.y) / h)).collect());
    }
    apply(m, &faces, out);
}

/// Spherical (latitude/longitude) projection around the selection's centre.
pub fn spherical(m: &mut Mesh, subset: Option<&[usize]>) {
    let faces = face_list(m, subset);
    let Some((c, _, _)) = bounds_of(m, &faces) else { return };
    let mut out = HashMap::new();
    for &f in &faces {
        let pts: Vec<Vec3> = m.faces[f].iter().map(|v| m.positions[*v as usize] - c).collect();
        let pole: Vec<bool> = pts.iter().map(|p| Vec2::new(p.x, p.z).length() < p.length().max(1e-6) * 1e-3).collect();
        let mut us: Vec<f32> = pts.iter().map(|p| turn(*p)).collect();
        fix_seam(&mut us, &pole);
        out.insert(f, pts.iter().zip(us).map(|(p, u)| Vec2::new(u, (p.y / p.length().max(1e-9)).clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5)).collect());
    }
    apply(m, &faces, out);
}

fn bounds_of(m: &Mesh, faces: &[usize]) -> Option<(Vec3, Vec3, Vec3)> {
    let pts: Vec<Vec3> = faces.iter().flat_map(|f| m.faces[*f].iter().map(|v| m.positions[*v as usize])).collect();
    if pts.is_empty() {
        return None;
    }
    let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    Some(((lo + hi) * 0.5, lo, hi))
}

/// Automatic projection: faces are grouped by their closest of 6 axis directions, each
/// connected group becomes a planar-projected shell, then all shells are packed into 0..1.
pub fn automatic(m: &mut Mesh, subset: Option<&[usize]>) {
    let faces = face_list(m, subset);
    if faces.is_empty() {
        return;
    }
    let dir_of = |n: Vec3| -> usize {
        let a = n.abs();
        if a.x >= a.y && a.x >= a.z { if n.x >= 0.0 { 0 } else { 1 } } else if a.y >= a.z { if n.y >= 0.0 { 2 } else { 3 } } else if n.z >= 0.0 { 4 } else { 5 }
    };
    let proj = |d: usize, p: Vec3| match d {
        0 => Vec2::new(-p.z, p.y),
        1 => Vec2::new(p.z, p.y),
        2 => Vec2::new(p.x, -p.z),
        3 => Vec2::new(p.x, p.z),
        4 => Vec2::new(p.x, p.y),
        _ => Vec2::new(-p.x, p.y),
    };
    let dirs: HashMap<usize, usize> = faces.iter().map(|f| (*f, dir_of(m.face_normal(*f)))).collect();
    // Connected components (shared edge, same direction).
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for &f in &faces {
        let fv = &m.faces[f];
        for i in 0..fv.len() {
            let (a, b) = (fv[i], fv[(i + 1) % fv.len()]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(f);
        }
    }
    let mut seen: HashSet<usize> = HashSet::new();
    let mut islands: Vec<Vec<(usize, Vec<Vec2>)>> = Vec::new();
    for &f0 in &faces {
        if !seen.insert(f0) {
            continue;
        }
        let d = dirs[&f0];
        let mut stack = vec![f0];
        let mut group = Vec::new();
        while let Some(f) = stack.pop() {
            group.push(f);
            let fv = &m.faces[f];
            for i in 0..fv.len() {
                let (a, b) = (fv[i], fv[(i + 1) % fv.len()]);
                for &g in &edge_faces[&(a.min(b), a.max(b))] {
                    if dirs[&g] == d && seen.insert(g) {
                        stack.push(g);
                    }
                }
            }
        }
        islands.push(group.into_iter().map(|f| (f, m.faces[f].iter().map(|v| proj(d, m.positions[*v as usize])).collect())).collect());
    }
    let packed = pack(islands, 0.02);
    apply(m, &faces, packed.into_iter().collect());
}

// ----------------------------------------------------------------- shells & layout

/// UV shells: groups of faces connected through shared UV indices.
pub fn shells(m: &Mesh) -> Vec<Vec<usize>> {
    if !m.has_uvs() {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..m.uvs.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for f in &m.face_uvs {
        for w in f.windows(2) {
            let (a, b) = (find(&mut parent, w[0] as usize), find(&mut parent, w[1] as usize));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut order = Vec::new();
    for (fi, f) in m.face_uvs.iter().enumerate() {
        if let Some(&u) = f.first() {
            let r = find(&mut parent, u as usize);
            if !groups.contains_key(&r) {
                order.push(r);
            }
            groups.entry(r).or_default().push(fi);
        }
    }
    order.into_iter().map(|r| groups.remove(&r).unwrap()).collect()
}

/// Pack islands of corner UVs into 0..1 (shelf packing, uniform scale, keeps proportions).
fn pack(islands: Vec<Vec<(usize, Vec<Vec2>)>>, pad: f32) -> HashMap<usize, Vec<Vec2>> {
    // Normalise each island to start at the origin; record its size.
    let mut items: Vec<(Vec2, Vec<(usize, Vec<Vec2>)>)> = islands
        .into_iter()
        .filter(|i| !i.is_empty())
        .map(|mut isl| {
            let (lo, hi) = isl.iter().flat_map(|(_, c)| c.iter()).fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
            for (_, c) in isl.iter_mut() {
                for p in c.iter_mut() {
                    *p -= lo;
                }
            }
            ((hi - lo).max(Vec2::splat(1e-6)), isl)
        })
        .collect();
    items.sort_by(|a, b| b.0.y.total_cmp(&a.0.y));
    let area: f32 = items.iter().map(|(s, _)| s.x * s.y).sum();
    let mut scale = (0.85 / area.max(1e-12)).sqrt();
    let max_dim = items.iter().map(|(s, _)| s.x.max(s.y)).fold(0.0, f32::max);
    scale = scale.min((1.0 - 2.0 * pad) / max_dim.max(1e-12));
    let place = |scale: f32| -> Option<Vec<Vec2>> {
        let (mut x, mut y, mut row_h) = (pad, pad, 0.0f32);
        let mut out = Vec::new();
        for (s, _) in &items {
            let w = s.x * scale;
            let h = s.y * scale;
            if x + w > 1.0 - pad + 1e-6 && x > pad {
                x = pad;
                y += row_h + pad;
                row_h = 0.0;
            }
            if y + h > 1.0 - pad + 1e-6 {
                return None;
            }
            out.push(Vec2::new(x, y));
            x += w + pad;
            row_h = row_h.max(h);
        }
        Some(out)
    };
    let mut offsets = None;
    for _ in 0..200 {
        if let Some(o) = place(scale) {
            offsets = Some(o);
            break;
        }
        scale *= 0.96;
    }
    let offsets = offsets.unwrap_or_else(|| vec![Vec2::ZERO; items.len()]);
    let mut out = HashMap::new();
    for ((_, isl), off) in items.into_iter().zip(offsets) {
        for (f, c) in isl {
            out.insert(f, c.into_iter().map(|p| off + p * scale).collect());
        }
    }
    out
}

/// Layout UVs: pack all existing shells into 0..1 without overlaps.
pub fn layout(m: &mut Mesh) {
    if !m.has_uvs() {
        automatic(m, None);
        return;
    }
    let corners = m.corner_uvs();
    let islands: Vec<Vec<(usize, Vec<Vec2>)>> = shells(m).into_iter().map(|s| s.into_iter().map(|f| (f, corners[f].clone())).collect()).collect();
    let faces: Vec<usize> = (0..m.faces.len()).collect();
    apply(m, &faces, pack(islands, 0.02));
}

/// Normalize: fit all UVs into 0..1 keeping proportions.
pub fn normalize(m: &mut Mesh) {
    if !m.has_uvs() {
        return;
    }
    let (lo, hi) = m.uvs.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let s = (hi - lo).max_element().max(1e-9);
    for p in &mut m.uvs {
        *p = (*p - lo) / s;
    }
}

/// Apply an affine transform to the given UV indices (all if `ids` is None) around `pivot`.
pub fn transform(m: &mut Mesh, ids: Option<&HashSet<u32>>, translate: Vec2, scale: Vec2, rotate_deg: f32, pivot: Option<Vec2>) {
    if !m.has_uvs() {
        return;
    }
    let pick: Vec<usize> = match ids {
        Some(s) => s.iter().map(|i| *i as usize).filter(|i| *i < m.uvs.len()).collect(),
        None => (0..m.uvs.len()).collect(),
    };
    if pick.is_empty() {
        return;
    }
    let pivot = pivot.unwrap_or_else(|| {
        let (lo, hi) = pick.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), i| (a.min(m.uvs[*i]), b.max(m.uvs[*i])));
        (lo + hi) * 0.5
    });
    let (s, c) = rotate_deg.to_radians().sin_cos();
    for i in pick {
        let d = (m.uvs[i] - pivot) * scale;
        let r = Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c);
        m.uvs[i] = pivot + r + translate;
    }
}

/// UV indices used by the given faces.
pub fn uvs_of_faces(m: &Mesh, faces: &[usize]) -> HashSet<u32> {
    if !m.has_uvs() {
        return HashSet::new();
    }
    faces.iter().filter_map(|f| m.face_uvs.get(*f)).flatten().copied().collect()
}

// ------------------------------------------------------------------ cut / sew / unfold

fn ekey(a: u32, b: u32) -> (u32, u32) {
    if a < b { (a, b) } else { (b, a) }
}

/// Faces using each (undirected) mesh edge, with the corner indices of both endpoints.
fn edge_corners(m: &Mesh) -> HashMap<(u32, u32), Vec<(usize, usize, usize)>> {
    let mut map: HashMap<(u32, u32), Vec<(usize, usize, usize)>> = HashMap::new();
    for (f, face) in m.faces.iter().enumerate() {
        let n = face.len();
        for k in 0..n {
            let (a, b) = (face[k], face[(k + 1) % n]);
            // (face, corner of the smaller vertex, corner of the larger vertex)
            let (ka, kb) = if a < b { (k, (k + 1) % n) } else { ((k + 1) % n, k) };
            map.entry(ekey(a, b)).or_default().push((f, ka, kb));
        }
    }
    map
}

/// Split UVs along the given mesh edges (vertex pairs). Returns the number of UVs added.
pub fn cut_edges(m: &mut Mesh, cuts: &HashSet<(u32, u32)>) -> usize {
    if !m.has_uvs() {
        return 0;
    }
    // Corners are (face, k) flattened.
    let mut offs = Vec::with_capacity(m.faces.len());
    let mut total = 0;
    for f in &m.faces {
        offs.push(total);
        total += f.len();
    }
    let mut parent: Vec<usize> = (0..total).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for (e, list) in edge_corners(m) {
        if cuts.contains(&e) || list.len() != 2 {
            continue;
        }
        let ((f, ka, kb), (g, la, lb)) = (list[0], list[1]);
        for (c1, c2) in [(offs[f] + ka, offs[g] + la), (offs[f] + kb, offs[g] + lb)] {
            let (fu, gu) = (m.face_uvs[f][c1 - offs[f]], m.face_uvs[g][c2 - offs[g]]);
            if fu == gu {
                let (a, b) = (find(&mut parent, c1), find(&mut parent, c2));
                if a != b {
                    parent[a] = b;
                }
            }
        }
    }
    // Corners sharing a UV but in different groups get their own copy.
    let mut owner: HashMap<u32, usize> = HashMap::new();
    let mut group_uv: HashMap<usize, u32> = HashMap::new();
    let mut added = 0;
    for f in 0..m.faces.len() {
        for k in 0..m.faces[f].len() {
            let c = offs[f] + k;
            let root = find(&mut parent, c);
            let u = m.face_uvs[f][k];
            if let Some(&nu) = group_uv.get(&root) {
                m.face_uvs[f][k] = nu;
                continue;
            }
            match owner.get(&u) {
                None => {
                    owner.insert(u, root);
                    group_uv.insert(root, u);
                }
                Some(&r) if r == root => {
                    group_uv.insert(root, u);
                }
                Some(_) => {
                    m.uvs.push(m.uvs[u as usize]);
                    let nu = (m.uvs.len() - 1) as u32;
                    group_uv.insert(root, nu);
                    m.face_uvs[f][k] = nu;
                    added += 1;
                }
            }
        }
    }
    added
}

/// UV shell id per UV index.
fn uv_shell_ids(m: &Mesh) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..m.uvs.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for f in &m.face_uvs {
        for w in f.windows(2) {
            let (a, b) = (find(&mut parent, w[0] as usize), find(&mut parent, w[1] as usize));
            if a != b {
                parent[a] = b;
            }
        }
    }
    (0..m.uvs.len()).map(|i| find(&mut parent, i)).collect()
}

/// Sew UVs along the given mesh edges. A separate shell is moved, rotated and scaled onto
/// the other side first (Maya's Move and Sew). Returns the number of edges sewn.
pub fn sew_edges(m: &mut Mesh, sews: &HashSet<(u32, u32)>) -> usize {
    if !m.has_uvs() {
        return 0;
    }
    let mut sewn = 0;
    let mut edges: Vec<(u32, u32)> = sews.iter().copied().collect();
    edges.sort();
    for e in edges {
        let ec = edge_corners(m);
        let Some(list) = ec.get(&e) else { continue };
        if list.len() != 2 {
            continue;
        }
        let ((f, ka, kb), (g, la, lb)) = (list[0], list[1]);
        let (fa, fb) = (m.face_uvs[f][ka], m.face_uvs[f][kb]);
        let (ga, gb) = (m.face_uvs[g][la], m.face_uvs[g][lb]);
        if fa == ga && fb == gb {
            continue;
        }
        let shell = uv_shell_ids(m);
        if shell[fa as usize] != shell[ga as usize] {
            // Similarity transform mapping g's edge onto f's edge, applied to g's shell.
            let (p0, p1) = (m.uvs[ga as usize], m.uvs[gb as usize]);
            let (q0, q1) = (m.uvs[fa as usize], m.uvs[fb as usize]);
            let (d, t) = (p1 - p0, q1 - q0);
            let den = d.length_squared().max(1e-12);
            // Complex ratio t / d.
            let s = Vec2::new((t.x * d.x + t.y * d.y) / den, (t.y * d.x - t.x * d.y) / den);
            let sid = shell[ga as usize];
            for (i, uv) in m.uvs.iter_mut().enumerate() {
                if shell[i] == sid {
                    let z = *uv - p0;
                    *uv = q0 + Vec2::new(s.x * z.x - s.y * z.y, s.x * z.y + s.y * z.x);
                }
            }
        } else {
            let (ma, mb) = ((m.uvs[fa as usize] + m.uvs[ga as usize]) * 0.5, (m.uvs[fb as usize] + m.uvs[gb as usize]) * 0.5);
            m.uvs[fa as usize] = ma;
            m.uvs[fb as usize] = mb;
        }
        for fu in m.face_uvs.iter_mut() {
            for u in fu.iter_mut() {
                if *u == ga {
                    *u = fa;
                } else if *u == gb {
                    *u = fb;
                }
            }
        }
        sewn += 1;
    }
    m.compact_uvs();
    sewn
}

/// Unfold UV shells with Least Squares Conformal Maps (two pinned UVs, conjugate gradient),
/// then rescale each shell to its 3D area and keep it where it was. `faces` limits the
/// shells to those containing the given faces. Returns the number of shells unfolded.
pub fn unfold(m: &mut Mesh, faces: Option<&[usize]>) -> usize {
    if !m.has_uvs() {
        return 0;
    }
    let want: Option<HashSet<usize>> = faces.map(|f| f.iter().copied().collect());
    // Texel density of the whole mesh (UV area per 3D area), so unfolded shells match it.
    let (mut a2, mut a3) = (0.0f64, 0.0f64);
    for (f, fu) in m.faces.iter().zip(&m.face_uvs) {
        for k in 1..f.len().saturating_sub(1) {
            let (a, b, c) = (m.positions[f[0] as usize], m.positions[f[k] as usize], m.positions[f[k + 1] as usize]);
            a3 += ((b - a).cross(c - a).length() * 0.5) as f64;
            let (ua, ub, uc) = (m.uvs[fu[0] as usize], m.uvs[fu[k] as usize], m.uvs[fu[k + 1] as usize]);
            a2 += ((ub - ua).perp_dot(uc - ua).abs() * 0.5) as f64;
        }
    }
    let density = if a3 > 1e-12 && a2 > 1e-12 { a2 / a3 } else { 1.0 };
    let mut count = 0;
    for shell in shells(m) {
        if let Some(w) = &want
            && !shell.iter().any(|f| w.contains(f))
        {
            continue;
        }
        if unfold_shell(m, &shell, density) {
            count += 1;
        }
    }
    count
}

fn unfold_shell(m: &mut Mesh, shell: &[usize], density: f64) -> bool {
    // Local indices of the shell's UVs and their 3D positions.
    let mut local: HashMap<u32, usize> = HashMap::new();
    let mut ids: Vec<u32> = Vec::new();
    let mut p3: Vec<Vec3> = Vec::new();
    for &f in shell {
        for (k, &u) in m.face_uvs[f].iter().enumerate() {
            local.entry(u).or_insert_with(|| {
                ids.push(u);
                p3.push(m.positions[m.faces[f][k] as usize]);
                ids.len() - 1
            });
        }
    }
    let n = ids.len();
    if n < 3 {
        return false;
    }
    // A closed shell (no UV border) can't be flattened without a seam.
    let mut border: HashMap<(u32, u32), i32> = HashMap::new();
    for &f in shell {
        let fu = &m.face_uvs[f];
        for k in 0..fu.len() {
            *border.entry(ekey(fu[k], fu[(k + 1) % fu.len()])).or_default() += 1;
        }
    }
    if !border.values().any(|c| *c == 1) {
        return false;
    }
    // Triangles (fan) with local indices.
    let mut tris: Vec<[usize; 3]> = Vec::new();
    for &f in shell {
        let fu = &m.face_uvs[f];
        for k in 1..fu.len().saturating_sub(1) {
            tris.push([local[&fu[0]], local[&fu[k]], local[&fu[k + 1]]]);
        }
    }
    // Pins: two far-apart UVs.
    let far = |from: usize| (0..n).max_by(|a, b| p3[*a].distance_squared(p3[from]).total_cmp(&p3[*b].distance_squared(p3[from]))).unwrap();
    let pa = far(0);
    let pb = far(pa);
    if pa == pb {
        return false;
    }
    let dist = p3[pa].distance(p3[pb]) as f64;
    // Rows of the conformal energy: unknowns are u_i (2i) and v_i (2i + 1).
    let mut rows: Vec<Vec<(usize, f64)>> = Vec::with_capacity(tris.len() * 2);
    let mut area3d = 0.0f64;
    for t in &tris {
        let (a, b, c) = (p3[t[0]], p3[t[1]], p3[t[2]]);
        let e1 = b - a;
        let nrm = e1.cross(c - a);
        let ar = nrm.length() as f64 * 0.5;
        if ar < 1e-14 || e1.length() < 1e-12 {
            continue;
        }
        area3d += ar;
        let x = e1.normalize();
        let y = nrm.normalize().cross(x);
        let q = [(0.0f64, 0.0f64), (e1.length() as f64, 0.0), ((c - a).dot(x) as f64, (c - a).dot(y) as f64)];
        let w = [(q[2].0 - q[1].0, q[2].1 - q[1].1), (q[0].0 - q[2].0, q[0].1 - q[2].1), (q[1].0 - q[0].0, q[1].1 - q[0].1)];
        let s = 1.0 / ar.sqrt();
        let mut re = Vec::with_capacity(6);
        let mut im = Vec::with_capacity(6);
        for j in 0..3 {
            let (wr, wi) = w[j];
            re.push((2 * t[j], wr * s));
            re.push((2 * t[j] + 1, -wi * s));
            im.push((2 * t[j], wi * s));
            im.push((2 * t[j] + 1, wr * s));
        }
        rows.push(re);
        rows.push(im);
    }
    if rows.is_empty() {
        return false;
    }
    let pinned = |var: usize| var / 2 == pa || var / 2 == pb;
    let pin_val = |var: usize| -> f64 {
        match (var / 2 == pb, var % 2) {
            (true, 0) => dist,
            _ => 0.0,
        }
    };
    // Normal equations restricted to free variables: (AᵀA) x = -Aᵀ A_p x_p.
    let nv = 2 * n;
    let apply = |x: &[f64], out: &mut [f64]| {
        out.iter_mut().for_each(|o| *o = 0.0);
        for r in &rows {
            let mut dot = 0.0;
            for &(j, a) in r {
                if !pinned(j) {
                    dot += a * x[j];
                }
            }
            for &(j, a) in r {
                if !pinned(j) {
                    out[j] += a * dot;
                }
            }
        }
    };
    let mut bvec = vec![0.0f64; nv];
    for r in &rows {
        let mut dot = 0.0;
        for &(j, a) in r {
            if pinned(j) {
                dot += a * pin_val(j);
            }
        }
        for &(j, a) in r {
            if !pinned(j) {
                bvec[j] -= a * dot;
            }
        }
    }
    // Conjugate gradient, warm-started from the current UVs mapped onto the pin frame.
    let (ua, ub) = (m.uvs[ids[pa] as usize], m.uvs[ids[pb] as usize]);
    let d0 = ub - ua;
    let den = (d0.length_squared() as f64).max(1e-12);
    let mut x = vec![0.0f64; nv];
    for i in 0..n {
        let z = m.uvs[ids[i] as usize] - ua;
        // Rotate/scale so the pins land on (0,0) and (dist,0).
        let (zr, zi) = (z.x as f64, z.y as f64);
        let (dr, di) = (d0.x as f64, d0.y as f64);
        let k = dist / den;
        x[2 * i] = (zr * dr + zi * di) * k;
        x[2 * i + 1] = (zi * dr - zr * di) * k;
    }
    for v in 0..nv {
        if pinned(v) {
            x[v] = pin_val(v);
        }
    }
    let mut ax = vec![0.0f64; nv];
    apply(&x, &mut ax);
    let mut r: Vec<f64> = (0..nv).map(|i| if pinned(i) { 0.0 } else { bvec[i] - ax[i] }).collect();
    let mut p = r.clone();
    let mut rs: f64 = r.iter().map(|v| v * v).sum();
    let rs0 = rs.max(1e-30);
    let mut ap = vec![0.0f64; nv];
    for _ in 0..(4 * nv).clamp(200, 20_000) {
        if rs < 1e-20 * rs0 || rs < 1e-24 {
            break;
        }
        apply(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap.abs() < 1e-300 {
            break;
        }
        let alpha = rs / pap;
        for i in 0..nv {
            if !pinned(i) {
                x[i] += alpha * p[i];
                r[i] -= alpha * ap[i];
            }
        }
        let rs_new: f64 = r.iter().map(|v| v * v).sum();
        let beta = rs_new / rs;
        for i in 0..nv {
            p[i] = if pinned(i) { 0.0 } else { r[i] + beta * p[i] };
        }
        rs = rs_new;
    }
    if x.iter().any(|v| !v.is_finite()) {
        return false;
    }
    let mut uv: Vec<Vec2> = (0..n).map(|i| Vec2::new(x[2 * i] as f32, x[2 * i + 1] as f32)).collect();
    // Keep the original orientation (no mirror) and 3D-proportional size.
    let mut area_uv = 0.0f64;
    for t in &tris {
        let (a, b, c) = (uv[t[0]], uv[t[1]], uv[t[2]]);
        area_uv += ((b - a).perp_dot(c - a) * 0.5) as f64;
    }
    if area_uv < 0.0 {
        for u in uv.iter_mut() {
            u.y = -u.y;
        }
        area_uv = -area_uv;
    }
    // Conformal maps keep areas of the same order; a collapse means the solve failed.
    if area_uv < area3d * 1e-4 {
        return false;
    }
    // Scale to the mesh's texel density and keep the shell where it was.
    let old_lo = ids.iter().map(|i| m.uvs[*i as usize]).fold(Vec2::splat(f32::MAX), Vec2::min);
    let target = area3d * density;
    let k = (target / area_uv.max(1e-30)).sqrt() as f32;
    let lo = uv.iter().fold(Vec2::splat(f32::MAX), |a, b| a.min(*b));
    for (i, u) in uv.iter().enumerate() {
        m.uvs[ids[i] as usize] = old_lo + (*u - lo) * k;
    }
    true
}

/// Mean |log(UV area / 3D area)| deviation from the shell's average ratio — 0 = no stretch.
pub fn stretch(m: &Mesh) -> f32 {
    if !m.has_uvs() {
        return 0.0;
    }
    let mut ratios = Vec::new();
    for (f, fu) in m.faces.iter().zip(&m.face_uvs) {
        for k in 1..f.len().saturating_sub(1) {
            let (a, b, c) = (m.positions[f[0] as usize], m.positions[f[k] as usize], m.positions[f[k + 1] as usize]);
            let a3 = (b - a).cross(c - a).length() * 0.5;
            let (ua, ub, uc) = (m.uvs[fu[0] as usize], m.uvs[fu[k] as usize], m.uvs[fu[k + 1] as usize]);
            let a2 = (ub - ua).perp_dot(uc - ua).abs() * 0.5;
            if a3 > 1e-10 && a2 > 1e-12 {
                ratios.push((a2 / a3).ln());
            }
        }
    }
    if ratios.is_empty() {
        return 0.0;
    }
    let mean = ratios.iter().sum::<f32>() / ratios.len() as f32;
    ratios.iter().map(|r| (r - mean).abs()).sum::<f32>() / ratios.len() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_unit(m: &Mesh) -> bool {
        m.uvs.iter().all(|p| p.x >= -1e-4 && p.y >= -1e-4 && p.x <= 1.0001 && p.y <= 1.0001)
    }

    /// Signed UV area of each face (positive = same winding as the 3D face).
    fn uv_area(m: &Mesh, f: usize) -> f32 {
        let c: Vec<Vec2> = m.face_uvs[f].iter().map(|i| m.uvs[*i as usize]).collect();
        (0..c.len()).map(|i| c[i].perp_dot(c[(i + 1) % c.len()])).sum::<f32>() * 0.5
    }

    #[test]
    fn primitives_have_valid_unflipped_uvs() {
        for p in [Primitive::default_cube(), Primitive::default_sphere(), Primitive::default_cylinder(), Primitive::default_cone(), Primitive::default_plane(), Primitive::default_torus()] {
            let m = p.build();
            assert!(m.has_uvs(), "{}", p.base_name());
            assert!(in_unit(&m), "{} uvs out of range", p.base_name());
            let flipped = (0..m.faces.len()).filter(|f| uv_area(&m, *f) < -1e-7).count();
            assert_eq!(flipped, 0, "{}: {flipped} faces have flipped UVs", p.base_name());
            let zero = (0..m.faces.len()).filter(|f| uv_area(&m, *f).abs() < 1e-9).count();
            assert_eq!(zero, 0, "{}: {zero} faces with zero UV area", p.base_name());
        }
        // Cube: 14 UVs like Maya's cross layout, one shell.
        let c = Primitive::default_cube().build();
        assert_eq!(c.uvs.len(), 14);
        assert_eq!(shells(&c).len(), 1);
    }

    #[test]
    fn projections_and_layout() {
        let mut m = Primitive::default_sphere().build();
        m.drop_uvs();
        planar(&mut m, None, 3);
        assert!(m.has_uvs() && in_unit(&m));
        cylindrical(&mut m, None);
        assert!(m.has_uvs() && in_unit(&m));
        spherical(&mut m, None);
        assert!(m.has_uvs());
        let mut c = Primitive::default_cube().build();
        automatic(&mut c, None);
        assert!(c.has_uvs() && in_unit(&c));
        assert_eq!(shells(&c).len(), 6, "cube auto-projects to 6 shells");
        // Shells don't overlap after layout.
        layout(&mut c);
        let sh = shells(&c);
        let boxes: Vec<(Vec2, Vec2)> = sh
            .iter()
            .map(|s| s.iter().flat_map(|f| c.face_uvs[*f].iter().map(|i| c.uvs[*i as usize])).fold((Vec2::splat(9.0), Vec2::splat(-9.0)), |(a, b), p| (a.min(p), b.max(p))))
            .collect();
        for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                let overlap = a.0.x < b.1.x - 1e-4 && b.0.x < a.1.x - 1e-4 && a.0.y < b.1.y - 1e-4 && b.0.y < a.1.y - 1e-4;
                assert!(!overlap, "shells {i} and {j} overlap");
            }
        }
        // Partial projection keeps the other faces' UVs.
        let mut s = Primitive::default_sphere().build();
        let before = s.corner_uvs();
        planar(&mut s, Some(&[0, 1]), 1);
        let after = s.corner_uvs();
        assert_eq!(before[10], after[10]);
        assert_ne!(before[0], after[0]);
        // Transform
        let mut p = Primitive::default_plane().build();
        transform(&mut p, None, Vec2::new(1.0, 0.0), Vec2::ONE, 0.0, None);
        assert!(p.uvs.iter().all(|u| u.x >= 1.0 - 1e-4));
    }

    #[test]
    fn uvs_survive_mesh_ops() {
        let mut m = Primitive::default_cube().build();
        m.extrude_faces(&[0], 0.5);
        assert!(m.has_uvs());
        let s = m.catmull_clark();
        assert!(s.has_uvs());
        let mut t = m.clone();
        t.triangulate_faces();
        assert!(t.has_uvs());
        t.delete_faces(&[0, 1, 2]);
        assert!(t.has_uvs());
        let mut a = Primitive::default_cube().build();
        let b = Primitive::default_sphere().build();
        a.append(&b);
        assert!(a.has_uvs());
        let shells3d = a.shells();
        assert!(shells3d.iter().all(|s| s.has_uvs()));
        let obj = crate::mesh::to_obj(&[("a".into(), a.clone())]);
        let back = crate::mesh::from_obj(&obj);
        assert!(back[0].1.has_uvs());
        for (x, y) in back[0].1.corner_uvs().iter().flatten().zip(a.corner_uvs().iter().flatten()) {
            assert!((*x - *y).length() < 1e-5);
        }
    }

    #[test]
    fn cut_sew_unfold() {
        // A cylinder with a planar projection from the side is badly stretched on its flanks.
        let mut m = Primitive::Cylinder { radius: 1.0, height: 2.0, axis_div: 16, height_div: 2, caps: true }.build();
        planar(&mut m, None, 2);
        let before = stretch(&m);
        // Cut a vertical seam (edges whose both ends sit at the max-x... use the +X column).
        let seam: HashSet<(u32, u32)> = m
            .edges()
            .into_iter()
            .filter(|(a, b)| {
                let (pa, pb) = (m.positions[*a as usize], m.positions[*b as usize]);
                pa.z.abs() < 1e-4 && pb.z.abs() < 1e-4 && pa.x > 0.9 && pb.x > 0.9
            })
            .map(|(a, b)| ekey(a, b))
            .collect();
        assert!(!seam.is_empty());
        // The caps would keep the side closed: cut around them too.
        let caps: HashSet<(u32, u32)> = m
            .edges()
            .into_iter()
            .filter(|(a, b)| {
                let (pa, pb) = (m.positions[*a as usize], m.positions[*b as usize]);
                (pa.y - pb.y).abs() < 1e-4 && pa.y.abs() > 0.99 && (pa.x * pa.x + pa.z * pa.z) > 0.9 && (pb.x * pb.x + pb.z * pb.z) > 0.9
            })
            .map(|(a, b)| ekey(a, b))
            .collect();
        let all: HashSet<(u32, u32)> = seam.union(&caps).copied().collect();
        let n0 = m.uvs.len();
        let added = cut_edges(&mut m, &all);
        assert!(added > 0 && m.uvs.len() == n0 + added && m.has_uvs());
        let shells_n = shells(&m).len();
        assert!(shells_n >= 3, "side + two caps: {shells_n}");
        assert_eq!(unfold(&mut m, None), shells_n);
        let after = stretch(&m);
        eprintln!("stretch {before:.3} -> {after:.3}");
        assert!(after < before * 0.25, "unfold removes stretch: {before} -> {after}");
        // A closed shell is left alone (needs a seam).
        let mut closed = Primitive::Sphere { radius: 1.0, axis_div: 12, height_div: 8 }.build();
        planar(&mut closed, None, 2);
        let snapshot = closed.uvs.clone();
        assert_eq!(unfold(&mut closed, None), 0);
        assert_eq!(closed.uvs, snapshot);
        // Sew the seam back: the side shell closes up again.
        let sewn = sew_edges(&mut m, &seam);
        assert!(sewn > 0 && m.has_uvs());
        assert_eq!(shells(&m).len(), shells_n, "sewing a shell to itself doesn't merge shells");
        // Sewing caps to the side merges shells.
        sew_edges(&mut m, &caps);
        assert_eq!(shells(&m).len(), 1);
    }
}
