//! Java `.properties` files, as Minecraft's `server.properties` uses them.
//!
//! Parsing follows `java.util.Properties.load`: `#`/`!` comments, `=`, `:`,
//! or whitespace separators, backslash escapes, `\uXXXX`, and lines
//! continued with a trailing backslash. Writing escapes the way
//! `Properties.store` does, including non-ASCII as `\uXXXX`, so the result
//! reads back correctly whichever charset the server decodes with (MOTDs are
//! full of `§`). Untouched lines are kept byte-for-byte.

use std::fmt::Write as _;

/// A parsed properties file that can be edited and written back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// Blank lines and comments.
    Other(String),
    /// A key/value pair and the source text it came from.
    Entry {
        key: String,
        value: String,
        raw: String,
    },
}

impl Properties {
    /// Parses `text`. Never fails; malformed escapes are kept literally.
    pub fn parse(text: &str) -> Self {
        let natural: Vec<&str> = split_lines(text);
        let mut lines = Vec::new();
        let mut i = 0;
        while i < natural.len() {
            let line = natural[i];
            i += 1;
            let trimmed = line.trim_start_matches([' ', '\t', '\x0c']);
            if trimmed.is_empty() || trimmed.starts_with(['#', '!']) {
                lines.push(Line::Other(line.to_owned()));
                continue;
            }
            // Join continuation lines.
            let mut raw = line.to_owned();
            let mut logical = trimmed.to_owned();
            while ends_with_odd_backslashes(&logical) && i < natural.len() {
                logical.pop();
                let next = natural[i];
                i += 1;
                raw.push('\n');
                raw.push_str(next);
                logical.push_str(next.trim_start_matches([' ', '\t', '\x0c']));
            }
            if ends_with_odd_backslashes(&logical) {
                logical.pop();
            }
            let (key, value) = split_entry(&logical);
            lines.push(Line::Entry { key, value, raw });
        }
        Self { lines }
    }

    /// The value for `key` (the last one, if repeated).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().rev().find_map(|line| match line {
            Line::Entry { key: k, value, .. } if k == key => Some(value.as_str()),
            _ => None,
        })
    }

    /// All keys and values in file order; for repeated keys, the value
    /// that wins (the last) at the position of its first appearance.
    pub fn entries(&self) -> Vec<(&str, &str)> {
        let mut out: Vec<(&str, &str)> = Vec::new();
        for line in &self.lines {
            if let Line::Entry { key, value, .. } = line {
                match out.iter_mut().find(|(k, _)| k == key) {
                    Some(slot) => slot.1 = value,
                    None => out.push((key, value)),
                }
            }
        }
        out
    }

    /// Sets `key` to `value`, rewriting only that line (the last one, if
    /// repeated), or appending it.
    pub fn set(&mut self, key: &str, value: &str) {
        let raw = format!("{}={}", escape(key, true), escape(value, false));
        let existing = self.lines.iter_mut().rev().find_map(|line| match line {
            Line::Entry {
                key: k,
                value: v,
                raw: r,
            } if k == key => Some((v, r)),
            _ => None,
        });
        match existing {
            Some((v, r)) => {
                if v != value {
                    *v = value.to_owned();
                    *r = raw;
                }
            }
            None => self.lines.push(Line::Entry {
                key: key.to_owned(),
                value: value.to_owned(),
                raw,
            }),
        }
    }

    /// Removes every line for `key`.
    pub fn remove(&mut self, key: &str) {
        self.lines
            .retain(|line| !matches!(line, Line::Entry { key: k, .. } if k == key));
    }
}

impl std::fmt::Display for Properties {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for line in &self.lines {
            match line {
                Line::Other(raw) | Line::Entry { raw, .. } => writeln!(f, "{raw}")?,
            }
        }
        Ok(())
    }
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find(['\n', '\r']) {
            Some(pos) => {
                out.push(&rest[..pos]);
                let skip = if rest[pos..].starts_with("\r\n") {
                    2
                } else {
                    1
                };
                rest = &rest[pos + skip..];
            }
            None => {
                out.push(rest);
                rest = "";
            }
        }
    }
    out
}

fn ends_with_odd_backslashes(s: &str) -> bool {
    s.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 1
}

/// Splits a logical line (leading whitespace removed) into key and value.
fn split_entry(line: &str) -> (String, String) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'=' | b':' | b' ' | b'\t' | b'\x0c' => break,
            _ => i += 1,
        }
    }
    let key_end = i.min(bytes.len());
    let mut j = key_end;
    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\x0c') {
        j += 1;
    }
    if j < bytes.len() && matches!(bytes[j], b'=' | b':') {
        j += 1;
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\x0c') {
            j += 1;
        }
    }
    (unescape(&line[..key_end]), unescape(&line[j..]))
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut pending_high: Option<u16> = None;
    while let Some(c) = chars.next() {
        if c != '\\' {
            flush_surrogate(&mut out, &mut pending_high);
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => push(&mut out, &mut pending_high, '\t'),
            Some('n') => push(&mut out, &mut pending_high, '\n'),
            Some('r') => push(&mut out, &mut pending_high, '\r'),
            Some('f') => push(&mut out, &mut pending_high, '\x0c'),
            Some('u') => {
                let hex: String = chars.clone().take(4).collect();
                match (hex.len() == 4)
                    .then(|| u16::from_str_radix(&hex, 16).ok())
                    .flatten()
                {
                    Some(unit) => {
                        for _ in 0..4 {
                            chars.next();
                        }
                        push_unit(&mut out, &mut pending_high, unit);
                    }
                    None => push(&mut out, &mut pending_high, 'u'),
                }
            }
            Some(other) => push(&mut out, &mut pending_high, other),
            None => {}
        }
    }
    flush_surrogate(&mut out, &mut pending_high);
    out
}

