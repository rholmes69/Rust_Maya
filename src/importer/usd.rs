//! USD importer: `.usda`, `.usdc` (via openusd, re-emitted as text), `.usdz` (unpacked).
//!
//! A small `.usda` parser (prims, typed attributes, relationships, connections, time samples,
//! metadata, list ops, variant sets), a composer (subLayers, references, payloads, inherits,
//! specializes, variant selections; MaterialX `.mtlx` documents become material prims) and a
//! scene builder:
//! Xform hierarchy (translate / rotate* / orient / scale / transform ops, animated or not),
//! Mesh (with face-varying or vertex UVs, left-handed orientation), intrinsic primitives,
//! PointInstancer, BasisCurves, UsdPreviewSurface / MaterialX materials with texture maps,
//! UsdLux lights, cameras, and **UsdSkel** (Skeleton → joints, SkelAnimation → keys, joint
//! influences → skinCluster, BlendShape → blend-shape targets with weight curves).
//! Z-up layers and metersPerUnit are converted.

use crate::app::App;
use crate::look::{CameraShape, Light, LightKind, MatKind, Material};
use crate::mesh::Mesh;
use crate::rig::{BlendShape, Skin};
use crate::scene::{Channel, Curve, NodeId, Shape, Tangent};
use glam::{Mat4, Quat, Vec2, Vec3};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------------------ values

#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Num(f64),
    Str(String),
    Asset(String),
    Path(String),
    Tok(String),
    Tuple(Vec<V>),
    List(Vec<V>),
    Dict(Vec<(String, V)>),
    None,
}

impl V {
    fn f(&self) -> Option<f64> {
        match self {
            V::Num(x) => Some(*x),
            V::Tok(t) if t == "true" => Some(1.0),
            V::Tok(t) if t == "false" => Some(0.0),
            _ => None,
        }
    }
    /// All numbers inside (flattened tuples / lists).
    fn nums(&self) -> Vec<f64> {
        let mut out = Vec::new();
        fn walk(v: &V, out: &mut Vec<f64>) {
            match v {
                V::Num(x) => out.push(*x),
                V::Tuple(x) | V::List(x) => x.iter().for_each(|y| walk(y, out)),
                _ => {}
            }
        }
        walk(self, &mut out);
        out
    }
    fn text(&self) -> Option<&str> {
        match self {
            V::Str(s) | V::Tok(s) | V::Asset(s) | V::Path(s) => Some(s),
            _ => None,
        }
    }
    fn list(&self) -> &[V] {
        match self {
            V::List(x) | V::Tuple(x) => x,
            _ => &[],
        }
    }
    fn vec3s(&self) -> Vec<Vec3> {
        self.list().iter().map(|t| t.nums()).filter(|n| n.len() >= 3).map(|n| Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32)).collect()
    }
    fn vec2s(&self) -> Vec<Vec2> {
        self.list().iter().map(|t| t.nums()).filter(|n| n.len() >= 2).map(|n| Vec2::new(n[0] as f32, n[1] as f32)).collect()
    }
    fn ints(&self) -> Vec<i64> {
        self.nums().iter().map(|x| *x as i64).collect()
    }
    fn strs(&self) -> Vec<String> {
        self.list().iter().filter_map(|v| v.text().map(String::from)).collect()
    }
    fn mat4(&self) -> Option<Mat4> {
        let n = self.nums();
        (n.len() == 16).then(|| Mat4::from_cols_array(&std::array::from_fn(|i| n[i] as f32)))
    }
    fn mat4s(&self) -> Vec<Mat4> {
        self.list().iter().filter_map(|m| m.mat4()).collect()
    }
    fn quats(&self) -> Vec<Quat> {
        // USD quaternions are (real, i, j, k).
        self.list().iter().map(|t| t.nums()).filter(|n| n.len() >= 4).map(|n| Quat::from_xyzw(n[1] as f32, n[2] as f32, n[3] as f32, n[0] as f32).normalize()).collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Prop {
    pub ty: String,
    pub name: String,
    pub value: Option<V>,
    pub samples: Vec<(f64, V)>,
    pub connect: Vec<String>,
    pub targets: Vec<String>,
    pub meta: Vec<(String, V)>,
    #[allow(dead_code)]
    pub is_rel: bool,
}

impl Prop {
    fn meta(&self, k: &str) -> Option<&V> {
        self.meta.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    /// Value at a time code: interpolated time samples, else the default.
    fn at(&self, t: Option<f64>) -> Option<V> {
        if self.samples.is_empty() || t.is_none() {
            return self.value.clone().or_else(|| self.samples.first().map(|s| s.1.clone()));
        }
        let t = t.unwrap();
        let s = &self.samples;
        if t <= s[0].0 {
            return Some(s[0].1.clone());
        }
        if t >= s[s.len() - 1].0 {
            return Some(s[s.len() - 1].1.clone());
        }
        let i = s.windows(2).position(|w| t >= w[0].0 && t <= w[1].0).unwrap_or(0);
        let (a, b) = (&s[i], &s[i + 1]);
        let k = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
        Some(lerp_v(&a.1, &b.1, k))
    }
}

fn lerp_v(a: &V, b: &V, k: f64) -> V {
    match (a, b) {
        (V::Num(x), V::Num(y)) => V::Num(x + (y - x) * k),
        (V::Tuple(x), V::Tuple(y)) if x.len() == y.len() => V::Tuple(x.iter().zip(y).map(|(p, q)| lerp_v(p, q, k)).collect()),
        (V::List(x), V::List(y)) if x.len() == y.len() => V::List(x.iter().zip(y).map(|(p, q)| lerp_v(p, q, k)).collect()),
        _ => if k < 0.5 { a.clone() } else { b.clone() },
    }
}

#[derive(Clone, Debug, Default)]
pub struct Prim {
    pub spec: String,
    pub ty: String,
    pub name: String,
    pub path: String,
    pub meta: Vec<(String, V)>,
    pub props: Vec<Prop>,
    pub children: Vec<Prim>,
    /// variantSet name → (variant name → body opinions).
    pub variants: Vec<(String, Vec<(String, Prim)>)>,
}

impl Prim {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
    fn val(&self, name: &str) -> Option<V> {
        self.prop(name).and_then(|p| p.at(None))
    }
    fn num(&self, name: &str) -> Option<f64> {
        self.val(name).and_then(|v| v.f().or_else(|| v.nums().first().copied()))
    }
    fn text(&self, name: &str) -> Option<String> {
        self.val(name).and_then(|v| v.text().map(String::from))
    }
    fn meta(&self, k: &str) -> Option<&V> {
        self.meta.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v)
    }
    fn api_schemas(&self) -> Vec<String> {
        self.meta.iter().filter(|(n, _)| n == "apiSchemas").flat_map(|(_, v)| v.strs()).collect()
    }
}

#[derive(Debug, Default)]
pub struct Layer {
    pub meta: Vec<(String, V)>,
    pub prims: Vec<Prim>,
}

impl Layer {
    fn meta_num(&self, k: &str) -> Option<f64> {
        self.meta.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.f())
    }
    fn meta_text(&self, k: &str) -> Option<String> {
        self.meta.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.text().map(String::from))
    }
}

// ------------------------------------------------------------------------------ lexer

#[derive(Clone, Debug, PartialEq)]
enum T {
    P(char),
    Str(String),
    Asset(String),
    Path(String),
    Num(f64),
    Id(String),
}

