//! Native effect gates. Every filesystem object is below a disposable inert fixture.
use super::*;
use std::os::windows::fs::OpenOptionsExt;
use std::{fs as host, sync::Arc};

// Break caught: retained no-delete directory anchors prevent the kernel's target
// open, or publication replaces a destination that arrived after the ownership check.
#[test]
fn rooted_publication_backup_collision_and_restore() {
    for directory in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        {
            let root = Dir::open(temp.path()).unwrap();
            let state = root.dir("state", true).unwrap();
            if directory {
                state
                    .dir("stage", true)
                    .unwrap()
                    .write_new("asset", b"old", false)
                    .unwrap();
            } else {
                state.write_new("stage", b"old", false).unwrap();
            }
            state.rename("stage", &root, "published").unwrap();
            root.rename("published", &state, "backup").unwrap();
            root.write_new("published", b"foreign", false).unwrap();
            assert!(state.rename("backup", &root, "published").is_err());
            assert_eq!(root.read("published", 20).unwrap().0, b"foreign");
            root.remove("published").unwrap();
            state.rename("backup", &root, "published").unwrap();
            let bytes = if directory {
                root.dir("published", false)
                    .unwrap()
                    .read("asset", 20)
                    .unwrap()
                    .0
            } else {
                root.read("published", 20).unwrap().0
            };
            assert_eq!(bytes, b"old");
            root.remove("published").unwrap();
            state.atomic("journal", b"one").unwrap();
            state.atomic("journal", b"two").unwrap();
            assert_eq!(state.read("journal", 20).unwrap().0, b"two");
        }
        temp.close().expect("all mutation and anchor handles close");
    }
}

// Break caught: shared cursor interleaves pages or a later scan starts at EOF.
#[test]
fn rooted_enumeration_pagination_aliases_unicode_and_mid_scan_failure() {
    let temp = tempfile::tempdir().unwrap();
    {
        let root = Arc::new(Dir::open(temp.path()).unwrap());
        let mut expected = Vec::new();
        for i in 0..900 {
            let name = format!("한글-{i:04}-{}", "x".repeat(100));
            root.write_new(&name, b"inert", false).unwrap();
            expected.push(name);
        }
        expected.sort();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let root = root.parent("unused", false).unwrap().0;
                let expected = &expected;
                scope.spawn(move || {
                    for _ in 0..3 {
                        assert_eq!(&root.list().unwrap(), expected);
                    }
                });
            }
        });
        TEST_READDIR_FAULT.with(|f| *f.borrow_mut() = Some((root.path.clone(), 300)));
        assert!(root.list().is_err());
        TEST_READDIR_FAULT.with(|f| *f.borrow_mut() = None);
        assert_eq!(root.list().unwrap(), expected);
    }
    temp.close().unwrap();
}

#[test]
fn rooted_recursive_removal_releases_handles_and_keeps_external_readers() {
    let temp = tempfile::tempdir().unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        root.dir("tree", true)
            .unwrap()
            .dir("nested", true)
            .unwrap()
            .write_new("asset", b"source", false)
            .unwrap();
        let external = host::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(temp.path().join("tree/nested/asset"))
            .unwrap();
        assert!(root.remove("tree").is_err());
        assert_eq!(
            host::read(temp.path().join("tree/nested/asset")).unwrap(),
            b"source"
        );
        drop(external);
        root.remove("tree").unwrap();
        assert!(root.kind("tree").unwrap().is_none());
    }
    temp.close().unwrap();
}

