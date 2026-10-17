//! Minecraft's text form of NBT ("SNBT"), as `/data get entity` prints it:
//! `{Health: 20.0f, Inventory: [{Slot: 0b, id: "minecraft:stone", count: 3}]}`.
//! Parsed into the same [`Tag`] as the binary format, so a running player's
//! data reads the same way as their saved file.

use crate::nbt::Tag;

/// Why SNBT couldn't be read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unreadable SNBT at {at}: {what}")]
pub struct SnbtError {
    /// Byte offset.
    pub at: usize,
    /// What was expected.
    pub what: &'static str,
}

/// Nesting deeper than this is refused (real player data is shallow).
const MAX_DEPTH: usize = 128;

/// Parses one SNBT value.
pub fn parse(text: &str) -> Result<Tag, SnbtError> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
        depth: 0,
    };
    let value = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("the end"));
    }
    Ok(value)
}

/// The entity data in `/data get entity` output ("Steve has the following
/// entity data: {…}"), or `None` if the output is something else (no such
/// entity, an unknown command).
pub fn entity_data(output: &str) -> Option<Tag> {
    let marker = "entity data: ";
    let start = output.find(marker)? + marker.len();
    parse(output[start..].trim()).ok()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, what: &'static str) -> SnbtError {
        SnbtError { at: self.i, what }
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Tag, SnbtError> {
        self.ws();
        match self.peek() {
            Some(b'{') => self.nested(Self::compound),
            Some(b'[') => self.nested(Self::list),
            Some(b'"' | b'\'') => Ok(Tag::String(self.quoted()?)),
            Some(_) => Ok(scalar(&self.bare()?)),
            None => Err(self.err("a value")),
        }
    }

    fn nested(&mut self, inner: fn(&mut Self) -> Result<Tag, SnbtError>) -> Result<Tag, SnbtError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("less nesting"));
        }
        let value = inner(self);
        self.depth -= 1;
        value
    }

    fn compound(&mut self) -> Result<Tag, SnbtError> {
        self.i += 1; // {
        let mut entries = Vec::new();
        if self.eat(b'}') {
            return Ok(Tag::Compound(entries));
        }
        loop {
            self.ws();
            let key = match self.peek() {
                Some(b'"' | b'\'') => self.quoted()?,
                _ => self.bare()?,
            };
            if !self.eat(b':') {
                return Err(self.err("':'"));
            }
            entries.push((key, self.value()?));
            if self.eat(b',') {
                continue;
            }
            if self.eat(b'}') {
                return Ok(Tag::Compound(entries));
            }
            return Err(self.err("',' or '}'"));
        }
    }

    fn list(&mut self) -> Result<Tag, SnbtError> {
        self.i += 1; // [
        self.ws();
        // Typed arrays: [B; 1b, 2b], [I; 1, 2], [L; 1L].
        if let (Some(kind @ (b'B' | b'I' | b'L')), Some(b';')) =
            (self.peek(), self.s.get(self.i + 1).copied())
        {
            self.i += 2;
            let mut numbers = Vec::new();
            if !self.eat(b']') {
                loop {
                    let n = match scalar(&self.bare()?) {
                        Tag::Byte(n) => i64::from(n),
                        Tag::Short(n) => i64::from(n),
                        Tag::Int(n) => i64::from(n),
                        Tag::Long(n) => n,
                        _ => return Err(self.err("a whole number")),
                    };
                    numbers.push(n);
                    if self.eat(b',') {
                        continue;
                    }
                    if self.eat(b']') {
                        break;
                    }
                    return Err(self.err("',' or ']'"));
                }
            }
            return Ok(match kind {
                b'B' => Tag::ByteArray(numbers.into_iter().map(|n| n as i8).collect()),
                b'I' => Tag::IntArray(numbers.into_iter().map(|n| n as i32).collect()),
                _ => Tag::LongArray(numbers),
            });
        }
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Tag::List(items));
        }
        loop {
            items.push(self.value()?);
            if self.eat(b',') {
                continue;
            }
            if self.eat(b']') {
                return Ok(Tag::List(items));
            }
            return Err(self.err("',' or ']'"));
        }
    }

    fn quoted(&mut self) -> Result<String, SnbtError> {
        let quote = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\\' => {
                    let Some(next) = self.peek() else { break };
                    self.i += 1;
                    out.push(match next {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
                c if c == quote => {
                    return String::from_utf8(out).map_err(|_| self.err("UTF-8 text"));
                }
                c => out.push(c),
            }
        }
        Err(self.err("a closing quote"))
    }

    /// An unquoted key or value: letters, digits, `_-.+`.
    fn bare(&mut self) -> Result<String, SnbtError> {
        self.ws();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+') {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return Err(self.err("a name or value"));
        }
        Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }
}