fn lex(src: &str) -> Result<Vec<T>, String> {
    let c: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    // Skip the "#usda 1.0" header line.
    if src.starts_with("#usda") {
        while i < c.len() && c[i] != '\n' {
            i += 1;
        }
    }
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() || ch == ';' {
            i += 1;
        } else if ch == '#' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '"' || ch == '\'' {
            let triple = i + 2 < c.len() && c[i + 1] == ch && c[i + 2] == ch;
            let mut s = String::new();
            if triple {
                i += 3;
                while i + 2 < c.len() && !(c[i] == ch && c[i + 1] == ch && c[i + 2] == ch) {
                    s.push(c[i]);
                    i += 1;
                }
                i += 3;
            } else {
                i += 1;
                while i < c.len() && c[i] != ch {
                    if c[i] == '\\' && i + 1 < c.len() {
                        i += 1;
                        s.push(match c[i] {
                            'n' => '\n',
                            't' => '\t',
                            o => o,
                        });
                    } else {
                        s.push(c[i]);
                    }
                    i += 1;
                }
                i += 1;
            }
            out.push(T::Str(s));
        } else if ch == '@' {
            let triple = i + 2 < c.len() && c[i + 1] == '@' && c[i + 2] == '@';
            let mut s = String::new();
            if triple {
                i += 3;
                while i + 2 < c.len() && !(c[i] == '@' && c[i + 1] == '@' && c[i + 2] == '@') {
                    s.push(c[i]);
                    i += 1;
                }
                i += 3;
            } else {
                i += 1;
                while i < c.len() && c[i] != '@' {
                    s.push(c[i]);
                    i += 1;
                }
                i += 1;
            }
            out.push(T::Asset(s));
        } else if ch == '<' {
            let mut s = String::new();
            i += 1;
            while i < c.len() && c[i] != '>' {
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(T::Path(s));
        } else if "()[]{}=,:".contains(ch) {
            out.push(T::P(ch));
            i += 1;
        } else if ch.is_ascii_digit() || ((ch == '-' || ch == '+' || ch == '.') && i + 1 < c.len() && (c[i + 1].is_ascii_digit() || c[i + 1] == '.')) {
            let st = i;
            i += 1;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.' || c[i] == 'e' || c[i] == 'E' || ((c[i] == '-' || c[i] == '+') && (c[i - 1] == 'e' || c[i - 1] == 'E'))) {
                i += 1;
            }
            let s: String = c[st..i].iter().collect();
            out.push(T::Num(s.parse().map_err(|_| format!("bad number '{s}'"))?));
        } else if ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '!' {
            let st = i;
            while i < c.len() && (c[i].is_alphanumeric() || "_:.!-|".contains(c[i])) {
                // ':' followed by whitespace is the time-sample separator, not a namespace.
                if c[i] == ':' && (i + 1 >= c.len() || c[i + 1].is_whitespace()) {
                    break;
                }
                i += 1;
            }
            let s: String = c[st..i].iter().collect();
            match s.as_str() {
                "inf" => out.push(T::Num(f64::INFINITY)),
                "-inf" => out.push(T::Num(f64::NEG_INFINITY)),
                "nan" => out.push(T::Num(f64::NAN)),
                _ => out.push(T::Id(s)),
            }
        } else {
            return Err(format!("unexpected character '{ch}'"));
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------------------ parser

struct Parser {
    t: Vec<T>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&T> {
        self.t.get(self.i)
    }
    fn peek2(&self) -> Option<&T> {
        self.t.get(self.i + 1)
    }
    fn next(&mut self) -> Option<T> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn eat(&mut self, p: char) -> bool {
        if self.peek() == Some(&T::P(p)) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, p: char) -> Result<(), String> {
        if self.eat(p) { Ok(()) } else { Err(format!("expected '{p}' near token {} ({:?})", self.i, self.peek())) }
    }
    fn is_id(&self, s: &str) -> bool {
        matches!(self.peek(), Some(T::Id(x)) if x == s)
    }

    fn value(&mut self) -> Result<V, String> {
        match self.next() {
            Some(T::Num(x)) => Ok(V::Num(x)),
            Some(T::Str(s)) => Ok(V::Str(s)),
            Some(T::Asset(s)) => {
                // Reference/payload: @file@</Prim> and an optional layer offset ( offset = 1 ).
                let mut parts = vec![V::Asset(s)];
                if let Some(T::Path(p)) = self.peek().cloned() {
                    self.i += 1;
                    parts.push(V::Path(p));
                }
                if matches!(self.peek(), Some(T::P('('))) && matches!(self.peek2(), Some(T::Id(k)) if k == "offset" || k == "scale") {
                    self.meta()?;
                }
                Ok(if parts.len() == 1 { parts.pop().unwrap() } else { V::Tuple(parts) })
            }
            Some(T::Path(s)) => {
                if matches!(self.peek(), Some(T::P('('))) && matches!(self.peek2(), Some(T::Id(k)) if k == "offset" || k == "scale") {
                    self.meta()?;
                }
                Ok(V::Path(s))
            }
            Some(T::Id(s)) if s == "None" => Ok(V::None),
            Some(T::Id(s)) => Ok(V::Tok(s)),
            Some(T::P('(')) => {
                let mut v = Vec::new();
                while !self.eat(')') {
                    v.push(self.value()?);
                    self.eat(',');
                }
                Ok(V::Tuple(v))
            }
            Some(T::P('[')) => {
                let mut v = Vec::new();
                while !self.eat(']') {
                    v.push(self.value()?);
                    self.eat(',');
                }
                Ok(V::List(v))
            }
            Some(T::P('{')) => {
                // Time samples ({ 1: v, 2: v }) parse as a dict with numeric keys.
                let mut d = Vec::new();
                while !self.eat('}') {
                    if let (Some(T::Num(k)), Some(T::P(':'))) = (self.peek().cloned(), self.peek2().cloned()) {
                        self.i += 2;
                        d.push((format!("{k}"), self.value()?));
                    } else {
                        // Typed dictionary entry: [type] name = value  (or dictionary name = {..}).
                        let a = self.next();
                        let name = if matches!(self.peek(), Some(T::P('='))) {
                            a
                        } else {
                            if self.eat('[') {
                                self.expect(']')?;
                            }
                            self.next()
                        };
                        let key = match name {
                            Some(T::Id(s)) | Some(T::Str(s)) => s,
                            other => return Err(format!("bad dictionary key {other:?}")),
                        };
                        self.expect('=')?;
                        d.push((key, self.value()?));
                    }
                    self.eat(',');
                }
                Ok(V::Dict(d))
            }
            other => Err(format!("unexpected {other:?} in value")),
        }
    }

    /// `( key = value ... )` metadata; list-op keywords are dropped, docs kept as "doc".
    fn meta(&mut self) -> Result<Vec<(String, V)>, String> {
        let mut m = Vec::new();
        self.expect('(')?;
        while !self.eat(')') {
            match self.peek().cloned() {
                Some(T::Str(s)) => {
                    self.i += 1;
                    m.push(("doc".into(), V::Str(s)));
                }
                Some(T::Id(k)) => {
                    self.i += 1;
                    let k0 = k.clone();
                    let key = if matches!(k.as_str(), "prepend" | "append" | "add" | "delete" | "reorder") {
                        match self.next() {
                            Some(T::Id(k2)) => k2,
                            o => return Err(format!("bad list op {o:?}")),
                        }
                    } else {
                        k
                    };
                    if self.eat('=') {
                        let v = self.value()?;
                        if k0 != "delete" {
                            m.push((key, v));
                        }
                    }
                }
                Some(T::P(',')) => {
                    self.i += 1;
                }
                other => return Err(format!("unexpected {other:?} in metadata")),
            }
        }
        Ok(m)
    }

    fn prim(&mut self, parent_path: &str) -> Result<Prim, String> {
        let spec = match self.next() {
            Some(T::Id(s)) => s,
            o => return Err(format!("expected def/over/class, got {o:?}")),
        };
        let mut ty = String::new();
        if let Some(T::Id(t)) = self.peek().cloned() {
            self.i += 1;
            ty = t;
        }
        let name = match self.next() {
            Some(T::Str(s)) => s,
            o => return Err(format!("expected prim name, got {o:?}")),
        };
        let path = format!("{parent_path}/{name}");
        let meta = if matches!(self.peek(), Some(T::P('('))) { self.meta()? } else { Vec::new() };
        let mut prim = Prim { spec, ty, name, path: path.clone(), meta, ..Default::default() };
        self.body(&mut prim)?;
        Ok(prim)
    }

    /// `{ properties, child prims, variantSets }` of a prim (or of a variant).
    fn body(&mut self, prim: &mut Prim) -> Result<(), String> {
        let path = prim.path.clone();
        self.expect('{')?;
        while !self.eat('}') {
            if self.is_id("def") || self.is_id("over") || self.is_id("class") {
                let c = self.prim(&path)?;
                prim.children.push(c);
            } else if self.is_id("reorder") && matches!(self.peek2(), Some(T::Id(k)) if k == "properties" || k == "nameChildren") {
                self.i += 2;
                self.expect('=')?;
                self.value()?;
            } else if self.is_id("variantSet") {
                // variantSet "set" = { "a" (meta) { body } "b" { body } }
                self.i += 1;
                let set = match self.next() {
                    Some(T::Str(s)) => s,
                    o => return Err(format!("bad variantSet name {o:?} in {path}")),
                };
                self.expect('=')?;
                self.expect('{')?;
                let mut vars = Vec::new();
                while !self.eat('}') {
                    let vname = match self.next() {
                        Some(T::Str(s)) => s,
                        o => return Err(format!("bad variant name {o:?} in {path}")),
                    };
                    let meta = if matches!(self.peek(), Some(T::P('('))) { self.meta()? } else { Vec::new() };
                    let mut v = Prim { spec: "over".into(), name: prim.name.clone(), path: path.clone(), meta, ..Default::default() };
                    self.body(&mut v)?;
                    vars.push((vname, v));
                }
                prim.variants.push((set, vars));
            } else {
                self.property(prim)?;
            }
        }
        Ok(())
    }

    fn property(&mut self, prim: &mut Prim) -> Result<(), String> {
        let mut words: Vec<String> = Vec::new();
        // Qualifiers, type and name.
        loop {
            match self.peek().cloned() {
                Some(T::Id(w)) if matches!(w.as_str(), "custom" | "uniform" | "varying" | "config" | "prepend" | "append" | "add" | "delete" | "reorder") => {
                    self.i += 1;
                }
                Some(T::Id(w)) => {
                    self.i += 1;
                    let mut w = w;
                    if self.peek() == Some(&T::P('[')) && self.peek2() == Some(&T::P(']')) {
                        self.i += 2;
                        w.push_str("[]");
                    }
                    words.push(w);
                    if words.len() == 2 {
                        break;
                    }
                }
                other => return Err(format!("bad property near {other:?} in {}", prim.path)),
            }
        }
        let (ty, full) = (words[0].clone(), words[1].clone());
        let is_rel = ty == "rel";
        let (name, suffix) = match full.rsplit_once('.') {
            Some((n, s)) if matches!(s, "connect" | "timeSamples" | "spline") => (n.to_string(), s.to_string()),
            _ => (full.clone(), String::new()),
        };
        let idx = match prim.props.iter().position(|p| p.name == name) {
            Some(i) => i,
            None => {
                prim.props.push(Prop { ty: ty.clone(), name: name.clone(), is_rel, ..Default::default() });
                prim.props.len() - 1
            }
        };
        if self.eat('=') {
            let v = self.value()?;
            let p = &mut prim.props[idx];
            match suffix.as_str() {
                "connect" => p.connect = paths_of(&v),
                "timeSamples" => {
                    if let V::Dict(d) = v {
                        p.samples = d.into_iter().filter_map(|(k, v)| k.parse::<f64>().ok().map(|t| (t, v))).collect();
                        p.samples.sort_by(|a, b| a.0.total_cmp(&b.0));
                    }
                }
                "spline" => {}
                _ if is_rel => p.targets = paths_of(&v),
                _ => p.value = Some(v),
            }
        }
        if matches!(self.peek(), Some(T::P('('))) {
            let m = self.meta()?;
            prim.props[idx].meta.extend(m);
        }
        Ok(())
    }
}

fn paths_of(v: &V) -> Vec<String> {
    match v {
        V::Path(p) => vec![p.clone()],
        V::List(l) => l.iter().filter_map(|x| if let V::Path(p) = x { Some(p.clone()) } else { None }).collect(),
        _ => Vec::new(),
    }
}

pub fn parse(src: &str) -> Result<Layer, String> {
    if !src.trim_start().starts_with("#usda") {
        return Err("not a USD ASCII layer (missing '#usda' header)".into());
    }
    let mut p = Parser { t: lex(src.trim_start())?, i: 0 };
    let mut layer = Layer::default();
    if matches!(p.peek(), Some(T::P('('))) {
        layer.meta = p.meta()?;
    }
    while p.peek().is_some() {
        let pr = p.prim("")?;
        layer.prims.push(pr);
    }
    Ok(layer)
}


/// Text of a USD layer: `.usda` as-is, binary crate files (`.usdc`, or `.usd` holding one)
/// converted to `.usda` text with openusd.
pub fn layer_text(path: &Path, bytes: Vec<u8>) -> Result<String, String> {
    if bytes.starts_with(b"PXR-USDC") {
        let data = openusd::usdc::CrateData::open(bytes, true).map_err(|e| format!("{}: can't read binary USD: {e}", path.display()))?;
        return openusd::usda::TextWriter::write_to_string(&data).map_err(|e| format!("{}: can't convert binary USD: {e}", path.display()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Unpack a `.usdz` package into a temp folder (textures and sublayers keep their relative
/// paths) and return the root layer's path.
pub fn unpack_usdz(path: &Path) -> Result<PathBuf, String> {
    use std::hash::{Hash, Hasher};
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{}: not a valid .usdz: {e}", path.display()))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok()).hash(&mut h);
    let dir = std::env::temp_dir().join("rust_maya_usdz").join(format!("{:016x}", h.finish()));
    let mut root = None;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).map_err(|e| format!("{}: {e}", path.display()))?;
        let Some(rel) = e.enclosed_name() else { continue };
        let out = dir.join(&rel);
        if e.is_dir() {
            continue;
        }
        if root.is_none() && rel.extension().is_some_and(|x| x == "usda" || x == "usdc" || x == "usd") {
            root = Some(out.clone());
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut e, &mut w).map_err(|e| e.to_string())?;
    }
    root.ok_or_else(|| format!("{}: package has no USD layer", path.display()))
}


// ------------------------------------------------------------------------------ MaterialX

/// A minimal XML element tree (enough for MaterialX documents).
#[derive(Debug, Default, Clone)]
struct Xml {
    tag: String,
    attrs: Vec<(String, String)>,
    kids: Vec<Xml>,
}

impl Xml {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

fn parse_xml(src: &str) -> Result<Xml, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Xml> = vec![Xml { tag: "#doc".into(), ..Default::default() }];
    let unescape = |s: &str| s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if src[i..].starts_with("<!--") {
            i = src[i..].find("-->").map(|k| i + k + 3).ok_or("unterminated XML comment")?;
            continue;
        }
        if src[i..].starts_with("<?") || src[i..].starts_with("<!") {
            i = src[i..].find('>').map(|k| i + k + 1).ok_or("bad XML declaration")?;
            continue;
        }
        let end = {
            // Find the closing '>' outside quotes.
            let mut q = None;
            let mut k = i + 1;
            while k < b.len() {
                match (q, b[k]) {
                    (None, b'"') | (None, b'\'') => q = Some(b[k]),
                    (Some(c), x) if c == x => q = None,
                    (None, b'>') => break,
                    _ => {}
                }
                k += 1;
            }
            k
        };
        if end >= b.len() {
            return Err("unterminated XML tag".into());
        }
        let inner = &src[i + 1..end];
        i = end + 1;
        if let Some(name) = inner.strip_prefix('/') {
            let el = stack.pop().ok_or("unbalanced XML")?;
            if el.tag != name.trim() {
                return Err(format!("XML: </{}> closes <{}>", name.trim(), el.tag));
            }
            stack.last_mut().ok_or("unbalanced XML")?.kids.push(el);
            continue;
        }
        let self_close = inner.trim_end().ends_with('/');
        let inner = inner.trim_end().trim_end_matches('/');
        let tag_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
        let mut el = Xml { tag: inner[..tag_end].to_string(), ..Default::default() };
        let mut rest = &inner[tag_end..];
        while let Some(eq) = rest.find('=') {
            let key = rest[..eq].trim().to_string();
            let after = rest[eq + 1..].trim_start();
            let Some(qc) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
            let Some(close) = after[1..].find(qc) else { break };
            el.attrs.push((key, unescape(&after[1..1 + close])));
            rest = &after[close + 2..];
        }
        if self_close {
            stack.last_mut().ok_or("unbalanced XML")?.kids.push(el);
        } else {
            stack.push(el);
        }
    }
    let mut doc = stack.pop().ok_or("empty XML")?;
    if !stack.is_empty() {
        return Err("unterminated XML element".into());
    }
    doc.kids.pop().filter(|r| r.tag == "materialx").ok_or_else(|| "not a MaterialX document".into())
}

/// A MaterialX document as a USD-style layer: `/MaterialX/Materials/<material>` prims, each
/// holding its surface shader and (copies of) the node graphs it reads, so a reference to a
/// material carries its whole network. Texture paths are made absolute.
fn mtlx_layer(path: &Path, src: &str) -> Result<Layer, String> {
    let root = parse_xml(src).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let prefix = root.attr("fileprefix").unwrap_or("").to_string();
    let by_name = |n: &str| root.kids.iter().find(|k| k.attr("name") == Some(n));
    let value = |s: &str| -> V {
        let n: Vec<V> = s.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).map(V::Num).collect();
        match n.len() {
            0 => V::Str(s.to_string()),
            1 => n.into_iter().next().unwrap(),
            _ => V::Tuple(n),
        }
    };
    // One node (shader / image / nodegraph-internal node) → a Shader prim under `parent`.
    fn node_prim(el: &Xml, parent: &str, scope: &str, prefix: &str, dir: &Path, value: &dyn Fn(&str) -> V) -> Prim {
        let name = el.attr("name").unwrap_or("node").to_string();
        let path = format!("{parent}/{name}");
        let mut p = Prim { spec: "def".into(), ty: "Shader".into(), name, path, ..Default::default() };
        p.props.push(Prop { ty: "token".into(), name: "info:id".into(), value: Some(V::Tok(format!("ND_{}", el.tag))), ..Default::default() });
        for inp in el.kids.iter().filter(|k| k.tag == "input") {
            let Some(n) = inp.attr("name") else { continue };
            let mut prop = Prop { ty: inp.attr("type").unwrap_or("float").into(), name: format!("inputs:{n}"), ..Default::default() };
            if let Some(ng) = inp.attr("nodegraph") {
                prop.connect = vec![format!("{scope}/{ng}.outputs:{}", inp.attr("output").unwrap_or("out"))];
            } else if let Some(nn) = inp.attr("nodename") {
                prop.connect = vec![format!("{parent}/{nn}.outputs:out")];
            } else if let Some(v) = inp.attr("value") {
                prop.value = Some(if inp.attr("type") == Some("filename") {
                    let f = format!("{prefix}{v}");
                    V::Asset(resolve_asset(dir, &f).to_string_lossy().replace('\\', "/"))
                } else {
                    value(v)
                });
            }
            p.props.push(prop);
        }
        p
    }
    let mut mats = Prim { spec: "def".into(), ty: "Scope".into(), name: "Materials".into(), path: "/MaterialX/Materials".into(), ..Default::default() };
    for m in root.kids.iter().filter(|k| k.tag == "surfacematerial" || (k.tag == "material")) {
        let Some(mname) = m.attr("name") else { continue };
        let mpath = format!("/MaterialX/Materials/{mname}");
        let mut mp = Prim { spec: "def".into(), ty: "Material".into(), name: mname.into(), path: mpath.clone(), ..Default::default() };
        let Some(sname) = m.kids.iter().find(|k| k.attr("name") == Some("surfaceshader")).and_then(|k| k.attr("nodename")) else { continue };
        let Some(sh) = by_name(sname) else { continue };
        mp.props.push(Prop { ty: "token".into(), name: "outputs:mtlx:surface".into(), connect: vec![format!("{mpath}/{sname}.outputs:out")], ..Default::default() });
        mp.children.push(node_prim(sh, &mpath, &mpath, &prefix, &dir, &value));
        // Node graphs the shader reads (one level; graphs reading graphs are rare).
        let graphs: BTreeSet<&str> = sh.kids.iter().filter_map(|k| k.attr("nodegraph")).collect();
        for g in graphs {
            let Some(ng) = by_name(g) else { continue };
            let gpath = format!("{mpath}/{g}");
            let gprefix = ng.attr("fileprefix").map(String::from).unwrap_or_else(|| prefix.clone());
            let mut gp = Prim { spec: "def".into(), ty: "NodeGraph".into(), name: g.into(), path: gpath.clone(), ..Default::default() };
            for k in &ng.kids {
                if k.tag == "output" {
                    if let (Some(on), Some(nn)) = (k.attr("name"), k.attr("nodename")) {
                        gp.props.push(Prop { ty: k.attr("type").unwrap_or("color3").into(), name: format!("outputs:{on}"), connect: vec![format!("{gpath}/{nn}.outputs:out")], ..Default::default() });
                    }
                } else if k.attr("name").is_some() {
                    gp.children.push(node_prim(k, &gpath, &mpath, &gprefix, &dir, &value));
                }
            }
            mp.children.push(gp);
        }
        mats.children.push(mp);
    }
    let top = Prim { spec: "def".into(), ty: "Scope".into(), name: "MaterialX".into(), path: "/MaterialX".into(), children: vec![mats], ..Default::default() };
    Ok(Layer { meta: vec![("defaultPrim".into(), V::Str("MaterialX".into()))], prims: vec![top] })
}

// ------------------------------------------------------------------------------ composition

/// Resolves the composition arcs our importer understands — subLayers, references, payloads,
/// inherits, specializes and selected variants — into one flattened prim tree (local opinions
/// are strongest; the rest follow USD's LIVRPS order).
struct Composer {
    cache: HashMap<PathBuf, Layer>,
    loading: Vec<PathBuf>,
    notes: Vec<String>,
}

fn resolve_asset(dir: &Path, a: &str) -> PathBuf {
    let p = Path::new(a);
    if p.is_absolute() { p.to_path_buf() } else { dir.join(a.trim_start_matches("./")) }
}

/// Make every asset path in a layer absolute (relative to the layer's own folder), so
/// opinions stay valid after being composed into another layer.
fn absolutize(p: &mut Prim, dir: &Path) {
    fn fix(v: &mut V, dir: &Path) {
        match v {
            V::Asset(a) if !a.is_empty() && !a.contains("://") => *a = resolve_asset(dir, a).to_string_lossy().replace('\\', "/"),
            V::Tuple(l) | V::List(l) => l.iter_mut().for_each(|x| fix(x, dir)),
            V::Dict(d) => d.iter_mut().for_each(|(_, x)| fix(x, dir)),
            _ => {}
        }
    }
    for (_, v) in &mut p.meta {
        fix(v, dir);
    }
    for pr in &mut p.props {
        if let Some(v) = &mut pr.value {
            fix(v, dir);
        }
        for (_, v) in &mut pr.samples {
            fix(v, dir);
        }
    }
    for c in &mut p.children {
        absolutize(c, dir);
    }
    for (_, vs) in &mut p.variants {
        for (_, b) in vs {
            absolutize(b, dir);
        }
    }
}

/// Rewrite prim paths (and relationship / connection targets) under `from` to `to`.
fn repath(p: &mut Prim, from: &str, to: &str) {
    let map = |s: &str| -> String {
        if s == from {
            to.to_string()
        } else if let Some(rest) = s.strip_prefix(from).filter(|r| r.starts_with('/') || r.starts_with('.')) {
            format!("{to}{rest}")
        } else {
            s.to_string()
        }
    };
    p.path = map(&p.path);
    for pr in &mut p.props {
        pr.targets = pr.targets.iter().map(|t| map(t)).collect();
        pr.connect = pr.connect.iter().map(|t| map(t)).collect();
    }
    for c in &mut p.children {
        repath(c, from, to);
    }
    for (_, vs) in &mut p.variants {
        for (_, b) in vs {
            repath(b, from, to);
        }
    }
}

const ARC_KEYS: [&str; 4] = ["inherits", "references", "payload", "specializes"];

/// Merge weaker opinions into `strong` (strong wins property by property).
fn merge_prim(strong: &mut Prim, weak: Prim) {
    if strong.ty.is_empty() {
        strong.ty = weak.ty;
    }
    if strong.spec == "over" && weak.spec != "over" {
        strong.spec = weak.spec;
    }
    for (k, v) in weak.meta {
        if ARC_KEYS.contains(&k.as_str()) {
            // Arcs from weaker layers still apply (appended).
            strong.meta.insert(0, (k, v));
        } else if k == "variants"
            && let V::Dict(wd) = v
        {
            // Variant selections merge entry by entry (nested variant sets add their own).
            match strong.meta.iter_mut().find(|(n, _)| n == "variants") {
                Some((_, V::Dict(sd))) => {
                    for (wk, wv) in wd {
                        if !sd.iter().any(|(n, _)| *n == wk) {
                            sd.push((wk, wv));
                        }
                    }
                }
                _ => strong.meta.push((k, V::Dict(wd))),
            }
        } else if !strong.meta.iter().any(|(n, _)| *n == k) {
            strong.meta.push((k, v));
        }
    }
    for wp in weak.props {
        match strong.props.iter_mut().find(|p| p.name == wp.name) {
            Some(sp) => {
                if sp.value.is_none() && sp.samples.is_empty() {
                    sp.value = wp.value;
                    sp.samples = wp.samples;
                }
                if sp.connect.is_empty() {
                    sp.connect = wp.connect;
                }
                if sp.targets.is_empty() {
                    sp.targets = wp.targets;
                }
                for (k, v) in wp.meta {
                    if !sp.meta.iter().any(|(n, _)| *n == k) {
                        sp.meta.push((k, v));
                    }
                }
            }
            None => strong.props.push(wp),
        }
    }
    merge_children(&mut strong.children, weak.children);
    // Same-named variant sets combine; matching variants merge their opinions.
    for (set, vs) in weak.variants {
        match strong.variants.iter_mut().find(|(s, _)| *s == set) {
            Some((_, svs)) => {
                for (vn, body) in vs {
                    match svs.iter_mut().find(|(n, _)| *n == vn) {
                        Some((_, sb)) => merge_prim(sb, body),
                        None => svs.push((vn, body)),
                    }
                }
            }
            None => strong.variants.push((set, vs)),
        }
    }
}

fn merge_children(strong: &mut Vec<Prim>, weak: Vec<Prim>) {
    for w in weak {
        match strong.iter_mut().find(|c| c.name == w.name) {
            Some(s) => merge_prim(s, w),
            None => strong.push(w),
        }
    }
}

fn find_prim<'a>(prims: &'a [Prim], path: &str) -> Option<&'a Prim> {
    let mut parts = path.trim_start_matches('/').split('/').filter(|s| !s.is_empty());
    let first = parts.next()?;
    let mut cur = prims.iter().find(|p| p.name == first)?;
    for n in parts {
        cur = cur.children.iter().find(|p| p.name == n)?;
    }
    Some(cur)
}

/// (asset, prim path) items of an arc's metadata value.
fn arc_items(v: &V) -> Vec<(Option<String>, Option<String>)> {
    match v {
        V::Asset(a) => vec![(Some(a.clone()), None)],
        V::Path(p) => vec![(None, Some(p.clone()))],
        V::Tuple(t) if matches!(t.first(), Some(V::Asset(_))) => {
            let a = if let Some(V::Asset(a)) = t.first() { Some(a.clone()) } else { None };
            let p = if let Some(V::Path(p)) = t.get(1) { Some(p.clone()) } else { None };
            vec![(a, p)]
        }
        V::List(l) | V::Tuple(l) => l.iter().flat_map(arc_items).collect(),
        _ => Vec::new(),
    }
}

impl Composer {
    fn new() -> Self {
        Composer { cache: HashMap::new(), loading: Vec::new(), notes: Vec::new() }
    }

    /// Read, parse, absolutize and compose a layer file (cached).
    fn load(&mut self, path: &Path) -> Result<Layer, String> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(l) = self.cache.get(&key) {
            return Ok(Layer { meta: l.meta.clone(), prims: l.prims.clone() });
        }
        if self.loading.contains(&key) || self.loading.len() > 32 {
            return Err(format!("{}: recursive or too-deep layer composition", path.display()));
        }
        if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("mtlx")) {
            let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let l = mtlx_layer(path, &src)?;
            self.cache.insert(key, Layer { meta: l.meta.clone(), prims: l.prims.clone() });
            return Ok(l);
        }
        let path = &if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("usdz")) { unpack_usdz(path)? } else { path.to_path_buf() };
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut layer = parse(&layer_text(path, bytes)?).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for p in &mut layer.prims {
            absolutize(p, &dir);
        }
        self.loading.push(key.clone());
        self.compose(&mut layer, &dir);
        self.loading.pop();
        self.cache.insert(key, Layer { meta: layer.meta.clone(), prims: layer.prims.clone() });
        Ok(layer)
    }

    /// Compose a parsed layer in place: subLayers, then per-prim arcs.
    fn compose(&mut self, layer: &mut Layer, dir: &Path) {
        let subs: Vec<String> = layer
            .meta
            .iter()
            .filter(|(k, _)| k == "subLayers")
            .flat_map(|(_, v)| arc_items(v))
            .filter_map(|(a, _)| a)
            .collect();
        for s in subs {
            match self.load(&resolve_asset(dir, &s)) {
                Ok(sub) => {
                    for (k, v) in sub.meta {
                        if matches!(k.as_str(), "upAxis" | "metersPerUnit" | "timeCodesPerSecond" | "framesPerSecond" | "startTimeCode" | "endTimeCode" | "defaultPrim") && !layer.meta.iter().any(|(n, _)| *n == k) {
                            layer.meta.push((k, v));
                        }
                    }
                    merge_children(&mut layer.prims, sub.prims);
                }
                Err(e) => self.notes.push(e),
            }
        }
        layer.meta.retain(|(k, _)| k != "subLayers");
        // Arcs may point at any prim of this layer (inherits / internal references).
        let mut prims = std::mem::take(&mut layer.prims);
        for _ in 0..4 {
            // Only the prims that internal arcs point at are copied (not the whole stage).
            fn internal_targets(p: &Prim, out: &mut BTreeSet<String>) {
                for (k, v) in &p.meta {
                    if ARC_KEYS.contains(&k.as_str()) {
                        out.extend(arc_items(v).into_iter().filter(|(a, _)| a.as_deref().unwrap_or("").is_empty()).filter_map(|(_, t)| t));
                    }
                }
                p.children.iter().for_each(|c| internal_targets(c, out));
                p.variants.iter().flat_map(|(_, vs)| vs).for_each(|(_, b)| internal_targets(b, out));
            }
            let mut targets = BTreeSet::new();
            prims.iter().for_each(|p| internal_targets(p, &mut targets));
            let stage: HashMap<String, Prim> = targets.into_iter().filter_map(|t| find_prim(&prims, &t).cloned().map(|p| (t, p))).collect();
            let mut changed = false;
            for p in &mut prims {
                changed |= self.compose_prim(p, &stage, dir, 0);
            }
            if !changed {
                break;
            }
        }
        layer.prims = prims;
    }

    /// Apply a prim's own arcs and selected variants, then recurse. Returns true if anything
    /// was composed.
    fn compose_prim(&mut self, p: &mut Prim, stage: &HashMap<String, Prim>, dir: &Path, depth: usize) -> bool {
        if depth > 64 {
            return false;
        }
        let mut changed = false;
        // Several rounds: a variant can add a reference whose prim adds another variant set.
        for _ in 0..8 {
            let mut round = false;
            // Variants (stronger than references).
            let sel: Vec<(String, String)> = p
                .meta
                .iter()
                .filter(|(k, _)| k == "variants")
                .flat_map(|(_, v)| if let V::Dict(d) = v { d.iter().filter_map(|(k, v)| v.text().map(|t| (k.clone(), t.to_string()))).collect() } else { Vec::new() })
                .collect();
            // Variant selections are resolved in the stage's root context only: a referenced
            // layer's default selection can be overridden by the prim that references it.
            let sets = if self.loading.len() <= 1 { std::mem::take(&mut p.variants) } else { Vec::new() };
            for (set, vars) in sets {
                let choice = sel.iter().rev().find(|(k, _)| *k == set).map(|(_, c)| c.clone());
                match choice.and_then(|c| vars.iter().find(|(n, _)| *n == c).map(|(_, b)| b.clone())) {
                    Some(body) => {
                        merge_prim(p, body);
                        round = true;
                    }
                    None => p.variants.push((set, vars)),
                }
            }
            // Inherits, references, payloads, specializes — in strength order.
            for key in ARC_KEYS {
                let items: Vec<_> = p.meta.iter().filter(|(k, _)| k == key).flat_map(|(_, v)| arc_items(v)).collect();
                if items.is_empty() {
                    continue;
                }
                p.meta.retain(|(k, _)| k != key);
                for (asset, target) in items {
                    let src = match &asset {
                        Some(a) if !a.is_empty() => match self.load(&resolve_asset(dir, a)) {
                            Ok(l) => {
                                let t = target.clone().or_else(|| l.meta.iter().find(|(k, _)| k == "defaultPrim").and_then(|(_, v)| v.text()).map(|d| format!("/{d}")));
                                let found = match &t {
                                    Some(t) => find_prim(&l.prims, t).cloned(),
                                    None => l.prims.iter().find(|x| x.spec == "def").cloned(),
                                };
                                if found.is_none() {
                                    self.notes.push(format!("{}: {} has no prim {}", p.path, a, t.unwrap_or_default()));
                                }
                                found
                            }
                            Err(e) => {
                                self.notes.push(e);
                                None
                            }
                        },
                        _ => {
                            let t = target.clone().unwrap_or_default();
                            let found = stage.get(&t).cloned();
                            if found.is_none() && !t.is_empty() {
                                self.notes.push(format!("{}: {key} target {t} not found", p.path));
                            }
                            found.map(|mut f| {
                                self.compose_prim(&mut f, stage, dir, depth + 1);
                                f
                            })
                        }
                    };
                    if let Some(mut src) = src {
                        let from = src.path.clone();
                        repath(&mut src, &from, &p.path);
                        src.name = p.name.clone();
                        if src.spec == "class" {
                            src.spec = "over".into();
                        }
                        merge_prim(p, src);
                        round = true;
                    }
                }
            }
            changed |= round;
            if !round {
                break;
            }
        }
        for c in &mut p.children {
            changed |= self.compose_prim(c, stage, dir, depth + 1);
        }
        changed
    }
}