thread_local! {
    static STOP_AFTER_PROGRESS_REMOVAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CHECKED_RENAME: std::cell::RefCell<Option<Box<dyn FnMut(&Path, &str) -> bool>>> = const { std::cell::RefCell::new(None) };
    pub(in crate::extensions) static CLEANUP_STOP: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static AFTER_READ_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_REMOVE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_MUTATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_VALIDATE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
pub(super) fn after_validate() {
    let hook = AFTER_VALIDATE.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

// Byte fixtures target malformed kernel record boundaries, independently of
// production structs or parser output. FILE_DIRECTORY_INFORMATION name starts 64.
#[test]
fn rooted_parser_rejects_truncation_offsets_utf16_and_escape_names() {
    fn record(name: &[u16]) -> Vec<u8> {
        let mut b = vec![0; 64];
        b[60..64].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
        for u in name {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b
    }
    let parse = super::windows::parse_records;
    assert_eq!(parse(&record(&[0xac00, 0xb098])).unwrap(), ["가나"]);
    assert_eq!(parse(&record(&[46])).unwrap(), Vec::<String>::new());
    for bad in [
        Vec::new(),
        vec![0; 63],
        record(&[]),
        record(&[0xd800]),
        record(&[47]),
        record(&[58]),
        record(&[0]),
    ] {
        assert!(parse(&bad).is_err());
    }
    let good = record(&[97]);
    for next in [1u32, 64, 65, 72, u32::MAX] {
        let mut bad = good.clone();
        bad[..4].copy_from_slice(&next.to_le_bytes());
        assert!(parse(&bad).is_err());
    }
    for len in [1u32, 4, u32::MAX] {
        let mut bad = good.clone();
        bad[60..64].copy_from_slice(&len.to_le_bytes());
        assert!(parse(&bad).is_err());
    }
}

pub(super) fn before_mutation() {
    let hook = BEFORE_MUTATION.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

fn junction(path: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::{Storage::FileSystem::*, System::IO::DeviceIoControl};
    let file = host::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let target = target
        .canonicalize()?
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    assert_eq!(&target[..4], &[92, 92, 63, 92]);
    let mut nt = vec![92u16, 63, 63, 92];
    nt.extend_from_slice(&target[4..]);
    let mut data = Vec::new();
    data.extend_from_slice(&0xa0000003u32.to_le_bytes()); // Microsoft mount point tag
    for v in [
        8 + (nt.len() + 2) * 2,
        0,
        0,
        nt.len() * 2,
        (nt.len() + 1) * 2,
        0,
    ] {
        data.extend_from_slice(&(v as u16).to_le_bytes());
    }
    for unit in nt.into_iter().chain([0, 0]) {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x900a4,
            data.as_ptr().cast(),
            data.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn canary(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, at: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in host::read_dir(at).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if entry.file_type().unwrap().is_dir() {
                out.push((format!("{name}/"), vec![]));
                walk(root, &path, out);
            } else {
                out.push((name, host::read(path).unwrap()));
            }
        }
    }
    let mut out = vec![];
    walk(root, root, &mut out);
    out.sort();
    out
}
fn external_fixture(root: &Path) -> PathBuf {
    let external = root.join("external");
    host::create_dir(&external).unwrap();
    host::write(external.join("asset"), b"EXTERNAL CANARY").unwrap();
    host::write(external.join("lock"), b"EXTERNAL LOCK").unwrap();
    host::create_dir(external.join("tree")).unwrap();
    host::write(external.join("tree/child"), b"EXTERNAL CHILD").unwrap();
    external
}
fn operation(dir: &Dir, other: &Dir, op: usize, retargeted: bool) {
    match op {
        0 => {
            let _ = dir.dir("created", true);
        }
        1 => {
            let r = dir.read("asset", 100);
            assert!(r.is_err());
        }
        2 => {
            let r = dir.read_private("asset", 100);
            assert!(r.is_err());
        }
        3 => {
            assert!(!matches!(dir.kind("asset"), Ok(Some(_))));
        }
        4 => {
            let r = dir.list();
            assert!(r.is_err() || r.unwrap().is_empty());
        }
        5 => {
            assert!(dir.lock("lock").is_err());
        }
        6 => {
            let _ = dir.write_new("created", b"owned", false);
        }
        7 => {
            let _ = dir.atomic("asset", b"owned");
        }
        8 => {
            let _ = dir.rename("asset", other, "moved");
        }
        9 => {
            let _ = other.rename("payload", dir, "published");
        }
        10 => {
            let _ = dir.remove("tree");
        }
        11 => {
            if retargeted {
                assert!(dir.sync_tree().is_err());
            } else {
                dir.sync_tree().unwrap();
            }
        }
        _ => unreachable!(),
    }
}

// The hook executes AFTER validation, immediately before the native child call.
// Mutations use byte/name canaries; read-only lock and sync also assert outcomes.
#[test]
fn rooted_post_validation_root_junction_all_child_families() {
    for op in 0..12 {
        let temp = tempfile::tempdir().unwrap();
        let external = external_fixture(temp.path());
        let before = canary(&external);
        let control = temp.path().join("control");
        host::create_dir(&control).unwrap();
        junction(&control, &external).expect("unguarded junction positive control required");
        assert_eq!(
            host::read(control.join("asset")).unwrap(),
            b"EXTERNAL CANARY"
        );
        host::remove_dir(control).unwrap();
        {
            let root = Dir::open(temp.path()).unwrap();
            let held = root.dir("held", true).unwrap();
            let other = root.dir("other", true).unwrap();
            other.write_new("payload", b"owned payload", false).unwrap();
            let path = held.path.clone();
            let target = external.clone();
            let hook: Box<dyn FnOnce()> = Box::new(move || {
                junction(&path, &target).expect("live empty-root junction effect");
            });
            if op == 9 {
                BEFORE_RENAME.with(|h| *h.borrow_mut() = Some(hook));
            } else {
                AFTER_VALIDATE.with(|h| *h.borrow_mut() = Some(hook));
            }
            operation(&held, &other, op, true);
            // sync_tree validates before its first list; all operations consume.
            assert!(AFTER_VALIDATE.with(|h| h.borrow().is_none()));
            assert!(BEFORE_RENAME.with(|h| h.borrow().is_none()));
            assert_eq!(
                canary(&external),
                before,
                "external effects for operation {op}"
            );
        }
        host::remove_dir(temp.path().join("held")).unwrap();
        temp.close()
            .expect("close every anchor before owned fixture cleanup");
    }
}

#[test]
fn rooted_final_reparse_and_ancestor_replacement_preserve_external_tree() {
    for op in 0..12 {
        let temp = tempfile::tempdir().unwrap();
        let external = external_fixture(temp.path());
        let before = canary(&external);
        {
            let root = Dir::open(temp.path()).unwrap();
            let parent = root.dir("ancestor", true).unwrap();
            let held = parent.dir("held", true).unwrap();
            let other = root.dir("other", true).unwrap();
            other.write_new("payload", b"owned", false).unwrap();
            // A deeper live anchor makes an ancestor nonempty. The attack must
            // replace that ancestor first; no-delete sharing pins that operation.
            let ancestor = parent.path.clone();
            let moved = root.path.join("moved");
            AFTER_VALIDATE.with(|h| {
                *h.borrow_mut() = Some(Box::new(move || {
                    assert!(host::rename(&ancestor, &moved).is_err());
                }))
            });
            operation(&held, &other, op, false);
            assert_eq!(canary(&external), before);
        }
        // Equivalent unguarded ancestor replacement succeeds after handles close.
        host::rename(temp.path().join("ancestor"), temp.path().join("moved")).unwrap();
        host::create_dir(temp.path().join("ancestor")).unwrap();
        junction(&temp.path().join("ancestor"), &external).unwrap();
        assert!(Dir::open(&temp.path().join("ancestor/tree")).is_err());
        host::remove_dir(temp.path().join("ancestor")).unwrap();
        {
            let root = Dir::open(temp.path()).unwrap();
            host::create_dir(temp.path().join("link")).unwrap();
            junction(&temp.path().join("link"), &external).unwrap();
            assert!(root.dir("link", false).is_err());
            assert!(root.read("link", 100).is_err());
            assert!(root.lock("link").is_err());
            assert!(root.rename("link", &root, "moved-link").is_err());
            assert!(root.remove("link").is_err());
            assert_eq!(canary(&external), before);
        }
        host::remove_dir(temp.path().join("link")).unwrap();
        temp.close().unwrap();
    }
}

#[test]
fn rooted_checked_rename_rejects_same_name_replacement() {
    let temp = tempfile::tempdir().unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        let state = root.dir("state", true).unwrap();
        root.write_new("owned", b"old", false).unwrap();
        let expected = super::super::Snapshot::one(b"old".to_vec()).fingerprint();
        let path = root.path.join("owned");
        BEFORE_MUTATION.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                host::remove_file(&path).unwrap();
                host::write(path, b"FOREIGN").unwrap();
            }))
        });
        assert!(
            root.rename_checked("owned", &state, "backup", &expected)
                .is_err()
        );
        assert_eq!(root.read("owned", 100).unwrap().0, b"FOREIGN");
        assert!(state.kind("backup").unwrap().is_none());
    }
    temp.close().unwrap();
}

