//! Path traversal, symlink escape, and zip-slip tests, plus the trash,
//! conflict detection, and archive round trips.
//!
//! Each test builds this layout in a temp folder:
//!
//! ```text
//! outside/secret.txt          must never be read, written, or listed
//! server/                     the sandbox root
//!   config/a.txt
//!   escape      -> ../outside              (relative symlink out)
//!   abs         -> <tmp>/outside           (absolute symlink out)
//!   secret_link -> ../outside/secret.txt   (file symlink out)
//!   inside      -> config                  (symlink that stays inside)
//! ```

use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use nn_files::{Expect, FileError, Limits, RelPath, ServerFiles};
use zip::write::SimpleFileOptions;

struct Fixture {
    _tmp: tempfile::TempDir,
    outside: PathBuf,
    root: PathBuf,
    files: ServerFiles,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    let root = tmp.path().join("server");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/a.txt"), "a").unwrap();
    symlink("../outside", root.join("escape")).unwrap();
    symlink(&outside, root.join("abs")).unwrap();
    symlink("../outside/secret.txt", root.join("secret_link")).unwrap();
    symlink("config", root.join("inside")).unwrap();
    let files = ServerFiles::open(&root).unwrap();
    Fixture {
        _tmp: tmp,
        outside,
        root,
        files,
    }
}

fn p(s: &str) -> RelPath {
    RelPath::parse(s).unwrap()
}

impl Fixture {
    /// Fails the test if anything outside the sandbox changed.
    fn assert_outside_untouched(&self) {
        let mut names: Vec<_> = fs::read_dir(&self.outside)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, vec!["secret.txt"], "nothing new appeared outside");
        assert_eq!(
            fs::read_to_string(self.outside.join("secret.txt")).unwrap(),
            "secret"
        );
        assert!(!self.outside.parent().unwrap().join("evil.txt").exists());
    }
}

#[test]
fn traversal_strings_are_rejected_before_touching_disk() {
    for bad in [
        "..",
        "../outside/secret.txt",
        "config/../../outside",
        "/etc/passwd",
        "a\\..\\b",
    ] {
        assert!(
            matches!(RelPath::parse(bad), Err(FileError::InvalidPath(_))),
            "{bad}"
        );
    }
}

#[test]
fn symlinks_cannot_lead_outside() {
    let f = fixture();
    let limit = nn_files::EDITOR_LIMIT;
    for path in ["escape/secret.txt", "abs/secret.txt", "secret_link"] {
        assert!(f.files.read(&p(path), limit).is_err(), "read {path}");
        assert!(f.files.open_read(&p(path)).is_err(), "download {path}");
    }
    for dir in ["escape", "abs"] {
        assert!(f.files.list(&p(dir)).is_err(), "list {dir}");
    }
    assert!(
        f.files
            .write(&p("escape/new.txt"), b"x", Expect::Any)
            .is_err()
    );
    assert!(f.files.write(&p("abs/new.txt"), b"x", Expect::Any).is_err());
    assert!(
        f.files.write(&p("secret_link"), b"x", Expect::Any).is_err(),
        "writing through a link is refused"
    );
    assert!(f.files.mkdir(&p("escape/dir")).is_err());
    assert!(
        f.files
            .rename(&p("config/a.txt"), &p("escape/a.txt"))
            .is_err()
    );
    assert!(f.files.copy(&p("config/a.txt"), &p("abs/a.txt")).is_err());
    assert!(
        f.files.copy(&p("escape"), &p("copied")).is_err(),
        "links aren't copied"
    );
    assert!(
        f.files
            .rename(&p("escape/secret.txt"), &p("stolen.txt"))
            .is_err()
    );
    assert!(
        f.files
            .copy(&p("escape/secret.txt"), &p("stolen.txt"))
            .is_err()
    );
    assert!(f.files.delete(&p("escape/secret.txt")).is_err());
    assert!(
        f.root.join("config/a.txt").exists(),
        "failed moves leave the source"
    );
    f.assert_outside_untouched();
}

#[test]
fn symlinks_inside_the_root_still_work() {
    let f = fixture();
    let content = f.files.read(&p("inside/a.txt"), 1024).unwrap();
    assert_eq!(content.data, b"a");
    let names: Vec<_> = f
        .files
        .list(&RelPath::root())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(
        names.contains(&"escape".to_owned()),
        "links are listed, not followed"
    );
}