/// UsdGeom intrinsic primitives (Cube, Sphere, Cylinder, Cone, Capsule, Plane) as polygon
/// meshes with UVs. USD's defaults: size 2 / radius 1 / height 2, axis Z.
fn primitive_mesh(p: &Prim) -> Option<Mesh> {
    use crate::mesh::Primitive;
    let n = |k: &str, d: f64| p.num(k).unwrap_or(d) as f32;
    let mut m = match p.ty.as_str() {
        "Cube" => {
            let s = n("size", 2.0);
            Primitive::Cube { width: s, height: s, depth: s, sx: 1, sy: 1, sz: 1 }.build()
        }
        "Sphere" => Primitive::Sphere { radius: n("radius", 1.0), axis_div: 24, height_div: 16 }.build(),
        "Cylinder" => Primitive::Cylinder { radius: n("radius", 1.0), height: n("height", 2.0), axis_div: 24, height_div: 1, caps: true }.build(),
        "Cone" => Primitive::Cone { radius: n("radius", 1.0), height: n("height", 2.0), axis_div: 24 }.build(),
        "Plane" => Primitive::Plane { width: n("width", 2.0), height: n("length", 2.0), sx: 1, sy: 1 }.build(),
        "Capsule" => {
            // A sphere (odd ring count, so no ring sits on the equator) split at the equator
            // and pulled apart by the cylinder height.
            let (r, h) = (n("radius", 0.5), n("height", 1.0));
            let mut m = Primitive::Sphere { radius: r, axis_div: 24, height_div: 17 }.build();
            for v in &mut m.positions {
                v.y += if v.y > 0.0 { h * 0.5 } else { -h * 0.5 };
            }
            m
        }
        _ => return None,
    };
    // Our primitives are built along +Y; USD's default axis is Z.
    let axis = p.text("axis").unwrap_or_else(|| "Z".into());
    let rot = match axis.as_str() {
        "X" => Some(Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)),
        "Y" => None,
        _ if p.ty == "Cube" || p.ty == "Sphere" => None,
        _ => Some(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
    };
    if let Some(q) = rot {
        for v in &mut m.positions {
            *v = q * *v;
        }
    }
    Some(m)
}