pub(super) fn before_rename() {
    let hook = BEFORE_RENAME.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[test]
fn rooted_enumeration_exact_limit_and_name_policy() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..16384 {
        host::File::create(temp.path().join(format!("entry-{i:05}"))).unwrap();
    }
    {
        let root = Dir::open(temp.path()).unwrap();
        assert_eq!(root.list().unwrap().len(), 16384);
        root.write_new("overflow", b"", false).unwrap();
        assert_eq!(root.list().err(), Some("TOTAL_FILES_16384"));
        for name in [
            "../escape",
            "..",
            ".",
            "CON",
            "a:stream",
            "a/b",
            "a\\b",
            "trailing.",
            "trailing ",
        ] {
            assert!(root.write_new(name, b"bad", false).is_err());
        }
        let longest = "x".repeat(240);
        root.write_new(&longest, b"exact", false).unwrap();
        assert_eq!(root.read(&longest, 10).unwrap().0, b"exact");
        assert!(root.write_new(&"x".repeat(241), b"bad", false).is_err());
    }
    temp.close().unwrap();
}

#[test]
fn rooted_initial_ancestor_chain_and_namespace_rejection() {
    let temp = tempfile::tempdir().unwrap();
    let external = external_fixture(temp.path());
    let before = canary(&external);
    let parent = temp.path().join("parent");
    host::create_dir(&parent).unwrap();
    let attack = parent.clone();
    let target = external.clone();
    AFTER_VALIDATE.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            junction(&attack, &target).expect("unguarded initial-chain positive control");
        }))
    });
    assert!(Dir::open(&parent.join("tree")).is_err());
    assert_eq!(canary(&external), before);
    for path in [
        r"\\.\C:\",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1\",
        r"C:\a\..\b",
    ] {
        assert!(Dir::open(Path::new(path)).is_err());
    }
    host::remove_dir(parent).unwrap();
    temp.close().unwrap();
}

