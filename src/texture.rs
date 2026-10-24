//! Image textures for the viewport: loading (PNG / JPEG / BMP / TGA), resizing to the GPU
//! texture-array size, mip-map generation and the built-in UV checker.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Every texture layer on the GPU has this size (images are resampled to it).
pub const TEX_SIZE: u32 = 1024;
/// Name used for the built-in UV checker texture.
pub const CHECKER: &str = "checker";

/// RGBA8 (sRGB) image with a full mip chain, level 0 = TEX_SIZE².
pub struct TexImage {
    pub mips: Vec<Vec<u8>>,
    /// Original size of the source image (for display).
    pub src_size: (u32, u32),
}

impl TexImage {
    fn from_rgba(img: image::RgbaImage, src_size: (u32, u32)) -> TexImage {
        let base = if img.width() == TEX_SIZE && img.height() == TEX_SIZE { img } else { image::imageops::resize(&img, TEX_SIZE, TEX_SIZE, image::imageops::FilterType::Triangle) };
        let mut mips = vec![base.into_raw()];
        let mut size = TEX_SIZE;
        while size > 1 {
            let prev = mips.last().unwrap();
            let half = size / 2;
            let mut next = vec![0u8; (half * half * 4) as usize];
            for y in 0..half {
                for x in 0..half {
                    for c in 0..4 {
                        let px = |xx: u32, yy: u32| prev[((yy * size + xx) * 4 + c) as usize] as u32;
                        let s = px(2 * x, 2 * y) + px(2 * x + 1, 2 * y) + px(2 * x, 2 * y + 1) + px(2 * x + 1, 2 * y + 1);
                        next[((y * half + x) * 4 + c) as usize] = ((s + 2) / 4) as u8;
                    }
                }
            }
            mips.push(next);
            size = half;
        }
        TexImage { mips, src_size }
    }

    /// Level-0 RGBA pixels.
    #[cfg(test)]
    pub fn base(&self) -> &[u8] {
        &self.mips[0]
    }

    /// Load an image file.
    pub fn load(path: &Path) -> Result<TexImage, String> {
        let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let src = (img.width(), img.height());
        // Image rows go top to bottom; UV v=0 is the bottom, so flip once here.
        let rgba = image::imageops::flip_vertical(&img.to_rgba8());
        Ok(TexImage::from_rgba(rgba, src))
    }

    /// Maya-style UV checker: 8×8 dark/light squares tinted by position, with an arrow-like
    /// marker in the lower-left corner square so orientation is visible.
    pub fn checker() -> TexImage {
        let n = TEX_SIZE;
        let cell = n / 8;
        let img = image::RgbaImage::from_fn(n, n, |x, y| {
            // y = 0 is the bottom row (v = 0), see `load`.
            let (cx, cy) = (x / cell, y / cell);
            let light = (cx + cy) % 2 == 0;
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let tint = [0.55 + 0.45 * u, 0.55 + 0.45 * v, 0.95 - 0.4 * u * v];
            let k = if light { 0.95 } else { 0.35 };
            // Grid lines.
            let line = x % cell < 3 || y % cell < 3;
            // Marker: a lighter triangle in the bottom-left cell (points toward +U, +V).
            let (lx, ly) = (x % cell, y % cell);
            let marker = cx == 0 && cy == 0 && lx > 20 && ly > 20 && lx + ly < cell;
            let c = if line { [0.15, 0.15, 0.15] } else if marker { [1.0, 0.85, 0.2] } else { [tint[0] * k, tint[1] * k, tint[2] * k] };
            image::Rgba([(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, 255])
        });
        TexImage::from_rgba(img, (n, n))
    }

    /// PNG bytes of level 0 (flipped back to image orientation), for export.
    pub fn to_png(&self) -> Vec<u8> {
        let img = image::RgbaImage::from_raw(TEX_SIZE, TEX_SIZE, self.mips[0].clone()).unwrap();
        let img = image::imageops::flip_vertical(&img);
        let mut out = std::io::Cursor::new(Vec::new());
        let _ = img.write_to(&mut out, image::ImageFormat::Png);
        out.into_inner()
    }
}

/// Loaded textures by name ("checker" or a file path). Failed loads are remembered.
#[derive(Default)]
pub struct TextureCache {
    map: HashMap<String, Result<Arc<TexImage>, String>>,
}

impl TextureCache {
    pub fn get(&mut self, name: &str) -> Result<Arc<TexImage>, String> {
        self.map
            .entry(name.to_string())
            .or_insert_with(|| if name == CHECKER { Ok(Arc::new(TexImage::checker())) } else { TexImage::load(Path::new(name)).map(Arc::new) })
            .clone()
    }

    /// Forget a texture so it is re-read from disk (Reload).
    pub fn reload(&mut self, name: &str) {
        self.map.remove(name);
    }
}

/// The ordered set of texture layers the renderer should have (layer 0 = checker).
pub struct TextureSet {
    pub key: u64,
    #[allow(dead_code)]
    pub names: Vec<String>,
    pub layers: Vec<Arc<TexImage>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checker_and_roundtrip() {
        let c = TexImage::checker();
        assert_eq!(c.mips.len(), 11);
        assert_eq!(c.mips[10].len(), 4);
        let png = c.to_png();
        let dir = std::env::temp_dir().join("rm_tex_test.png");
        std::fs::write(&dir, &png).unwrap();
        let back = TexImage::load(&dir).unwrap();
        assert_eq!(back.base(), c.base());
        let mut cache = TextureCache::default();
        assert!(cache.get("/no/such/file.png").is_err());
        assert!(cache.get(CHECKER).is_ok());
    }
}