// ------------------------------------------------------------------------------ builder

#[derive(Default)]
pub struct Report {
    pub nodes: usize,
    pub meshes: usize,
    pub skins: usize,
    pub joints: usize,
    pub blend_targets: usize,
    pub materials: usize,
    pub textures: usize,
    pub lights: usize,
    pub cameras: usize,
    pub curves: usize,
    pub anim_curves: usize,
    pub notes: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Imported USD: {} nodes, {} meshes, {} skins ({} joints), {} blend-shape targets, {} materials ({} textures), {} lights, {} cameras, {} curves, {} animation curves",
            self.nodes, self.meshes, self.skins, self.joints, self.blend_targets, self.materials, self.textures, self.lights, self.cameras, self.curves, self.anim_curves
        );
        if !self.notes.is_empty() {
            let mut n = self.notes.clone();
            n.dedup();
            s.push_str(&format!(" — {}", n.join("; ")));
        }
        s
    }
}

fn is_geom(ty: &str) -> bool {
    matches!(ty, "Mesh" | "Cube" | "Sphere" | "Cylinder" | "Cone" | "Capsule" | "Plane" | "BasisCurves" | "NurbsCurves" | "DistantLight" | "SphereLight" | "RectLight" | "DomeLight" | "DiskLight" | "CylinderLight" | "Camera")
}

fn has_xform_ops(p: &Prim) -> bool {
    p.props.iter().any(|x| x.name.starts_with("xformOp:"))
}

/// Prims that only hold shading networks.
fn is_shading_only(p: &Prim) -> bool {
    matches!(p.ty.as_str(), "Material" | "Shader" | "NodeGraph") || (p.ty == "Scope" && !p.children.is_empty() && p.children.iter().all(is_shading_only))
}

fn srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055 }
}

fn unwrap_euler(prev: Vec3, mut e: Vec3) -> Vec3 {
    for i in 0..3 {
        while e[i] - prev[i] > 180.0 {
            e[i] -= 360.0;
        }
        while e[i] - prev[i] < -180.0 {
            e[i] += 360.0;
        }
    }
    e
}

/// Rotation matrix for rotateXYZ-style ops (angles in degrees; first letter applied first).
fn rot_ops(order: &str, a: Vec3) -> Mat4 {
    let r = |axis: char| match axis {
        'X' => Mat4::from_rotation_x(a.x.to_radians()),
        'Y' => Mat4::from_rotation_y(a.y.to_radians()),
        _ => Mat4::from_rotation_z(a.z.to_radians()),
    };
    let ch: Vec<char> = order.chars().collect();
    // "XYZ": rotate X first → M = Rz · Ry · Rx.
    r(ch[2]) * r(ch[1]) * r(ch[0])
}

/// Local transform of a prim at a time (None = default / first sample).
fn local_xform(p: &Prim, t: Option<f64>) -> Mat4 {
    let order: Vec<String> = p.val("xformOpOrder").map(|v| v.strs()).unwrap_or_default();
    let mut m = Mat4::IDENTITY;
    for op in &order {
        let (inv, op) = match op.strip_prefix("!invert!") {
            Some(o) => (true, o),
            None => (false, op.as_str()),
        };
        let Some(prop) = p.prop(op) else { continue };
        let Some(v) = prop.at(t) else { continue };
        let n = v.nums();
        let kind = op.trim_start_matches("xformOp:").split(':').next().unwrap_or("");
        let v3 = || if n.len() >= 3 { Vec3::new(n[0] as f32, n[1] as f32, n[2] as f32) } else { Vec3::ZERO };
        let mut om = match kind {
            "translate" => Mat4::from_translation(v3()),
            "scale" => Mat4::from_scale(if n.len() >= 3 { v3() } else { Vec3::splat(n.first().copied().unwrap_or(1.0) as f32) }),
            "orient" if n.len() >= 4 => Mat4::from_quat(Quat::from_xyzw(n[1] as f32, n[2] as f32, n[3] as f32, n[0] as f32).normalize()),
            "rotateX" => Mat4::from_rotation_x((n.first().copied().unwrap_or(0.0) as f32).to_radians()),
            "rotateY" => Mat4::from_rotation_y((n.first().copied().unwrap_or(0.0) as f32).to_radians()),
            "rotateZ" => Mat4::from_rotation_z((n.first().copied().unwrap_or(0.0) as f32).to_radians()),
            k if k.starts_with("rotate") && k.len() == 9 => rot_ops(&k[6..], v3()),
            "transform" => v.mat4().unwrap_or(Mat4::IDENTITY),
            _ => Mat4::IDENTITY,
        };
        if inv {
            om = om.inverse();
        }
        m *= om;
    }
    m
}

/// All time codes at which the prim's transform is sampled.
fn xform_times(p: &Prim) -> Vec<f64> {
    let mut s: BTreeSet<i64> = BTreeSet::new();
    for pr in p.props.iter().filter(|x| x.name.starts_with("xformOp:")) {
        for (t, _) in &pr.samples {
            s.insert((t * 1000.0).round() as i64);
        }
    }
    s.into_iter().map(|t| t as f64 / 1000.0).collect()
}

struct Builder<'a> {
    app: &'a mut App,
    dir: PathBuf,
    rep: Report,
    /// USD path → node.
    nodes: HashMap<String, NodeId>,
    /// Material prim path → material id.
    mats: HashMap<String, u32>,
    all: HashMap<String, &'a Prim>,
    root_fix: Mat4,
    times: (f64, f64),
    skels: Vec<SkelInfo>,
    /// Materials whose diffuse colour is the mesh's displayColor primvar.
    dc_mats: BTreeSet<u32>,
    /// (material, display colour bits) → tinted copy.
    tinted: HashMap<(u32, [u32; 3]), u32>,
}

fn index_prims<'a>(p: &'a Prim, out: &mut HashMap<String, &'a Prim>) {
    out.insert(p.path.clone(), p);
    for c in &p.children {
        index_prims(c, out);
    }
}

fn set_trs(app: &mut App, id: NodeId, m: Mat4) {
    let (s, r, t) = m.to_scale_rotation_translation();
    if let Some(n) = app.scene.node_mut(id) {
        n.translate = t;
        n.rotate = crate::rig::quat_euler(r.normalize());
        n.scale = s;
        n.joint_orient = Vec3::ZERO;
    }
}