#[test]
fn rooted_checked_cleanup_refuses_foreign_replacement() {
    let temp = tempfile::tempdir().unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        root.write_new("asset-owned", b"old", false).unwrap();
        let expected = super::super::Snapshot::one(b"old".to_vec()).fingerprint();
        let path = root.path.join("asset-owned");
        BEFORE_MUTATION.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                host::remove_file(&path).unwrap();
                host::write(path, b"FOREIGN").unwrap();
            }))
        });
        assert!(root.remove_checked("asset-owned", &expected).is_err());
        assert_eq!(root.read("asset-owned", 100).unwrap().0, b"FOREIGN");
    }
    temp.close().unwrap();
}

pub(super) fn after_read_open() {
    let hook = AFTER_READ_OPEN.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}
pub(super) fn before_remove() {
    let hook = BEFORE_REMOVE.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[test]
fn rooted_scan_completion_requires_terminal_and_rejects_no_progress_or_overflow() {
    use windows_sys::Win32::Foundation::{
        STATUS_BUFFER_OVERFLOW, STATUS_NO_MORE_FILES, STATUS_PENDING,
    };
    let page = super::windows::scan_page;
    assert_eq!(page(0, 66, 65536), Ok(true));
    assert_eq!(page(STATUS_NO_MORE_FILES, 0, 65536), Ok(false));
    for (status, bytes) in [
        (0, 0),
        (0, 65537),
        (STATUS_BUFFER_OVERFLOW, 64),
        (STATUS_PENDING, 64),
        (-1, 64),
        (STATUS_NO_MORE_FILES, 64),
    ] {
        assert!(page(status, bytes, 65536).is_err());
    }
}

#[test]
fn rooted_live_regular_read_denies_write_append_delete_and_lock_contends() {
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_APPEND_DATA, FILE_WRITE_DATA};
    let temp = tempfile::tempdir().unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        root.write_new("asset", b"immutable", false).unwrap();
        let path = root.path.join("asset");
        AFTER_READ_OPEN.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                for access in [FILE_WRITE_DATA, FILE_APPEND_DATA, DELETE] {
                    assert!(
                        host::OpenOptions::new()
                            .access_mode(access)
                            .share_mode(7)
                            .open(&path)
                            .is_err()
                    );
                }
                assert!(host::remove_file(&path).is_err());
            }))
        });
        assert_eq!(root.read("asset", 100).unwrap().0, b"immutable");
        root.write_new("lock", b"", false).unwrap();
        let lock = root.lock("lock").unwrap();
        assert_eq!(root.lock("lock").err(), Some("PROFILE_RECONCILE_BUSY"));
        drop(lock);
        drop(root.lock("lock").unwrap());
    }
    temp.close().unwrap();
}

#[test]
fn rooted_recursive_mutation_preserves_late_foreign_entry() {
    let temp = tempfile::tempdir().unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        root.dir("tree", true)
            .unwrap()
            .write_new("owned", b"old", false)
            .unwrap();
        let foreign = root.path.join("tree/foreign");
        BEFORE_REMOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                host::write(&foreign, b"FOREIGN").unwrap();
            }))
        });
        assert!(root.remove("tree").is_err());
        assert_eq!(
            host::read(root.path.join("tree/foreign")).unwrap(),
            b"FOREIGN"
        );
    }
    temp.close().unwrap();
}

