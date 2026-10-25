//! Minimal OpenVDB writer (file format 224, uncompressed): dense float grids become a
//! standard `Tree_float_5_4_3` (root → 32³ internal → 16³ internal → 8³ leaves) holding only
//! the leaves with non-zero voxels. Readable by OpenVDB 6+ (Houdini, Blender, Maya Bifrost,
//! USD `OpenVDBAsset`).

use glam::DMat4;

/// One dense grid: `values[x + nx * (y + ny * z)]`, voxel (x, y, z) centred at index (x, y, z)
/// mapped to the world by `index_to_world`.
pub struct VdbGrid<'a> {
    pub name: &'a str,
    pub dims: [usize; 3],
    pub values: &'a [f32],
    pub index_to_world: DMat4,
    /// "fog volume" for densities; "unknown" otherwise.
    pub class: &'a str,
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put_str(b: &mut Vec<u8>, s: &str) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s.as_bytes());
}
fn put_mask(b: &mut Vec<u8>, words: &[u64]) {
    for w in words {
        b.extend_from_slice(&w.to_le_bytes());
    }
}
fn set_bit(words: &mut [u64], i: usize) {
    words[i >> 6] |= 1 << (i & 63);
}

/// A leaf: origin (voxel coords), active mask, 512 values (x-major order).
struct Leaf {
    origin: [usize; 3],
    mask: [u64; 8],
    vals: Vec<f32>,
}

fn leaves(g: &VdbGrid) -> Vec<Leaf> {
    let [nx, ny, nz] = g.dims;
    let mut out = Vec::new();
    for lx in 0..nx.div_ceil(8) {
        for ly in 0..ny.div_ceil(8) {
            for lz in 0..nz.div_ceil(8) {
                let mut leaf = Leaf { origin: [lx * 8, ly * 8, lz * 8], mask: [0; 8], vals: vec![0.0; 512] };
                let mut any = false;
                for x in 0..8 {
                    for y in 0..8 {
                        for z in 0..8 {
                            let (gx, gy, gz) = (lx * 8 + x, ly * 8 + y, lz * 8 + z);
                            if gx >= nx || gy >= ny || gz >= nz {
                                continue;
                            }
                            let v = g.values[gx + nx * (gy + ny * gz)];
                            if v.abs() > 1e-7 && v.is_finite() {
                                let o = (x << 6) | (y << 3) | z;
                                leaf.vals[o] = v;
                                set_bit(&mut leaf.mask, o);
                                any = true;
                            }
                        }
                    }
                }
                if any {
                    out.push(leaf);
                }
            }
        }
    }
    out
}

