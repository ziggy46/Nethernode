//! Searching a server's saved logs: `logs/latest.log` and the dated
//! `logs/*.log.gz` files Minecraft keeps from earlier runs. These outlive
//! restarts (unlike the console's backlog) and hold chat as well as
//! everything else. Newest files first; within a file, lines in order.

use std::io::Read;
use std::time::{Duration, Instant};

use nn_files::{EntryKind, FileError, RelPath};
use nn_node::Folder;
use tokio::io::AsyncReadExt;

/// Matches returned, at most (the newest ones).
pub(crate) const MAX_MATCHES: usize = 300;
/// Log files looked at, at most (the newest ones).
const MAX_FILES: usize = 200;
/// Of a file, at most this much is read (the end of a plain log; a bigger
/// compressed one is skipped).
const MAX_FILE: u64 = 64 * 1024 * 1024;
/// Uncompressed text searched per file, at most.
const MAX_TEXT: u64 = 256 * 1024 * 1024;
/// A search stops after this long, with what it found so far.
const TIME_LIMIT: Duration = Duration::from_secs(20);
/// Longest line returned.
const MAX_LINE: usize = 1000;

/// A matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    /// The file's name in `logs/`.
    pub file: String,
    pub file_modified_ms: i64,
    /// 1-based.
    pub line: u32,
    pub text: String,
}

/// What a search found.
#[derive(Debug, Default)]
pub(crate) struct Found {
    pub matches: Vec<Match>,
    pub files_searched: u32,
    pub files_total: u32,
    /// Stopped early: too many matches, files, or too long.
    pub truncated: bool,
}

/// Searches the server's logs for lines containing `query` (any case);
/// with `chat_only`, only chat lines, and an empty query matches all chat.
pub(crate) async fn search(
    folder: &Folder,
    query: &str,
    chat_only: bool,
) -> Result<Found, FileError> {
    let started = Instant::now();
    let logs = RelPath::parse("logs")?;
    let mut files: Vec<_> = match folder.list(&logs).await {
        Ok(entries) => entries,
        Err(FileError::NotFound(_)) => Vec::new(),
        Err(e) => return Err(e),
    }
    .into_iter()
    .filter(|e| {
        e.kind == EntryKind::File && (e.name == "latest.log" || e.name.ends_with(".log.gz"))
    })
    .collect();
    files.sort_by(|a, b| {
        (b.name == "latest.log", b.modified_ms, &b.name).cmp(&(
            a.name == "latest.log",
            a.modified_ms,
            &a.name,
        ))
    });
    let mut found = Found {
        files_total: u32::try_from(files.len()).unwrap_or(u32::MAX),
        truncated: files.len() > MAX_FILES,
        ..Found::default()
    };
    let needle = query.to_lowercase();
    for entry in files.into_iter().take(MAX_FILES) {
        if started.elapsed() > TIME_LIMIT {
            found.truncated = true;
            break;
        }
        let gz = entry.name.ends_with(".gz");
        if gz && entry.size > MAX_FILE {
            continue;
        }
        let rel = RelPath::parse(&format!("logs/{}", entry.name))?;
        let reader = match folder.open_read(&rel).await {
            Ok(reader) => reader,
            // Rotated away meanwhile.
            Err(FileError::NotFound(_)) => continue,
            Err(e) => return Err(e),
        };
        let start = if gz {
            0
        } else {
            reader.size().saturating_sub(MAX_FILE)
        };
        let mut bytes = Vec::new();
        reader
            .from_offset(start)
            .await?
            .take(MAX_FILE)
            .read_to_end(&mut bytes)
            .await?;
        let (needle, name, modified) = (needle.clone(), entry.name.clone(), entry.modified_ms);
        let hits = tokio::task::spawn_blocking(move || {
            let text = if gz { gunzip(&bytes) } else { bytes };
            // A tail read can start mid-line; skip to the next whole one.
            let text = if start > 0 {
                text.iter()
                    .position(|&b| b == b'\n')
                    .map_or(&text[..0], |i| &text[i + 1..])
                    .to_vec()
            } else {
                text
            };
            find(
                &String::from_utf8_lossy(&text),
                &needle,
                chat_only,
                &name,
                modified,
            )
        })
        .await
        .map_err(|e| FileError::Io(std::io::Error::other(e.to_string())))?;
        found.files_searched += 1;
        let room = MAX_MATCHES - found.matches.len();
        if hits.len() > room {
            // Keep the newest lines of the oldest file that fits.
            found
                .matches
                .extend(hits.into_iter().rev().take(room).rev());
            found.truncated = true;
            break;
        }
        found.matches.extend(hits);
        if found.matches.len() == MAX_MATCHES {
            break;
        }
    }
    Ok(found)
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // A damaged file gives what could be read.
    let _ = flate2::read::MultiGzDecoder::new(bytes)
        .take(MAX_TEXT)
        .read_to_end(&mut out);
    out
}