impl Builder<'_> {
    fn name_of(p: &Prim) -> String {
        p.meta("customData")
            .and_then(|d| match d {
                V::Dict(e) => e.iter().find(|(k, _)| k == "mayaName").and_then(|(_, v)| v.text().map(String::from)),
                _ => None,
            })
            .unwrap_or_else(|| p.name.clone())
    }

    fn new_node(&mut self, p: &Prim, shape: Shape, parent: Option<NodeId>) -> NodeId {
        let name = Self::name_of(p);
        let id = self.app.scene.add_node(&name, shape, parent);
        self.app.apply_name(id, Some(&name));
        self.nodes.insert(p.path.clone(), id);
        self.rep.nodes += 1;
        id
    }

    /// Transform (plus animation keys) of a prim onto a node.
    fn apply_xform(&mut self, p: &Prim, id: NodeId, is_root: bool) {
        let fix = if is_root { self.root_fix } else { Mat4::IDENTITY };
        let times = xform_times(p);
        set_trs(self.app, id, fix * local_xform(p, times.first().copied()));
        if p.text("visibility").as_deref() == Some("invisible")
            && let Some(n) = self.app.scene.node_mut(id)
        {
            n.visible = false;
        }
        if times.len() > 1 {
            let mut curves: Vec<Curve> = (0..9).map(|_| Curve { tangent: Tangent::Linear, ..Default::default() }).collect();
            let mut prev = self.app.scene.node(id).map(|n| n.rotate).unwrap_or(Vec3::ZERO);
            for t in &times {
                let (s, r, tr) = (fix * local_xform(p, Some(*t))).to_scale_rotation_translation();
                let e = unwrap_euler(prev, crate::rig::quat_euler(r.normalize()));
                prev = e;
                let vals = [tr.x, tr.y, tr.z, e.x, e.y, e.z, s.x, s.y, s.z];
                for (c, v) in curves.iter_mut().zip(vals) {
                    c.set_key(*t as f32, v);
                }
            }
            let chans = [Channel::TranslateX, Channel::TranslateY, Channel::TranslateZ, Channel::RotateX, Channel::RotateY, Channel::RotateZ, Channel::ScaleX, Channel::ScaleY, Channel::ScaleZ];
            if let Some(n) = self.app.scene.node_mut(id) {
                for (ch, c) in chans.into_iter().zip(curves) {
                    // Only channels that move.
                    let vals: Vec<f32> = c.keys.iter().map(|k| k.value).collect();
                    if vals.iter().any(|v| (v - vals[0]).abs() > 1e-5) {
                        n.anim.insert(ch, c);
                        self.rep.anim_curves += 1;
                    }
                }
            }
            self.times.0 = self.times.0.min(times[0]);
            self.times.1 = self.times.1.max(*times.last().unwrap());
        }
    }

    fn walk(&mut self, p: &Prim, parent: Option<NodeId>, is_root: bool) {
        let inactive = matches!(p.meta("active"), Some(V::Tok(t)) if t == "false") || matches!(p.meta("active"), Some(V::Num(n)) if *n == 0.0);
        if inactive || is_shading_only(p) || matches!(p.ty.as_str(), "SkelAnimation" | "BlendShape" | "GeomSubset") || p.spec == "class" {
            return;
        }
        if p.ty == "PointInstancer" {
            self.point_instancer(p, parent, is_root);
            return;
        }
        if p.ty == "Skeleton" {
            let id = self.new_node(p, Shape::Group, parent);
            self.apply_xform(p, id, is_root);
            self.skeleton(p, id);
            return;
        }
        if is_geom(&p.ty) {
            let id = self.new_node(p, Shape::Group, parent);
            self.apply_xform(p, id, is_root);
            self.geometry(p, id);
            for c in &p.children {
                self.walk(c, Some(id), false);
            }
            return;
        }
        // Transformable / grouping prim. A single geometry child without its own transform
        // becomes this node's shape (Maya transform + shape).
        let id = self.new_node(p, Shape::Group, parent);
        self.apply_xform(p, id, is_root);
        let geo: Vec<&Prim> = p.children.iter().filter(|c| is_geom(&c.ty)).collect();
        let others = p.children.iter().filter(|c| !is_geom(&c.ty) && !is_shading_only(c) && c.spec != "class").count();
        let merge = (p.ty != "SkelRoot" && geo.len() == 1 && others == 0 && !has_xform_ops(geo[0]) && geo[0].children.iter().all(|c| matches!(c.ty.as_str(), "BlendShape" | "GeomSubset"))).then(|| geo[0].path.clone());
        for c in &p.children {
            if Some(&c.path) == merge.as_ref() {
                self.nodes.insert(c.path.clone(), id);
                self.geometry(c, id);
            } else {
                self.walk(c, Some(id), false);
            }
        }
    }

    /// UsdGeomPointInstancer: one group per instance (position / orientation / scale) holding a
    /// copy of its prototype. Prototypes themselves aren't drawn.
    fn point_instancer(&mut self, p: &Prim, parent: Option<NodeId>, is_root: bool) {
        let id = self.new_node(p, Shape::Group, parent);
        self.apply_xform(p, id, is_root);
        let protos = p.prop("prototypes").map(|r| r.targets.clone()).unwrap_or_default();
        let idx = p.val("protoIndices").map(|v| v.ints()).unwrap_or_default();
        let pos = p.val("positions").map(|v| v.vec3s()).unwrap_or_default();
        let rot = p.val("orientations").map(|v| v.quats()).unwrap_or_default();
        let scl = p.val("scales").map(|v| v.vec3s()).unwrap_or_default();
        let hidden: BTreeSet<i64> = p.val("invisibleIds").map(|v| v.ints()).unwrap_or_default().into_iter().collect();
        let find = |path: &str| -> Option<&Prim> {
            let rel = path.strip_prefix(&p.path)?;
            let mut cur = p;
            for n in rel.split('/').filter(|x| !x.is_empty()) {
                cur = cur.children.iter().find(|c| c.name == n)?;
            }
            Some(cur)
        };
        let n = idx.len().min(pos.len());
        if n > 2000 {
            self.rep.notes.push(format!("{}: {} instances, only the first 2000 imported", p.name, n));
        }
        for i in 0..n.min(2000) {
            if hidden.contains(&(i as i64)) {
                continue;
            }
            let Some(proto) = protos.get(idx[i].max(0) as usize).and_then(|t| find(t)) else { continue };
            let inst = Prim { spec: "def".into(), ty: "Xform".into(), name: format!("{}_{}", proto.name, i + 1), path: format!("{}/__instance_{i}", p.path), ..Default::default() };
            let gid = self.new_node(&inst, Shape::Group, Some(id));
            let m = Mat4::from_scale_rotation_translation(scl.get(i).copied().unwrap_or(Vec3::ONE), rot.get(i).copied().unwrap_or(Quat::IDENTITY), pos[i]);
            set_trs(self.app, gid, m);
            self.walk(proto, Some(gid), false);
        }
    }

    fn geometry(&mut self, p: &Prim, id: NodeId) {
        match p.ty.as_str() {
            "Mesh" | "Cube" | "Sphere" | "Cylinder" | "Cone" | "Capsule" | "Plane" => self.mesh(p, id),
            "BasisCurves" => {
                let pts = p.val("points").map(|v| v.vec3s()).unwrap_or_default();
                let counts = p.val("curveVertexCounts").map(|v| v.ints()).unwrap_or_default();
                let n = counts.first().map(|c| *c as usize).unwrap_or(pts.len()).min(pts.len());
                let closed = p.text("wrap").as_deref() == Some("periodic");
                if counts.len() > 1 {
                    self.rep.notes.push(format!("{}: only the first of {} curves imported", p.name, counts.len()));
                }
                if let Some(nd) = self.app.scene.node_mut(id) {
                    nd.shape = Shape::Curve { points: pts[..n].to_vec(), closed, degree: 1, knots: Vec::new() };
                }
                self.rep.curves += 1;
            }
            "NurbsCurves" => {
                let pts = p.val("points").map(|v| v.vec3s()).unwrap_or_default();
                let counts = p.val("curveVertexCounts").map(|v| v.ints()).unwrap_or_default();
                let n = counts.first().map(|c| *c as usize).unwrap_or(pts.len()).min(pts.len());
                let order = p.val("order").map(|v| v.ints()).and_then(|o| o.first().copied()).unwrap_or(4).max(2) as usize;
                let knots: Vec<f32> = p.val("knots").map(|v| v.nums()).unwrap_or_default().iter().map(|k| *k as f32).take(n + order).collect();
                if counts.len() > 1 {
                    self.rep.notes.push(format!("{}: only the first of {} curves imported", p.name, counts.len()));
                }
                let c = crate::nurbs::from_usd(&pts[..n], order, &knots);
                if let Some(nd) = self.app.scene.node_mut(id) {
                    nd.shape = c.into_shape();
                }
                self.rep.curves += 1;
            }
            "Camera" => {
                let focal = p.num("focalLength").unwrap_or(50.0) as f32;
                let vap = p.num("verticalAperture").unwrap_or(15.2908) as f32;
                let clip = p.val("clippingRange").map(|v| v.nums()).unwrap_or_default();
                let mut cs = CameraShape { focal: (24.0 * focal / vap.max(0.01)).clamp(2.5, 3500.0), ..Default::default() };
                if clip.len() == 2 {
                    cs.near = clip[0] as f32;
                    cs.far = clip[1] as f32;
                }
                if let Some(fs) = p.num("fStop").filter(|f| *f > 0.0) {
                    cs.dof = true;
                    cs.fstop = fs as f32;
                    cs.focus_distance = p.num("focusDistance").unwrap_or(5.0) as f32;
                }
                if let Some(nd) = self.app.scene.node_mut(id) {
                    nd.shape = Shape::Camera(cs);
                }
                self.rep.cameras += 1;
            }
            _ => self.light(p, id),
        }
    }

    fn light(&mut self, p: &Prim, id: NodeId) {
        let g = |n: &str| p.num(&format!("inputs:{n}")).or_else(|| p.num(n));
        let col = p.val("inputs:color").or_else(|| p.val("color")).map(|v| v.nums()).filter(|n| n.len() >= 3).map(|n| [srgb(n[0] as f32), srgb(n[1] as f32), srgb(n[2] as f32)]).unwrap_or([1.0; 3]);
        let intensity = g("intensity").unwrap_or(1.0) as f32 * 2f32.powf(g("exposure").unwrap_or(0.0) as f32);
        let shaping = p.api_schemas().iter().any(|a| a == "ShapingAPI") || p.prop("inputs:shaping:cone:angle").is_some();
        let kind = match p.ty.as_str() {
            "DistantLight" => LightKind::Directional,
            "DomeLight" => LightKind::Ambient,
            "RectLight" => LightKind::Area,
            _ if shaping => LightKind::Spot,
            _ => LightKind::Point,
        };
        let mut l = Light { kind, color: col, intensity, ..Default::default() };
        if kind == LightKind::Spot {
            let angle = g("shaping:cone:angle").unwrap_or(45.0) as f32;
            let soft = g("shaping:cone:softness").unwrap_or(0.0) as f32;
            l.penumbra = (angle * soft).clamp(0.0, 45.0);
            l.cone = (2.0 * (angle - l.penumbra)).clamp(1.0, 179.0);
        }
        l.shadows = p.num("inputs:shadow:enable").map(|v| v > 0.5).unwrap_or(kind != LightKind::Ambient);
        if kind == LightKind::Area
            && let Some(n) = self.app.scene.node_mut(id)
        {
            let (w, h) = (g("width").unwrap_or(1.0) as f32, g("height").unwrap_or(1.0) as f32);
            n.scale *= Vec3::new(w * 0.5, h * 0.5, 1.0);
        }
        if let Some(n) = self.app.scene.node_mut(id) {
            n.shape = Shape::Light(l);
        }
        self.rep.lights += 1;
    }

    fn mesh(&mut self, p: &Prim, id: NodeId) {
        let mesh = match primitive_mesh(p) {
            Some(m) => m,
            None => {
                let pts = p.val("points").map(|v| v.vec3s()).unwrap_or_default();
                let counts = p.val("faceVertexCounts").map(|v| v.ints()).unwrap_or_default();
                let idx = p.val("faceVertexIndices").map(|v| v.ints()).unwrap_or_default();
                if p.prop("points").is_some_and(|x| x.samples.len() > 1) {
                    self.rep.notes.push(format!("{}: point animation (time-sampled points) not imported — first sample used", p.name));
                }
                let left = p.text("orientation").as_deref() == Some("leftHanded");
                let mut mesh = Mesh { positions: pts.clone(), ..Default::default() };
                let mut corner_ids: Vec<Vec<usize>> = Vec::new(); // face-vertex (corner) index per face corner
                let mut c = 0usize;
                for &n in &counts {
                    let n = n.max(0) as usize;
                    if c + n > idx.len() {
                        break;
                    }
                    let mut f: Vec<u32> = idx[c..c + n].iter().map(|x| *x as u32).collect();
                    let mut cs: Vec<usize> = (c..c + n).collect();
                    c += n;
                    if n < 3 || f.iter().any(|v| *v as usize >= pts.len()) {
                        continue;
                    }
                    if left {
                        f.reverse();
                        cs.reverse();
                    }
                    mesh.faces.push(f);
                    corner_ids.push(cs);
                }
                // UVs: the first texCoord2f / float2 primvar.
                let uvp = p.props.iter().find(|x| x.name.starts_with("primvars:") && !x.name.ends_with(":indices") && (x.ty.starts_with("texCoord2") || x.ty.starts_with("float2")) && x.name != "primvars:skel:jointWeights");
                if let Some(uvp) = uvp {
                    let vals = uvp.at(None).map(|v| v.vec2s()).unwrap_or_default();
                    let ind = p.prop(&format!("{}:indices", uvp.name)).and_then(|x| x.at(None)).map(|v| v.ints()).or_else(|| uvp.meta("indices").map(|v| v.ints()));
                    let interp = uvp.meta("interpolation").and_then(|v| v.text().map(String::from)).unwrap_or("vertex".into());
                    let get = |i: usize| -> Vec2 {
                        let j = ind.as_ref().map(|x| x.get(i).copied().unwrap_or(0) as usize).unwrap_or(i);
                        vals.get(j).copied().unwrap_or(Vec2::ZERO)
                    };
                    if !vals.is_empty() {
                        let corners: Vec<Vec<Vec2>> = mesh
                            .faces
                            .iter()
                            .zip(&corner_ids)
                            .enumerate()
                            .map(|(fi, (f, cs))| match interp.as_str() {
                                "faceVarying" => cs.iter().map(|ci| get(*ci)).collect(),
                                "uniform" => vec![get(fi); f.len()],
                                "constant" => vec![get(0); f.len()],
                                _ => f.iter().map(|v| get(*v as usize)).collect(),
                            })
                            .collect();
                        mesh.set_corner_uvs(&corners);
                    }
                }
                mesh
            }
        };
        let nverts = mesh.positions.len();
        // Material binding (or display colour).
        let binding = p.prop("material:binding").and_then(|r| r.targets.first().cloned());
        let mut mat = binding.and_then(|b| self.material(&b));
        let color = p.val("primvars:displayColor").map(|v| v.nums()).filter(|n| n.len() >= 3).map(|n| [srgb(n[0] as f32), srgb(n[1] as f32), srgb(n[2] as f32)]);
        if let (Some(mid), Some(c)) = (mat, color)
            && self.dc_mats.contains(&mid)
        {
            mat = Some(self.tinted_material(mid, c));
        }
        if let Some(n) = self.app.scene.node_mut(id) {
            n.shape = Shape::Poly { history: None, mesh, rev: crate::scene::next_rev() };
            n.material = mat;
            if let Some(c) = color {
                n.color = c;
            }
        }
        // Blend shapes (UsdSkel BlendShape prims).
        let names = p.val("skel:blendShapes").map(|v| v.strs()).unwrap_or_default();
        let targets = p.prop("skel:blendShapeTargets").map(|r| r.targets.clone()).unwrap_or_default();
        if !targets.is_empty() {
            let mut bs = BlendShape::default();
            for (k, tp) in targets.iter().enumerate() {
                let Some(t) = self.all.get(tp) else { continue };
                let offs = t.val("offsets").map(|v| v.vec3s()).unwrap_or_default();
                let pidx = t.val("pointIndices").map(|v| v.ints());
                let mut deltas = vec![Vec3::ZERO; nverts];
                match pidx {
                    Some(ix) => {
                        for (o, i) in offs.iter().zip(ix) {
                            if let Some(d) = deltas.get_mut(i as usize) {
                                *d = *o;
                            }
                        }
                    }
                    None => {
                        for (d, o) in deltas.iter_mut().zip(&offs) {
                            *d = *o;
                        }
                    }
                }
                let nm = names.get(k).cloned().unwrap_or_else(|| t.name.clone());
                bs.add_target(nm, deltas);
                self.rep.blend_targets += 1;
            }
            if let Some(n) = self.app.scene.node_mut(id) {
                n.blend = Some(bs);
            }
        }
        self.rep.meshes += 1;
    }

    /// Material from a UsdPreviewSurface network (created once per Material prim).
    /// A copy of a displayColor-driven material with the mesh's colour.
    fn tinted_material(&mut self, mid: u32, c: [f32; 3]) -> u32 {
        let key = (mid, c.map(f32::to_bits));
        if let Some(t) = self.tinted.get(&key) {
            return *t;
        }
        let Some(base) = self.app.material(mid).cloned() else { return mid };
        let id = if self.tinted.keys().any(|k| k.0 == mid) {
            let id = self.app.create_material(MatKind::StandardSurface, Some(&base.name));
            self.rep.materials += 1;
            id
        } else {
            mid // first user keeps the original
        };
        let m = self.app.material_mut(id).unwrap();
        let (nid, name) = (m.id, m.name.clone());
        *m = Material { id: nid, name, color: c, ..base };
        self.tinted.insert(key, id);
        id
    }

    /// The texture node feeding a connection: follows node-graph outputs and pass-through
    /// nodes (normalmap, convert, extract, …) until a node with `inputs:file`.
    fn texture_node(&self, conn: &str, depth: usize) -> Option<&Prim> {
        if depth > 12 {
            return None;
        }
        let (path, out) = conn.split_once('.').unwrap_or((conn, ""));
        let p = self.all.get(path)?;
        if p.prop("inputs:file").is_some() {
            return Some(p);
        }
        if let Some(c) = p.prop(out).and_then(|x| x.connect.first()) {
            return self.texture_node(c, depth + 1);
        }
        let ins = p.props.iter().filter(|x| x.name.starts_with("inputs:") && !x.connect.is_empty());
        let mut ins: Vec<&Prop> = ins.collect();
        ins.sort_by_key(|x| x.name != "inputs:in");
        ins.into_iter().find_map(|x| self.texture_node(&x.connect[0], depth + 1))
    }

    fn material(&mut self, path: &str) -> Option<u32> {
        if let Some(m) = self.mats.get(path) {
            return Some(*m);
        }
        let mp = *self.all.get(path)?;
        // UsdPreviewSurface (outputs:surface) or a MaterialX network (outputs:mtlx:surface).
        let surf_path = ["outputs:surface", "outputs:mtlx:surface"]
            .iter()
            .find_map(|o| mp.prop(o).and_then(|x| x.connect.first().cloned()))
            .or_else(|| mp.props.iter().find(|x| x.name.starts_with("outputs:") && x.name.ends_with(":surface")).and_then(|x| x.connect.first().cloned()))
            .map(|c| c.split('.').next().unwrap_or("").to_string());
        // Through node-graph outputs to the shader itself.
        let shader = surf_path.and_then(|s| {
            let mut cur = s;
            for _ in 0..8 {
                let p = self.all.get(&cur)?;
                if p.ty != "NodeGraph" {
                    return Some(*p);
                }
                let next = p.props.iter().find(|x| x.name.starts_with("outputs:") && !x.connect.is_empty())?.connect[0].clone();
                cur = next.split('.').next().unwrap_or("").to_string();
            }
            None
        });
        let tex0 = self.rep.textures;
        let mut reads_dc = false;
        let id = self.app.create_material(MatKind::StandardSurface, Some(&mp.name));
        let mut m = self.app.material(id).unwrap().clone();
        m.specular = 0.5;
        if let Some(sh) = shader {
            // Input value or connected texture (file path, scale, repeat).
            // Names are tried in order (UsdPreviewSurface, MaterialX standard_surface, OpenPBR).
            let input = |me: &Self, n: &str| -> (Option<Vec<f64>>, Option<(String, Vec<f64>, [f32; 2])>) {
                let names: &[&str] = match n {
                    "diffuseColor" => &["diffuseColor", "base_color"],
                    "roughness" => &["roughness", "specular_roughness"],
                    "metallic" => &["metallic", "metalness", "base_metalness"],
                    "normal" => &["normal", "geometry_normal"],
                    "emissiveColor" => &["emissiveColor", "emission_color"],
                    "opacity" => &["opacity", "geometry_opacity"],
                    _ => &[n][..],
                };
                let Some(pr) = names.iter().find_map(|k| sh.prop(&format!("inputs:{k}"))) else { return (None, None) };
                let tex = pr.connect.first().and_then(|c| me.texture_node(c, 0)).and_then(|t| {
                    let file = t.val("inputs:file").and_then(|v| v.text().map(String::from))?;
                    let scale = t.val("inputs:scale").map(|v| v.nums()).unwrap_or_default();
                    let rep = t
                        .prop("inputs:st")
                        .and_then(|s| s.connect.first())
                        .and_then(|c| me.all.get(c.split('.').next().unwrap_or("")))
                        .and_then(|x| x.val("inputs:scale"))
                        .map(|v| v.nums())
                        .filter(|n| n.len() >= 2)
                        .map(|n| [n[0] as f32, n[1] as f32])
                        .unwrap_or([1.0, 1.0]);
                    Some((file, scale, rep))
                });
                (pr.at(None).map(|v| v.nums()), tex)
            };
            let resolve = |f: &str| -> String {
                let p = Path::new(f);
                if p.is_absolute() { f.to_string() } else { self.dir.join(f.trim_start_matches("./")).display().to_string() }
            };
            let (dc, dtex) = input(self, "diffuseColor");
            // diffuseColor ← UsdPrimvarReader(displayColor): each mesh tints its own copy.
            reads_dc = ["diffuseColor", "base_color"].iter().filter_map(|k| sh.prop(&format!("inputs:{k}"))).filter_map(|p| p.connect.first()).filter_map(|c| self.all.get(c.split('.').next().unwrap_or(""))).any(|r| {
                r.text("info:id").is_some_and(|i| i.starts_with("UsdPrimvarReader") || i.starts_with("ND_geompropvalue")) && r.text("inputs:varname").or_else(|| r.text("inputs:geomprop")).is_some_and(|v| v == "displayColor")
            });
            if let Some(c) = dc.filter(|c| c.len() >= 3) {
                m.color = [srgb(c[0] as f32), srgb(c[1] as f32), srgb(c[2] as f32)];
            }
            if let Some((f, _, rep)) = dtex {
                m.color_map = Some(resolve(&f));
                m.repeat_uv = rep;
                self.rep.textures += 1;
            }
            let (r, rtex) = input(self, "roughness");
            m.roughness = r.and_then(|v| v.first().copied()).unwrap_or(0.5) as f32;
            if let Some((f, s, _)) = rtex {
                m.roughness_map = Some(resolve(&f));
                m.roughness = s.first().copied().unwrap_or(1.0) as f32;
                self.rep.textures += 1;
            }
            let (mt, mtex) = input(self, "metallic");
            m.metalness = mt.and_then(|v| v.first().copied()).unwrap_or(0.0) as f32;
            if let Some((f, s, _)) = mtex {
                m.metalness_map = Some(resolve(&f));
                m.metalness = s.first().copied().unwrap_or(1.0) as f32;
                self.rep.textures += 1;
            }
            if let (_, Some((f, _, _))) = input(self, "normal") {
                m.normal_map = Some(resolve(&f));
                self.rep.textures += 1;
            }
            if let (_, Some((f, _, _))) = input(self, "occlusion") {
                m.occlusion_map = Some(resolve(&f));
                self.rep.textures += 1;
            }
            let (e, etex) = input(self, "emissiveColor");
            // MaterialX weights emission separately (standard_surface `emission`, OpenPBR
            // `emission_luminance` in nits); no weight means no emission.
            let is_mtlx = sh.text("info:id").is_some_and(|i| i.starts_with("ND_"));
            let ew = if is_mtlx { sh.num("inputs:emission").or_else(|| sh.num("inputs:emission_luminance").map(|l| (l / 1000.0).min(10.0))).unwrap_or(0.0) } else { 1.0 };
            let (e, etex) = if ew > 0.0 { (e.map(|c| c.iter().map(|x| x * ew).collect::<Vec<_>>()), etex) } else { (None, None) };
            if let Some(c) = e.filter(|c| c.len() >= 3) {
                m.emission = [srgb(c[0] as f32), srgb(c[1] as f32), srgb(c[2] as f32)];
            }
            if let Some((f, s, _)) = etex {
                m.emission_map = Some(resolve(&f));
                m.emission = if s.len() >= 3 { [srgb(s[0] as f32), srgb(s[1] as f32), srgb(s[2] as f32)] } else { [1.0; 3] };
                self.rep.textures += 1;
            }
            if let (Some(o), _) = input(self, "opacity") {
                let o = if o.is_empty() { 1.0 } else { o.iter().sum::<f64>() / o.len() as f64 };
                m.transparency = (1.0 - o as f32).clamp(0.0, 1.0);
            }
            // MaterialX transmission (glass-like): approximate with tinted transparency.
            if let Some(t) = sh.num("inputs:transmission").filter(|t| is_mtlx && *t > 0.0) {
                m.transparency = (0.75 * t as f32).clamp(0.0, 0.95);
                if let Some(c) = sh.val("inputs:transmission_color").map(|v| v.nums()).filter(|c| c.len() >= 3) {
                    m.color = [srgb(c[0] as f32), srgb(c[1] as f32), srgb(c[2] as f32)];
                }
            }
            if let (Some(s), _) = input(self, "specularColor") {
                m.specular = (s.iter().sum::<f64>() / s.len().max(1) as f64) as f32;
            }
        }
        // Instanced assets repeat the same material under many paths: reuse an identical
        // one created by this import instead of making a copy per instance.
        let same = self.mats.values().copied().collect::<BTreeSet<u32>>().into_iter().find(|&o| {
            self.app.material(o).is_some_and(|x| {
                let mut a = x.clone();
                a.id = m.id;
                a.name = m.name.clone();
                a == m && x.name.trim_end_matches(|c: char| c.is_ascii_digit()) == mp.name.trim_end_matches(|c: char| c.is_ascii_digit())
            })
        });
        if let Some(o) = same {
            self.app.scene.materials.retain(|x| x.id != id);
            self.rep.textures = tex0;
            self.mats.insert(path.to_string(), o);
            return Some(o);
        }
        *self.app.material_mut(id).unwrap() = m;
        self.mats.insert(path.to_string(), id);
        self.rep.materials += 1;
        if reads_dc {
            self.dc_mats.insert(id);
        }
        Some(id)
    }

    /// Joints under the skeleton's node; animation from its SkelAnimation.
    fn skeleton(&mut self, sk: &Prim, skel_node: NodeId) {
        let joints = sk.val("joints").map(|v| v.strs()).unwrap_or_default();
        let bind = sk.val("bindTransforms").map(|v| v.mat4s()).unwrap_or_default();
        let rest = sk.val("restTransforms").map(|v| v.mat4s()).unwrap_or_default();
        let mut ids: Vec<NodeId> = Vec::with_capacity(joints.len());
        let mut index: HashMap<String, usize> = HashMap::new();
        for (j, path) in joints.iter().enumerate() {
            let parent = path.rsplit_once('/').and_then(|(pp, _)| index.get(pp).map(|i| ids[*i]));
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            let id = self.app.scene.add_node(&name, Shape::Joint { radius: 0.05 }, Some(parent.unwrap_or(skel_node)));
            self.app.apply_name(id, Some(&name));
            index.insert(path.clone(), j);
            ids.push(id);
            self.rep.joints += 1;
        }
        let parent_of = |j: usize| joints[j].rsplit_once('/').and_then(|(pp, _)| index.get(pp).copied());
        // Local rest from restTransforms, else from the bind transforms.
        for j in 0..ids.len() {
            let local = rest.get(j).copied().unwrap_or_else(|| match (parent_of(j), bind.get(j)) {
                (Some(p), Some(b)) => bind.get(p).map(|pb| pb.inverse() * *b).unwrap_or(*b),
                (None, Some(b)) => *b,
                _ => Mat4::IDENTITY,
            });
            set_trs(self.app, ids[j], local);
        }
        // Animation.
        let anim = sk.prop("skel:animationSource").and_then(|r| r.targets.first()).and_then(|p| self.all.get(p)).cloned();
        if let Some(an) = &anim {
            let aj = an.val("joints").map(|v| v.strs()).unwrap_or_default();
            let map: Vec<Option<usize>> = aj.iter().map(|p| index.get(p).copied()).collect();
            let mut times: BTreeSet<i64> = BTreeSet::new();
            for n in ["translations", "rotations", "scales"] {
                if let Some(pr) = an.prop(n) {
                    for (t, _) in &pr.samples {
                        times.insert((t * 1000.0).round() as i64);
                    }
                }
            }
            let times: Vec<f64> = times.into_iter().map(|t| t as f64 / 1000.0).collect();
            let at = |t: Option<f64>| -> (Vec<Vec3>, Vec<Quat>, Vec<Vec3>) {
                (
                    an.prop("translations").and_then(|p| p.at(t)).map(|v| v.vec3s()).unwrap_or_default(),
                    an.prop("rotations").and_then(|p| p.at(t)).map(|v| v.quats()).unwrap_or_default(),
                    an.prop("scales").and_then(|p| p.at(t)).map(|v| v.vec3s()).unwrap_or_default(),
                )
            };
            // Pose at the first sample (or the defaults).
            let (tr, ro, sc) = at(times.first().copied());
            for (k, j) in map.iter().enumerate() {
                let Some(j) = *j else { continue };
                if let Some(n) = self.app.scene.node_mut(ids[j]) {
                    if let Some(t) = tr.get(k) {
                        n.translate = *t;
                    }
                    if let Some(r) = ro.get(k) {
                        n.rotate = crate::rig::quat_euler(*r);
                    }
                    if let Some(s) = sc.get(k) {
                        n.scale = *s;
                    }
                }
            }
            if times.len() > 1 {
                let mut curves: Vec<[Curve; 9]> = (0..map.len()).map(|_| std::array::from_fn(|_| Curve { tangent: Tangent::Linear, ..Default::default() })).collect();
                let mut prev: Vec<Vec3> = map.iter().map(|j| j.and_then(|j| self.app.scene.node(ids[j])).map(|n| n.rotate).unwrap_or(Vec3::ZERO)).collect();
                for t in &times {
                    let (tr, ro, sc) = at(Some(*t));
                    for k in 0..map.len() {
                        let tv = tr.get(k).copied().unwrap_or(Vec3::ZERO);
                        let e = unwrap_euler(prev[k], crate::rig::quat_euler(ro.get(k).copied().unwrap_or(Quat::IDENTITY)));
                        prev[k] = e;
                        let sv = sc.get(k).copied().unwrap_or(Vec3::ONE);
                        for (c, v) in [tv.x, tv.y, tv.z, e.x, e.y, e.z, sv.x, sv.y, sv.z].into_iter().enumerate() {
                            curves[k][c].set_key(*t as f32, v);
                        }
                    }
                }
                let chans = [Channel::TranslateX, Channel::TranslateY, Channel::TranslateZ, Channel::RotateX, Channel::RotateY, Channel::RotateZ, Channel::ScaleX, Channel::ScaleY, Channel::ScaleZ];
                for (k, cs) in curves.into_iter().enumerate() {
                    let Some(j) = map[k] else { continue };
                    let n = self.app.scene.node_mut(ids[j]).unwrap();
                    for (ch, c) in chans.into_iter().zip(cs) {
                        let vals: Vec<f32> = c.keys.iter().map(|x| x.value).collect();
                        if vals.iter().any(|v| (v - vals[0]).abs() > 1e-5) {
                            n.anim.insert(ch, c);
                            self.rep.anim_curves += 1;
                        }
                    }
                }
                self.times.0 = self.times.0.min(times[0]);
                self.times.1 = self.times.1.max(*times.last().unwrap());
            }
        }
        self.skels.push(SkelInfo { path: sk.path.clone(), node: skel_node, joints: joints.clone(), ids, bind, anim: anim.map(|a| a.path.clone()) });
    }
}