#[test]
fn rooted_source_reparse_after_acquisition_moves_only_the_acquired_object() {
    let temp = tempfile::tempdir().unwrap();
    let external = external_fixture(temp.path());
    let before = canary(&external);
    let control = temp.path().join("control");
    host::create_dir(&control).unwrap();
    junction(&control, &external).expect("unguarded source-retarget control");
    host::remove_dir(control).unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        let state = root.dir("state", true).unwrap();
        drop(state.dir("stage", true).unwrap());
        let path = state.path.join("stage");
        let target = external.clone();
        BEFORE_RENAME.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                junction(&path, &target).expect("live DELETE source object retarget");
            }))
        });
        let _ = state.rename("stage", &root, "published");
        assert!(BEFORE_RENAME.with(|h| h.borrow().is_none()));
        assert_eq!(canary(&external), before);
        // A moved junction must never become an accepted directory for descent.
        if host::symlink_metadata(root.path.join("published")).is_ok() {
            assert!(root.dir("published", false).is_err());
        }
    }
    for path in [
        temp.path().join("state/stage"),
        temp.path().join("published"),
    ] {
        if host::symlink_metadata(&path).is_ok() {
            host::remove_dir(path).unwrap();
        }
    }
    temp.close().unwrap();
}

#[test]
fn rooted_recursive_mutation_retarget_after_acquisition_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let external = external_fixture(temp.path());
    let before = canary(&external);
    let control = temp.path().join("control");
    host::create_dir(&control).unwrap();
    junction(&control, &external).unwrap();
    host::remove_dir(control).unwrap();
    {
        let root = Dir::open(temp.path()).unwrap();
        drop(root.dir("tree", true).unwrap());
        let path = root.path.join("tree");
        let target = external.clone();
        BEFORE_REMOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                junction(&path, &target).expect("live empty mutation-root retarget");
            }))
        });
        assert!(root.remove("tree").is_err());
        assert!(BEFORE_REMOVE.with(|h| h.borrow().is_none()));
        assert_eq!(canary(&external), before);
    }
    host::remove_dir(temp.path().join("tree")).unwrap();
    temp.close().unwrap();
}

#[test]
fn fix1_short_checked_file_directory_names_and_atomic_replacement() {
    for name in ["x", "가", "xy", "가나", "😀"] {
        for directory in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            {
                let root = Dir::open(temp.path()).unwrap();
                let state = root.dir("state", true).unwrap();
                if directory {
                    state
                        .dir("stage", true)
                        .unwrap()
                        .write_new("content", b"old", false)
                        .unwrap();
                } else {
                    state.write_new("stage", b"old", false).unwrap();
                }
                let expected = super::super::scan::snapshot(
                    &state,
                    "stage",
                    true,
                    &mut super::super::Budget::new(),
                    false,
                )
                .unwrap()
                .fingerprint();
                state
                    .rename_checked("stage", &root, name, &expected)
                    .unwrap();
                if directory {
                    assert_eq!(
                        root.dir(name, false)
                            .unwrap()
                            .read("content", 10)
                            .unwrap()
                            .0,
                        b"old"
                    );
                } else {
                    assert_eq!(root.read(name, 10).unwrap().0, b"old");
                }
                root.remove(name).unwrap();
                state.atomic(name, b"old").unwrap();
                state.atomic(name, b"new").unwrap();
                assert_eq!(state.read(name, 10).unwrap().0, b"new");
            }
            temp.close().unwrap();
        }
    }
}

#[test]
fn fix1_registry_relative_normal_verbatim_drive_unc_both_directions() {
    for roots in [
        [r"C:\owned\plugins", r"\\?\C:\owned\plugins"],
        [
            r"\\server\share\owned\plugins",
            r"\\?\UNC\server\share\owned\plugins",
        ],
    ] {
        for root in roots {
            for base in roots {
                for tail in [r"cache\demo\1", "cache/demo/1"] {
                    let path = Path::new(base).join(tail);
                    assert_eq!(
                        super::registry_relative(Path::new(root), &path).unwrap(),
                        "cache/demo/1"
                    );
                }
                assert!(
                    super::registry_relative(Path::new(root), &Path::new(base).join("../escape"))
                        .is_err()
                );
            }
        }
    }
    for path in [
        r"C:\owned\plugins-sibling\cache\demo",
        r"D:\owned\plugins\cache\demo",
        r"\\.\C:\owned\plugins\cache\demo",
        r"\\?\GLOBALROOT\Device\cache\demo",
    ] {
        assert!(super::registry_relative(Path::new(r"C:\owned\plugins"), Path::new(path)).is_err());
    }
}