/// Matching lines of one file.
fn find(text: &str, needle: &str, chat_only: bool, file: &str, modified: i64) -> Vec<Match> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = plain(raw);
        if chat_only && !is_chat(&line) {
            continue;
        }
        if !needle.is_empty() && !line.to_lowercase().contains(needle) {
            continue;
        }
        let mut line = line;
        if line.len() > MAX_LINE {
            let mut end = MAX_LINE;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line.truncate(end);
            line.push('…');
        }
        out.push(Match {
            file: file.to_owned(),
            file_modified_ms: modified,
            line: u32::try_from(i + 1).unwrap_or(u32::MAX),
            text: line,
        });
    }
    out
}

/// A line without color codes (ANSI escapes, and `§` formatting codes).
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // CSI: ESC [ … final byte in @–~.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
            }
            '§' => {
                chars.next();
            }
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// A player's chat message (`<Steve> hi`, `[Not Secure] <Steve> hi`) or an
/// emote (`* Steve waves`).
fn is_chat(line: &str) -> bool {
    let Some((_, message)) = line.split_once("]: ") else {
        return false;
    };
    let message = message.strip_prefix("[Not Secure] ").unwrap_or(message);
    message.starts_with('<') || message.starts_with("* ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "[12:00:00] [Server thread/INFO]: Starting minecraft server\n\
        [12:00:05] [Server thread/INFO]: Steve joined the game\n\
        [12:00:06] [Server thread/INFO]: <Steve> hello there\n\
        [12:00:07] [Server thread/INFO]: [Not Secure] <Alex> \u{a7}aHELLO\n\
        [12:00:08] [Server thread/INFO]: * Steve waves\n\
        [12:00:09] [Server thread/INFO]: [WorldEdit] Loading hello\n";

    #[test]
    fn finds_lines_in_any_case() {
        let hits = find(LOG, "hello", false, "latest.log", 1);
        assert_eq!(hits.iter().map(|m| m.line).collect::<Vec<_>>(), [3, 4, 6]);
        assert_eq!(
            hits[1].text, "[12:00:07] [Server thread/INFO]: [Not Secure] <Alex> HELLO",
            "formatting codes are removed"
        );
    }

    #[test]
    fn chat_only_skips_everything_else() {
        let chat = find(LOG, "", true, "latest.log", 1);
        assert_eq!(chat.iter().map(|m| m.line).collect::<Vec<_>>(), [3, 4, 5]);
        assert_eq!(find(LOG, "hello", true, "latest.log", 1).len(), 2);
    }

    #[test]
    fn colors_are_removed() {
        assert_eq!(
            plain("\x1b[33m[12:00 WARN]: hi\x1b[0m\r"),
            "[12:00 WARN]: hi"
        );
    }

    #[test]
    fn compressed_logs_are_read() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(LOG.as_bytes()).unwrap();
        let text = gunzip(&gz.finish().unwrap());
        assert_eq!(text, LOG.as_bytes());
    }
}
