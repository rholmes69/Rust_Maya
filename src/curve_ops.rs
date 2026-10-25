//! Undoable curve and surface actions (Create › Curve Tools, the Curves and Surfaces menus,
//! Deform › Wire) and their MEL commands.

use crate::app::{App, CompMode};
use crate::nurbs::{self, Curve, ParamMode, SurfaceHistory, SurfaceOp};
use crate::scene::{NodeId, Shape};
use glam::Vec3;

/// Options used by the Curves / Surfaces / Wire menu items.
#[derive(Clone, Debug)]
pub struct CurveOpts {
    pub rebuild_spans: u32,
    pub rebuild_degree: u32,
    pub revolve_axis: usize,
    pub revolve_sections: u32,
    pub loft_sections: u32,
    pub loft_close: bool,
    pub extrude_length: f32,
    pub samples: u32,
    /// 0 = automatic (a fifth of the mesh size).
    pub wire_dropoff: f32,
    pub circle_radius: f32,
    pub circle_sections: u32,
}

impl Default for CurveOpts {
    fn default() -> Self {
        CurveOpts { rebuild_spans: 4, rebuild_degree: 3, revolve_axis: 1, revolve_sections: 8, loft_sections: 1, loft_close: false, extrude_length: 1.0, samples: 24, wire_dropoff: 0.0, circle_radius: 1.0, circle_sections: 8 }
    }
}

/// CV / EP curve tool state: points clicked so far (world space).
#[derive(Clone, Debug, Default)]
pub struct CurveTool {
    pub points: Vec<Vec3>,
    pub degree: u32,
}

impl App {
    /// Preview of the curve being drawn with the CV / EP tool.
    pub fn curve_tool_preview(&self) -> Option<Curve> {
        let pts = &self.curve_tool.points;
        if pts.len() < 2 {
            return None;
        }
        let d = self.curve_tool.degree.max(1) as usize;
        if self.tool == crate::app::Tool::CurveEp { nurbs::interpolate(pts, d, ParamMode::Chord) } else { Some(Curve::new(pts.clone(), d, false)) }
    }

    /// Enter in the CV / EP tool: create the curve.
    pub fn finish_curve_tool(&mut self) -> Result<Option<NodeId>, String> {
        let pts = std::mem::take(&mut self.curve_tool.points);
        if pts.len() < 2 {
            return Ok(None);
        }
        let d = self.curve_tool.degree.max(1) as usize;
        let id = if self.tool == crate::app::Tool::CurveEp { self.create_ep_curve(pts, d, false, None)? } else { self.create_curve(pts, d, false, None)? };
        Ok(Some(id))
    }

    /// MEL move / rotate / scale of the selected components (with soft selection), around the
    /// selection pivot. `rel` = relative; absolute move puts the selection's centre at `v`.
    pub fn transform_components(&mut self, kind: &str, v: Vec3, rel: bool) -> Result<(), String> {
        let pivot = self.pivot().ok_or("Nothing selected.")?;
        let weights = self.soft_weights();
        if weights.is_empty() {
            return Err("Nothing selected.".into());
        }
        self.checkpoint();
        let rot = glam::Quat::from_euler(glam::EulerRot::XYZ, v.x.to_radians(), v.y.to_radians(), v.z.to_radians());
        let mut per: std::collections::HashMap<NodeId, Vec<(u32, f32)>> = std::collections::HashMap::new();
        for (id, vtx, w) in weights {
            per.entry(id).or_default().push((vtx, w));
        }
        for (id, list) in per {
            let w = self.scene.world_matrix(id);
            let inv = w.inverse();
            let Some(pts) = self.scene.node_mut(id).and_then(|n| n.points_mut()) else { continue };
            for (vtx, wt) in list {
                let Some(p) = pts.get_mut(vtx as usize) else { continue };
                let wp = w.transform_point3(*p);
                let target = match kind {
                    "move" => {
                        if rel { wp + v } else { wp + (v - pivot) }
                    }
                    "rotate" => pivot + rot * (wp - pivot),
                    _ => pivot + (wp - pivot) * v,
                };
                *p = inv.transform_point3(wp.lerp(target, wt));
            }
        }
        Ok(())
    }

