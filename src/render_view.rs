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

/// `C:/out/shot` → `C:/out/shot.0007.png`; a known image extension on `base` is kept
/// (`shot.exr` → `shot.0007.exr`).
pub fn frame_path(base: &str, frame: i32) -> String {
    for ext in ["png", "exr", "jpg", "jpeg", "tga", "bmp"] {
        let n = base.len();
        if n > ext.len() + 1 && base[n - ext.len() - 1..].eq_ignore_ascii_case(&format!(".{ext}")) {
            return format!("{}.{frame:04}.{ext}", &base[..n - ext.len() - 1]);
        }
    }
    format!("{base}.{frame:04}.png")
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
        for v in &g.volumes {
            v.node.hash(&mut h);
            format!("{:?}{:?}{}{}{}{:?}{}", v.world, v.smoke_color, v.density_scale, v.anisotropy, v.fire_intensity, v.fire_color, v.fire_temp).hash(&mut h);
            let d: &[u8] = bytemuck::cast_slice(&v.density);
            let t: &[u8] = bytemuck::cast_slice(&v.temp);
            d.hash(&mut h);
            t.hash(&mut h);
        }
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
        let mut s = RtScene::build(&g, cam);
        self.tag_objects(&mut s);
        let s = Arc::new(s);
        self.render.scene_cache = Some((gen_key, cam.key(), s.clone()));
        (gen_key, s)
    }

    /// Material groups of the path-tracer objects (for the material-ID pass and mattes).
    fn tag_objects(&self, sc: &mut RtScene) {
        sc.obj_mats = sc.obj_nodes.iter().map(|&id| self.scene.node(id).and_then(|n| n.material).map(|m| m + 1).unwrap_or(0)).collect();
    }

    /// Resolve a matte list into (name, object ids): `all` = one matte per visible object,
    /// `materials` = one per material; otherwise comma-separated object, group or material
    /// names (a group's matte covers everything under it).
    pub fn matte_sets(&self, job: &RenderJob, spec: &str) -> Result<Vec<(String, std::collections::HashSet<u32>)>, String> {
        use std::collections::HashSet;
        let objs: Vec<(u32, crate::scene::NodeId)> = job.obj_nodes.iter().enumerate().map(|(i, &n)| (i as u32 + 1, n)).collect();
        let under = |root: crate::scene::NodeId, mut n: crate::scene::NodeId| loop {
            if n == root {
                return true;
            }
            match self.scene.node(n).and_then(|x| x.parent) {
                Some(p) => n = p,
                None => return false,
            }
        };
        let name_of = |id: crate::scene::NodeId| self.scene.node(id).map(|n| n.name.clone()).unwrap_or_else(|| format!("obj{id}"));
        let mut out: Vec<(String, HashSet<u32>)> = Vec::new();
        for item in spec.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            match item {
                "all" | "objects" => {
                    let mut all: Vec<(String, HashSet<u32>)> = objs.iter().map(|&(o, n)| (name_of(n), HashSet::from([o]))).collect();
                    all.sort_by(|a, b| a.0.cmp(&b.0));
                    out.extend(all);
                }
                "materials" => {
                    let mut groups: std::collections::BTreeMap<String, HashSet<u32>> = Default::default();
                    for (k, &(o, _)) in objs.iter().enumerate() {
                        let m = job.obj_mats.get(k).copied().unwrap_or(0);
                        let name = if m == 0 { "default".to_string() } else { self.material(m - 1).map(|x| x.name.clone()).unwrap_or_else(|| format!("mat{}", m - 1)) };
                        groups.entry(name).or_default().insert(o);
                    }
                    out.extend(groups);
                }
                name => {
                    if let Some(id) = self.scene.find_by_name(name) {
                        out.push((name.to_string(), objs.iter().filter(|(_, n)| under(id, *n)).map(|x| x.0).collect()));
                    } else if let Some(m) = self.material_by_name(name) {
                        out.push((name.to_string(), objs.iter().enumerate().filter(|(k, _)| job.obj_mats.get(*k) == Some(&(m + 1))).map(|(_, x)| x.0).collect()));
                    } else {
                        return Err(format!("No object or material matches name: {name}"));
                    }
                }
            }
        }
        Ok(out)
    }

    /// Write mattes for `spec` (see [`App::matte_sets`]) as `<root>_matte_<name>[.####].<ext>`.
    pub fn save_mattes(&self, job: &RenderJob, spec: &str, root: &str, frame: Option<i32>, ext: &str) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        for (name, set) in self.matte_sets(job, spec)? {
            let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
            let file = match frame {
                Some(f) => format!("{root}_matte_{safe}.{f:04}.{ext}"),
                None => format!("{root}_matte_{safe}.{ext}"),
            };
            job.save_matte(&job.matte(&set), std::path::Path::new(&file))?;
            files.push(file);
        }
        Ok(files)
    }

    /// Camera and path-tracer scene for a render. With motion blur on, geometry and camera
    /// are taken at shutter open and close (centred on the current frame) and interpolated
    /// per sample; the static cache is bypassed.
    fn render_setup(&mut self, rc: RenderCam, s: &RenderSettings) -> (u64, RtCamera, Arc<RtScene>) {
        let cam = self.render_camera(rc);
        if !s.motion_blur || s.shutter <= 0.0 {
            let (k, sc) = self.render_scene(&cam);
            return (k, cam, sc);
        }
        let t = self.scene.time;
        let half = s.shutter.clamp(0.0, 2.0) * 0.5;
        self.scene.sim_anchor = Some(t.round());
        self.set_time(t - half);
        let cam0 = self.render_camera(rc);
        let g0 = self.render_geometry();
        self.set_time(t + half);
        let cam1 = self.render_camera(rc);
        let g1 = self.render_geometry();
        self.scene.sim_anchor = None;
        self.set_time(t);
        let cam = cam0.with_motion(&cam1);
        let mut sc = RtScene::build_motion(&g0, Some(&g1), &cam);
        // The key is the current frame's content (what IPR compares against).
        drop((g0, g1));
        let g = self.render_geometry();
        // Volumes from mid-shutter.
        sc.set_volumes(&g.volumes);
        self.tag_objects(&mut sc);
        (self.render_content_key(&g), cam, Arc::new(sc))
    }

    /// Start (or restart) a progressive render in the Render View.
    pub fn start_render(&mut self) {
        let settings = self.render.settings.clone();
        let (gen_key, cam, scene) = self.render_setup(self.render.camera, &settings);
        if let Some(j) = &self.render.job {
            j.cancel();
        }
        // A region render keeps the previous image outside the region.
        let base = settings.region.and_then(|_| self.render.job.as_ref().filter(|j| j.size() == (settings.width, settings.height)).map(|j| j.final_linear()));
        let cam_key = self.render_camera(self.render.camera).key();
        self.render.job_key = (gen_key, cam_key, settings.key());
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
        let (_, c, scene) = self.render_setup(rc, &settings);
        Ok(crate::raytrace::render_blocking(scene, c, settings))
    }

    /// Render `frames` (blocking): beauty to `base.####.ext`, or with `out.passes` every pass
    /// as `base_<pass>.####.ext`, plus mattes. `on_frame(frame, seconds, files)` runs after
    /// each frame (seconds < 0 = skipped because it already exists).
    pub fn render_frames(&mut self, cam: Option<&str>, settings: RenderSettings, frames: &[i32], base: &str, out: &FrameOutput, on_frame: &mut dyn FnMut(i32, f32, &[String])) -> Result<usize, String> {
        let t0 = self.scene.time;
        let mut n = 0;
        let fp0 = frame_path(base, 0);
        let ext = std::path::Path::new(&fp0).extension().and_then(|e| e.to_str()).unwrap_or("png").to_string();
        let root = base_without_ext(base).to_string();
        let res = (|| {
            for &f in frames {
                // Beauty file = "this frame is done" (written last).
                let beauty = if out.passes { format!("{root}_beauty.{f:04}.{ext}") } else { frame_path(base, f) };
                if out.skip_existing && std::fs::metadata(&beauty).is_ok_and(|m| m.len() > 0) {
                    on_frame(f, -1.0, std::slice::from_ref(&beauty));
                    continue;
                }
                self.set_time(f as f32);
                let mut job = self.render_now(cam, settings.clone())?;
                let mut files = Vec::new();
                if let Some(spec) = &out.mattes {
                    files.extend(self.save_mattes(&job, spec, &root, Some(f), &ext)?);
                }
                if out.passes {
                    // shot.0007.png → shot_<pass>.0007.png
                    for pass in crate::raytrace::Pass::ALL.into_iter().filter(|p| *p != crate::raytrace::Pass::Beauty) {
                        let file = format!("{root}_{}.{f:04}.{ext}", pass.file_tag());
                        job.save_pass(pass, std::path::Path::new(&file), settings.exposure, settings.tonemap)?;
                        files.push(file);
                    }
                }
                // Write the beauty under a temporary name and rename it, so an interrupted
                // render never leaves a partial file that a resume would skip.
                let tmp = format!("{root}.partial.{f:04}.{ext}");
                job.save_png(std::path::Path::new(&tmp), settings.exposure, settings.tonemap)?;
                let _ = std::fs::remove_file(&beauty);
                std::fs::rename(&tmp, &beauty).map_err(|e| format!("{beauty}: {e}"))?;
                files.insert(0, beauty);
                n += 1;
                on_frame(f, job.elapsed().as_secs_f32(), &files);
            }
            Ok::<(), String>(())
        })();
        self.set_time(t0);
        res.map(|_| n)
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

/// What a frame render writes besides the beauty.
#[derive(Clone, Debug, Default)]
pub struct FrameOutput {
    /// Every pass as `<base>_<pass>.####.ext`.
    pub passes: bool,
    /// Matte spec (`all`, `materials`, or names) → `<base>_matte_<name>.####.ext`.
    pub mattes: Option<String>,
    /// Skip frames whose beauty file already exists (resume).
    pub skip_existing: bool,
}

pub fn base_without_ext(base: &str) -> &str {
    for ext in ["png", "exr", "jpg", "jpeg", "tga", "bmp"] {
        let n = base.len();
        if n > ext.len() + 1 && base[n - ext.len() - 1..].eq_ignore_ascii_case(&format!(".{ext}")) {
            return &base[..n - ext.len() - 1];
        }
    }
    base
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

    #[test]
    fn id_mattes_cover_objects_groups_and_materials() {
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "polySphere -r 1 -n ball; move -1.2 0 0 ball; polyCube -w 1.5 -h 1.5 -d 1.5 -n box; move 1.2 0 0 box; polyPlane -w 10 -h 10 -n floor; move 0 -1 0 floor; group -n props ball box").unwrap();
        execute(&mut app, "materialPreset -p gold box; camera -n cam1 -p 0 1 7 -aim 0 0 0").unwrap();
        let s = crate::raytrace::RenderSettings { width: 80, height: 60, spp: 16, bounces: 1, denoise: false, ..Default::default() };
        let job = app.render_now(Some("cam1"), s).unwrap();
        assert_eq!(job.obj_nodes.len(), 3);
        let sets = app.matte_sets(&job, "all").unwrap();
        let names: Vec<&str> = sets.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(names, ["ball", "box", "floor"]);
        let mattes: Vec<Vec<f32>> = sets.iter().map(|(_, set)| job.matte(set)).collect();
        let sum = |m: &[f32]| m.iter().sum::<f32>();
        // Every pixel's object mattes add up to its coverage (all 1 here: the floor fills the
        // background), and edges are antialiased.
        let lin = job.linear();
        let mut soft = 0;
        for i in 0..lin.len() {
            let tot: f32 = mattes.iter().map(|m| m[i]).sum();
            assert!((tot - lin[i][3]).abs() < 1e-4, "pixel {i}: mattes {tot} vs alpha {}", lin[i][3]);
            if mattes[0][i] > 0.05 && mattes[0][i] < 0.95 {
                soft += 1;
            }
        }
        assert!(soft > 10, "antialiased ball edge ({soft} soft pixels)");
        assert!(sum(&mattes[0]) > 100.0 && sum(&mattes[1]) > 100.0);
        // A group's matte is its children's.
        let props = app.matte_sets(&job, "props").unwrap();
        let pm = job.matte(&props[0].1);
        assert!((sum(&pm) - sum(&mattes[0]) - sum(&mattes[1])).abs() < 0.5);
        // Materials: the gold box alone, everything else on the default material.
        let mats = app.matte_sets(&job, "materials").unwrap();
        assert_eq!(mats.len(), 2);
        let gold = mats.iter().find(|(n, _)| n != "default").unwrap();
        assert!((sum(&job.matte(&gold.1)) - sum(&mattes[1])).abs() < 0.5);
        let by_name = app.matte_sets(&job, &gold.0).unwrap();
        assert_eq!(by_name[0].1, gold.1);
        assert!(app.matte_sets(&job, "nothing_here").is_err());
        // Files.
        let dir = std::env::temp_dir().join(format!("rm_mattes_{}", std::process::id()));
        let root = dir.join("shot").to_string_lossy().into_owned();
        let files = app.save_mattes(&job, "ball,props", &root, Some(3), "png").unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("shot_matte_ball.0003.png"));
        let img = image::open(&files[0]).unwrap().to_rgba8();
        assert!(img.pixels().any(|p| p[0] == 255) && img.pixels().any(|p| p[0] > 20 && p[0] < 235));
        // MEL.
        let out = dir.join("still.png").to_string_lossy().into_owned();
        let r = execute(&mut app, &format!("render -x 40 -y 30 -spp 4 -cam cam1 -o {out} -mattes box")).unwrap();
        assert!(r.contains("1 mattes"), "{r}");
        assert!(dir.join("still_matte_box.png").exists());
        // Material-ID pass: the ball and floor share a colour, the box differs.
        let mid = job.pass_linear(crate::raytrace::Pass::MaterialId);
        let oid = job.pass_linear(crate::raytrace::Pass::ObjectId);
        let solid = |m: &[f32]| m.iter().position(|&v| v > 0.999).unwrap();
        let (ib, ix, ifl) = (solid(&mattes[0]), solid(&mattes[1]), solid(&mattes[2]));
        assert_eq!(mid[ib], mid[ifl]);
        assert_ne!(mid[ib], mid[ix]);
        assert_ne!(oid[ib], oid[ifl]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_time_evaluates_rig_for_batch_renders() {
        // Batch and sequence renders only call set_time (no UI frame), so constraints must
        // be evaluated there.
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "spaceLocator -n drv; polyCube -n c; pointConstraint drv c; currentTime 1; setKeyframe -at translateX -v 0 drv; currentTime 11; setKeyframe -at translateX -v 5 drv").unwrap();
        let c = app.scene.find_by_name("c").unwrap();
        app.set_time(6.0);
        let x = app.scene.world_matrix(c).w_axis.x;
        assert!((x - 2.5).abs() < 1e-3, "constrained cube follows at frame 6: {x}");
    }
}