struct SkelInfo {
    path: String,
    node: NodeId,
    joints: Vec<String>,
    ids: Vec<NodeId>,
    bind: Vec<Mat4>,
    anim: Option<String>,
}


pub fn import(app: &mut App, path: &Path) -> Result<Report, String> {
    let head = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let unpacked = if head.starts_with(b"PK") { Some(unpack_usdz(path)?) } else { None };
    let path = unpacked.as_deref().unwrap_or(path);
    let bytes = if unpacked.is_some() { std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))? } else { head };
    let t0 = std::time::Instant::now();
    let timing = std::env::var("USD_TIMING").is_ok();
    let src = layer_text(path, bytes)?;
    let mut layer = parse(&src).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut comp = Composer::new();
    if let Ok(k) = path.canonicalize() {
        comp.loading.push(k);
    }
    comp.compose(&mut layer, &dir);
    if timing {
        eprintln!("usd: parse+compose {:.2}s", t0.elapsed().as_secs_f32());
    }
    let mut all = HashMap::new();
    for p in &layer.prims {
        index_prims(p, &mut all);
    }
    // Up axis and units → Y-up metres.
    let mut fix = Mat4::IDENTITY;
    if let Some(m) = layer.meta_num("metersPerUnit").filter(|m| (*m - 1.0).abs() > 1e-9 && *m > 0.0) {
        fix = Mat4::from_scale(Vec3::splat(m as f32));
    }
    if layer.meta_text("upAxis").as_deref() == Some("Z") {
        fix = Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2) * fix;
    }
    app.begin_batch();
    app.checkpoint();
    if let Some(fps) = layer.meta_num("timeCodesPerSecond").or_else(|| layer.meta_num("framesPerSecond")) {
        app.scene.fps = fps as f32;
    }
    let mut b = Builder { app, dir: path.parent().map(Path::to_path_buf).unwrap_or_default(), rep: Report::default(), nodes: HashMap::new(), mats: HashMap::new(), all, root_fix: fix, times: (f64::MAX, f64::MIN), skels: Vec::new(), dc_mats: BTreeSet::new(), tinted: HashMap::new() };
    let mut roots = Vec::new();
    for p in &layer.prims {
        if p.spec == "class" {
            continue;
        }
        b.walk(p, None, true);
        if let Some(id) = b.nodes.get(&p.path) {
            roots.push(*id);
        }
    }
    b.rep.notes.extend(comp.notes);
    if timing {
        eprintln!("usd: build {:.2}s", t0.elapsed().as_secs_f32());
    }
    // Skinned meshes: bind to their skeletons.
    let skels = std::mem::take(&mut b.skels);
    let mesh_paths: Vec<String> = b.all.values().filter(|p| p.ty == "Mesh").map(|p| p.path.clone()).collect();
    for mp in mesh_paths {
        let p = b.all[&mp].clone();
        let Some(&mid) = b.nodes.get(&mp) else { continue };
        // skel:skeleton on the mesh, or inherited from an ancestor (SkelBindingAPI).
        let mut skel_path = None;
        let mut cur = Some(mp.clone());
        while let Some(c) = cur {
            if let Some(t) = b.all.get(&c).and_then(|x| x.prop("skel:skeleton")).and_then(|r| r.targets.first().cloned()) {
                skel_path = Some(t);
                break;
            }
            cur = c.rsplit_once('/').map(|(a, _)| a.to_string()).filter(|a| !a.is_empty());
        }
        let Some(sk) = skel_path.and_then(|s| skels.iter().find(|k| k.path == s)) else { continue };
        let ji = p.prop("primvars:skel:jointIndices");
        let jw = p.prop("primvars:skel:jointWeights");
        let (Some(ji), Some(jw)) = (ji, jw) else { continue };
        let el = ji.meta("elementSize").and_then(|v| v.f()).unwrap_or(1.0) as usize;
        let constant = ji.meta("interpolation").and_then(|v| v.text().map(String::from)).as_deref() == Some("constant");
        let idx = ji.at(None).map(|v| v.ints()).unwrap_or_default();
        let wts = jw.at(None).map(|v| v.nums()).unwrap_or_default();
        // Optional per-mesh joint subset.
        let mesh_joints: Option<Vec<usize>> = p.val("skel:joints").map(|v| v.strs().iter().filter_map(|j| sk.joints.iter().position(|x| x == j)).collect());
        let nv = b.app.scene.node(mid).and_then(|n| n.mesh()).map(|m| m.positions.len()).unwrap_or(0);
        let geom_bind = p.val("primvars:skel:geomBindTransform").and_then(|v| v.mat4()).unwrap_or(Mat4::IDENTITY);
        let skel_world = b.app.scene.world_matrix(sk.node);
        let weights: Vec<Vec<(u32, f32)>> = (0..nv)
            .map(|v| {
                let base = if constant { 0 } else { v * el };
                let mut w: Vec<(u32, f32)> = (0..el)
                    .filter_map(|k| {
                        let j = *idx.get(base + k)? as usize;
                        let j = mesh_joints.as_ref().map(|m| m.get(j).copied()).unwrap_or(Some(j))?;
                        let x = *wts.get(base + k)? as f32;
                        (x > 1e-6 && j < sk.ids.len()).then_some((j as u32, x))
                    })
                    .collect();
                let s: f32 = w.iter().map(|x| x.1).sum::<f32>().max(1e-9);
                w.iter_mut().for_each(|x| x.1 /= s);
                w.sort_by(|a, c| c.1.total_cmp(&a.1));
                w
            })
            .collect();
        if weights.len() != nv || nv == 0 {
            continue;
        }
        let inv_bind: Vec<Mat4> = (0..sk.ids.len()).map(|j| (skel_world * sk.bind.get(j).copied().unwrap_or(Mat4::IDENTITY)).inverse()).collect();
        let bind_pose = sk.ids.iter().filter_map(|j| b.app.scene.node(*j).map(|n| (*j, n.translate, n.rotate, n.joint_orient))).collect();
        let max_inf = weights.iter().map(|w| w.len()).max().unwrap_or(4).max(1);
        b.app.scene.node_mut(mid).unwrap().skin = Some(Skin { joints: sk.ids.clone(), inv_bind, bind_shape: skel_world * geom_bind, weights, max_influences: max_inf, bind_pose });
        b.rep.skins += 1;
        // Blend-shape weights from the skeleton's animation.
        if let Some(an) = sk.anim.as_ref().and_then(|a| b.all.get(a)).cloned() {
            let names = an.val("blendShapes").map(|v| v.strs()).unwrap_or_default();
            let wp = an.prop("blendShapeWeights").cloned();
            let mesh_names = p.val("skel:blendShapes").map(|v| v.strs()).unwrap_or_default();
            if let (Some(wp), Some(bs)) = (wp, b.app.scene.node_mut(mid).and_then(|n| n.blend.as_mut())) {
                for (k, mn) in mesh_names.iter().enumerate() {
                    let Some(ai) = names.iter().position(|x| x == mn) else { continue };
                    if k >= bs.targets.len() {
                        continue;
                    }
                    if wp.samples.len() > 1 {
                        let mut c = Curve { tangent: Tangent::Linear, ..Default::default() };
                        for (t, v) in &wp.samples {
                            c.set_key(*t as f32, v.nums().get(ai).copied().unwrap_or(0.0) as f32);
                        }
                        bs.curves[k] = Some(c);
                        b.rep.anim_curves += 1;
                        b.times.0 = b.times.0.min(wp.samples[0].0);
                        b.times.1 = b.times.1.max(wp.samples[wp.samples.len() - 1].0);
                    } else if let Some(v) = wp.at(None) {
                        bs.weights[k] = v.nums().get(ai).copied().unwrap_or(0.0) as f32;
                    }
                }
            }
        }
    }
    let mut rep = std::mem::take(&mut b.rep);
    let times = b.times;
    drop(b);
    // Playback range from the layer (or the imported keys).
    let (s, e) = (layer.meta_num("startTimeCode"), layer.meta_num("endTimeCode"));
    match (s, e) {
        (Some(s), Some(e)) if e > s => {
            app.scene.play_start = s as f32;
            app.scene.play_end = e as f32;
        }
        _ if times.1 > times.0 => {
            app.scene.play_start = times.0 as f32;
            app.scene.play_end = times.1 as f32;
        }
        _ => {}
    }
    app.scene.anim_start = app.scene.anim_start.min(app.scene.play_start);
    app.scene.anim_end = app.scene.anim_end.max(app.scene.play_end);
    if fix != Mat4::IDENTITY {
        rep.notes.push("converted to Y-up metres".into());
    }
    app.sel = roots;
    let t = app.scene.play_start;
    app.set_time(t);
    app.end_batch();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::execute;

    fn scene_with_everything(dir: &Path) -> App {
        let mut app = App::new();
        app.scene.nodes.clear();
        let png = dir.join("art.png");
        std::fs::write(&png, crate::texture::TexImage::checker().to_png()).unwrap();
        for m in [
            "polyCube -w 0.5 -h 0.8 -d 0.3 -sy 4 -n torso; move 0 1.2 0; polyCube -w 1.6 -h 0.12 -d 0.12 -sx 12 -n arms; move 0 1.47 0".to_string(),
            "polyCube -w 0.4 -h 0.9 -d 0.15 -sy 6 -sx 2 -n legs; move 0 0.45 0; polyUnite torso arms legs; rename polySurface1 body".into(),
            "duplicate -n bodyFat body; scale -r 1.3 1 1.3 bodyFat; blendShape -n bs bodyFat body; delete bodyFat".into(),
            "rigAnimate -anim walk -s 1 -e 24 body".into(),
            "setKeyframe -t 1 -at bodyFat -v 0 body; setKeyframe -t 24 -at bodyFat -v 1 body".into(),
            "group -n char_grp root_ctrl; move 2 0 1 char_grp; rotate 0 30 0 char_grp".into(),
            "polySphere -r 0.5 -n ball; setKeyframe -t 1 -at ty -v 0.5 ball; setKeyframe -t 12 -at ty -v 3 ball; setKeyframe -t 24 -at ty -v 0.5 ball; setKeyframe -t 1 -at rz -v 0 ball; setKeyframe -t 24 -at rz -v 270 ball".into(),
            "shadingNode -asShader standardSurface -n art -rgb 0.9 0.2 0.2; setAttr art.roughness 0.35; setAttr art.metalness 0.5".into(),
            format!("setAttr art.colorMap \"{}\"; setAttr art.normalMap \"{}\"; setAttr art.repeatUV 2 3; hyperShade -assign art ball", png.display(), png.display()),
            "directionalLight -n sun -i 1.5 -rot -50 30 0 -sh on; spotLight -n key -ca 40 -pen 6 -p 0 5 5 -aim 0 0 0; areaLight -n soft -w 3 -h 1 -i 2 -p 3 4 0 -aim 0 0 0; ambientLight -n amb -i 0.2".into(),
            "camera -n shot -fl 50 -p 0 2 10 -aim 0 1 0; playbackOptions -min 1 -max 24".into(),
        ] {
            execute(&mut app, &m).unwrap_or_else(|e| panic!("{m}: {e}"));
        }
        app
    }

    /// The source evaluated like the exporter samples it: frame by frame from the start
    /// (IK solves warm-start from the previous frame).
    fn source_points(app: &App, name: &str, frame: f32) -> Vec<Vec3> {
        let mut sc = app.scene.clone();
        let mut f = app.scene.play_start;
        while f <= frame + 1e-4 {
            sc.time = f;
            sc.evaluate();
            crate::rig::evaluate(&mut sc);
            f += 1.0;
        }
        let id = sc.find_by_name(name).unwrap();
        crate::rig::deformed_positions(&sc, id).unwrap_or_else(|| {
            let w = sc.world_matrix(id);
            sc.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        })
    }

    fn world_points(app: &mut App, name: &str, frame: f32) -> Vec<Vec3> {
        app.set_time(frame);
        crate::rig::evaluate(&mut app.scene);
        let id = app.scene.find_by_name(name).unwrap_or_else(|| panic!("missing {name}"));
        crate::rig::deformed_positions(&app.scene, id).unwrap_or_else(|| {
            let w = app.scene.world_matrix(id);
            app.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect()
        })
    }

    fn check_round_trip(src: &mut App, path: &Path) {
        let mut dst = App::new();
        dst.scene.nodes.clear();
        let msg = execute(&mut dst, &format!("file -import \"{}\"", path.display())).unwrap();
        eprintln!("{msg}");
        assert!(msg.contains("1 skins") && msg.contains("1 blend-shape targets"), "{msg}");
        for f in [1.0, 7.0, 13.0, 24.0] {
            for name in ["body", "ball"] {
                let a = source_points(src, name, f);
                let b = world_points(&mut dst, name, f);
                assert_eq!(a.len(), b.len(), "{name}");
                let d = a.iter().zip(&b).map(|(x, y)| x.distance(*y)).fold(0.0f32, f32::max);
                assert!(d < 2e-3, "{name} frame {f}: max deviation {d}");
            }
        }
        let s = &dst.scene;
        let light = |n: &str| match &s.node(s.find_by_name(n).unwrap()).unwrap().shape {
            Shape::Light(l) => l.clone(),
            o => panic!("{n} is {o:?}"),
        };
        assert_eq!(light("sun").kind, LightKind::Directional);
        assert!((light("sun").intensity - 1.5).abs() < 1e-4 && light("sun").shadows);
        let key = light("key");
        assert_eq!(key.kind, LightKind::Spot);
        assert!((key.cone - 40.0).abs() < 0.1 && (key.penumbra - 6.0).abs() < 0.1, "{key:?}");
        assert_eq!(light("soft").kind, LightKind::Area);
        let soft = s.node(s.find_by_name("soft").unwrap()).unwrap();
        assert!((soft.scale.x - 1.5).abs() < 1e-3 && (soft.scale.y - 0.5).abs() < 1e-3, "{:?}", soft.scale);
        assert_eq!(light("amb").kind, LightKind::Ambient);
        match &s.node(s.find_by_name("shot").unwrap()).unwrap().shape {
            Shape::Camera(c) => assert!((c.focal - 50.0).abs() < 1e-3),
            o => panic!("{o:?}"),
        }
        let art = dst.scene.materials.iter().find(|m| m.name == "art").expect("material");
        assert!(art.color_map.as_ref().is_some_and(|p| Path::new(p).exists()), "{:?}", art.color_map);
        assert!(art.normal_map.is_some() && art.repeat_uv == [2.0, 3.0]);
        assert!((art.roughness - 0.35).abs() < 1e-4 && (art.metalness - 0.5).abs() < 1e-4);
        assert_eq!((dst.scene.play_start, dst.scene.play_end), (1.0, 24.0));
        // Controls (curves) came back as curves.
        assert!(matches!(s.node(s.find_by_name("L_foot_ctrl").unwrap()).unwrap().shape, Shape::Curve { .. }));
    }

    #[test]
    fn usda_round_trip() {
        let dir = std::env::temp_dir().join("rm_usd_rt");
        let _ = std::fs::create_dir_all(&dir);
        let mut src = scene_with_everything(&dir);
        let usda = dir.join("scene.usda");
        execute(&mut src, &format!("file -export \"{}\"", usda.display())).unwrap();
        check_round_trip(&mut src, &usda);
    }

    /// Re-serialise our export with Pixar's USD (different formatting, explicit defaults)
    /// and import that: `pip install usd-core`, then `cargo test usd_pxr -- --ignored`.
    #[test]
    #[ignore]
    fn usd_pxr_reserialised_round_trip() {
        let dir = std::env::temp_dir().join("rm_usd_rt_pxr");
        let _ = std::fs::create_dir_all(&dir);
        let mut src = scene_with_everything(&dir);
        let usda = dir.join("scene.usda");
        execute(&mut src, &format!("file -export \"{}\"", usda.display())).unwrap();
        let flat = dir.join("flat.usda");
        let py = format!("from pxr import Usd\ns = Usd.Stage.Open(r'{}')\ns.Export(r'{}')\n", usda.display(), flat.display());
        let out = std::process::Command::new("python3").arg("-c").arg(py).output().expect("python3");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        check_round_trip(&mut src, &flat);
    }

    #[test]
    fn z_up_units_ops_and_left_handed() {
        let dir = std::env::temp_dir().join("rm_usd_zup");
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("zup.usda");
        std::fs::write(
            &f,
            r#"#usda 1.0
(
    defaultPrim = "Root"
    metersPerUnit = 0.01
    upAxis = "Z"
    """A test layer"""
)

def Xform "Root" (
    kind = "component"
)
{
    double3 xformOp:translate = (0, 0, 100)
    float3 xformOp:rotateXYZ.timeSamples = {
        0: (0, 0, 0),
        10: (0, 0, 90),
    }
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]

    def Mesh "tri" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (100, 0, 0), (0, 100, 0)]
        uniform token orientation = "leftHanded"
        texCoord2f[] primvars:st = [(0, 0), (1, 0), (0, 1)] (
            interpolation = "vertex"
        )
        matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 50, 1) )
        uniform token[] xformOpOrder = ["xformOp:transform"]
    }
}
"#,
        )
        .unwrap();
        let mut app = App::new();
        app.scene.nodes.clear();
        let msg = execute(&mut app, &format!("file -import \"{}\"", f.display())).unwrap();
        assert!(msg.contains("1 meshes") && msg.contains("Y-up"), "{msg}");
        let id = app.scene.find_by_name("tri").unwrap();
        app.set_time(0.0);
        let w = app.scene.world_matrix(id);
        let m = app.scene.node(id).unwrap().mesh().unwrap().clone();
        let p: Vec<Vec3> = m.positions.iter().map(|p| w.transform_point3(*p)).collect();
        // Z-up 1.5 m up (root 1 m + mesh 0.5 m) → Y = 1.5; (100,0,0) cm → (1,0,0) m; (0,100,0) → (0,0,-1).
        assert!(p[0].distance(Vec3::new(0.0, 1.5, 0.0)) < 1e-4, "{:?}", p[0]);
        assert!(p[1].distance(Vec3::new(1.0, 1.5, 0.0)) < 1e-4, "{:?}", p[1]);
        assert!(p[2].distance(Vec3::new(0.0, 1.5, -1.0)) < 1e-4, "{:?}", p[2]);
        // Left-handed winding flipped: the face normal still points along +Z-up (= +Y).
        let fp: Vec<Vec3> = m.faces[0].iter().map(|v| p[*v as usize]).collect();
        let n = (fp[1] - fp[0]).cross(fp[2] - fp[0]);
        assert!(n.y < 0.0, "left-handed triangle faces down after the flip: {n:?}");
        assert!(m.has_uvs());
        // Animated rotateXYZ: by frame 10 the root has turned 90° around Z-up (Y).
        app.set_time(10.0);
        let w = app.scene.world_matrix(id);
        let q = w.transform_point3(Vec3::new(100.0, 0.0, 0.0));
        assert!(q.distance(Vec3::new(0.0, 1.5, -1.0)) < 1e-3, "{q:?}");
        assert!(execute(&mut app, "file -import x.usdc").is_err());
    }

    /// Composition: subLayers, a binary (.usdc) reference with a variant overridden by the
    /// referencing prim, inherits from a class, a PointInstancer, active = false, intrinsic
    /// primitives (axis), and a MaterialX look with an image texture.
    #[test]
    fn composition_usdc_instancer_and_materialx() {
        let dir = std::env::temp_dir().join("rm_usd_compose");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tex")).unwrap();
        image::RgbImage::from_pixel(4, 4, image::Rgb([200, 20, 20])).save(dir.join("tex/red.png")).unwrap();
        let geo = r#"#usda 1.0
(
    defaultPrim = "Asset"
)
def Xform "Asset" (
    variants = {
        string shape = "box"
    }
    prepend variantSets = "shape"
)
{
    variantSet "shape" = {
        "ball" {
            def Sphere "Geo"
            {
                double radius = 0.5
            }
        }
        "box" {
            def Cube "Geo"
            {
                double size = 1
            }
        }
    }
}
"#;
        let data = openusd::usda::parse(geo).unwrap();
        openusd::usdc::CrateWriter::write_to_file(&data, dir.join("geo.usdc")).unwrap();
        std::fs::write(
            dir.join("look.mtlx"),
            r#"<?xml version="1.0"?>
<materialx version="1.38">
  <nodegraph name="NG_red">
    <image name="img" type="color3"><input name="file" type="filename" value="tex/red.png" /></image>
    <output name="base_color_output" type="color3" nodename="img" />
  </nodegraph>
  <standard_surface name="SR_red" type="surfaceshader">
    <input name="base_color" type="color3" nodegraph="NG_red" output="base_color_output" />
    <input name="metalness" type="float" value="0.25" />
    <input name="specular_roughness" type="float" value="0.3" />
  </standard_surface>
  <surfacematerial name="M_red" type="material">
    <input name="surfaceshader" type="surfaceshader" nodename="SR_red" />
  </surfacematerial>
</materialx>
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("sub.usda"),
            r#"#usda 1.0
over "World"
{
    def Cone "FromSub"
    {
    }
}
"#,
        )
        .unwrap();
        let main = r#"#usda 1.0
(
    subLayers = [@./sub.usda@]
)
class "_Raised"
{
    double3 xformOp:translate = (0, 5, 0)
    uniform token[] xformOpOrder = ["xformOp:translate"]
}
def Xform "World"
{
    def Xform "A" (
        prepend references = @./geo.usdc@
        variants = {
            string shape = "ball"
        }
    )
    {
        double3 xformOp:translate = (2, 0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
    def Xform "B" (
        prepend references = @./geo.usdc@
    )
    {
        over "Geo"
        {
            rel material:binding = </World/Looks/M_red>
        }
    }
    def Xform "C" (
        inherits = </_Raised>
    )
    {
        def Cylinder "Cyl"
        {
        }
    }
    def Scope "Looks" (
        prepend references = @./look.mtlx@</MaterialX/Materials>
    )
    {
    }
    def PointInstancer "PI"
    {
        point3f[] positions = [(0, 0, -3), (1, 0, -3), (2, 0, -3)]
        int[] protoIndices = [0, 0, 0]
        rel prototypes = </World/PI/Proto>
        def Xform "Proto"
        {
            def Cube "Box"
            {
                double size = 0.5
            }
        }
    }
    def Xform "Off" (
        active = false
    )
    {
        def Cube "Hidden"
        {
        }
    }
}
"#;
        std::fs::write(dir.join("main.usda"), main).unwrap();
        let mut app = App::new();
        app.scene.nodes.clear();
        let r = import(&mut app, &dir.join("main.usda")).unwrap();
        assert!(r.notes.is_empty(), "{:?}", r.notes);
        let bounds = |app: &App, name: &str| {
            let id = app.scene.find_by_name(name).unwrap_or_else(|| panic!("missing {name}"));
            let w = app.scene.world_matrix(id);
            let ps: Vec<Vec3> = app.scene.node(id).unwrap().mesh().unwrap().positions.iter().map(|p| w.transform_point3(*p)).collect();
            let lo = ps.iter().fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
            let hi = ps.iter().fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
            (lo, hi, ps.len())
        };
        // A: the referencing prim's variant selection (ball) beats the asset's default (box).
        let (lo, hi, n) = bounds(&app, "A");
        assert!(n > 8 && (lo - Vec3::new(1.5, -0.5, -0.5)).length() < 1e-3 && (hi - Vec3::new(2.5, 0.5, 0.5)).length() < 1e-3, "{lo} {hi} {n}");
        // B: the default variant (box, size 1) from the .usdc, with the MaterialX look.
        let (lo, hi, n) = bounds(&app, "B");
        assert!(n == 8 && (lo + Vec3::splat(0.5)).length() < 1e-4 && (hi - Vec3::splat(0.5)).length() < 1e-4, "{lo} {hi} {n}");
        let bid = app.scene.find_by_name("B").unwrap();
        let m = app.material(app.scene.node(bid).unwrap().material.expect("B has a material")).unwrap();
        assert_eq!(m.name, "M_red");
        assert!(m.color_map.as_deref().is_some_and(|p| p.ends_with("tex/red.png") && Path::new(p).exists()), "{:?}", m.color_map);
        assert!((m.metalness - 0.25).abs() < 1e-6 && (m.roughness - 0.3).abs() < 1e-6);
        // C: inherited translate; Cylinder along Z (radius 1, height 2).
        let (lo, hi, _) = bounds(&app, "C");
        assert!((lo - Vec3::new(-1.0, 4.0, -1.0)).length() < 1e-3 && (hi - Vec3::new(1.0, 6.0, 1.0)).length() < 1e-3, "{lo} {hi}");
        // PointInstancer: three boxes at the instance positions.
        let pi = app.scene.find_by_name("PI").unwrap();
        let groups: Vec<_> = app.scene.nodes.iter().filter(|n| n.parent == Some(pi)).map(|n| n.id).collect();
        assert_eq!(groups.len(), 3);
        for (g, x) in groups.iter().zip([0.0, 1.0, 2.0]) {
            let box_id = app.scene.nodes.iter().find(|n| n.parent == Some(*g)).unwrap().id;
            let w = app.scene.world_matrix(box_id);
            assert!(w.transform_point3(Vec3::ZERO).distance(Vec3::new(x, 0.0, -3.0)) < 1e-4);
        }
        // SubLayer contributes a cone (axis Z: apex toward +Z); the inactive prim is skipped.
        let (lo, hi, _) = bounds(&app, "FromSub");
        assert!((hi.z - 1.0).abs() < 1e-4 && (lo.z + 1.0).abs() < 1e-4, "{lo} {hi}");
        assert!(app.scene.find_by_name("Off").is_none() && app.scene.find_by_name("Hidden").is_none());
        // The same layer packaged as .usdz imports identically.
        let z = dir.join("pkg.usdz");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
            let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for f in ["main.usda", "sub.usda", "geo.usdc", "look.mtlx", "tex/red.png"] {
                w.start_file(f, opt).unwrap();
                std::io::Write::write_all(&mut w, &std::fs::read(dir.join(f)).unwrap()).unwrap();
            }
            w.finish().unwrap();
        }
        let mut app2 = App::new();
        app2.scene.nodes.clear();
        let r2 = import(&mut app2, &z).unwrap();
        assert_eq!((r2.meshes, r2.materials, r2.textures), (r.meshes, r.materials, r.textures));
        assert_eq!(bounds(&app2, "A").0, bounds(&app, "A").0);
    }
}