#[test]
fn deleting_a_link_trashes_the_link_not_its_target() {
    let f = fixture();
    let item = f.files.delete(&p("escape")).unwrap();
    assert!(fs::symlink_metadata(f.root.join("escape")).is_err());
    f.assert_outside_untouched();
    f.files.restore(&item.id).unwrap();
    assert!(
        fs::symlink_metadata(f.root.join("escape"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn copying_a_folder_skips_links() {
    let f = fixture();
    symlink("../../outside", f.root.join("config/out")).unwrap();
    let report = f.files.copy(&p("config"), &p("config2")).unwrap();
    assert_eq!((report.files, report.skipped_links), (1, 1));
    assert!(f.root.join("config2/a.txt").exists());
    assert!(fs::symlink_metadata(f.root.join("config2/out")).is_err());
}

#[test]
fn moves_refuse_overwrites_and_self_nesting() {
    let f = fixture();
    fs::create_dir(f.root.join("world")).unwrap();
    assert!(matches!(
        f.files.rename(&p("world"), &p("world/inner")),
        Err(FileError::Invalid(_))
    ));
    assert!(matches!(
        f.files.rename(&p("config/a.txt"), &p("config")),
        Err(FileError::AlreadyExists(_))
    ));
    assert!(matches!(
        f.files.rename(&p("config/a.txt"), &p("missing/a.txt")),
        Err(FileError::NotFound(_))
    ));
    f.files
        .rename(&p("config/a.txt"), &p("world/b.txt"))
        .unwrap();
    assert_eq!(fs::read_to_string(f.root.join("world/b.txt")).unwrap(), "a");
}

#[test]
fn writes_detect_concurrent_changes() {
    let f = fixture();
    let read = f.files.read(&p("config/a.txt"), 1024).unwrap();
    // The server rewrites the file meanwhile.
    fs::write(f.root.join("config/a.txt"), "changed").unwrap();
    assert!(matches!(
        f.files
            .write(&p("config/a.txt"), b"mine", Expect::Version(read.version)),
        Err(FileError::Conflict)
    ));
    let fresh = f.files.read(&p("config/a.txt"), 1024).unwrap();
    let v2 = f
        .files
        .write(&p("config/a.txt"), b"mine", Expect::Version(fresh.version))
        .unwrap();
    assert_eq!(f.files.read(&p("config/a.txt"), 1024).unwrap().version, v2);
    assert!(matches!(
        f.files.write(&p("config/a.txt"), b"x", Expect::New),
        Err(FileError::AlreadyExists(_))
    ));
    // No temporary files are left behind.
    let names: Vec<_> = f
        .files
        .list(&p("config"))
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["a.txt"]);
}

#[test]
fn writes_keep_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let path = f.root.join("config/a.txt");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    f.files
        .write(&p("config/a.txt"), b"new", Expect::Any)
        .unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn reads_are_size_limited_and_flag_binary() {
    let f = fixture();
    fs::write(f.root.join("big.bin"), vec![0u8; 2048]).unwrap();
    assert!(matches!(
        f.files.read(&p("big.bin"), 1024),
        Err(FileError::TooLarge { .. })
    ));
    assert!(!f.files.read(&p("big.bin"), 4096).unwrap().is_text);
    assert!(f.files.read(&p("config/a.txt"), 10).unwrap().is_text);
    assert!(matches!(
        f.files.read(&p("config"), 10),
        Err(FileError::IsDirectory(_))
    ));
}

#[test]
fn trash_round_trip() {
    let f = fixture();
    let item = f.files.delete(&p("config")).unwrap();
    assert!(!f.root.join("config").exists());
    let names: Vec<_> = f
        .files
        .list(&RelPath::root())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(!names.contains(&".trash".to_owned()), "the trash is hidden");
    assert_eq!(f.files.trash().unwrap(), vec![item.clone()]);

    // Something new took its place: restoring must not clobber it.
    fs::create_dir(f.root.join("config")).unwrap();
    assert!(matches!(
        f.files.restore(&item.id),
        Err(FileError::AlreadyExists(_))
    ));
    fs::remove_dir(f.root.join("config")).unwrap();
    assert_eq!(f.files.restore(&item.id).unwrap(), p("config"));
    assert_eq!(
        fs::read_to_string(f.root.join("config/a.txt")).unwrap(),
        "a"
    );
    assert!(f.files.trash().unwrap().is_empty());

    // Missing parents are recreated on restore.
    let item = f.files.delete(&p("config/a.txt")).unwrap();
    fs::remove_dir(f.root.join("config")).unwrap();
    f.files.restore(&item.id).unwrap();
    assert!(f.root.join("config/a.txt").exists());

    // IDs are never paths.
    for bad in ["../../outside", "..", "01ARZ3NDEKTSV4RRFFQ69G5FAV/../x"] {
        assert!(f.files.restore(bad).is_err());
        assert!(f.files.purge(bad).is_err());
    }
    f.files.delete(&p("config")).unwrap();
    f.files.empty_trash().unwrap();
    assert!(f.files.trash().unwrap().is_empty());
    assert!(RelPath::parse(".trash/x").is_err());
}

#[test]
fn purge_expired_keeps_recent_items() {
    let f = fixture();
    f.files.delete(&p("config/a.txt")).unwrap();
    assert_eq!(
        f.files
            .purge_expired(std::time::Duration::from_secs(60))
            .unwrap(),
        0
    );
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(f.files.purge_expired(std::time::Duration::ZERO).unwrap(), 1);
    assert!(f.files.trash().unwrap().is_empty());
}

// ----- zip-slip and archives -----

fn make_zip(root: &Path, name: &str, build: impl FnOnce(&mut zip::ZipWriter<Cursor<Vec<u8>>>)) {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    build(&mut zip);
    fs::write(root.join(name), zip.finish().unwrap().into_inner()).unwrap();
}

fn add(zip: &mut zip::ZipWriter<Cursor<Vec<u8>>>, name: &str, data: &[u8]) {
    zip.start_file(name, SimpleFileOptions::default()).unwrap();
    zip.write_all(data).unwrap();
}

fn noop(_: u64, _: u64) {}

#[test]
fn zip_slip_entries_fail_the_whole_archive() {
    for evil in [
        "../evil.txt",
        "../../evil.txt",
        "a/../../evil.txt",
        "/evil.txt",
        "..\\evil.txt",
        "a\\..\\..\\evil.txt",
        "C:/evil.txt",
        ".trash/evil.txt",
    ] {
        let f = fixture();
        make_zip(&f.root, "bad.zip", |z| {
            add(z, "ok.txt", b"fine");
            add(z, evil, b"pwned");
        });
        let result = f.files.extract(
            &p("bad.zip"),
            &RelPath::root(),
            true,
            Limits::default(),
            &mut noop,
        );
        assert!(result.is_err(), "{evil} must be rejected");
        assert!(
            !f.root.join("ok.txt").exists(),
            "{evil}: nothing is extracted"
        );
        f.assert_outside_untouched();
    }
}

#[test]
fn zip_symlink_entries_are_skipped() {
    let f = fixture();
    make_zip(&f.root, "links.zip", |z| {
        z.add_symlink("link", "../../outside", SimpleFileOptions::default())
            .unwrap();
        add(z, "link/evil.txt", b"pwned");
    });
    fs::create_dir(f.root.join("dest")).unwrap();
    let report = f
        .files
        .extract(
            &p("links.zip"),
            &p("dest"),
            true,
            Limits::default(),
            &mut noop,
        )
        .unwrap();
    assert_eq!(report.skipped_links, 1);
    assert!(
        !fs::symlink_metadata(f.root.join("dest/link"))
            .unwrap()
            .is_symlink()
    );
    assert!(
        f.root.join("dest/link/evil.txt").is_file(),
        "landed inside, as a plain file"
    );
    f.assert_outside_untouched();
}

#[test]
fn extraction_cannot_follow_existing_links_out() {
    let f = fixture();
    make_zip(&f.root, "through.zip", |z| {
        add(z, "escape/evil.txt", b"pwned")
    });
    assert!(
        f.files
            .extract(
                &p("through.zip"),
                &RelPath::root(),
                true,
                Limits::default(),
                &mut noop
            )
            .is_err()
    );
    f.assert_outside_untouched();
}

#[test]
fn extraction_limits() {
    let f = fixture();
    make_zip(&f.root, "big.zip", |z| {
        z.start_file("zeros", SimpleFileOptions::default()).unwrap();
        z.write_all(&vec![0u8; 1024 * 1024]).unwrap();
    });
    let tight = Limits {
        max_entries: 10,
        max_bytes: 1000,
    };
    assert!(matches!(
        f.files
            .extract(&p("big.zip"), &p("out"), true, tight, &mut noop),
        Err(FileError::TooLarge { .. })
    ));
    make_zip(&f.root, "many.zip", |z| {
        for i in 0..20 {
            add(z, &format!("f{i}"), b"x");
        }
    });
    assert!(matches!(
        f.files
            .extract(&p("many.zip"), &p("out"), true, tight, &mut noop),
        Err(FileError::TooLarge { .. })
    ));
}

#[test]
fn extraction_respects_overwrite() {
    let f = fixture();
    make_zip(&f.root, "cfg.zip", |z| add(z, "config/a.txt", b"from zip"));
    assert!(matches!(
        f.files.extract(
            &p("cfg.zip"),
            &RelPath::root(),
            false,
            Limits::default(),
            &mut noop
        ),
        Err(FileError::Invalid(m)) if m.contains("already exists")
    ));
    assert_eq!(
        fs::read_to_string(f.root.join("config/a.txt")).unwrap(),
        "a"
    );
    f.files
        .extract(
            &p("cfg.zip"),
            &RelPath::root(),
            true,
            Limits::default(),
            &mut noop,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("config/a.txt")).unwrap(),
        "from zip"
    );
}

#[test]
fn compress_then_extract_round_trips() {
    let f = fixture();
    fs::create_dir_all(f.root.join("world/region")).unwrap();
    fs::write(f.root.join("world/level.dat"), b"level").unwrap();
    fs::write(f.root.join("world/region/r.0.0.mca"), vec![7u8; 100_000]).unwrap();
    symlink("../../outside", f.root.join("world/out")).unwrap();

    let mut last = (0, 0);
    let report = f
        .files
        .compress(&[p("world"), p("config")], &p("backup.zip"), &mut |d, t| {
            last = (d, t)
        })
        .unwrap();
    assert_eq!((report.files, report.skipped_links), (3, 1));
    assert_eq!(last.0, last.1, "progress reaches the total");
    assert!(matches!(
        f.files.compress(&[p("world")], &p("backup.zip"), &mut noop),
        Err(FileError::AlreadyExists(_))
    ));
    assert!(
        f.files
            .compress(&[p("world"), p("config/a.txt")], &p("x.zip"), &mut noop)
            .is_err(),
        "sources must share a folder"
    );

    f.files.mkdir(&p("restored")).unwrap();
    let report = f
        .files
        .extract(
            &p("backup.zip"),
            &p("restored"),
            false,
            Limits::default(),
            &mut noop,
        )
        .unwrap();
    assert_eq!(report.files, 3);
    assert_eq!(
        fs::read(f.root.join("restored/world/region/r.0.0.mca")).unwrap(),
        vec![7u8; 100_000]
    );
    assert_eq!(
        fs::read_to_string(f.root.join("restored/config/a.txt")).unwrap(),
        "a"
    );
    assert!(!f.root.join("restored/world/out").exists());
}

#[test]
fn compressing_into_the_source_folder_skips_the_archive_itself() {
    let f = fixture();
    let report = f
        .files
        .compress(&[p("config")], &p("config/self.zip"), &mut noop)
        .unwrap();
    assert_eq!(report.files, 1, "only a.txt, not the archive being written");
}

/// When NetherNode runs as root, files it creates must belong to the
/// server folder's owner, and replaced files keep their owner. Only
/// meaningful as root; run on Linux inside a container.
#[cfg(target_os = "linux")]
#[test]
fn root_hands_files_to_the_folder_owner() {
    use std::os::unix::fs::{MetadataExt, chown};
    if !rustix_is_root() {
        eprintln!("skipped: needs root");
        return;
    }
    let f = fixture();
    chown(&f.root, Some(1234), Some(1234)).unwrap();
    chown(f.root.join("config/a.txt"), Some(4321), Some(4321)).unwrap();
    let files = ServerFiles::open(&f.root).unwrap();
    let uid = |rel: &str| fs::symlink_metadata(f.root.join(rel)).unwrap().uid();

    files.write(&p("new.txt"), b"x", Expect::New).unwrap();
    assert_eq!(uid("new.txt"), 1234, "new files go to the folder owner");
    files.write(&p("config/a.txt"), b"y", Expect::Any).unwrap();
    assert_eq!(uid("config/a.txt"), 4321, "replaced files keep their owner");
    files.mkdir(&p("made")).unwrap();
    assert_eq!(uid("made"), 1234);
    files.copy(&p("config"), &p("copied")).unwrap();
    assert_eq!((uid("copied"), uid("copied/a.txt")), (1234, 1234));
    make_zip(&f.root, "z.zip", |z| add(z, "deep/dir/f.txt", b"z"));
    files
        .extract(
            &p("z.zip"),
            &RelPath::root(),
            false,
            Limits::default(),
            &mut noop,
        )
        .unwrap();
    assert_eq!(
        (uid("deep"), uid("deep/dir"), uid("deep/dir/f.txt")),
        (1234, 1234, 1234)
    );
    files
        .compress(&[p("deep")], &p("deep.zip"), &mut noop)
        .unwrap();
    assert_eq!(uid("deep.zip"), 1234);
    let item = files.delete(&p("new.txt")).unwrap();
    assert_eq!(uid(".trash"), 1234);
    files.restore(&item.id).unwrap();
    assert_eq!(uid("new.txt"), 1234);
}

#[cfg(target_os = "linux")]
fn rustix_is_root() -> bool {
    // The test binary runs as whatever user invoked it; the root folder of
    // `/proc/self` is owned by that user.
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self")
        .map(|m| m.uid() == 0)
        .unwrap_or(false)
}