fn push(out: &mut String, pending: &mut Option<u16>, c: char) {
    flush_surrogate(out, pending);
    out.push(c);
}

/// UTF-16 code units from `\uXXXX`, pairing surrogates.
fn push_unit(out: &mut String, pending: &mut Option<u16>, unit: u16) {
    match (pending.take(), unit) {
        (Some(high), 0xDC00..=0xDFFF) => {
            let c = char::decode_utf16([high, unit]).next();
            out.push(c.and_then(Result::ok).unwrap_or('\u{FFFD}'));
        }
        (prev, 0xD800..=0xDBFF) => {
            if prev.is_some() {
                out.push('\u{FFFD}');
            }
            *pending = Some(unit);
        }
        (prev, _) => {
            if prev.is_some() {
                out.push('\u{FFFD}');
            }
            out.push(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'));
        }
    }
}

fn flush_surrogate(out: &mut String, pending: &mut Option<u16>) {
    if pending.take().is_some() {
        out.push('\u{FFFD}');
    }
}

/// Escapes like `Properties.store`: keys escape every space, values only a
/// leading one; non-printable and non-ASCII become `\uXXXX`.
fn escape(s: &str, is_key: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        match c {
            ' ' if is_key || i == 0 => out.push_str("\\ "),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\x0c' => out.push_str("\\f"),
            '=' | ':' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04X}");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn reads_a_minecraft_file() {
        let text = "#Minecraft server properties\n#Tue Oct 13 18:10:41 UTC 2026\n\
            accepts-transfers=false\nmotd=\\u00A7cNether \\u00A7lSMP\nlevel-seed=\n\
            difficulty = hard\nwhite-list:true\nspaced\\ key\\ name value\n";
        let p = Properties::parse(text);
        assert_eq!(p.get("motd"), Some("§cNether §lSMP"));
        assert_eq!(p.get("level-seed"), Some(""));
        assert_eq!(p.get("difficulty"), Some("hard"));
        assert_eq!(p.get("white-list"), Some("true"));
        assert_eq!(p.get("spaced key name"), Some("value"));
        assert_eq!(p.to_string(), text, "unchanged files round-trip exactly");
    }

    #[test]
    fn continuation_lines() {
        let p = Properties::parse("a=one \\\n    two\nb=x\\\\\nc=y\n");
        assert_eq!(p.get("a"), Some("one two"));
        assert_eq!(
            p.get("b"),
            Some("x\\"),
            "an escaped backslash doesn't continue"
        );
        assert_eq!(p.get("c"), Some("y"));
    }

    #[test]
    fn set_rewrites_only_that_line() {
        let mut p = Properties::parse("# keep me\nb=2\na=1\n");
        p.set("a", "§aHi: there");
        p.set("new", " lead");
        assert_eq!(
            p.to_string(),
            "# keep me\nb=2\na=\\u00A7aHi\\: there\nnew=\\ lead\n"
        );
        assert_eq!(
            Properties::parse(&p.to_string()).get("a"),
            Some("§aHi: there")
        );
    }

    #[test]
    fn repeated_keys_last_wins() {
        let mut p = Properties::parse("a=1\nb=2\na=3\n");
        assert_eq!(p.get("a"), Some("3"));
        assert_eq!(p.entries(), vec![("a", "3"), ("b", "2")]);
        p.set("a", "4");
        assert_eq!(p.to_string(), "a=1\nb=2\na=4\n");
        p.remove("a");
        assert_eq!(p.to_string(), "b=2\n");
    }

    #[test]
    fn astral_characters_use_surrogate_pairs() {
        let mut p = Properties::default();
        p.set("motd", "🔥 hi");
        assert_eq!(p.to_string(), "motd=\\uD83D\\uDD25 hi\n");
        assert_eq!(Properties::parse(&p.to_string()).get("motd"), Some("🔥 hi"));
    }

    proptest! {
        #[test]
        fn set_then_parse_round_trips(
            entries in proptest::collection::btree_map("\\PC{1,12}|[ =:#!\\\\]{1,4}", "\\PC{0,20}|[ \\t\\n\\r=:#!\\\\§]{0,6}", 0..8)
        ) {
            let mut p = Properties::default();
            for (k, v) in &entries {
                p.set(k, v);
            }
            let back = Properties::parse(&p.to_string());
            for (k, v) in &entries {
                prop_assert_eq!(back.get(k), Some(v.as_str()));
            }
            prop_assert_eq!(back.entries().len(), entries.len());
        }

        #[test]
        fn parsing_is_stable(text in "\\PC{0,200}") {
            let once = Properties::parse(&text);
            let twice = Properties::parse(&once.to_string());
            prop_assert_eq!(once.entries(), twice.entries());
        }
    }
}