#[cfg(test)]
mod corpus_tests {
    use crate::app::App;
    use glam::Vec3;

    /// `USD_CORPUS=dir cargo test usd_corpus -- --ignored --nocapture` imports every .usda below dir.
    #[test]
    #[ignore]
    fn usd_corpus() {
        let Ok(dir) = std::env::var("USD_CORPUS") else { return };
        let mut files = Vec::new();
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "usda" || x == "usd" || x == "usdc" || x == "usdz") && !p.to_string_lossy().ends_with(".flat.usda") {
                    out.push(p);
                }
            }
        }
        walk(std::path::Path::new(&dir), &mut files);
        files.sort();
        let (mut ok, mut bad, mut diff) = (0, 0, 0);
        // World-space summary of all imported mesh points: (mesh count, min, max).
        fn mesh_bounds(app: &mut App) -> (usize, Vec3, Vec3) {
            crate::rig::evaluate(&mut app.scene);
            let (mut n, mut lo, mut hi) = (0, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let ids: Vec<_> = app.scene.nodes.iter().filter(|x| x.mesh().is_some()).map(|x| x.id).collect();
            for id in ids {
                n += 1;
                let w = app.scene.world_matrix(id);
                for p in &app.scene.node(id).unwrap().mesh().unwrap().positions {
                    let q = w.transform_point3(*p);
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
            }
            (n, lo, hi)
        }
        for f in &files {
            let mut app = App::new();
            app.scene.nodes.clear();
            match super::import(&mut app, f) {
                Ok(r) => {
                    ok += 1;
                    eprintln!("OK   {} — {}", f.display(), r.summary());
                    // Composition check against pxr's flattened stage, when present.
                    let flat = f.with_extension("flat.usda");
                    if flat.exists() {
                        let mut fa = App::new();
                        fa.scene.nodes.clear();
                        if super::import(&mut fa, &flat).is_ok() {
                            let (a, b) = (mesh_bounds(&mut app), mesh_bounds(&mut fa));
                            let close = a.0 == b.0 && (a.0 == 0 || (a.1 - b.1).abs().max_element() < 1e-3 * (1.0 + b.2.abs().max_element()) && (a.2 - b.2).abs().max_element() < 1e-3 * (1.0 + b.2.abs().max_element()));
                            if !close {
                                diff += 1;
                                eprintln!("DIFF {} — ours {a:?} vs flattened {b:?}", f.display());
                                let names = |app: &App| app.scene.nodes.iter().filter(|x| x.mesh().is_some()).map(|x| x.name.clone()).collect::<Vec<_>>();
                                eprintln!("     meshes: ours {:?}\n             flat {:?}", names(&app), names(&fa));
                                // Per-mesh world bounds, to point at the prims that differ.
                                let per = |app: &App| -> Vec<(String, Vec3, Vec3, usize)> {
                                    app.scene.nodes.iter().filter_map(|x| {
                                        let m = x.mesh()?;
                                        let w = app.scene.world_matrix(x.id);
                                        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                                        for p in &m.positions {
                                            let q = w.transform_point3(*p);
                                            lo = lo.min(q);
                                            hi = hi.max(q);
                                        }
                                        Some((x.name.clone(), lo, hi, m.positions.len()))
                                    }).collect()
                                };
                                for (x, y) in per(&app).iter().zip(per(&fa)).filter(|(x, y)| (x.1 - y.1).abs().max_element() > 1e-3 || (x.2 - y.2).abs().max_element() > 1e-3).take(4) {
                                    eprintln!("     {x:?}\n  vs {y:?}");
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    bad += 1;
                    eprintln!("FAIL {} — {e}", f.display());
                }
            }
        }
        eprintln!("{ok} imported, {bad} failed, {diff} differ from pxr-flattened");
    }

    /// `USD_RENDER=file.usd[a|c|z] USD_OUT=x.png [USD_CAM="px py pz ax ay az"] cargo test usd_render -- --ignored`
    /// path-traces an imported stage (adds a studio light rig when the stage has no lights).
    #[test]
    #[ignore]
    fn usd_render() {
        let Ok(f) = std::env::var("USD_RENDER") else { return };
        let out = std::env::var("USD_OUT").unwrap_or_else(|_| "/tmp/usd_render.png".into());
        let mut app = App::new();
        app.scene.nodes.clear();
        let r = super::import(&mut app, std::path::Path::new(&f)).unwrap();
        eprintln!("{}", r.summary());
        if std::env::var("USD_DEBUG").is_ok() {
            for m in &app.scene.materials {
                eprintln!("mat {} color {:?} emis {:?} map {:?} exists {:?}", m.name, m.color, m.emission, m.color_map, m.color_map.as_ref().map(|p| std::path::Path::new(p).exists()));
            }
            for n in app.scene.nodes.iter().filter(|n| n.mesh().is_some()) {
                eprintln!("mesh {} mat {:?} uvs {}", n.name, n.material, n.mesh().unwrap().has_uvs());
            }
        }
        let cam = std::env::var("USD_CAM").unwrap_or_else(|_| "0 1 3 0 0 0".into());
        let c: Vec<&str> = cam.split_whitespace().collect();
        crate::commands::execute(&mut app, &format!("camera -p {} {} {} -aim {} {} {} -n rcam", c[0], c[1], c[2], c[3], c[4], c[5])).unwrap();
        if r.lights == 0 {
            crate::commands::execute(&mut app, "lightRig -preset studio").unwrap();
        }
        let spp = std::env::var("USD_SPP").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
        let dim = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let s = crate::raytrace::RenderSettings { width: dim("USD_W", 640), height: dim("USD_H", 400), spp, env: 0.4, env_background: true, ..Default::default() };
        let mut job = app.render_now(Some("rcam"), s).unwrap();
        eprintln!("rendered in {:.1}s, {} tris", job.elapsed().as_secs_f32(), job.tri_count);
        job.save_png(std::path::Path::new(&out), 0.0, crate::raytrace::ToneMap::Filmic).unwrap();
    }

}