/// An unquoted value: a number with an optional type suffix, a boolean, or
/// otherwise a string.
fn scalar(s: &str) -> Tag {
    match s {
        "true" => return Tag::Byte(1),
        "false" => return Tag::Byte(0),
        _ => {}
    }
    let (body, suffix) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() && i > 0 => (&s[..i], Some(c.to_ascii_lowercase())),
        _ => (s, None),
    };
    let number = match suffix {
        Some('b') => body.parse().ok().map(Tag::Byte),
        Some('s') => body.parse().ok().map(Tag::Short),
        Some('l') => body.parse().ok().map(Tag::Long),
        Some('f') => body.parse().ok().map(Tag::Float),
        Some('d') => body.parse().ok().map(Tag::Double),
        None if body.contains(['.', 'e', 'E']) => body.parse().ok().map(Tag::Double),
        None => body.parse().ok().map(Tag::Int),
        _ => None,
    };
    number.unwrap_or_else(|| Tag::String(s.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playerdata;

    #[test]
    fn values() {
        assert_eq!(parse("3b").unwrap(), Tag::Byte(3));
        assert_eq!(parse("-7s").unwrap(), Tag::Short(-7));
        assert_eq!(parse("42").unwrap(), Tag::Int(42));
        assert_eq!(parse("9L").unwrap(), Tag::Long(9));
        assert_eq!(parse("0.5f").unwrap(), Tag::Float(0.5));
        assert_eq!(parse("20.0d").unwrap(), Tag::Double(20.0));
        assert_eq!(parse("1.5").unwrap(), Tag::Double(1.5));
        assert_eq!(parse("true").unwrap(), Tag::Byte(1));
        assert_eq!(
            parse("\"a \\\"b\\\"\"").unwrap(),
            Tag::String("a \"b\"".into())
        );
        assert!(parse("'it''").is_err());
        assert_eq!(parse("[I; 1, -2]").unwrap(), Tag::IntArray(vec![1, -2]));
        assert_eq!(parse("[]").unwrap(), Tag::List(vec![]));
        assert!(parse("{a: 1").is_err());
        assert!(parse(&"[".repeat(500)).is_err(), "deep nesting is refused");
    }

    #[test]
    fn a_running_players_data_reads_like_their_file() {
        let out = r#"Steve has the following entity data: {DataVersion: 4440, Pos: [12.5d, 64.0d, -3.25d], Dimension: "minecraft:overworld", Health: 18.0f, foodLevel: 17, XpLevel: 5, XpP: 0.25f, playerGameType: 0, SelectedItemSlot: 2, Inventory: [{Slot: 0b, id: "minecraft:diamond_sword", count: 1, components: {"minecraft:enchantments": {"minecraft:sharpness": 5}, "minecraft:damage": 12}}, {Slot: 1b, id: "minecraft:oak_planks", count: 64}], equipment: {head: {id: "minecraft:iron_helmet", count: 1}}, EnderItems: [{Slot: 3b, id: "minecraft:ender_pearl", count: 16}]}"#;
        let data = playerdata::parse(&entity_data(out).unwrap());
        assert_eq!(data.health, Some(18.0));
        assert_eq!(data.food, Some(17));
        assert_eq!(data.selected_slot, Some(2));
        assert_eq!(data.position, Some([12.5, 64.0, -3.25]));
        assert_eq!(data.inventory.len(), 2);
        assert_eq!(data.inventory[0].id, "minecraft:diamond_sword");
        assert_eq!(data.inventory[1].count, 64);
        assert_eq!(
            data.equipment.head.as_ref().map(|i| i.id.as_str()),
            Some("minecraft:iron_helmet")
        );
        assert_eq!(data.ender_chest[0].count, 16);
        assert_eq!(entity_data("No entity was found"), None);
    }
}
