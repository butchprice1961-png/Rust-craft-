//! Protocol 47 data types: big-endian numbers, VarInts, length-prefixed
//! strings, packed block positions and item slots.

use glam::IVec3;

use crate::{Error, Result};

/// A read cursor over one packet body. Every read checks the length, so a
/// short or hostile packet is an error, never a panic.
pub struct Buf<'a>(pub &'a [u8]);

macro_rules! fixed {
    ($($name:ident: $t:ty),*) => {$(
        pub fn $name(&mut self) -> Result<$t> {
            Ok(<$t>::from_be_bytes(self.take(size_of::<$t>())?.try_into().unwrap()))
        }
    )*};
}

impl<'a> Buf<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Malformed("packet ends early"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fixed!(u8: u8, i8: i8, u16: u16, i16: i16, i32: i32, i64: i64, f32: f32, f64: f64);

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn varint(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            value |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(Error::Malformed("VarInt longer than 5 bytes"))
    }

    /// A VarInt that must be a length between 0 and `max`.
    pub fn length(&mut self, max: usize) -> Result<usize> {
        let n = self.varint()?;
        if n < 0 || n as usize > max {
            return Err(Error::Malformed("length out of range"));
        }
        Ok(n as usize)
    }

    pub fn string(&mut self, max_chars: usize) -> Result<String> {
        let n = self.length(max_chars * 4)?;
        let s = std::str::from_utf8(self.take(n)?).map_err(|_| Error::Malformed("string is not UTF-8"))?;
        Ok(s.to_string())
    }

    pub fn bytes(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = self.length(max)?;
        self.take(n)
    }

    pub fn position(&mut self) -> Result<IVec3> {
        let v = self.i64()?;
        Ok(IVec3::new((v >> 38) as i32, ((v >> 26) & 0xFFF) as i32, ((v << 38) >> 38) as i32))
    }

    /// An item stack; NBT (enchantments, names) is skipped.
    pub fn slot(&mut self) -> Result<Option<Item>> {
        let id = self.i16()?;
        if id < 0 {
            return Ok(None);
        }
        let count = self.u8()?;
        let damage = self.i16()?;
        if self.0.first() == Some(&0) {
            self.take(1)?;
        } else {
            let (_, _, used) = rustcraft_core::nbt::read(self.0).map_err(|_| Error::Malformed("bad item NBT"))?;
            self.take(used)?;
        }
        Ok(Some(Item { id, count, damage }))
    }

    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub id: i16,
    pub count: u8,
    pub damage: i16,
}

/// Writing helpers for packet bodies.
pub trait Put {
    fn varint(&mut self, v: i32) -> &mut Self;
    fn string(&mut self, s: &str) -> &mut Self;
    fn position(&mut self, p: IVec3) -> &mut Self;
    fn slot(&mut self, item: Option<Item>) -> &mut Self;
    fn be(&mut self, bytes: &[u8]) -> &mut Self;
}

impl Put for Vec<u8> {
    fn varint(&mut self, v: i32) -> &mut Self {
        let mut v = v as u32;
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.push(b);
                return self;
            }
            self.push(b | 0x80);
        }
    }

    fn string(&mut self, s: &str) -> &mut Self {
        self.varint(s.len() as i32);
        self.extend_from_slice(s.as_bytes());
        self
    }

    fn position(&mut self, p: IVec3) -> &mut Self {
        let v = ((p.x as i64 & 0x3FF_FFFF) << 38) | ((p.y as i64 & 0xFFF) << 26) | (p.z as i64 & 0x3FF_FFFF);
        self.be(&v.to_be_bytes())
    }

    fn slot(&mut self, item: Option<Item>) -> &mut Self {
        match item {
            None => self.be(&(-1i16).to_be_bytes()),
            Some(i) => self.be(&i.id.to_be_bytes()).be(&[i.count]).be(&i.damage.to_be_bytes()).be(&[0]),
        }
    }

    fn be(&mut self, bytes: &[u8]) -> &mut Self {
        self.extend_from_slice(bytes);
        self
    }
}

