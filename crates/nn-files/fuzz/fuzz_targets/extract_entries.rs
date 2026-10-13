//! Well-formed archives with hostile entry names (`..`, absolute paths,
//! backslashes, symlinks): must never panic or escape.
#![no_main]

mod common;

use std::io::{Cursor, Write};

use arbitrary::Arbitrary;
use zip::write::SimpleFileOptions;

#[derive(Debug, Arbitrary)]
enum Entry {
    File { name: String, data: Vec<u8> },
    Dir { name: String },
    Link { name: String, target: String },
}

libfuzzer_sys::fuzz_target!(|entries: Vec<Entry>| {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    for entry in entries.iter().take(64) {
        let _ = match entry {
            Entry::File { name, data } => zip
                .start_file(name.as_str(), options)
                .and_then(|()| Ok(zip.write_all(data)?)),
            Entry::Dir { name } => zip.add_directory(name.as_str(), options),
            Entry::Link { name, target } => zip.add_symlink(name.as_str(), target.as_str(), options),
        };
    }
    if let Ok(cursor) = zip.finish() {
        common::extract_and_check(&cursor.into_inner());
    }
});
