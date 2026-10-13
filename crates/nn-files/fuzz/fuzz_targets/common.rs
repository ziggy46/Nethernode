//! Shared harness: extract an archive inside a sandbox and check that
//! nothing appeared outside it.

use std::ffi::OsString;
use std::fs;

use nn_files::{Limits, RelPath, ServerFiles};

pub fn extract_and_check(archive: &[u8]) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("server");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("in.zip"), archive).unwrap();
    let files = ServerFiles::open(&root).unwrap();
    let limits = Limits {
        max_entries: 2_000,
        max_bytes: 8 * 1024 * 1024,
    };
    let _ = files.extract(
        &RelPath::parse("in.zip").unwrap(),
        &RelPath::parse("out").unwrap(),
        true,
        limits,
        &mut |_, _| {},
    );
    let beside: Vec<OsString> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(beside, vec![OsString::from("server")], "extraction escaped the server folder");
    // Everything written lives under `out/` (plus the archive itself).
    for entry in fs::read_dir(&root).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(name == "in.zip" || name == "out", "wrote {name:?} outside the destination");
    }
}
