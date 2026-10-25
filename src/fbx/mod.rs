//! Autodesk FBX: a generic node tree with a binary reader / writer and an ASCII reader
//! (FBX 7.x), plus the scene importer (`import`) and exporter (`export`).
//!
//! Binary layout: a 27-byte header ("Kaydara FBX Binary  \0\x1a\0" + u32 version), then node
//! records: end offset, property count and byte length (u32, or u64 from version 7500), a
//! name, typed properties (scalars, strings, raw bytes, and arrays that may be zlib-deflated),
//! nested records and a null terminator record.

pub mod export;
pub mod import;

use std::io::{Read, Write};

#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Bool(bool),
    I16(i16),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Str(String),
    Raw(Vec<u8>),
    ArrBool(Vec<bool>),
    ArrI32(Vec<i32>),
    ArrI64(Vec<i64>),
    ArrF32(Vec<f32>),
    ArrF64(Vec<f64>),
}

impl Prop {
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            Prop::Bool(b) => *b as i32 as f64,
            Prop::I16(v) => *v as f64,
            Prop::I32(v) => *v as f64,
            Prop::I64(v) => *v as f64,
            Prop::F32(v) => *v as f64,
            Prop::F64(v) => *v,
            _ => return None,
        })
    }
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            Prop::Bool(b) => *b as i64,
            Prop::I16(v) => *v as i64,
            Prop::I32(v) => *v as i64,
            Prop::I64(v) => *v,
            Prop::F32(v) => *v as i64,
            Prop::F64(v) => *v as i64,
            _ => return None,
        })
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Prop::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn to_f64s(&self) -> Vec<f64> {
        match self {
            Prop::ArrF64(v) => v.clone(),
            Prop::ArrF32(v) => v.iter().map(|x| *x as f64).collect(),
            Prop::ArrI32(v) => v.iter().map(|x| *x as f64).collect(),
            Prop::ArrI64(v) => v.iter().map(|x| *x as f64).collect(),
            Prop::ArrBool(v) => v.iter().map(|x| *x as i32 as f64).collect(),
            other => other.as_f64().into_iter().collect(),
        }
    }
    pub fn to_i64s(&self) -> Vec<i64> {
        match self {
            Prop::ArrI32(v) => v.iter().map(|x| *x as i64).collect(),
            Prop::ArrI64(v) => v.clone(),
            Prop::ArrF64(v) => v.iter().map(|x| *x as i64).collect(),
            Prop::ArrF32(v) => v.iter().map(|x| *x as i64).collect(),
            Prop::ArrBool(v) => v.iter().map(|x| *x as i64).collect(),
            other => other.as_i64().into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: &str, props: Vec<Prop>) -> Node {
        Node { name: name.to_string(), props, children: Vec::new() }
    }
    pub fn with(mut self, children: Vec<Node>) -> Node {
        self.children = children;
        self
    }
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    /// A child's values as numbers: a binary array, an ASCII `*N { a: ... }` list, or inline values.
    pub fn array_f64(&self, name: &str) -> Vec<f64> {
        let Some(c) = self.child(name) else { return Vec::new() };
        if let Some(a) = c.child("a") {
            return a.props.iter().filter_map(|p| p.as_f64()).collect();
        }
        if c.props.len() == 1 {
            return c.props[0].to_f64s();
        }
        c.props.iter().filter_map(|p| p.as_f64()).collect()
    }
    pub fn array_i64(&self, name: &str) -> Vec<i64> {
        let Some(c) = self.child(name) else { return Vec::new() };
        if let Some(a) = c.child("a") {
            return a.props.iter().filter_map(|p| p.as_i64()).collect();
        }
        if c.props.len() == 1 {
            return c.props[0].to_i64s();
        }
        c.props.iter().filter_map(|p| p.as_i64()).collect()
    }
    pub fn str_of(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(|c| c.props.first()).and_then(|p| p.as_str())
    }
    pub fn num_of(&self, name: &str) -> Option<f64> {
        self.child(name).and_then(|c| c.props.first()).and_then(|p| p.as_f64())
    }
    /// `Properties70` entries: P: "name", "type", "label", "flags", values...
    pub fn p70(&self, key: &str) -> Option<&[Prop]> {
        let p = self.child("Properties70").or_else(|| self.child("Properties60"))?;
        p.children.iter().find(|c| (c.name == "P" || c.name == "Property") && c.props.first().and_then(|x| x.as_str()) == Some(key)).map(|c| {
            let skip = if c.name == "P" { 4 } else { 3 };
            &c.props[skip.min(c.props.len())..]
        })
    }
    pub fn p70_f64(&self, key: &str) -> Option<f64> {
        self.p70(key).and_then(|v| v.first()).and_then(|p| p.as_f64())
    }
    pub fn p70_vec3(&self, key: &str) -> Option<[f64; 3]> {
        let v = self.p70(key)?;
        if v.len() < 3 {
            return None;
        }
        Some([v[0].as_f64()?, v[1].as_f64()?, v[2].as_f64()?])
    }
}