pub(super) fn cleanup_after_dispose() -> R<()> {
    CLEANUP_STOP.with(|count| match count.get() {
        Some(0) | Some(1) => {
            count.set(None);
            Err("TEST_CLEANUP_INTERRUPTED")
        }
        Some(n) => {
            count.set(Some(n - 1));
            Ok(())
        }
        None => Ok(()),
    })
}

pub(super) fn checked_rename_boundary(parent: &Path, from: &str) {
    let hook = CHECKED_RENAME.with(|h| h.borrow_mut().take());
    if let Some(mut hook) = hook {
        if !hook(parent, from) {
            CHECKED_RENAME.with(|h| *h.borrow_mut() = Some(hook));
        }
    }
}

// Construct an actually published replacement with an intact two-child backup.
// The helper returns no filesystem handles; restart tests reopen everything.
fn cleanup_fixture(path: &Path) {
    use super::super::*;
    let root = Dir::open(path).unwrap();
    let state = root.dir(STATE, true).unwrap();
    let before = Snapshot {
        directory: true,
        dirs: vec![],
        files: vec![
            FileData {
                path: "a".into(),
                bytes: b"OLD A".to_vec(),
                exec: false,
            },
            FileData {
                path: "b".into(),
                bytes: b"OLD B".to_vec(),
                exec: false,
            },
        ],
    };
    let after = Snapshot::one(b"NEW".to_vec());
    before.stage(&state, "backup-restart").unwrap();
    after.stage(&root, "published").unwrap();
    let target = Target::file("published");
    let intent = Intent {
        entry: Entry {
            unit: None,
            target,
            kind: "commands".into(),
            source: after.fingerprint(),
            written: Some(after.fingerprint()),
            state: "owned".into(),
        },
        before: Some(before.fingerprint()),
        after: Some(after.fingerprint()),
        stage: "asset-restart".into(),
        backup: "backup-restart".into(),
        cleanup_complete: Vec::new(),
    };
    state
        .atomic("intent.json", &serde_json::to_vec(&intent).unwrap())
        .unwrap();
    persist(
        &state,
        &Manifest {
            version: 1,
            root: "fixture".into(),
            entries: Default::default(),
        },
    )
    .unwrap();
}
fn recover_fixture(path: &Path) -> R<()> {
    use super::super::*;
    let root = Dir::open(path)?;
    let state = root.dir(STATE, false)?;
    let mut manifest: Manifest =
        serde_json::from_slice(&state.read(MANIFEST, METADATA_BYTES)?.0).unwrap();
    recover(&root, &state, &mut manifest)
}

#[test]
fn fix1_interrupted_cleanup_reopens_record_and_recovers_twice() {
    let temp = tempfile::tempdir().unwrap();
    cleanup_fixture(temp.path());
    CLEANUP_STOP.with(|v| v.set(Some(1)));
    assert_eq!(
        recover_fixture(temp.path()),
        Err("TEST_CLEANUP_INTERRUPTED")
    );
    let state = temp.path().join(super::super::STATE);
    assert!(!state.join("backup-restart/a").exists());
    assert_eq!(
        host::read(state.join("backup-restart/b")).unwrap(),
        b"OLD B"
    );
    let progress: serde_json::Value =
        serde_json::from_slice(&host::read(state.join("cleanup-progress.json")).unwrap()).unwrap();
    assert_eq!(progress["pending"], 0);
    assert_eq!(progress["done"], 0);
    // recover_fixture's stack owns every handle; no in-memory plan survives.
    recover_fixture(temp.path()).unwrap();
    recover_fixture(temp.path()).unwrap();
    assert_eq!(host::read(temp.path().join("published")).unwrap(), b"NEW");
    assert!(!state.join("backup-restart").exists());
    assert!(!state.join("cleanup.json").exists());
    assert!(!state.join("intent.json").exists());
    temp.close().unwrap();
}

