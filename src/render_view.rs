//! Render View state and the App side of rendering: scene snapshots for the path tracer,
//! render cameras, IPR (re-render on change) and image sequences.

use crate::app::{App, CompMode, Tool};
use crate::raytrace::{RenderJob, RenderSettings, RtCamera, RtScene, ToneMap};
use crate::scene::{NodeId, Shape};
use std::sync::Arc;

/// Which camera the Render View renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderCam {
    /// The active viewport's camera.
    ActiveView,
    Node(NodeId),
}

/// An image sequence being rendered frame by frame (UI, non-blocking).
#[derive(Clone, Debug)]
pub struct SeqState {
    pub start: i32,
    pub end: i32,
    pub frame: i32,
    /// Output path without frame number / extension (`C:/out/shot` → shot.0001.png).
    pub base: String,
    pub restore_time: f32,
}

pub struct RenderViewState {
    pub open: bool,
    pub settings: RenderSettings,
    pub camera: RenderCam,
    pub job: Option<RenderJob>,
    /// Re-render automatically when the scene, camera or settings change.
    pub ipr: bool,
    /// (content key, camera key, settings key) of the current job.
    pub job_key: (u64, u64, u64),
    pub seq: Option<SeqState>,
    pub last_message: String,
    /// Cached path-tracer scene by content key (BVH build is the expensive part).
    scene_cache: Option<(u64, u64, Arc<RtScene>)>,
    /// (geometry generation, content key): selection-only changes don't re-render.
    content_key: (u64, u64),
}

impl Default for RenderViewState {
    fn default() -> Self {
        RenderViewState {
            open: false,
            settings: RenderSettings::default(),
            camera: RenderCam::ActiveView,
            job: None,
            ipr: false,
            job_key: (0, 0, 0),
            seq: None,
            last_message: String::new(),
            scene_cache: None,
            content_key: (u64::MAX, 0),
        }
    }
}

pub fn frame_path(base: &str, frame: i32) -> String {
    let b = base.strip_suffix(".png").unwrap_or(base);
    format!("{b}.{frame:04}.png")
}

impl App {
    /// Viewport geometry without selection / paint-weight tints, for rendering.
    fn render_geometry(&mut self) -> Arc<crate::render::FrameGeometry> {
        let tinted = (self.comp_mode == CompMode::Face && !self.comp_sel.is_empty()) || self.tool == Tool::PaintWeights;
        if !tinted {
            return self.build_geometry();
        }
        let (mode, tool) = (self.comp_mode, self.tool);
        let sel = std::mem::take(&mut self.comp_sel);
        self.comp_mode = CompMode::Object;
        self.tool = Tool::Select;
        let g = self.build_geometry();
        self.comp_mode = mode;
        self.tool = tool;
        self.comp_sel = sel;
        g
    }

    /// The render camera (falls back to the active view if a camera node is gone).
    pub fn render_camera(&mut self, cam: RenderCam) -> RtCamera {
        self.sync_look_through();
        if let RenderCam::Node(id) = cam
            && let Some(Shape::Camera(cs)) = self.scene.node(id).map(|n| n.shape.clone())
        {
            return RtCamera::from_node(self.scene.world_matrix(id), &cs);
        }
        RtCamera::from_view(&self.views[self.active_view].cam)
    }

