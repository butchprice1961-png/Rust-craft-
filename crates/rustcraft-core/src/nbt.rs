//! Named Binary Tag (NBT), the format of Minecraft world files and of item
//! data in network packets.
//!
//! Input is treated as untrusted: lengths are checked against the remaining
//! bytes before allocating and nesting is capped, as the game itself does.
//! Strings are read as UTF-8 (lossily); Java's modified UTF-8 only differs
//! for NUL and characters outside the Basic Multilingual Plane.

use std::collections::BTreeMap;

pub type Compound = BTreeMap<String, Tag>;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NbtError {
    #[error("NBT data ended early")]
    Eof,
    #[error("unknown NBT tag type {0}")]
    BadType(u8),
    #[error("NBT nested deeper than {MAX_DEPTH} levels")]
    TooDeep,
    #[error("NBT root is not a compound")]
    NotCompound,
}

const MAX_DEPTH: usize = 512;

impl Tag {
    fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Tag::Byte(v) => v.into(),
            Tag::Short(v) => v.into(),
            Tag::Int(v) => v.into(),
            Tag::Long(v) => v,
            _ => return None,
        })
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Tag::Float(v) => Some(v.into()),
            Tag::Double(v) => Some(v),
            _ => self.as_i64().map(|v| v as f64),
        }
    }
}

/// Typed lookups on a compound.
pub trait CompoundExt {
    fn int(&self, key: &str) -> Option<i64>;
    fn float(&self, key: &str) -> Option<f64>;
    fn bytes(&self, key: &str) -> Option<&[u8]>;
    fn string(&self, key: &str) -> Option<&str>;
    fn list(&self, key: &str) -> Option<&[Tag]>;
    fn compound(&self, key: &str) -> Option<&Compound>;
}

impl CompoundExt for Compound {
    fn int(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_i64()
    }
    fn float(&self, key: &str) -> Option<f64> {
        self.get(key)?.as_f64()
    }
    fn bytes(&self, key: &str) -> Option<&[u8]> {
        match self.get(key)? {
            Tag::ByteArray(b) => Some(b),
            _ => None,
        }
    }
    fn string(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }
    fn list(&self, key: &str) -> Option<&[Tag]> {
        match self.get(key)? {
            Tag::List(l) => Some(l),
            _ => None,
        }
    }
    fn compound(&self, key: &str) -> Option<&Compound> {
        match self.get(key)? {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], NbtError> {
        if n > self.data.len() {
            return Err(NbtError::Eof);
        }
        let (head, rest) = self.data.split_at(n);
        self.data = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NbtError> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.array::<1>()?[0])
    }

    fn len(&mut self, elem_size: usize) -> Result<usize, NbtError> {
        let n = i32::from_be_bytes(self.array()?);
        let n = usize::try_from(n).map_err(|_| NbtError::Eof)?;
        // Reject lengths the remaining input cannot satisfy before allocating.
        if n.saturating_mul(elem_size) > self.data.len() {
            return Err(NbtError::Eof);
        }
        Ok(n)
    }

    fn string(&mut self) -> Result<String, NbtError> {
        let n = u16::from_be_bytes(self.array()?) as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn payload(&mut self, id: u8, depth: usize) -> Result<Tag, NbtError> {
        if depth > MAX_DEPTH {
            return Err(NbtError::TooDeep);
        }
        Ok(match id {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(i16::from_be_bytes(self.array()?)),
            3 => Tag::Int(i32::from_be_bytes(self.array()?)),
            4 => Tag::Long(i64::from_be_bytes(self.array()?)),
            5 => Tag::Float(f32::from_be_bytes(self.array()?)),
            6 => Tag::Double(f64::from_be_bytes(self.array()?)),
            7 => {
                let n = self.len(1)?;
                Tag::ByteArray(self.take(n)?.to_vec())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let elem = self.u8()?;
                let n = self.len(if elem == 0 { 0 } else { 1 })?;
                if n > 0 && elem == 0 {
                    return Err(NbtError::BadType(0));
                }
                let mut items = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    items.push(self.payload(elem, depth + 1)?);
                }
                Tag::List(items)
            }
            10 => Tag::Compound(self.compound(depth + 1)?),
            11 => {
                let n = self.len(4)?;
                Tag::IntArray(
                    self.take(n * 4)?.chunks_exact(4).map(|c| i32::from_be_bytes(c.try_into().unwrap())).collect(),
                )
            }
            12 => {
                let n = self.len(8)?;
                Tag::LongArray(
                    self.take(n * 8)?.chunks_exact(8).map(|c| i64::from_be_bytes(c.try_into().unwrap())).collect(),
                )
            }
            other => return Err(NbtError::BadType(other)),
        })
    }

