//! Scene graph (DAG of transform nodes), animation curves and display layers.

use crate::mesh::{Mesh, Primitive, TriMesh};
use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub type NodeId = u64;

static REV: AtomicU64 = AtomicU64::new(1);
pub fn next_rev() -> u64 {
    REV.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Channel {
    TranslateX,
    TranslateY,
    TranslateZ,
    RotateX,
    RotateY,
    RotateZ,
    ScaleX,
    ScaleY,
    ScaleZ,
    Visibility,
}

impl Channel {
    pub const ALL: [Channel; 10] = [
        Channel::TranslateX,
        Channel::TranslateY,
        Channel::TranslateZ,
        Channel::RotateX,
        Channel::RotateY,
        Channel::RotateZ,
        Channel::ScaleX,
        Channel::ScaleY,
        Channel::ScaleZ,
        Channel::Visibility,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Channel::TranslateX => "Translate X",
            Channel::TranslateY => "Translate Y",
            Channel::TranslateZ => "Translate Z",
            Channel::RotateX => "Rotate X",
            Channel::RotateY => "Rotate Y",
            Channel::RotateZ => "Rotate Z",
            Channel::ScaleX => "Scale X",
            Channel::ScaleY => "Scale Y",
            Channel::ScaleZ => "Scale Z",
            Channel::Visibility => "Visibility",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Channel::TranslateX => "tx",
            Channel::TranslateY => "ty",
            Channel::TranslateZ => "tz",
            Channel::RotateX => "rx",
            Channel::RotateY => "ry",
            Channel::RotateZ => "rz",
            Channel::ScaleX => "sx",
            Channel::ScaleY => "sy",
            Channel::ScaleZ => "sz",
            Channel::Visibility => "v",
        }
    }
    pub fn long(self) -> &'static str {
        match self {
            Channel::TranslateX => "translateX",
            Channel::TranslateY => "translateY",
            Channel::TranslateZ => "translateZ",
            Channel::RotateX => "rotateX",
            Channel::RotateY => "rotateY",
            Channel::RotateZ => "rotateZ",
            Channel::ScaleX => "scaleX",
            Channel::ScaleY => "scaleY",
            Channel::ScaleZ => "scaleZ",
            Channel::Visibility => "visibility",
        }
    }
    pub fn parse(s: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.short() == s || c.long().eq_ignore_ascii_case(s))
    }
    pub fn default_value(self) -> f32 {
        match self {
            Channel::ScaleX | Channel::ScaleY | Channel::ScaleZ | Channel::Visibility => 1.0,
            _ => 0.0,
        }
    }
    pub fn translate() -> [Channel; 3] {
        [Channel::TranslateX, Channel::TranslateY, Channel::TranslateZ]
    }
    pub fn rotate() -> [Channel; 3] {
        [Channel::RotateX, Channel::RotateY, Channel::RotateZ]
    }
    pub fn scale() -> [Channel; 3] {
        [Channel::ScaleX, Channel::ScaleY, Channel::ScaleZ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Tangent {
    /// Auto (clamped spline) — Maya's default.
    Auto,
    Linear,
    Stepped,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub frame: f32,
    pub value: f32,
    /// Per-key tangent override (None = the curve's default tangent type).
    #[serde(default)]
    pub tan: Option<KeyTan>,
}

/// Maya-style tangent types for one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TanKind {
    /// Clamped spline (no overshoot).
    Auto,
    /// Smooth spline (Catmull-Rom, may overshoot).
    Spline,
    Linear,
    Flat,
    /// Hold the value until the next key.
    Stepped,
    /// User-edited slopes (in_slope / out_slope).
    Fixed,
}

impl TanKind {
    pub const MENU: [TanKind; 5] = [TanKind::Auto, TanKind::Spline, TanKind::Linear, TanKind::Flat, TanKind::Stepped];
    pub fn name(self) -> &'static str {
        match self {
            TanKind::Auto => "auto",
            TanKind::Spline => "spline",
            TanKind::Linear => "linear",
            TanKind::Flat => "flat",
            TanKind::Stepped => "step",
            TanKind::Fixed => "fixed",
        }
    }
    pub fn parse(s: &str) -> Option<TanKind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" | "clamped" => TanKind::Auto,
            "spline" | "smooth" => TanKind::Spline,
            "linear" => TanKind::Linear,
            "flat" => TanKind::Flat,
            "step" | "stepped" => TanKind::Stepped,
            "fixed" => TanKind::Fixed,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyTan {
    pub kind: TanKind,
    /// Slopes in value units per frame (used by `Fixed`).
    pub in_slope: f32,
    pub out_slope: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub keys: Vec<Key>,
    pub tangent: Tangent,
}

impl Default for Curve {
    fn default() -> Self {
        Curve { keys: Vec::new(), tangent: Tangent::Auto }
    }
}

impl Curve {
    pub fn set_key(&mut self, frame: f32, value: f32) {
        match self.keys.iter().position(|k| (k.frame - frame).abs() < 1e-4) {
            Some(i) => self.keys[i].value = value,
            None => {
                self.keys.push(Key { frame, value, tan: None });
                self.keys.sort_by(|a, b| a.frame.total_cmp(&b.frame));
            }
        }
    }

    pub fn remove_key(&mut self, frame: f32) {
        self.keys.retain(|k| (k.frame - frame).abs() >= 1e-4);
    }

    pub fn has_key_at(&self, frame: f32) -> bool {
        self.keys.iter().any(|k| (k.frame - frame).abs() < 1e-4)
    }

    /// Clamped auto slope at key i (flat at extrema and at the ends).
    fn auto_slope(&self, i: usize) -> f32 {
        let k = &self.keys;
        if i == 0 || i + 1 >= k.len() {
            return 0.0;
        }
        let (p, c, n) = (k[i - 1], k[i], k[i + 1]);
        // Clamp: flat tangent at local extrema (prevents overshoot).
        if (c.value - p.value) * (n.value - c.value) <= 0.0 {
            return 0.0;
        }
        (n.value - p.value) / (n.frame - p.frame)
    }

    /// Catmull-Rom slope at key i (one-sided at the ends).
    fn spline_slope(&self, i: usize) -> f32 {
        let k = &self.keys;
        let n = k.len();
        if n < 2 {
            return 0.0;
        }
        let (a, b) = (k[i.saturating_sub(1)], k[(i + 1).min(n - 1)]);
        if (b.frame - a.frame).abs() < 1e-6 { 0.0 } else { (b.value - a.value) / (b.frame - a.frame) }
    }

    /// Tangent type of key i (its override or the curve default).
    pub fn kind_of(&self, i: usize) -> TanKind {
        match self.keys.get(i).and_then(|k| k.tan) {
            Some(t) => t.kind,
            None => match self.tangent {
                Tangent::Auto => TanKind::Auto,
                Tangent::Linear => TanKind::Linear,
                Tangent::Stepped => TanKind::Stepped,
            },
        }
    }

    fn secant(&self, a: usize, b: usize) -> f32 {
        let (ka, kb) = (self.keys[a], self.keys[b]);
        if (kb.frame - ka.frame).abs() < 1e-6 { 0.0 } else { (kb.value - ka.value) / (kb.frame - ka.frame) }
    }

    /// Outgoing slope of key i (value per frame).
    pub fn out_slope(&self, i: usize) -> f32 {
        let n = self.keys.len();
        match self.kind_of(i) {
            TanKind::Auto => self.auto_slope(i),
            TanKind::Spline => self.spline_slope(i),
            TanKind::Linear => if i + 1 < n { self.secant(i, i + 1) } else if i > 0 { self.secant(i - 1, i) } else { 0.0 },
            TanKind::Flat | TanKind::Stepped => 0.0,
            TanKind::Fixed => self.keys[i].tan.map(|t| t.out_slope).unwrap_or(0.0),
        }
    }

    /// Incoming slope of key i (value per frame).
    pub fn in_slope(&self, i: usize) -> f32 {
        match self.kind_of(i) {
            TanKind::Auto => self.auto_slope(i),
            TanKind::Spline => self.spline_slope(i),
            TanKind::Linear => if i > 0 { self.secant(i - 1, i) } else if i + 1 < self.keys.len() { self.secant(i, i + 1) } else { 0.0 },
            TanKind::Flat | TanKind::Stepped => 0.0,
            TanKind::Fixed => self.keys[i].tan.map(|t| t.in_slope).unwrap_or(0.0),
        }
    }

    /// Set the tangent type of key i (Fixed keeps the current slopes).
    pub fn set_key_kind(&mut self, i: usize, kind: TanKind) {
        if i >= self.keys.len() {
            return;
        }
        let (inn, out) = (self.in_slope(i), self.out_slope(i));
        self.keys[i].tan = Some(KeyTan { kind, in_slope: inn, out_slope: out });
    }

    /// Set explicit slopes for key i (makes it Fixed).
    pub fn set_key_slopes(&mut self, i: usize, in_slope: f32, out_slope: f32) {
        if let Some(k) = self.keys.get_mut(i) {
            k.tan = Some(KeyTan { kind: TanKind::Fixed, in_slope, out_slope });
        }
    }

    pub fn evaluate(&self, t: f32) -> Option<f32> {
        let k = &self.keys;
        if k.is_empty() {
            return None;
        }
        if t <= k[0].frame {
            return Some(k[0].value);
        }
        if t >= k[k.len() - 1].frame {
            return Some(k[k.len() - 1].value);
        }
        let i = k.iter().rposition(|key| key.frame <= t).unwrap();
        let (a, b) = (k[i], k[i + 1]);
        let dt = b.frame - a.frame;
        let s = (t - a.frame) / dt;
        let (ka, kb) = (self.kind_of(i), self.kind_of(i + 1));
        if ka == TanKind::Stepped {
            return Some(a.value);
        }
        if ka == TanKind::Linear && kb == TanKind::Linear {
            return Some(a.value + (b.value - a.value) * s);
        }
        let m0 = self.out_slope(i) * dt;
        let m1 = self.in_slope(i + 1) * dt;
        let s2 = s * s;
        let s3 = s2 * s;
        Some((2.0 * s3 - 3.0 * s2 + 1.0) * a.value + (s3 - 2.0 * s2 + s) * m0 + (-2.0 * s3 + 3.0 * s2) * b.value + (s3 - s2) * m1)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Shape {
    /// A transform with no shape (group).
    Group,
    Locator,
    /// Skeleton joint (drawn as a sphere with bones to its child joints).
    Joint { radius: f32 },
    /// IK handle: the node's position is the goal for the chain start → end.
    IkHandle {
        start: NodeId,
        end: NodeId,
        pole: Option<NodeId>,
        /// Stretchy IK: joints lengthen to reach goals beyond the chain length.
        #[serde(default)]
        stretch: bool,
        /// Twist of the rotate plane around the start → goal axis (degrees).
        #[serde(default)]
        twist: f32,
        /// Rest translations of the chain joints (for stretch).
        #[serde(default)]
        rest: Vec<(NodeId, Vec3)>,
    },
    /// Cluster deformer handle (the node's transform drives the cluster).
    ClusterHandle,
    /// Light (shines down its local -Z axis, like Maya).
    Light(crate::look::Light),
    /// Camera (looks down its local -Z axis).
    Camera(crate::look::CameraShape),
    /// Lattice (FFD) cage: `div` points per axis, points in local space (-0.5..0.5 at rest).
    Lattice { div: [u32; 3], points: Vec<Vec3> },
    /// Control curve (linear NURBS-style curve used for rig controls).
    /// NURBS curve (non-rational B-spline): CVs, degree (1 = polyline; older scenes load as
    /// degree 1) and the knot vector of open curves (empty = clamped uniform).
    Curve {
        points: Vec<Vec3>,
        closed: bool,
        #[serde(default = "crate::nurbs::default_degree")]
        degree: u32,
        #[serde(default)]
        knots: Vec<f32>,
    },
    /// Particle system (emitter + simulation settings; the simulated particles are cached).
    Particles(crate::fx::Particles),
    /// Dynamics field (air, gravity, radial, vortex, turbulence, …) placed by its transform.
    Field(crate::fx::Field),
    /// Hair system: dynamic strands grown from a mesh, or one dynamic curve.
    Hair(crate::hair::Hair),
    /// Smoke / fire container (voxel fluid simulation, rendered as a volume).
    Fluid(crate::fluid::Fluid),
    Poly {
        /// Construction history. `None` once history is deleted or components were edited.
        history: Option<Primitive>,
        mesh: Mesh,
        #[serde(skip, default = "next_rev")]
        rev: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub parent: Option<NodeId>,
    pub translate: Vec3,
    /// Euler degrees, rotate order XYZ.
    pub rotate: Vec3,
    pub scale: Vec3,
    pub visible: bool,
    pub shape: Shape,
    /// Diffuse colour (sRGB).
    pub color: [f32; 3],
    pub layer: Option<u64>,
    pub anim: BTreeMap<Channel, Curve>,
    #[serde(default)]
    pub expanded: bool,
    /// Joint orient (degrees, applied before rotate), like Maya's jointOrient.
    #[serde(default)]
    pub joint_orient: Vec3,
    #[serde(default)]
    pub skin: Option<crate::rig::Skin>,
    #[serde(default)]
    pub constraints: Vec<crate::rig::Constraint>,
    #[serde(default)]
    pub driven: Vec<crate::rig::DrivenKey>,
    #[serde(default)]
    pub blend: Option<crate::rig::BlendShape>,
    #[serde(default)]
    pub clusters: Vec<crate::rig::Cluster>,
    /// Assigned shared material (Hypershade); None = per-object lambert using `color`.
    #[serde(default)]
    pub material: Option<u32>,
    /// Lattice deformers bound to this mesh.
    #[serde(default)]
    pub lattices: Vec<crate::deform::LatticeBind>,
    /// Wrap deformer (this mesh follows an influence mesh).
    #[serde(default)]
    pub wrap: Option<crate::deform::WrapBind>,
    /// Wire deformers (this mesh follows curves).
    #[serde(default)]
    pub wires: Vec<crate::deform::WireBind>,
    /// Surface construction history (revolve / loft / extrude / planar from curves).
    #[serde(default)]
    pub surf: Option<crate::nurbs::SurfaceHistory>,
    /// Cloth simulation (the simulated points replace this mesh's points).
    #[serde(default)]
    pub cloth: Option<crate::fx::Cloth>,
}

impl Node {
    pub fn local_matrix(&self) -> Mat4 {
        let q = if self.joint_orient == Vec3::ZERO { self.rotation_quat() } else { crate::rig::euler_quat(self.joint_orient) * self.rotation_quat() };
        Mat4::from_scale_rotation_translation(self.scale, q, self.translate)
    }

    pub fn rotation_quat(&self) -> Quat {
        let r = self.rotate * (std::f32::consts::PI / 180.0);
        Quat::from_euler(EulerRot::ZYX, r.z, r.y, r.x)
    }

    pub fn get(&self, c: Channel) -> f32 {
        match c {
            Channel::TranslateX => self.translate.x,
            Channel::TranslateY => self.translate.y,
            Channel::TranslateZ => self.translate.z,
            Channel::RotateX => self.rotate.x,
            Channel::RotateY => self.rotate.y,
            Channel::RotateZ => self.rotate.z,
            Channel::ScaleX => self.scale.x,
            Channel::ScaleY => self.scale.y,
            Channel::ScaleZ => self.scale.z,
            Channel::Visibility => {
                if self.visible { 1.0 } else { 0.0 }
            }
        }
    }

    pub fn set(&mut self, c: Channel, v: f32) {
        match c {
            Channel::TranslateX => self.translate.x = v,
            Channel::TranslateY => self.translate.y = v,
            Channel::TranslateZ => self.translate.z = v,
            Channel::RotateX => self.rotate.x = v,
            Channel::RotateY => self.rotate.y = v,
            Channel::RotateZ => self.rotate.z = v,
            Channel::ScaleX => self.scale.x = v,
            Channel::ScaleY => self.scale.y = v,
            Channel::ScaleZ => self.scale.z = v,
            Channel::Visibility => self.visible = v >= 0.5,
        }
    }

    pub fn mesh(&self) -> Option<&Mesh> {
        match &self.shape {
            Shape::Poly { mesh, .. } => Some(mesh),
            _ => None,
        }
    }

    /// Mutable mesh access. Bumps the revision and, if `drop_history`, removes construction history.
    pub fn mesh_mut(&mut self, drop_history: bool) -> Option<&mut Mesh> {
        if drop_history && matches!(self.shape, Shape::Poly { .. }) {
            self.surf = None;
        }
        match &mut self.shape {
            Shape::Poly { mesh, rev, history } => {
                *rev = next_rev();
                if drop_history {
                    *history = None;
                }
                Some(mesh)
            }
            _ => None,
        }
    }

    pub fn mesh_rev(&self) -> u64 {
        match &self.shape {
            Shape::Poly { rev, .. } => *rev,
            _ => 0,
        }
    }

    pub fn history(&self) -> Option<&Primitive> {
        match &self.shape {
            Shape::Poly { history, .. } => history.as_ref(),
            _ => None,
        }
    }

    /// Editable points: mesh vertices or lattice points (component mode).
    pub fn points(&self) -> Option<&Vec<Vec3>> {
        match &self.shape {
            Shape::Poly { mesh, .. } => Some(&mesh.positions),
            Shape::Lattice { points, .. } => Some(points),
            Shape::Curve { points, .. } => Some(points),
            _ => None,
        }
    }

    pub fn points_mut(&mut self) -> Option<&mut Vec<Vec3>> {
        if matches!(self.shape, Shape::Poly { .. }) {
            return self.mesh_mut(true).map(|m| &mut m.positions);
        }
        match &mut self.shape {
            Shape::Lattice { points, .. } | Shape::Curve { points, .. } => Some(points),
            _ => None,
        }
    }

    pub fn set_history(&mut self, prim: Primitive) {
        if let Shape::Poly { history, mesh, rev } = &mut self.shape {
            *mesh = prim.build();
            *history = Some(prim);
            *rev = next_rev();
        }
    }

    pub fn shape_name(&self) -> Option<String> {
        match self.shape {
            Shape::Group => None,
            Shape::Locator | Shape::Curve { .. } => Some(shape_name_for(&self.name)),
            Shape::Joint { .. } => None,
            Shape::IkHandle { .. } => None,
            Shape::ClusterHandle | Shape::Light(_) | Shape::Camera(_) | Shape::Lattice { .. } | Shape::Particles(_) | Shape::Field(_) | Shape::Hair(_) | Shape::Fluid(_) => Some(format!("{}Shape", self.name)),
            Shape::Poly { .. } => Some(shape_name_for(&self.name)),
        }
    }

    pub fn is_animated(&self) -> bool {
        self.anim.values().any(|c| !c.keys.is_empty())
    }
}

/// pCube1 -> pCubeShape1
pub fn shape_name_for(name: &str) -> String {
    let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = &name[base.len()..];
    format!("{base}Shape{digits}")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    /// Template: drawn as grey wireframe and unselectable.
    pub template: bool,
    pub color: [f32; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub nodes: Vec<Node>,
    pub next_id: u64,
    pub layers: Vec<Layer>,
    /// Animation range (outer) and playback range (inner).
    pub anim_start: f32,
    pub anim_end: f32,
    pub play_start: f32,
    pub play_end: f32,
    pub fps: f32,
    pub time: f32,
    #[serde(default)]
    pub materials: Vec<crate::look::Material>,
    /// Pose library.
    #[serde(default)]
    pub poses: Vec<crate::anim_ops::Pose>,
    /// Render settings and render camera name, saved with the scene for batch rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_settings: Option<crate::raytrace::RenderSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_camera: Option<String>,
    /// Frame particle simulations are taken from while the time is between frames (set by
    /// motion-blurred renders so both shutter ends see the same particles).
    #[serde(skip)]
    pub sim_anchor: Option<f32>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            nodes: Vec::new(),
            next_id: 1,
            layers: Vec::new(),
            anim_start: 1.0,
            anim_end: 200.0,
            play_start: 1.0,
            play_end: 120.0,
            fps: 24.0,
            time: 1.0,
            materials: Vec::new(),
            poses: Vec::new(),
            render_settings: None,
            render_camera: None,
            sim_anchor: None,
        }
    }
}

impl Scene {
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
    pub fn find_by_name(&self, name: &str) -> Option<NodeId> {
        self.nodes.iter().find(|n| n.name == name).map(|n| n.id)
    }

    pub fn alloc_id(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn name_exists(&self, name: &str) -> bool {
        self.nodes.iter().any(|n| n.name == name)
    }

    /// Maya-style unique naming: "pCube" -> pCube1, pCube2 ...; "pCube3" stays if free.
    pub fn unique_name(&self, wanted: &str) -> String {
        let base = wanted.trim_end_matches(|c: char| c.is_ascii_digit());
        let base = if base.is_empty() { "node" } else { base };
        if wanted != base && !self.name_exists(wanted) {
            return wanted.to_string();
        }
        (1..).map(|i| format!("{base}{i}")).find(|n| !self.name_exists(n)).unwrap()
    }

    pub fn add_node(&mut self, name: &str, shape: Shape, parent: Option<NodeId>) -> NodeId {
        let id = self.alloc_id();
        let name = self.unique_name(name);
        self.nodes.push(Node {
            id,
            name,
            parent,
            translate: Vec3::ZERO,
            rotate: Vec3::ZERO,
            scale: Vec3::ONE,
            visible: true,
            shape,
            color: [0.62, 0.62, 0.62],
            layer: None,
            anim: BTreeMap::new(),
            expanded: true,
            joint_orient: Vec3::ZERO,
            skin: None,
            constraints: Vec::new(),
            driven: Vec::new(),
            blend: None,
            clusters: Vec::new(),
            material: None,
            lattices: Vec::new(),
            wrap: None,
            wires: Vec::new(),
            surf: None,
            cloth: None,
        });
        id
    }

    pub fn add_primitive(&mut self, prim: Primitive) -> NodeId {
        let name = format!("p{}", prim.base_name());
        let mesh = prim.build();
        self.add_node(&name, Shape::Poly { history: Some(prim), mesh, rev: next_rev() }, None)
    }

    pub fn children(&self, id: Option<NodeId>) -> Vec<NodeId> {
        self.nodes.iter().filter(|n| n.parent == id).map(|n| n.id).collect()
    }

    /// The node plus all its descendants (node first).
    pub fn subtree(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![id];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i];
            out.extend(self.children(Some(cur)));
            i += 1;
        }
        out
    }

    pub fn is_ancestor(&self, ancestor: NodeId, mut id: NodeId) -> bool {
        while let Some(p) = self.node(id).and_then(|n| n.parent) {
            if p == ancestor {
                return true;
            }
            id = p;
        }
        false
    }

    pub fn world_matrix(&self, id: NodeId) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        let mut cur = Some(id);
        let mut guard = 0;
        while let Some(c) = cur {
            let Some(n) = self.node(c) else { break };
            m = n.local_matrix() * m;
            cur = n.parent;
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        m
    }

    pub fn parent_matrix(&self, id: NodeId) -> Mat4 {
        match self.node(id).and_then(|n| n.parent) {
            Some(p) => self.world_matrix(p),
            None => Mat4::IDENTITY,
        }
    }

    /// Effective visibility: own flag, ancestors and display layer.
    pub fn is_visible(&self, id: NodeId) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            let Some(n) = self.node(c) else { return false };
            if !n.visible {
                return false;
            }
            if let Some(l) = n.layer
                && self.layers.iter().any(|ly| ly.id == l && !ly.visible) {
                    return false;
                }
            cur = n.parent;
        }
        true
    }

    pub fn is_template(&self, id: NodeId) -> bool {
        self.node(id)
            .and_then(|n| n.layer)
            .is_some_and(|l| self.layers.iter().any(|ly| ly.id == l && ly.template))
    }

    pub fn delete(&mut self, ids: &[NodeId]) {
        let mut doomed = std::collections::HashSet::new();
        for &id in ids {
            if self.node(id).is_some() {
                doomed.extend(self.subtree(id));
            }
        }
        self.nodes.retain(|n| !doomed.contains(&n.id));
    }

    /// Re-parent keeping the world transform.
    pub fn reparent(&mut self, id: NodeId, new_parent: Option<NodeId>) -> bool {
        if let Some(p) = new_parent
            && (p == id || self.is_ancestor(id, p)) {
                return false;
            }
        let world = self.world_matrix(id);
        let pinv = new_parent.map(|p| self.world_matrix(p).inverse()).unwrap_or(Mat4::IDENTITY);
        let local = pinv * world;
        let (s, r, t) = local.to_scale_rotation_translation();
        let (z, y, x) = r.to_euler(EulerRot::ZYX);
        // Move node to the end so it appears last among its new siblings.
        let idx = self.nodes.iter().position(|n| n.id == id).unwrap();
        let mut n = self.nodes.remove(idx);
        n.parent = new_parent;
        n.translate = t;
        n.rotate = if n.joint_orient == Vec3::ZERO {
            Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI)
        } else {
            crate::rig::quat_euler(crate::rig::euler_quat(n.joint_orient).inverse() * r)
        };
        n.scale = s;
        self.nodes.push(n);
        true
    }

    /// Duplicate subtrees; returns new root ids.
    pub fn duplicate(&mut self, ids: &[NodeId]) -> Vec<NodeId> {
        let mut roots = Vec::new();
        for &id in ids {
            if self.node(id).is_none() {
                continue;
            }
            let tree = self.subtree(id);
            let mut map = std::collections::HashMap::new();
            for &old in &tree {
                let nid = self.alloc_id();
                map.insert(old, nid);
            }
            for &old in &tree {
                let mut n = self.node(old).unwrap().clone();
                n.id = map[&old];
                n.name = self.unique_name(&n.name);
                if old != id {
                    n.parent = n.parent.map(|p| map[&p]);
                }
                if let Shape::Poly { rev, .. } = &mut n.shape {
                    *rev = next_rev();
                }
                self.nodes.push(n);
            }
            roots.push(map[&id]);
        }
        roots
    }

    /// Apply animation curves at the current time.
    pub fn evaluate(&mut self) {
        let t = self.time;
        for n in &mut self.nodes {
            if let Some(b) = n.blend.as_mut() {
                for (i, c) in b.curves.iter().enumerate() {
                    if let (Some(c), Some(w)) = (c, b.weights.get_mut(i)) {
                        if let Some(v) = c.evaluate(t) {
                            *w = v;
                        }
                    }
                }
            }
            let vals: Vec<(Channel, f32)> =
                n.anim.iter().filter_map(|(c, curve)| curve.evaluate(t).map(|v| (*c, v))).collect();
            for (c, v) in vals {
                n.set(c, v);
            }
        }
    }

    pub fn set_key(&mut self, id: NodeId, channels: &[Channel]) {
        let t = self.time;
        if let Some(n) = self.node_mut(id) {
            for &c in channels {
                let v = n.get(c);
                let curve = n.anim.entry(c).or_default();
                if c == Channel::Visibility {
                    curve.tangent = Tangent::Stepped;
                }
                curve.set_key(t, v);
            }
        }
    }

    /// All key frames of the given nodes (sorted, unique).
    pub fn key_frames(&self, ids: &[NodeId]) -> Vec<f32> {
        let mut v: Vec<f32> = ids
            .iter()
            .filter_map(|id| self.node(*id))
            .flat_map(|n| n.anim.values().flat_map(|c| c.keys.iter().map(|k| k.frame)))
            .collect();
        v.sort_by(|a, b| a.total_cmp(b));
        v.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        v
    }
}