#[test]
fn fix1_cleanup_restart_preserves_inserted_replaced_modified_and_unproven_legacy() {
    for attack in ["insert", "replace", "modify", "legacy"] {
        let temp = tempfile::tempdir().unwrap();
        cleanup_fixture(temp.path());
        let state = temp.path().join(super::super::STATE);
        let backup = state.join("backup-restart");
        if attack != "legacy" {
            CLEANUP_STOP.with(|v| v.set(Some(1)));
            assert_eq!(
                recover_fixture(temp.path()),
                Err("TEST_CLEANUP_INTERRUPTED")
            );
        } else {
            host::remove_file(backup.join("a")).unwrap();
        }
        match attack {
            "insert" => host::write(backup.join("foreign"), b"FOREIGN").unwrap(),
            "replace" => {
                host::rename(backup.join("b"), state.join("kept-original-b")).unwrap();
                host::write(backup.join("b"), b"OLD B").unwrap();
            }
            "modify" => host::write(backup.join("b"), b"FOREIGN").unwrap(),
            _ => (),
        }
        let before = canary(&backup);
        assert!(recover_fixture(temp.path()).is_err());
        assert!(recover_fixture(temp.path()).is_err());
        assert_eq!(canary(&backup), before);
        assert!(state.join("intent.json").exists());
        assert_eq!(host::read(temp.path().join("published")).unwrap(), b"NEW");
        temp.close().unwrap();
    }
}

#[test]
fn fix1_cleanup_record_rejects_corruption_and_exact_intent_mismatch() {
    for corruption in [
        "version",
        "duplicate",
        "escape",
        "unknown-field",
        "intent",
        "oversize",
        "planned-absence",
        "progress-oversize",
    ] {
        let temp = tempfile::tempdir().unwrap();
        cleanup_fixture(temp.path());
        CLEANUP_STOP.with(|v| v.set(Some(1)));
        assert_eq!(
            recover_fixture(temp.path()),
            Err("TEST_CLEANUP_INTERRUPTED")
        );
        let state = temp.path().join(super::super::STATE);
        let path = state.join("cleanup.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&host::read(&path).unwrap()).unwrap();
        match corruption {
            "version" => record["version"] = 2.into(),
            "duplicate" => {
                let duplicate = record["nodes"][0].clone();
                record["nodes"].as_array_mut().unwrap().insert(0, duplicate);
            }
            "escape" => record["nodes"][0]["path"] = "../escape".into(),
            "unknown-field" => record["unknown"] = true.into(),
            "intent" => record["intent"] = "0".repeat(64).into(),
            "planned-absence" => (),
            _ => (),
        }
        let bytes = if corruption == "oversize" {
            vec![b' '; super::windows_cleanup::RECORD_LIMIT + 1]
        } else {
            serde_json::to_vec(&record).unwrap()
        };
        if corruption == "progress-oversize" {
            host::write(state.join("cleanup-progress.json"), vec![b' '; 1025]).unwrap();
        } else if corruption == "planned-absence" {
            let path = state.join("cleanup-progress.json");
            let mut progress: serde_json::Value =
                serde_json::from_slice(&host::read(&path).unwrap()).unwrap();
            progress["pending"] = serde_json::Value::Null;
            host::write(path, serde_json::to_vec(&progress).unwrap()).unwrap();
        } else {
            host::write(&path, bytes).unwrap();
        }
        let before = canary(&state.join("backup-restart"));
        assert!(recover_fixture(temp.path()).is_err());
        assert_eq!(canary(&state.join("backup-restart")), before);
        assert!(state.join("intent.json").exists());
        temp.close().unwrap();
    }
}

#[test]
fn fix1_retargeted_empty_asset_is_contained_never_prepared_and_old_asset_restored() {
    use super::super::*;
    for family in ["commands", "workflows"] {
        for replacement in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            let dest = temp.path().join("profile");
            host::create_dir_all(source.join(family).join("empty")).unwrap();
            host::create_dir(&dest).unwrap();
            let external = external_fixture(temp.path());
            let canaries = canary(&external);
            let control = temp.path().join("control");
            host::create_dir(&control).unwrap();
            junction(&control, &external).unwrap();
            host::remove_dir(control).unwrap();
            let old_manifest = if replacement {
                host::write(source.join(family).join("empty/old.md"), b"PRIOR OWNED").unwrap();
                let report = reconcile(crate::Tool::Claude, &source, &dest);
                assert!(
                    !report.entries.iter().any(|e| e.state == "failed"),
                    "{report:?}"
                );
                host::remove_file(source.join(family).join("empty/old.md")).unwrap();
                Some(host::read(dest.join(STATE).join(MANIFEST)).unwrap())
            } else {
                None
            };
            let target = external.clone();
            CHECKED_RENAME.with(|h| {
                *h.borrow_mut() = Some(Box::new(move |parent, from| {
                    if !from.starts_with("asset-") {
                        return false;
                    }
                    junction(&parent.join(from), &target)
                        .expect("retarget already-fingerprinted empty stage");
                    true
                }))
            });
            let report = reconcile(crate::Tool::Claude, &source, &dest);
            assert!(CHECKED_RENAME.with(|h| h.borrow().is_none()));
            assert!(
                report
                    .entries
                    .iter()
                    .any(|e| e.state == "failed" && e.reason == "PUBLICATION_OBJECT_CHANGED"),
                "{report:?}"
            );
            assert!(
                !report
                    .entries
                    .iter()
                    .any(|e| e.state == "prepared" && e.name.contains("empty")),
                "{report:?}"
            );
            let target = dest.join(family).join("empty");
            if replacement {
                use std::os::windows::fs::MetadataExt;
                assert_eq!(
                    host::symlink_metadata(&target).unwrap().file_attributes() & 0x400,
                    0
                );
                assert_eq!(host::read(target.join("old.md")).unwrap(), b"PRIOR OWNED");
                assert_eq!(
                    host::read(dest.join(STATE).join(MANIFEST)).unwrap(),
                    old_manifest.unwrap()
                );
            } else {
                assert!(host::symlink_metadata(&target).is_err());
                let manifest: Manifest =
                    serde_json::from_slice(&host::read(dest.join(STATE).join(MANIFEST)).unwrap())
                        .unwrap();
                assert!(
                    !manifest
                        .entries
                        .values()
                        .any(|e| e.target.path == format!("{family}/empty"))
                );
            }
            let intent: Intent =
                serde_json::from_slice(&host::read(dest.join(STATE).join("intent.json")).unwrap())
                    .unwrap();
            assert!(host::symlink_metadata(dest.join(STATE).join(&intent.stage)).is_ok());
            assert!(!dest.join(STATE).join(&intent.backup).exists());
            assert_eq!(canary(&external), canaries);
            // The rejected object remains private and linked to the retained intent;
            // test-only fixture teardown unlinks that junction after all handles close.
            host::remove_dir(dest.join(STATE).join(&intent.stage)).unwrap();
            temp.close().unwrap();
        }
    }
}