    fn compound(&mut self, depth: usize) -> Result<Compound, NbtError> {
        let mut map = Compound::new();
        loop {
            let id = self.u8()?;
            if id == 0 {
                return Ok(map);
            }
            let name = self.string()?;
            map.insert(name, self.payload(id, depth)?);
        }
    }
}

/// Reads an uncompressed root compound, returning its name, contents and
/// the number of bytes consumed.
pub fn read(data: &[u8]) -> Result<(String, Compound, usize), NbtError> {
    let mut r = Reader { data };
    match r.u8()? {
        10 => {}
        _ => return Err(NbtError::NotCompound),
    }
    let name = r.string()?;
    let root = r.compound(0)?;
    Ok((name, root, data.len() - r.data.len()))
}

/// Writes a root compound.
pub fn write(name: &str, root: &Compound) -> Vec<u8> {
    let mut out = vec![10];
    write_string(&mut out, name);
    write_compound(&mut out, root);
    out
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    let bytes = &s.as_bytes()[..s.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn write_compound(out: &mut Vec<u8>, map: &Compound) {
    for (name, tag) in map {
        out.push(tag.id());
        write_string(out, name);
        write_payload(out, tag);
    }
    out.push(0);
}

fn write_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::ByteArray(b) => {
            out.extend_from_slice(&(b.len() as i32).to_be_bytes());
            out.extend_from_slice(b);
        }
        Tag::String(s) => write_string(out, s),
        Tag::List(items) => {
            out.push(items.first().map_or(0, Tag::id));
            out.extend_from_slice(&(items.len() as i32).to_be_bytes());
            for item in items {
                write_payload(out, item);
            }
        }
        Tag::Compound(c) => write_compound(out, c),
        Tag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes()));
        }
        Tag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            v.iter().for_each(|x| out.extend_from_slice(&x.to_be_bytes()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_types() {
        let mut inner = Compound::new();
        inner.insert("name".into(), Tag::String("Bananrama".into()));
        let mut root = Compound::new();
        for (k, v) in [
            ("b", Tag::Byte(-5)),
            ("s", Tag::Short(300)),
            ("i", Tag::Int(-70000)),
            ("l", Tag::Long(1 << 40)),
            ("f", Tag::Float(0.5)),
            ("d", Tag::Double(-2.25)),
            ("ba", Tag::ByteArray(vec![1, 2, 3])),
            ("str", Tag::String("héllo".into())),
            ("list", Tag::List(vec![Tag::Int(1), Tag::Int(2)])),
            ("empty", Tag::List(vec![])),
            ("c", Tag::Compound(inner)),
            ("ia", Tag::IntArray(vec![7, -7])),
            ("la", Tag::LongArray(vec![9])),
        ] {
            root.insert(k.into(), v);
        }
        let bytes = write("hello world", &root);
        let (name, parsed, used) = read(&bytes).unwrap();
        assert_eq!((name.as_str(), &parsed, used), ("hello world", &root, bytes.len()));
        assert_eq!(parsed.int("s"), Some(300));
        assert_eq!(parsed.compound("c").and_then(|c| c.string("name")), Some("Bananrama"));
    }

    #[test]
    fn hostile_input_is_rejected_without_huge_allocations() {
        // Byte array claiming 2 GiB of data.
        let mut evil = vec![10, 0, 0, 7, 0, 1, b'x'];
        evil.extend_from_slice(&i32::MAX.to_be_bytes());
        assert_eq!(read(&evil).unwrap_err(), NbtError::Eof);
        // List of compounds nested beyond the depth limit.
        let mut deep = vec![10, 0, 0];
        for _ in 0..600 {
            deep.extend_from_slice(&[10, 0, 1, b'a']);
        }
        assert_eq!(read(&deep).unwrap_err(), NbtError::TooDeep);
        assert_eq!(read(&[8, 0, 0]).unwrap_err(), NbtError::NotCompound);
        assert_eq!(read(&[10, 0, 0, 99, 0, 0]).unwrap_err(), NbtError::BadType(99));
        for cut in 0..20 {
            let full = write("", &Compound::from([("x".to_string(), Tag::Long(1))]));
            if cut < full.len() {
                assert!(read(&full[..cut]).is_err());
            }
        }
    }
}