/// Cached triangulation for rendering & picking.
pub struct MeshCache {
    pub rev: u64,
    pub tri: TriMesh,
    /// Faces per vertex (built on first deformation).
    pub vert_faces: Option<Vec<Vec<u32>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_order_xyz() {
        let mut s = Scene::default();
        let id = s.add_node("g", Shape::Group, None);
        let n = s.node_mut(id).unwrap();
        n.rotate = Vec3::new(90.0, 0.0, 0.0);
        let p = n.local_matrix().transform_point3(Vec3::Y);
        assert!((p - Vec3::Z).length() < 1e-5, "{p}");
        // XYZ order: X applied first, then Y, then Z.
        n.rotate = Vec3::new(90.0, 90.0, 0.0);
        let p = n.local_matrix().transform_point3(Vec3::Y);
        // Rx(90): Y->Z ; Ry(90): Z->X
        assert!((p - Vec3::X).length() < 1e-5, "{p}");
    }

    #[test]
    fn naming() {
        let mut s = Scene::default();
        let a = s.add_primitive(Primitive::default_cube());
        let b = s.add_primitive(Primitive::default_cube());
        assert_eq!(s.node(a).unwrap().name, "pCube1");
        assert_eq!(s.node(b).unwrap().name, "pCube2");
        assert_eq!(shape_name_for("pCube2"), "pCubeShape2");
        let d = s.duplicate(&[a]);
        assert_eq!(s.node(d[0]).unwrap().name, "pCube3");
    }