/// Plain text of a chat component (JSON), e.g. a kick message.
pub fn chat_text(json: &str) -> String {
    fn walk(v: &serde_json::Value, out: &mut String) {
        match v {
            serde_json::Value::String(s) => out.push_str(s),
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(o) => {
                if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                    out.push_str(t);
                }
                if let Some(key) = o.get("translate").and_then(|t| t.as_str()) {
                    let args: Vec<String> = o
                        .get("with")
                        .and_then(|w| w.as_array())
                        .into_iter()
                        .flatten()
                        .map(|a| {
                            let mut s = String::new();
                            walk(a, &mut s);
                            s
                        })
                        .collect();
                    out.push_str(&translate(key, &args));
                }
                if let Some(extra) = o.get("extra") {
                    walk(extra, out);
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(v) => walk(&v, &mut out),
        Err(_) => out.push_str(json),
    }
    strip_formatting(&out)
}

/// The few translation keys servers use for everyday chat. RustCraft ships
/// no language files; other keys show as the key and its arguments.
fn translate(key: &str, args: &[String]) -> String {
    let template = match key {
        "chat.type.text" => "<%s> %s",
        "chat.type.announcement" => "[%s] %s",
        "chat.type.emote" => "* %s %s",
        "chat.type.admin" => "[%s: %s]",
        "multiplayer.player.joined" => "%s joined the game",
        "multiplayer.player.left" => "%s left the game",
        "commands.message.display.incoming" => "%s whispers to you: %s",
        "commands.message.display.outgoing" => "You whisper to %s: %s",
        _ if args.is_empty() => return key.to_string(),
        _ => return format!("{key} ({})", args.join(", ")),
    };
    let mut args = args.iter();
    template.split("%s").enumerate().fold(String::new(), |mut out, (i, part)| {
        if i > 0 {
            out.push_str(args.next().map_or("", String::as_str));
        }
        out.push_str(part);
        out
    })
}

/// Removes `§x` formatting codes.
pub fn strip_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_match_the_protocol_examples() {
        let cases: [(i32, &[u8]); 7] = [
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (25565, &[0xdd, 0xc7, 0x01]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        ];
        for (value, bytes) in cases {
            let mut out = Vec::new();
            out.varint(value);
            assert_eq!(out, bytes, "{value}");
            assert_eq!(Buf(bytes).varint().unwrap(), value);
        }
        assert!(Buf(&[0xff; 6]).varint().is_err());
        assert!(Buf(&[0x80]).varint().is_err());
    }

    #[test]
    fn positions_and_slots_round_trip() {
        for p in [IVec3::new(0, 0, 0), IVec3::new(-1, 255, -1), IVec3::new(30_000_000, 64, -30_000_000)] {
            let mut out = Vec::new();
            out.position(p);
            assert_eq!(Buf(&out).position().unwrap(), p);
        }
        let mut out = Vec::new();
        out.slot(Some(Item { id: 1, count: 64, damage: 3 })).slot(None);
        let mut b = Buf(&out);
        assert_eq!(b.slot().unwrap(), Some(Item { id: 1, count: 64, damage: 3 }));
        assert_eq!(b.slot().unwrap(), None);
        // A slot with NBT (a named item) is skipped past correctly.
        let mut tag = rustcraft_core::nbt::Compound::new();
        tag.insert("Unbreakable".into(), rustcraft_core::nbt::Tag::Byte(1));
        let mut out = Vec::new();
        out.be(&276i16.to_be_bytes()).be(&[1]).be(&0i16.to_be_bytes()).be(&rustcraft_core::nbt::write("", &tag));
        out.be(&[7]);
        let mut b = Buf(&out);
        assert_eq!(b.slot().unwrap().unwrap().id, 276);
        assert_eq!(b.rest(), [7]);
    }

    #[test]
    fn hostile_lengths_are_errors() {
        let mut out = Vec::new();
        out.varint(1 << 30);
        assert!(Buf(&out).string(16).is_err());
        let mut out = Vec::new();
        out.varint(-5);
        assert!(Buf(&out).bytes(100).is_err());
        assert!(Buf(&[0, 0]).i32().is_err());
    }

    #[test]
    fn chat_components_become_plain_text() {
        assert_eq!(chat_text(r#"{"text":"§cBanned: ","extra":[{"text":"cheating"}]}"#), "Banned: cheating");
        assert_eq!(chat_text(r#""plain""#), "plain");
        assert_eq!(chat_text(r#"{"translate":"chat.type.text","with":["Steve",{"text":"hi"}]}"#), "<Steve> hi");
        assert_eq!(chat_text(r#"{"translate":"death.attack.fall","with":["Alex"]}"#), "death.attack.fall (Alex)");
        assert_eq!(chat_text("not json"), "not json");
    }
}