pub(super) fn cleanup_after_progress_removal() -> R<()> {
    if STOP_AFTER_PROGRESS_REMOVAL.replace(false) {
        Err("TEST_METADATA_RETIREMENT_INTERRUPTED")
    } else {
        Ok(())
    }
}

#[test]
fn fix1_cleanup_metadata_retirement_restart_and_completed_target_reappearance() {
    for foreign in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        cleanup_fixture(temp.path());
        STOP_AFTER_PROGRESS_REMOVAL.set(true);
        assert_eq!(
            recover_fixture(temp.path()),
            Err("TEST_METADATA_RETIREMENT_INTERRUPTED")
        );
        let state = temp.path().join(super::super::STATE);
        assert!(state.join("cleanup.json").exists());
        assert!(!state.join("cleanup-progress.json").exists());
        assert!(!state.join("backup-restart").exists());
        if foreign {
            host::create_dir(state.join("backup-restart")).unwrap();
            host::write(state.join("backup-restart/a"), b"OLD A").unwrap();
            host::write(state.join("backup-restart/b"), b"OLD B").unwrap();
            let before = canary(&state.join("backup-restart"));
            assert_eq!(
                recover_fixture(temp.path()),
                Err("CLEANUP_COMPLETED_TARGET_REAPPEARED")
            );
            assert_eq!(
                recover_fixture(temp.path()),
                Err("CLEANUP_COMPLETED_TARGET_REAPPEARED")
            );
            assert_eq!(canary(&state.join("backup-restart")), before);
            assert!(state.join("intent.json").exists());
        } else {
            recover_fixture(temp.path()).unwrap();
            recover_fixture(temp.path()).unwrap();
            assert!(!state.join("cleanup.json").exists());
            assert!(!state.join("intent.json").exists());
        }
        temp.close().unwrap();
    }
}

#[test]
fn fix1_cleanup_metadata_serializer_enforces_exact_plan_bound() {
    let exact = "x".repeat(16 * 1024 * 1024 - 2);
    assert_eq!(
        super::windows_cleanup::encode(&exact, 16 * 1024 * 1024)
            .unwrap()
            .len(),
        16 * 1024 * 1024
    );
    assert!(super::windows_cleanup::encode(&(exact + "x"), 16 * 1024 * 1024).is_err());
}