    #[test]
    fn reparent_keeps_world() {
        let mut s = Scene::default();
        let a = s.add_node("a", Shape::Group, None);
        let b = s.add_node("b", Shape::Group, None);
        s.node_mut(a).unwrap().translate = Vec3::new(5.0, 0.0, 0.0);
        s.node_mut(a).unwrap().rotate = Vec3::new(0.0, 90.0, 0.0);
        s.node_mut(b).unwrap().translate = Vec3::new(1.0, 2.0, 3.0);
        let before = s.world_matrix(b).transform_point3(Vec3::ZERO);
        assert!(s.reparent(b, Some(a)));
        let after = s.world_matrix(b).transform_point3(Vec3::ZERO);
        assert!((before - after).length() < 1e-4);
        assert!(!s.reparent(a, Some(b)), "cycle must be refused");
    }

    #[test]
    fn curve_eval() {
        let mut c = Curve::default();
        c.set_key(1.0, 0.0);
        c.set_key(11.0, 10.0);
        assert_eq!(c.evaluate(0.0), Some(0.0));
        assert!((c.evaluate(6.0).unwrap() - 5.0).abs() < 1e-4);
        c.tangent = Tangent::Linear;
        assert!((c.evaluate(3.5).unwrap() - 2.5).abs() < 1e-4);
        c.tangent = Tangent::Stepped;
        assert_eq!(c.evaluate(10.9), Some(0.0));
    }

    #[test]
    fn save_load_roundtrip() {
        let mut s = Scene::default();
        let a = s.add_primitive(Primitive::default_sphere());
        s.time = 10.0;
        s.set_key(a, &Channel::ALL);
        let json = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&json).unwrap();
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.nodes[0].anim.len(), 10);
        assert_eq!(back.nodes[0].mesh().unwrap(), s.nodes[0].mesh().unwrap());
    }
}