/// A parsed document: the top-level nodes and the file version.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub version: u32,
    pub nodes: Vec<Node>,
}

impl Doc {
    pub fn get(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.name == name)
    }
}

const MAGIC: &[u8] = b"Kaydara FBX Binary  \x00";

pub fn parse(bytes: &[u8]) -> Result<Doc, String> {
    if bytes.starts_with(MAGIC) {
        parse_binary(bytes)
    } else {
        parse_ascii(&String::from_utf8_lossy(bytes))
    }
}

// ------------------------------------------------------------------------------ binary reader

struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.b.len() {
            return Err(format!("FBX: unexpected end of file at byte {}", self.pos));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn read_array<T>(r: &mut Rd, elem: usize, conv: impl Fn(&[u8]) -> T) -> Result<Vec<T>, String> {
    let len = r.u32()? as usize;
    let enc = r.u32()?;
    let clen = r.u32()? as usize;
    let raw = r.take(clen)?;
    let data: Vec<u8> = if enc == 1 {
        let mut out = Vec::with_capacity(len * elem);
        flate2::read::ZlibDecoder::new(raw).read_to_end(&mut out).map_err(|e| format!("FBX: bad compressed array ({e})"))?;
        out
    } else {
        raw.to_vec()
    };
    if data.len() < len * elem {
        return Err("FBX: array shorter than its length".into());
    }
    Ok(data[..len * elem].chunks_exact(elem).map(conv).collect())
}

fn read_prop(r: &mut Rd) -> Result<Prop, String> {
    let t = r.u8()?;
    Ok(match t {
        b'C' => Prop::Bool(r.u8()? != 0),
        b'Y' => Prop::I16(i16::from_le_bytes(r.take(2)?.try_into().unwrap())),
        b'I' => Prop::I32(i32::from_le_bytes(r.take(4)?.try_into().unwrap())),
        b'L' => Prop::I64(i64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        b'F' => Prop::F32(f32::from_le_bytes(r.take(4)?.try_into().unwrap())),
        b'D' => Prop::F64(f64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        b'S' => {
            let n = r.u32()? as usize;
            Prop::Str(String::from_utf8_lossy(r.take(n)?).into_owned())
        }
        b'R' => {
            let n = r.u32()? as usize;
            Prop::Raw(r.take(n)?.to_vec())
        }
        b'b' | b'c' => Prop::ArrBool(read_array(r, 1, |b| b[0] != 0)?),
        b'i' => Prop::ArrI32(read_array(r, 4, |b| i32::from_le_bytes(b.try_into().unwrap()))?),
        b'l' => Prop::ArrI64(read_array(r, 8, |b| i64::from_le_bytes(b.try_into().unwrap()))?),
        b'f' => Prop::ArrF32(read_array(r, 4, |b| f32::from_le_bytes(b.try_into().unwrap()))?),
        b'd' => Prop::ArrF64(read_array(r, 8, |b| f64::from_le_bytes(b.try_into().unwrap()))?),
        other => return Err(format!("FBX: unknown property type '{}' at byte {}", other as char, r.pos - 1)),
    })
}

fn read_node(r: &mut Rd, wide: bool) -> Result<Option<Node>, String> {
    let (end, nprops, _plen) = if wide { (r.u64()? as usize, r.u64()? as usize, r.u64()?) } else { (r.u32()? as usize, r.u32()? as usize, r.u32()? as u64) };
    let name_len = r.u8()? as usize;
    if end == 0 {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(r.take(name_len)?).into_owned();
    let mut props = Vec::with_capacity(nprops);
    for _ in 0..nprops {
        props.push(read_prop(r)?);
    }
    let mut children = Vec::new();
    while r.pos < end {
        match read_node(r, wide)? {
            Some(n) => children.push(n),
            None => break,
        }
    }
    if end > r.b.len() {
        return Err("FBX: node extends past the end of the file".into());
    }
    r.pos = end;
    Ok(Some(Node { name, props, children }))
}

fn parse_binary(b: &[u8]) -> Result<Doc, String> {
    if b.len() < 27 {
        return Err("FBX: file too short".into());
    }
    let version = u32::from_le_bytes(b[23..27].try_into().unwrap());
    let wide = version >= 7500;
    let mut r = Rd { b, pos: 27 };
    let mut nodes = Vec::new();
    let min_rec = if wide { 25 } else { 13 };
    while r.pos + min_rec <= b.len() {
        match read_node(&mut r, wide)? {
            Some(n) => nodes.push(n),
            None => break,
        }
    }
    Ok(Doc { version, nodes })
}

// ------------------------------------------------------------------------------ ASCII reader

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Key(String),
    Str(String),
    Num(String),
    Word(String),
    Star(usize),
    Open,
    Close,
    Comma,
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            ';' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '{' => {
                out.push(Tok::Open);
                i += 1;
            }
            '}' => {
                out.push(Tok::Close);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '"' => {
                let mut j = i + 1;
                let mut st = String::new();
                while j < c.len() && c[j] != '"' {
                    st.push(c[j]);
                    j += 1;
                }
                out.push(Tok::Str(st));
                i = j + 1;
            }
            '*' => {
                let mut j = i + 1;
                let mut n = String::new();
                while j < c.len() && c[j].is_ascii_digit() {
                    n.push(c[j]);
                    j += 1;
                }
                out.push(Tok::Star(n.parse().unwrap_or(0)));
                i = j;
            }
            c0 if c0.is_whitespace() => i += 1,
            _ => {
                let mut j = i;
                let mut w = String::new();
                while j < c.len() && !c[j].is_whitespace() && !matches!(c[j], ',' | '{' | '}' | '"' | ';') {
                    if c[j] == ':' {
                        break;
                    }
                    w.push(c[j]);
                    j += 1;
                }
                if j < c.len() && c[j] == ':' && !w.is_empty() && !w.starts_with(|x: char| x.is_ascii_digit() || x == '-' || x == '+' || x == '.') {
                    out.push(Tok::Key(w));
                    i = j + 1;
                } else if w.is_empty() {
                    i = j + 1;
                } else {
                    let is_num = w.parse::<f64>().is_ok();
                    out.push(if is_num { Tok::Num(w) } else { Tok::Word(w) });
                    i = j;
                }
            }
        }
    }
    out
}

fn ascii_nodes(t: &[Tok], i: &mut usize) -> Vec<Node> {
    let mut out = Vec::new();
    while *i < t.len() {
        match &t[*i] {
            Tok::Close => {
                *i += 1;
                return out;
            }
            Tok::Key(k) => {
                let mut node = Node::new(k, Vec::new());
                *i += 1;
                // Values until the next key / brace on this level.
                loop {
                    match t.get(*i) {
                        Some(Tok::Str(s)) => node.props.push(Prop::Str(s.clone())),
                        Some(Tok::Num(n)) => node.props.push(if n.contains(['.', 'e', 'E']) { Prop::F64(n.parse().unwrap_or(0.0)) } else { n.parse::<i64>().map(Prop::I64).unwrap_or_else(|_| Prop::F64(n.parse().unwrap_or(0.0))) }),
                        Some(Tok::Word(w)) => node.props.push(match w.as_str() {
                            "T" | "Y" => Prop::Bool(true),
                            "F" | "N" => Prop::Bool(false),
                            _ => Prop::Str(w.clone()),
                        }),
                        Some(Tok::Star(_)) | Some(Tok::Comma) => {}
                        _ => break,
                    }
                    *i += 1;
                }
                if t.get(*i) == Some(&Tok::Open) {
                    *i += 1;
                    node.children = ascii_nodes(t, i);
                }
                out.push(node);
            }
            _ => *i += 1,
        }
    }
    out
}

fn parse_ascii(s: &str) -> Result<Doc, String> {
    let toks = tokenize(s);
    let mut i = 0;
    let nodes = ascii_nodes(&toks, &mut i);
    if nodes.is_empty() {
        return Err("FBX: not an FBX file".into());
    }
    let version = nodes.iter().find(|n| n.name == "FBXHeaderExtension").and_then(|h| h.num_of("FBXVersion")).unwrap_or(0.0) as u32;
    Ok(Doc { version, nodes })
}

// ------------------------------------------------------------------------------ binary writer

fn write_array<T: Copy>(out: &mut Vec<u8>, code: u8, v: &[T], elem: usize, bytes: impl Fn(T) -> Vec<u8>) {
    out.push(code);
    let mut raw = Vec::with_capacity(v.len() * elem);
    for x in v {
        raw.extend(bytes(*x));
    }
    out.extend((v.len() as u32).to_le_bytes());
    if raw.len() > 128 {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        let z = enc.finish().unwrap();
        out.extend(1u32.to_le_bytes());
        out.extend((z.len() as u32).to_le_bytes());
        out.extend(z);
    } else {
        out.extend(0u32.to_le_bytes());
        out.extend((raw.len() as u32).to_le_bytes());
        out.extend(raw);
    }
}

fn write_prop(out: &mut Vec<u8>, p: &Prop) {
    match p {
        Prop::Bool(b) => out.extend([b'C', *b as u8]),
        Prop::I16(v) => {
            out.push(b'Y');
            out.extend(v.to_le_bytes());
        }
        Prop::I32(v) => {
            out.push(b'I');
            out.extend(v.to_le_bytes());
        }
        Prop::I64(v) => {
            out.push(b'L');
            out.extend(v.to_le_bytes());
        }
        Prop::F32(v) => {
            out.push(b'F');
            out.extend(v.to_le_bytes());
        }
        Prop::F64(v) => {
            out.push(b'D');
            out.extend(v.to_le_bytes());
        }
        Prop::Str(s) => {
            out.push(b'S');
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        }
        Prop::Raw(b) => {
            out.push(b'R');
            out.extend((b.len() as u32).to_le_bytes());
            out.extend(b);
        }
        Prop::ArrBool(v) => write_array(out, b'b', v, 1, |x| vec![x as u8]),
        Prop::ArrI32(v) => write_array(out, b'i', v, 4, |x| x.to_le_bytes().to_vec()),
        Prop::ArrI64(v) => write_array(out, b'l', v, 8, |x| x.to_le_bytes().to_vec()),
        Prop::ArrF32(v) => write_array(out, b'f', v, 4, |x| x.to_le_bytes().to_vec()),
        Prop::ArrF64(v) => write_array(out, b'd', v, 8, |x| x.to_le_bytes().to_vec()),
    }
}

/// FBX 7.4 (32-bit offsets) node record.
fn write_node(out: &mut Vec<u8>, base: usize, n: &Node) {
    let start = out.len();
    out.extend([0u8; 12]); // end offset, prop count, prop length (patched below)
    out.push(n.name.len() as u8);
    out.extend(n.name.as_bytes());
    let pstart = out.len();
    for p in &n.props {
        write_prop(out, p);
    }
    let plen = out.len() - pstart;
    // Nested list (with its null terminator) when there are children — and for nodes without
    // properties, which readers expect to open an (empty) scope.
    if !n.children.is_empty() || n.props.is_empty() {
        for c in &n.children {
            write_node(out, base, c);
        }
        out.extend([0u8; 13]);
    }
    let end = (base + out.len()) as u32;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&(n.props.len() as u32).to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&(plen as u32).to_le_bytes());
}

/// Fixed file id / creation time / footer id used by Blender's exporter (a combination the FBX
/// SDK accepts).
pub const FILE_ID: [u8; 16] = [0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b, 0xfc, 0xf1];
pub const CREATION_TIME: &str = "1970-01-01 10:00:00:000";
const FOOT_ID: [u8; 16] = [0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e];
const FOOT_MAGIC: [u8; 16] = [0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b];

pub fn write_binary(nodes: &[Node], version: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(MAGIC);
    out.extend([0x1a, 0x00]);
    out.extend(version.to_le_bytes());
    for n in nodes {
        let mut buf = Vec::new();
        write_node(&mut buf, out.len(), n);
        out.extend(buf);
    }
    out.extend([0u8; 13]);
    out.extend(FOOT_ID);
    out.extend([0u8; 4]);
    let pad = {
        let p = ((out.len() + 15) & !15) - out.len();
        if p == 0 { 16 } else { p }
    };
    out.extend(vec![0u8; pad]);
    out.extend(version.to_le_bytes());
    out.extend([0u8; 120]);
    out.extend(FOOT_MAGIC);
    out
}

// ------------------------------------------------------------------------------ helpers

/// FBX time: 46,186,158,000 ticks per second.
pub const TICKS_PER_SECOND: f64 = 46_186_158_000.0;

/// Object name without the class suffix / prefix ("Name\0\x01Model" or "Model::Name").
pub fn object_name(s: &str) -> String {
    if let Some(i) = s.find("\u{0}\u{1}") {
        return s[..i].to_string();
    }
    if let Some(i) = s.find("::") {
        return s[i + 2..].to_string();
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_round_trip_and_ascii() {
        let doc = vec![
            Node::new("Header", vec![Prop::I32(7400), Prop::Str("hi".into())]).with(vec![Node::new("Arr", vec![Prop::ArrF64((0..100).map(|x| x as f64 * 0.5).collect())]), Node::new("Small", vec![Prop::ArrI32(vec![1, -2, 3])])]),
            Node::new("Empty", vec![Prop::Bool(true), Prop::I64(-5), Prop::F32(1.5), Prop::Raw(vec![1, 2, 3])]),
        ];
        let bytes = write_binary(&doc, 7400);
        let back = parse(&bytes).unwrap();
        assert_eq!(back.version, 7400);
        assert_eq!(back.nodes, doc);
        assert_eq!(back.nodes[0].array_f64("Arr")[99], 49.5);
        assert_eq!(back.nodes[0].array_i64("Small"), vec![1, -2, 3]);
        let ascii = r#"; FBX 7.3.0 project file
FBXHeaderExtension:  {
	FBXVersion: 7300
}
Objects:  {
	Model: 123, "Model::Cube", "Mesh" {
		Properties70:  {
			P: "Lcl Translation", "Lcl Translation", "", "A",1.5,-2,3e1
		}
		Shading: T
	}
	Geometry: 9, "Geometry::", "Mesh" {
		Vertices: *6 {
			a: 0,1,2,
3,4.5,5
		}
		PolygonVertexIndex: *3 {
			a: 0,1,-3
		}
	}
}
Connections:  {
	C: "OO",123,0
}"#;
        let d = parse(ascii.as_bytes()).unwrap();
        assert_eq!(d.version, 7300);
        let objs = d.get("Objects").unwrap();
        let m = objs.child("Model").unwrap();
        assert_eq!(m.props[0].as_i64(), Some(123));
        assert_eq!(object_name(m.props[1].as_str().unwrap()), "Cube");
        assert_eq!(m.p70_vec3("Lcl Translation"), Some([1.5, -2.0, 30.0]));
        let g = objs.child("Geometry").unwrap();
        assert_eq!(g.array_f64("Vertices"), vec![0.0, 1.0, 2.0, 3.0, 4.5, 5.0]);
        assert_eq!(g.array_i64("PolygonVertexIndex"), vec![0, 1, -3]);
        assert_eq!(d.get("Connections").unwrap().children[0].props.len(), 3);
    }
}