/// Serialise grids into an OpenVDB file image.
pub fn vdb_bytes(grids: &[VdbGrid]) -> Result<Vec<u8>, String> {
    for g in grids {
        let n = g.dims[0] * g.dims[1] * g.dims[2];
        if g.values.len() != n || g.dims.iter().any(|&d| d == 0 || d > 4096) {
            return Err(format!("VDB grid {}: bad dimensions", g.name));
        }
    }
    let mut b = Vec::new();
    b.extend_from_slice(&0x5644_4220i64.to_le_bytes());
    put_u32(&mut b, 224);
    put_u32(&mut b, 10);
    put_u32(&mut b, 0);
    b.push(1); // has grid offsets
    let uuid = {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(b.len() ^ grids.len());
        let a = h.finish();
        h.write_u64(a);
        let c = h.finish();
        let hex = format!("{a:016x}{c:016x}");
        format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
    };
    b.extend_from_slice(uuid.as_bytes());
    put_u32(&mut b, 0); // file metadata
    put_u32(&mut b, grids.len() as u32);
    for g in grids {
        put_str(&mut b, g.name);
        put_str(&mut b, "Tree_float_5_4_3");
        put_str(&mut b, "");
        let offs = b.len();
        b.extend_from_slice(&[0u8; 24]);
        let grid_pos = b.len() as i64;
        put_u32(&mut b, 0); // no compression
        // Grid metadata.
        put_u32(&mut b, 2);
        put_str(&mut b, "class");
        put_str(&mut b, "string");
        put_str(&mut b, g.class);
        put_str(&mut b, "name");
        put_str(&mut b, "string");
        put_str(&mut b, g.name);
        // Transform.
        put_str(&mut b, "AffineMap");
        for v in g.index_to_world.to_cols_array() {
            b.extend_from_slice(&v.to_le_bytes());
        }
        // Topology.
        let lv = leaves(g);
        put_u32(&mut b, 1); // buffer count
        b.extend_from_slice(&0.0f32.to_le_bytes()); // background
        put_u32(&mut b, 0); // tiles
        if lv.is_empty() {
            put_u32(&mut b, 0);
        } else {
            // Root children: 4096³ internal nodes, keyed by origin (lexicographic).
            let mut roots: Vec<[usize; 3]> = lv.iter().map(|l| l.origin.map(|c| c & !4095)).collect();
            roots.sort();
            roots.dedup();
            put_u32(&mut b, roots.len() as u32);
            let mut order: Vec<usize> = Vec::new();
            for r in &roots {
                for c in r {
                    b.extend_from_slice(&(*c as i32).to_le_bytes());
                }
                let in_root: Vec<usize> = (0..lv.len()).filter(|&i| lv[i].origin.map(|c| c & !4095) == *r).collect();
                // Level-2 node: 32³ children of 128³.
                let idx2 = |o: [usize; 3]| (((o[0] >> 7) & 31) << 10) | (((o[1] >> 7) & 31) << 5) | ((o[2] >> 7) & 31);
                let mut kids2: Vec<usize> = in_root.iter().map(|&i| idx2(lv[i].origin)).collect();
                kids2.sort();
                kids2.dedup();
                // 32³ = 32768 entries.
                let mut cm = vec![0u64; 32768 / 64];
                for &k in &kids2 {
                    set_bit(&mut cm, k);
                }
                put_mask(&mut b, &cm);
                put_mask(&mut b, &vec![0u64; 32768 / 64]);
                b.push(6); // NO_MASK_AND_ALL_VALS
                b.extend(std::iter::repeat_n(0u8, 32768 * 4));
                for &k2 in &kids2 {
                    let in2: Vec<usize> = in_root.iter().copied().filter(|&i| idx2(lv[i].origin) == k2).collect();
                    let idx1 = |o: [usize; 3]| (((o[0] >> 3) & 15) << 8) | (((o[1] >> 3) & 15) << 4) | ((o[2] >> 3) & 15);
                    let mut kids1: Vec<(usize, usize)> = in2.iter().map(|&i| (idx1(lv[i].origin), i)).collect();
                    kids1.sort();
                    // 16³ = 4096 entries.
                    let mut cm1 = vec![0u64; 4096 / 64];
                    for &(k, _) in &kids1 {
                        set_bit(&mut cm1, k);
                    }
                    put_mask(&mut b, &cm1);
                    put_mask(&mut b, &vec![0u64; 4096 / 64]);
                    b.push(6);
                    b.extend(std::iter::repeat_n(0u8, 4096 * 4));
                    for &(_, i) in &kids1 {
                        put_mask(&mut b, &lv[i].mask);
                        order.push(i);
                    }
                }
            }
            let block_pos = b.len() as i64;
            for i in order {
                put_mask(&mut b, &lv[i].mask);
                b.push(6);
                for v in &lv[i].vals {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
            let end_pos = b.len() as i64;
            b[offs..offs + 8].copy_from_slice(&grid_pos.to_le_bytes());
            b[offs + 8..offs + 16].copy_from_slice(&block_pos.to_le_bytes());
            b[offs + 16..offs + 24].copy_from_slice(&end_pos.to_le_bytes());
            continue;
        }
        let block_pos = b.len() as i64;
        let end_pos = b.len() as i64;
        b[offs..offs + 8].copy_from_slice(&grid_pos.to_le_bytes());
        b[offs + 8..offs + 16].copy_from_slice(&block_pos.to_le_bytes());
        b[offs + 16..offs + 24].copy_from_slice(&end_pos.to_le_bytes());
    }
    Ok(b)
}