    fn add_curve_node(&mut self, base: &str, c: Curve, name: Option<&str>) -> NodeId {
        let id = self.scene.add_node(base, c.into_shape(), None);
        self.apply_name(id, name);
        if let Some(n) = self.scene.node_mut(id) {
            n.color = [0.85, 0.85, 0.9];
        }
        self.sel = vec![id];
        if self.comp_mode != CompMode::Object {
            self.comp_mode = CompMode::Object;
            self.comp_sel.clear();
        }
        id
    }

    /// CV curve through (world-space) control points.
    pub fn create_curve(&mut self, cvs: Vec<Vec3>, degree: usize, closed: bool, name: Option<&str>) -> Result<NodeId, String> {
        let c = Curve::new(cvs, degree, closed);
        if !c.is_valid() {
            return Err(format!("curve: needs at least {} points", if closed { 3 } else { 2 }));
        }
        self.checkpoint();
        Ok(self.add_curve_node("curve1", c, name))
    }

    /// EP curve: passes through every edit point.
    pub fn create_ep_curve(&mut self, pts: Vec<Vec3>, degree: usize, closed: bool, name: Option<&str>) -> Result<NodeId, String> {
        let c = if closed { nurbs::interpolate_closed(&pts, degree) } else { nurbs::interpolate(&pts, degree, ParamMode::Chord) };
        let c = c.ok_or(format!("curve -ep: needs at least {} distinct points", if closed { 3 } else { 2 }))?;
        self.checkpoint();
        Ok(self.add_curve_node("curve1", c, name))
    }

    pub fn create_circle(&mut self, radius: f32, sections: usize, normal: Vec3, center: Vec3, name: Option<&str>) -> NodeId {
        self.checkpoint();
        self.add_curve_node("nurbsCircle1", nurbs::circle(radius, sections, normal, center), name)
    }

    fn curve_ids(&self, ids: &[NodeId]) -> Result<Vec<NodeId>, String> {
        let c: Vec<NodeId> = ids.iter().copied().filter(|i| self.scene.node(*i).is_some_and(|n| matches!(n.shape, Shape::Curve { .. }))).collect();
        if c.is_empty() { Err("Select one or more curves.".into()) } else { Ok(c) }
    }

    fn set_curve(&mut self, id: NodeId, c: Curve) {
        if let Some(n) = self.scene.node_mut(id) {
            n.shape = c.into_shape();
        }
    }

    pub fn rebuild_curves(&mut self, ids: &[NodeId], spans: usize, degree: usize) -> Result<usize, String> {
        let ids = self.curve_ids(ids)?;
        let rebuilt: Vec<(NodeId, Curve)> = ids.iter().filter_map(|i| Curve::from_shape(&self.scene.node(*i)?.shape)?.rebuild(spans, degree).map(|c| (*i, c))).collect();
        if rebuilt.is_empty() {
            return Err("rebuildCurve: nothing to rebuild".into());
        }
        self.checkpoint();
        let n = rebuilt.len();
        for (i, c) in rebuilt {
            self.set_curve(i, c);
        }
        Ok(n)
    }

    pub fn reverse_curves(&mut self, ids: &[NodeId]) -> Result<usize, String> {
        let ids = self.curve_ids(ids)?;
        self.checkpoint();
        for i in &ids {
            if let Some(c) = Curve::from_shape(&self.scene.node(*i).unwrap().shape) {
                self.set_curve(*i, c.reversed());
            }
        }
        Ok(ids.len())
    }

    pub fn open_close_curves(&mut self, ids: &[NodeId]) -> Result<usize, String> {
        let ids = self.curve_ids(ids)?;
        self.checkpoint();
        for i in &ids {
            if let Some(c) = Curve::from_shape(&self.scene.node(*i).unwrap().shape)
                && c.cvs.len() >= 3
            {
                self.set_curve(*i, c.toggled_closed());
            }
        }
        Ok(ids.len())
    }

