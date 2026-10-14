//! CurseForge mods that have to be downloaded by hand. When a pack needs
//! files whose authors don't allow other apps to download them, the server
//! image stops and writes a table of them to `MODS_NEED_DOWNLOAD.txt`:
//!
//! ```text
//! Mod                   Version Name   Filename          Download page
//! ====================  =============  ================  ==============
//! Some Mod              some-mod-1.2   some-mod-1.2.jar  https://…/download/123
//! ```
//!
//! Columns are found from the `=` divider line. Long mod names are cut to
//! 20 characters (with `...`), and `.jar` is dropped from version names.

/// The file the image writes in the server's folder.
pub(crate) const NEEDS_DOWNLOAD_FILE: &str = "MODS_NEED_DOWNLOAD.txt";

/// One file to download by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualFile {
    pub mod_name: String,
    pub version_name: String,
    pub filename: String,
    pub download_page: String,
}

/// Parses the table. Unreadable lines are skipped.
pub(crate) fn parse(text: &str) -> Vec<ManualFile> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(divider) = lines
        .iter()
        .position(|l| l.contains('=') && l.chars().all(|c| c == '=' || c == ' '))
    else {
        return Vec::new();
    };
    // Each run of `=` is a column: (start, width).
    let mut columns = Vec::new();
    let chars: Vec<char> = lines[divider].chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '=' {
            let start = i;
            while i < chars.len() && chars[i] == '=' {
                i += 1;
            }
            columns.push((start, i - start));
        } else {
            i += 1;
        }
    }
    if columns.len() < 4 {
        return Vec::new();
    }
    lines[divider + 1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let chars: Vec<char> = line.chars().collect();
            let cell = |n: usize| -> String {
                let (start, width) = columns[n];
                // The last column isn't padded past its text.
                let end = if n + 1 == columns.len() {
                    chars.len()
                } else {
                    (start + width).min(chars.len())
                };
                chars
                    .get(start..end)
                    .map(|c| c.iter().collect::<String>())
                    .unwrap_or_default()
                    .trim()
                    .to_owned()
            };
            let filename = cell(2);
            let download_page = cell(3);
            (!filename.is_empty() && download_page.starts_with("https://")).then(|| ManualFile {
                mod_name: cell(0),
                version_name: cell(1),
                filename,
                download_page,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_image_table() {
        let text = "\
Mod                      Version Name                Filename                        Download page
=======================  ==========================  ==============================  ===================================================================
Biomes O' Plenty         BiomesOPlenty-1.20.1-18.0   BiomesOPlenty-1.20.1-18.0.jar   https://www.curseforge.com/minecraft/mc-mods/biomes-o-plenty/download/5000
A Very Long Mod Name...  long-mod-2.0                long-mod-2.0.jar                https://www.curseforge.com/minecraft/mc-mods/long/download/42
";
        let files = parse(text);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].mod_name, "Biomes O' Plenty");
        assert_eq!(files[0].filename, "BiomesOPlenty-1.20.1-18.0.jar");
        assert_eq!(
            files[0].download_page,
            "https://www.curseforge.com/minecraft/mc-mods/biomes-o-plenty/download/5000"
        );
        assert_eq!(files[1].mod_name, "A Very Long Mod Name...");
        assert_eq!(files[1].version_name, "long-mod-2.0");
    }

    #[test]
    fn junk_is_ignored() {
        assert!(parse("").is_empty());
        assert!(parse("hello\nworld").is_empty());
        assert!(parse("a  b  c  d\n=  =  =  =\nx  y  z  not-a-link").is_empty());
    }
}