    /// Hash of what the renderer sees (shaded triangles, lights, textures), recomputed only
    /// when the viewport geometry is rebuilt.
    fn render_content_key(&mut self, g: &crate::render::FrameGeometry) -> u64 {
        if self.render.content_key.0 == g.generation {
            return self.render.content_key.1;
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for c in g.tris.iter().chain(&g.tris_transp) {
            let bytes: &[u8] = bytemuck::cast_slice(&c.data);
            bytes.hash(&mut h);
        }
        let lb: &[u8] = bytemuck::cast_slice(&g.lights);
        lb.hash(&mut h);
        g.textures.as_ref().map(|t| t.key).hash(&mut h);
        let k = h.finish();
        self.render.content_key = (g.generation, k);
        k
    }

    fn render_scene(&mut self, cam: &RtCamera) -> (u64, Arc<RtScene>) {
        let g = self.render_geometry();
        // The default key light follows the camera, so the cache key includes it when used.
        let gen_key = self.render_content_key(&g);
        if let Some((k, ck, s)) = &self.render.scene_cache
            && *k == gen_key
            && (!s.default_light || *ck == cam.key())
        {
            return (gen_key, s.clone());
        }
        let s = Arc::new(RtScene::build(&g, cam));
        self.render.scene_cache = Some((gen_key, cam.key(), s.clone()));
        (gen_key, s)
    }

    /// Start (or restart) a progressive render in the Render View.
    pub fn start_render(&mut self) {
        let cam = self.render_camera(self.render.camera);
        let (gen_key, scene) = self.render_scene(&cam);
        if let Some(j) = &self.render.job {
            j.cancel();
        }
        let settings = self.render.settings.clone();
        // A region render keeps the previous image outside the region.
        let base = settings.region.and_then(|_| self.render.job.as_ref().filter(|j| j.size() == (settings.width, settings.height)).map(|j| j.final_linear()));
        self.render.job_key = (gen_key, cam.key(), settings.key());
        self.render.job = Some(RenderJob::start_with_base(scene, cam, settings, base));
        self.render.open = true;
    }

    pub fn stop_render(&mut self) {
        if let Some(j) = &self.render.job {
            j.cancel();
        }
        if self.render.seq.take().is_some() {
            self.render.last_message = "Sequence stopped.".into();
        }
    }

    /// Render synchronously with a camera node name (None = active view).
    pub fn render_now(&mut self, cam: Option<&str>, settings: RenderSettings) -> Result<RenderJob, String> {
        let rc = match cam {
            Some(n) => {
                let id = self.scene.find_by_name(n).ok_or_else(|| format!("No object matches name: {n}"))?;
                if !self.is_camera(id) {
                    return Err(format!("{n} is not a camera"));
                }
                RenderCam::Node(id)
            }
            None => self.render.camera,
        };
        let c = self.render_camera(rc);
        let (_, scene) = self.render_scene(&c);
        Ok(crate::raytrace::render_blocking(scene, c, settings))
    }

    /// Render frames start..=end to `base.####.png` (blocking).
    pub fn render_sequence_now(&mut self, cam: Option<&str>, settings: RenderSettings, start: i32, end: i32, base: &str) -> Result<usize, String> {
        let t0 = self.scene.time;
        let mut n = 0;
        for f in start..=end {
            self.set_time(f as f32);
            let job = self.render_now(cam, settings.clone())?;
            job.save_png(std::path::Path::new(&frame_path(base, f)), settings.exposure, settings.tonemap)?;
            n += 1;
        }
        self.set_time(t0);
        Ok(n)
    }

    /// Start a non-blocking sequence render in the Render View.
    pub fn start_sequence(&mut self, start: i32, end: i32, base: String) {
        self.render.seq = Some(SeqState { start, end: end.max(start), frame: start, base, restore_time: self.scene.time });
        self.set_time(start as f32);
        self.start_render();
    }

    /// Called every UI frame: IPR restarts and sequence stepping. Returns true while work is
    /// in progress (so the UI keeps repainting).
    pub fn render_tick(&mut self) -> bool {
        // Sequence: save finished frames and advance.
        if let Some(seq) = self.render.seq.clone() {
            let done = self.render.job.as_ref().is_some_and(|j| j.is_done() && !j.was_cancelled());
            if done {
                let path = frame_path(&seq.base, seq.frame);
                let (exp, tm) = (self.render.settings.exposure, self.render.settings.tonemap);
                if let Err(e) = self.render.job.as_ref().unwrap().save_png(std::path::Path::new(&path), exp, tm) {
                    self.render.last_message = e;
                    self.render.seq = None;
                    return false;
                }
                if seq.frame >= seq.end {
                    self.render.last_message = format!("Sequence done: {} frames → {}", seq.end - seq.start + 1, frame_path(&seq.base, seq.start));
                    self.render.seq = None;
                    self.set_time(seq.restore_time);
                    return false;
                }
                let next = seq.frame + 1;
                if let Some(s) = self.render.seq.as_mut() {
                    s.frame = next;
                }
                self.set_time(next as f32);
                self.start_render();
            }
            return true;
        }
        if self.render.ipr && self.render.open {
            let cam = self.render_camera(self.render.camera);
            let g = self.render_geometry();
            let ck = self.render_content_key(&g);
            let key = (ck, cam.key(), self.render.settings.key());
            if key != self.render.job_key || self.render.job.is_none() {
                self.start_render();
            }
        }
        self.render.job.as_ref().is_some_and(|j| !j.is_done())
    }

    pub fn save_render(&mut self, path: &str) -> Result<(), String> {
        let denoise = self.render.settings.denoise;
        let job = self.render.job.as_mut().ok_or("Nothing rendered yet.")?;
        job.settings.denoise = denoise;
        job.save_png(std::path::Path::new(path), self.render.settings.exposure, self.render.settings.tonemap)
    }

    /// Save beauty, albedo, normal, depth and object-ID images next to `path`.
    pub fn save_render_passes(&mut self, path: &str) -> Result<Vec<String>, String> {
        let denoise = self.render.settings.denoise;
        let job = self.render.job.as_mut().ok_or("Nothing rendered yet.")?;
        job.settings.denoise = denoise;
        job.save_passes(std::path::Path::new(path), self.render.settings.exposure, self.render.settings.tonemap)
    }
}

pub fn parse_tonemap(s: &str) -> Option<ToneMap> {
    match s.to_lowercase().as_str() {
        "srgb" | "linear" | "clamp" => Some(ToneMap::Srgb),
        "filmic" | "aces" => Some(ToneMap::Filmic),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;

    #[test]
    fn ipr_restarts_on_edits_not_selection() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polyCube -n a; polySphere -n b; renderSettings -x 32 -y 24 -spp 2").unwrap();
        app.render.open = true;
        app.render.ipr = true;
        app.render_tick();
        let id0 = app.render.job.as_ref().unwrap().id;
        app.render_tick();
        assert_eq!(app.render.job.as_ref().unwrap().id, id0, "no change, no restart");
        execute(&mut app, "select b").unwrap();
        app.render_tick();
        assert_eq!(app.render.job.as_ref().unwrap().id, id0, "selection doesn't restart");
        execute(&mut app, "move 0 3 0 b").unwrap();
        app.render_tick();
        assert_ne!(app.render.job.as_ref().unwrap().id, id0, "edit restarts");
        let id1 = app.render.job.as_ref().unwrap().id;
        execute(&mut app, "renderSettings -spp 3").unwrap();
        app.render_tick();
        assert_ne!(app.render.job.as_ref().unwrap().id, id1, "settings restart");
    }
}