    /// Attach `b` to the end of `a` (a is replaced, b is kept, like Maya's default).
    pub fn attach_curves(&mut self, a: NodeId, b: NodeId) -> Result<NodeId, String> {
        self.curve_ids(&[a, b]).ok().filter(|v| v.len() == 2).ok_or("attachCurve: select two curves")?;
        let ca = nurbs::world_curve(&self.scene, a).unwrap();
        let cb = nurbs::world_curve(&self.scene, b).unwrap();
        if ca.closed || cb.closed {
            return Err("attachCurve: open both curves first (closed curves can't be attached).".into());
        }
        // Join at the nearest pair of ends.
        let (a0, a1, b0, b1) = (ca.cvs[0], *ca.cvs.last().unwrap(), cb.cvs[0], *cb.cvs.last().unwrap());
        let d = [a1.distance(b0), a1.distance(b1), a0.distance(b0), a0.distance(b1)];
        let k = (0..4).min_by(|x, y| d[*x].total_cmp(&d[*y])).unwrap();
        let (ca, cb) = match k {
            0 => (ca, cb),
            1 => (ca, cb.reversed()),
            2 => (ca.reversed(), cb),
            _ => (ca.reversed(), cb.reversed()),
        };
        let j = ca.attach(&cb).transformed(self.scene.world_matrix(a).inverse());
        self.checkpoint();
        self.set_curve(a, j);
        self.sel = vec![a];
        Ok(a)
    }

    /// Split a curve at parameter `t`: the original keeps the first part, a new curve gets the
    /// rest. Closed curves are opened at `t` instead.
    pub fn detach_curve(&mut self, id: NodeId, t: f32) -> Result<Option<NodeId>, String> {
        let c = self.curve_ids(&[id]).and_then(|_| Curve::from_shape(&self.scene.node(id).unwrap().shape).ok_or_else(String::new))?;
        let (l, r) = c.detach(t).ok_or("detachCurve: the parameter must be inside the curve (not at its ends)")?;
        self.checkpoint();
        self.set_curve(id, l);
        if r.cvs.is_empty() {
            return Ok(None);
        }
        let name = self.scene.node(id).map(|n| format!("{}_detached", n.name)).unwrap_or_default();
        let parent = self.scene.node(id).and_then(|n| n.parent);
        let (t0, r0, s0) = self.scene.node(id).map(|n| (n.translate, n.rotate, n.scale)).unwrap();
        let nid = self.scene.add_node(&name, r.into_shape(), parent);
        self.apply_name(nid, Some(&name));
        if let Some(n) = self.scene.node_mut(nid) {
            (n.translate, n.rotate, n.scale) = (t0, r0, s0);
            n.color = [0.85, 0.85, 0.9];
        }
        self.sel = vec![id, nid];
        Ok(Some(nid))
    }

    /// Insert a surface node built from curves (with construction history).
    fn create_surface(&mut self, base: &str, op: SurfaceOp, inputs: Vec<NodeId>, samples: u32, history: bool, name: Option<&str>) -> Result<NodeId, String> {
        let mut h = SurfaceHistory { op, inputs, samples: samples.clamp(3, 512), key: 0 };
        let mesh = nurbs::build_surface(&self.scene, NodeId::default(), &h).ok_or(format!("{}: the curves don't make a surface", h.op_name()))?;
        self.checkpoint();
        let id = self.scene.add_node(base, Shape::Poly { history: None, mesh, rev: crate::scene::next_rev() }, None);
        self.apply_name(id, name);
        if history {
            h.key = 0;
            if let Some(n) = self.scene.node_mut(id) {
                n.surf = Some(h);
            }
            nurbs::update_history(&mut self.scene);
        }
        self.sel = vec![id];
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn revolve_curves(&mut self, ids: &[NodeId], axis: Vec3, pivot: Vec3, sections: u32, start: f32, sweep: f32, samples: u32, history: bool, name: Option<&str>) -> Result<Vec<NodeId>, String> {
        let ids = self.curve_ids(ids)?;
        let mut out = Vec::new();
        for c in ids {
            out.push(self.create_surface("revolvedSurface1", SurfaceOp::Revolve { axis, pivot, sections: sections.clamp(3, 256), sweep: sweep.clamp(-360.0, 360.0), start }, vec![c], samples, history, name)?);
        }
        self.sel = out.clone();
        Ok(out)
    }

    pub fn loft_curves(&mut self, ids: &[NodeId], sections: u32, close: bool, samples: u32, history: bool, name: Option<&str>) -> Result<NodeId, String> {
        let ids = self.curve_ids(ids)?;
        if ids.len() < 2 {
            return Err("loft: select two or more curves (in order).".into());
        }
        self.create_surface("loftedSurface1", SurfaceOp::Loft { sections: sections.clamp(1, 64), close }, ids, samples, history, name)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn extrude_curve(&mut self, profile: NodeId, path: Option<NodeId>, length: f32, dir: Vec3, samples: u32, history: bool, name: Option<&str>) -> Result<NodeId, String> {
        let mut inputs = self.curve_ids(&[profile])?;
        if let Some(p) = path {
            inputs.extend(self.curve_ids(&[p])?);
        }
        self.create_surface("extrudedSurface1", SurfaceOp::Extrude { length, dir }, inputs, samples, history, name)
    }

    pub fn planar_curve(&mut self, id: NodeId, samples: u32, history: bool, name: Option<&str>) -> Result<NodeId, String> {
        let ids = self.curve_ids(&[id])?;
        let c = Curve::from_shape(&self.scene.node(id).unwrap().shape).unwrap();
        if !c.closed && c.cvs[0].distance(*c.cvs.last().unwrap()) > 1e-4 {
            return Err("planarSrf: the curve must be closed (Curves › Open/Close).".into());
        }
        self.create_surface("planarTrimmedSurface1", SurfaceOp::Planar, ids, samples, history, name)
    }
}

// ------------------------------------------------------------------------------ MEL

fn take_vecs(toks: &mut Vec<String>, flags: &[&str]) -> Vec<Vec3> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if flags.iter().any(|f| toks[i] == format!("-{f}")) {
            let n: Vec<f32> = toks.iter().skip(i + 1).take(3).filter_map(|t| t.parse().ok()).collect();
            if n.len() == 3 {
                out.push(Vec3::new(n[0], n[1], n[2]));
                toks.drain(i..i + 4);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `-flag value` (removed from the tokens).
fn take_val(toks: &mut Vec<String>, flags: &[&str]) -> Option<String> {
    let i = toks.iter().position(|t| flags.iter().any(|f| *t == format!("-{f}")))?;
    let v = toks.get(i + 1).cloned()?;
    toks.drain(i..i + 2);
    Some(v)
}

fn take_flag(toks: &mut Vec<String>, flags: &[&str]) -> bool {
    match toks.iter().position(|t| flags.iter().any(|f| *t == format!("-{f}"))) {
        Some(i) => {
            toks.remove(i);
            true
        }
        None => false,
    }
}

fn num<T: std::str::FromStr>(v: Option<String>, d: T) -> T {
    v.and_then(|s| s.parse().ok()).unwrap_or(d)
}

fn on(v: Option<String>, d: bool) -> bool {
    match v.as_deref() {
        Some("0" | "off" | "false") => false,
        Some(_) => true,
        None => d,
    }
}

fn node_name(app: &App, id: NodeId) -> String {
    app.scene.node(id).map(|n| n.name.clone()).unwrap_or_default()
}

/// Curve / surface / wire MEL commands. None = not one of these commands.
pub fn mel(app: &mut App, tokens: &[String]) -> Option<Result<String, String>> {
    let (cmd, rest) = tokens.split_first()?;
    let mut t: Vec<String> = rest.to_vec();
    let r = match cmd.as_str() {
        "curve" => (|| {
            let eps = take_vecs(&mut t, &["ep", "editPoint"]);
            let cvs = take_vecs(&mut t, &["p", "point"]);
            let degree = num(take_val(&mut t, &["d", "degree"]), 3usize);
            let closed = take_flag(&mut t, &["per", "periodic"]);
            let name = take_val(&mut t, &["n", "name"]);
            let id = if !eps.is_empty() { app.create_ep_curve(eps, degree, closed, name.as_deref())? } else { app.create_curve(cvs, degree, closed, name.as_deref())? };
            Ok(node_name(app, id))
        })(),
        "circle" => {
            // NURBS circle (degree 3). Default normal +Y (flat, like a rig control); -c offsets
            // the CVs, -p places the node.
            let nr = take_vecs(&mut t, &["nr", "normal"]).first().copied().unwrap_or(Vec3::Y);
            let c = take_vecs(&mut t, &["c", "center"]).first().copied().unwrap_or(Vec3::ZERO);
            let p = take_vecs(&mut t, &["p", "position"]).first().copied();
            let r = num(take_val(&mut t, &["r", "radius"]), 1.0f32);
            let s = num(take_val(&mut t, &["s", "sections"]), 8usize);
            let name = take_val(&mut t, &["n", "name"]);
            let id = app.create_circle(r, s, nr, c, name.as_deref());
            if let Some(n) = app.scene.node_mut(id) {
                n.color = [0.2, 0.55, 1.0];
            }
            if let Some(p) = p {
                crate::rig::set_world_position(&mut app.scene, id, p);
            }
            Ok(node_name(app, id))
        }
        "rebuildCurve" => (|| {
            let s = num(take_val(&mut t, &["s", "spans"]), 4usize);
            let d = num(take_val(&mut t, &["d", "degree"]), 3usize);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let n = app.rebuild_curves(&ids, s, d)?;
            Ok(format!("{n} rebuilt"))
        })(),
        "reverseCurve" => (|| {
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            app.reverse_curves(&ids).map(|n| format!("{n} reversed"))
        })(),
        "closeCurve" | "openCloseCurve" => (|| {
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            app.open_close_curves(&ids).map(|n| format!("{n} toggled"))
        })(),
        "attachCurve" => (|| {
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            if ids.len() < 2 {
                return Err("attachCurve curveA curveB".to_string());
            }
            let id = app.attach_curves(ids[0], ids[1])?;
            Ok(node_name(app, id))
        })(),
        "detachCurve" => (|| {
            // detachCurve curve.u[2.5]   or   detachCurve -pf 0.5 curve (fraction of the range)
            let frac: Option<f32> = take_val(&mut t, &["pf", "fraction"]).and_then(|v| v.parse().ok());
            let mut target = None;
            let mut param = None;
            for x in t.iter().filter(|x| !x.starts_with('-')) {
                if let Some((n, rest)) = x.split_once(".u[") {
                    target = Some(n.to_string());
                    param = rest.trim_end_matches(']').parse::<f32>().ok();
                } else {
                    target = Some(x.clone());
                }
            }
            let ids = crate::commands::targets(app, &target.into_iter().collect::<Vec<_>>())?;
            let id = *ids.first().ok_or("detachCurve: which curve?")?;
            let c = nurbs::world_curve(&app.scene, id).ok_or("detachCurve: not a curve")?;
            let (a, b) = c.domain();
            let u = match (param, frac) {
                (Some(p), _) => p,
                (None, Some(f)) => a + (b - a) * f,
                _ => a + (b - a) * 0.5,
            };
            let nid = app.detach_curve(id, u)?;
            Ok(nid.map(|n| format!("{} {}", node_name(app, id), node_name(app, n))).unwrap_or_else(|| node_name(app, id)))
        })(),
        "revolve" => (|| {
            let axis = take_vecs(&mut t, &["ax", "axis"]).first().copied().unwrap_or(Vec3::Y);
            let pivot = take_vecs(&mut t, &["p", "pivot"]).first().copied().unwrap_or(Vec3::ZERO);
            let sections = num(take_val(&mut t, &["s", "sections"]), 8u32);
            let ssw = num(take_val(&mut t, &["ssw", "startSweep"]), 0.0f32);
            let esw = num(take_val(&mut t, &["esw", "endSweep"]), 360.0f32);
            let samples = num(take_val(&mut t, &["sp", "samples"]), 24u32);
            let ch = on(take_val(&mut t, &["ch", "constructionHistory"]), true);
            let _ = take_val(&mut t, &["po", "polygon"]);
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let out = app.revolve_curves(&ids, axis, pivot, sections, ssw, esw - ssw, samples, ch, name.as_deref())?;
            Ok(out.iter().map(|i| node_name(app, *i)).collect::<Vec<_>>().join(" "))
        })(),
        "loft" => (|| {
            let sections = num(take_val(&mut t, &["ss", "sectionSpans", "s"]), 1u32);
            let close = take_flag(&mut t, &["c", "close"]);
            let samples = num(take_val(&mut t, &["sp", "samples"]), 24u32);
            let ch = on(take_val(&mut t, &["ch", "constructionHistory"]), true);
            let _ = take_val(&mut t, &["po", "polygon"]);
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let id = app.loft_curves(&ids, sections, close, samples, ch, name.as_deref())?;
            Ok(node_name(app, id))
        })(),
        "extrude" => (|| {
            let dir = take_vecs(&mut t, &["dir", "direction", "d"]).first().copied().unwrap_or(Vec3::ZERO);
            let length = num(take_val(&mut t, &["l", "length"]), 1.0f32);
            let samples = num(take_val(&mut t, &["sp", "samples"]), 24u32);
            let ch = on(take_val(&mut t, &["ch", "constructionHistory"]), true);
            let _ = take_val(&mut t, &["po", "polygon"]);
            let _ = take_val(&mut t, &["et", "extrudeType"]);
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let profile = *ids.first().ok_or("extrude profile [path]")?;
            let id = app.extrude_curve(profile, ids.get(1).copied(), length, dir, samples, ch, name.as_deref())?;
            Ok(node_name(app, id))
        })(),
        "planarSrf" => (|| {
            let samples = num(take_val(&mut t, &["sp", "samples"]), 32u32);
            let ch = on(take_val(&mut t, &["ch", "constructionHistory"]), true);
            let _ = take_val(&mut t, &["po", "polygon"]);
            let name = take_val(&mut t, &["n", "name"]);
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let ids = crate::commands::targets(app, &objs)?;
            let id = app.planar_curve(*ids.first().ok_or("planarSrf curve")?, samples, ch, name.as_deref())?;
            Ok(node_name(app, id))
        })(),
        "wire" => (|| {
            // wire -w curve [-dds 0 dist] [-en 1] [-rot 1] meshes   |   wire -e -w curve ...
            let edit = take_flag(&mut t, &["e", "edit"]);
            let wire = take_val(&mut t, &["w", "wire"]);
            let dds = match t.iter().position(|x| x == "-dds" || x == "-dropoffDistance") {
                Some(i) => {
                    let a: Option<f32> = t.get(i + 1).and_then(|v| v.parse().ok());
                    let b: Option<f32> = t.get(i + 2).and_then(|v| v.parse().ok());
                    match (a, b) {
                        (Some(_), Some(d)) => {
                            t.drain(i..i + 3);
                            Some(d)
                        }
                        (Some(d), None) => {
                            t.drain(i..i + 2);
                            Some(d)
                        }
                        _ => return Err("wire -dds [index] distance".to_string()),
                    }
                }
                None => None,
            };
            let en: Option<f32> = take_val(&mut t, &["en", "envelope"]).and_then(|v| v.parse().ok());
            let rot: Option<f32> = take_val(&mut t, &["rot", "rotation"]).and_then(|v| v.parse().ok());
            let objs: Vec<String> = t.iter().filter(|x| !x.starts_with('-')).cloned().collect();
            let mut ids = crate::commands::targets(app, &objs)?;
            let curve = match wire {
                Some(w) => app.scene.find_by_name(&w).ok_or_else(|| format!("No object matches name: {w}"))?,
                None => {
                    let i = ids.iter().rposition(|i| app.scene.node(*i).is_some_and(|n| matches!(n.shape, Shape::Curve { .. }))).ok_or("wire: give the curve with -w (or select it last)")?;
                    ids.remove(i)
                }
            };
            if edit {
                return app.edit_wire(curve, dds, en, rot).map(|n| format!("{n} edited"));
            }
            let n = app.create_wire(curve, &ids, dds.unwrap_or(0.0), en.unwrap_or(1.0), rot.unwrap_or(1.0))?;
            Ok(format!("{n} vertices bound"))
        })(),
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::commands::execute;
    use crate::nurbs::Curve;
    use glam::Vec3;

    fn world_pts(app: &App, name: &str) -> Vec<Vec3> {
        let id = app.scene.find_by_name(name).unwrap();
        crate::rig::deformed_positions(&app.scene, id).unwrap_or_else(|| {
            let w = app.scene.world_matrix(id);
            app.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        })
    }

    #[test]
    fn curve_commands_and_component_edit() {
        let mut app = App::new();
        app.scene.nodes.clear();
        assert_eq!(execute(&mut app, "curve -d 3 -p 0 0 0 -p 1 1 0 -p 2 0 0 -p 3 1 0 -p 4 0 0 -n c1").unwrap(), "c1");
        let id = app.scene.find_by_name("c1").unwrap();
        let c = Curve::from_shape(&app.scene.node(id).unwrap().shape).unwrap();
        assert_eq!((c.cvs.len(), c.degree, c.spans()), (5, 3, 2));
        // EP curve passes through its points.
        execute(&mut app, "curve -ep 0 0 0 -ep 1 2 0 -ep 3 1 0 -ep 4 0 0 -n ep1").unwrap();
        let e = crate::nurbs::world_curve(&app.scene, app.scene.find_by_name("ep1").unwrap()).unwrap();
        for p in [Vec3::new(1.0, 2.0, 0.0), Vec3::new(3.0, 1.0, 0.0)] {
            assert!(e.point(e.closest_param(p)).distance(p) < 1e-3);
        }
        execute(&mut app, "rebuildCurve -s 6 -d 3 c1").unwrap();
        let c = Curve::from_shape(&app.scene.node(id).unwrap().shape).unwrap();
        assert_eq!(c.spans(), 6);
        execute(&mut app, "circle -r 2 -nr 0 1 0 -n ring").unwrap();
        let r = crate::nurbs::world_curve(&app.scene, app.scene.find_by_name("ring").unwrap()).unwrap();
        assert!(r.closed && r.tessellate(8).iter().all(|p| p.y.abs() < 1e-5 && (p.length() - 2.0).abs() < 0.02));
        // Move a CV through component selection.
        execute(&mut app, "select ring.cv[0]; move -r 0 1 0").unwrap();
        let rc = Curve::from_shape(&app.scene.node(app.scene.find_by_name("ring").unwrap()).unwrap().shape).unwrap();
        assert!((rc.cvs[0].y - 1.0).abs() < 1e-5 && rc.cvs[1].y.abs() < 1e-6);
        let pp = execute(&mut app, "pointPosition ring.cv[0]").unwrap();
        assert!(pp.ends_with(" 1 0") || pp.contains(" 1 "), "{pp}");
        app.undo();
        let rc = Curve::from_shape(&app.scene.node(app.scene.find_by_name("ring").unwrap()).unwrap().shape).unwrap();
        assert!(rc.cvs[0].y.abs() < 1e-6, "undo restores the CV");
        // Detach / attach / reverse / open-close.
        let out = execute(&mut app, "select -cl; detachCurve -pf 0.5 ep1").unwrap();
        assert_eq!(out, "ep1 ep1_detached");
        execute(&mut app, "attachCurve ep1 ep1_detached").unwrap();
        let j = crate::nurbs::world_curve(&app.scene, app.scene.find_by_name("ep1").unwrap()).unwrap();
        assert!(j.point(j.closest_param(Vec3::new(3.0, 1.0, 0.0))).distance(Vec3::new(3.0, 1.0, 0.0)) < 1e-3);
        execute(&mut app, "reverseCurve ep1; closeCurve c1").unwrap();
        assert!(Curve::from_shape(&app.scene.node(id).unwrap().shape).unwrap().closed);
        assert!(execute(&mut app, "curve -p 0 0 0").is_err());
    }

    #[test]
    fn surfaces_keep_history_and_wire_deforms() {
        let mut app = App::new();
        app.scene.nodes.clear();
        // Vase: revolve a profile; moving a CV reshapes the surface (history).
        execute(&mut app, "curve -d 3 -p 0.5 0 0 -p 1 0.5 0 -p 0.6 1.5 0 -p 0.8 2 0 -n prof; revolve -s 16 -n vase prof").unwrap();
        let top = |app: &App| world_pts(app, "vase").iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((top(&app) - 2.0).abs() < 1e-4);
        execute(&mut app, "select prof.cv[3]; move -r 0 1 0; select -cl").unwrap();
        crate::rig::evaluate(&mut app.scene);
        assert!((top(&app) - 3.0).abs() < 1e-4, "history follows the profile: {}", top(&app));
        // Moving the surface node moves it; editing the curve then reshapes it in place.
        execute(&mut app, "move -r 5 0 0 vase; select prof.cv[3]; move -r 0 -1 0; select -cl").unwrap();
        crate::rig::evaluate(&mut app.scene);
        let xs: Vec<f32> = world_pts(&app, "vase").iter().map(|p| p.x).collect();
        assert!(xs.iter().all(|x| *x > 3.0), "the surface keeps its own offset");
        assert!((top(&app) - 2.0).abs() < 1e-4);
        // Editing the surface itself drops its history.
        let vid = app.scene.find_by_name("vase").unwrap();
        assert!(app.scene.node(vid).unwrap().surf.is_some());
        app.scene.node_mut(vid).unwrap().mesh_mut(true);
        assert!(app.scene.node(vid).unwrap().surf.is_none());
        // Loft / extrude / planar.
        execute(&mut app, "circle -r 1 -nr 0 1 0 -n a; circle -r 0.5 -nr 0 1 0 -c 0 2 0 -n b; loft -ss 2 -n tube a b").unwrap();
        assert!(app.scene.node(app.scene.find_by_name("tube").unwrap()).unwrap().mesh().unwrap().faces.len() >= 24);
        execute(&mut app, "curve -d 1 -p 0 0 0 -p 0 0 -4 -n path; extrude -n pipe a path; planarSrf -n cap b").unwrap();
        let pipe = world_pts(&app, "pipe");
        assert!(pipe.iter().map(|p| p.z).fold(f32::MAX, f32::min) < -3.9);
        assert_eq!(app.scene.node(app.scene.find_by_name("cap").unwrap()).unwrap().mesh().unwrap().faces.len(), 1);
        // Wire: a plane follows a curve lying across it.
        execute(&mut app, "polyPlane -w 4 -h 4 -sx 8 -sy 8 -n sheet; curve -d 1 -p -2 0 0 -p 2 0 0 -n w1; wire -w w1 -dds 0 1 sheet").unwrap();
        let before = world_pts(&app, "sheet");
        assert!(crate::rig::has_deformers(&app.scene, app.scene.find_by_name("sheet").unwrap()));
        execute(&mut app, "select w1.cv[0:1]; move -r 0 1 0; select -cl").unwrap();
        let after = world_pts(&app, "sheet");
        for (b, a) in before.iter().zip(&after) {
            let dz = b.z.abs();
            let expect = crate::deform::wire_falloff(dz, 1.0);
            assert!((a.y - b.y - expect).abs() < 1e-3, "vertex at z {dz}: moved {} expected {expect}", a.y - b.y);
        }
        // Envelope 0 turns it off.
        execute(&mut app, "wire -e -w w1 -en 0").unwrap();
        let off = world_pts(&app, "sheet");
        assert!(off.iter().zip(&before).all(|(a, b)| a.distance(*b) < 1e-5));
    }

    #[test]
    fn curves_history_and_wires_survive_save_and_usd() {
        let dir = std::env::temp_dir().join("rm_curve_io");
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new();
        app.scene.nodes.clear();
        execute(&mut app, "curve -d 3 -p 0.5 0 0 -p 1 0.5 0 -p 0.6 1.5 0 -p 0.8 2 0 -n prof; revolve -s 12 -n vase prof").unwrap();
        execute(&mut app, "circle -r 1.5 -nr 0 1 0 -n ring; polyPlane -w 2 -h 2 -sx 4 -sy 4 -n sheet; curve -d 1 -p -1 0 0 -p 1 0 0 -n w1; wire -w w1 -dds 0 0.5 sheet").unwrap();
        // .rma round trip keeps degree, history and the wire.
        let rma = dir.join("curves.rma");
        app.save_to(rma.clone());
        let mut b = App::new();
        b.open_from(rma);
        let prof = b.scene.find_by_name("prof").unwrap();
        assert!(matches!(b.scene.node(prof).unwrap().shape, crate::scene::Shape::Curve { degree: 3, .. }));
        assert!(b.scene.node(b.scene.find_by_name("vase").unwrap()).unwrap().surf.is_some());
        assert_eq!(b.scene.node(b.scene.find_by_name("sheet").unwrap()).unwrap().wires.len(), 1);
        execute(&mut b, "select prof.cv[3]; move -r 0 1 0; select -cl").unwrap();
        crate::rig::evaluate(&mut b.scene);
        let top = world_pts(&b, "vase").iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((top - 3.0).abs() < 1e-4, "history still live after load: {top}");
        // USD: cubic curves export as NurbsCurves and import back with the same shape.
        let usda = dir.join("curves.usda");
        let opts = crate::export::ExportOptions::from_app(&app, false);
        crate::export::export_path(&app, &usda, &opts).unwrap();
        let text = std::fs::read_to_string(&usda).unwrap();
        assert!(text.contains("def NurbsCurves") && text.contains("int[] order = [4]"));
        let mut c = App::new();
        c.scene.nodes.clear();
        crate::importer::import_path(&mut c, &usda).unwrap();
        for name in ["prof", "ring"] {
            let a = crate::nurbs::world_curve(&app.scene, app.scene.find_by_name(name).unwrap()).unwrap();
            let id = c.scene.nodes.iter().find(|n| n.name.starts_with(name) && matches!(n.shape, crate::scene::Shape::Curve { .. })).map(|n| n.id).unwrap_or_else(|| panic!("{name} imported"));
            let r = crate::nurbs::world_curve(&c.scene, id).unwrap();
            assert_eq!((r.closed, r.degree, r.cvs.len()), (a.closed, a.degree, a.cvs.len()), "{name}");
            let (x, y) = a.domain();
            for k in 0..=10 {
                let p = a.point(x + (y - x) * k as f32 / 10.0);
                assert!(r.point(r.closest_param(p)).distance(p) < 1e-3, "{name} shape");
            }
        }
    }
}
