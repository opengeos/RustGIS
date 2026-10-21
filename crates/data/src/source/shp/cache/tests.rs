use super::*;
use crate::source::FeatureSource;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rustgis-cache-test-{}", unique()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        // Only the unique direct child of the test temp directory belongs to this fixture.
        if self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn key(n: usize) -> String {
    format!("{n:016x}-0000000000000001")
}

#[test]
fn cache_test_index_roots_are_scoped_and_thread_local() {
    let first = Folder::new();
    let second = Folder::new();
    let original = index_dir();
    with_index_dir(first.0.clone(), || {
        assert_eq!(index_dir(), first.0);
        let outer = first.0.clone();
        std::thread::spawn(move || assert_ne!(index_dir(), outer)).join().unwrap();
        let result = std::panic::catch_unwind(|| with_index_dir(second.0.clone(), || panic!("test unwind")));
        assert!(result.is_err());
        assert_eq!(index_dir(), first.0);
    });
    assert_eq!(index_dir(), original);
}
fn ready(root: &Path, n: usize, bytes: u64, budget: u64) -> Lease {
    let mut lease = reserve_index(root, &key(n), budget).unwrap();
    File::options().write(true).open(lease.path()).unwrap().set_len(bytes).unwrap();
    lease.commit().unwrap();
    lease
}

#[test]
fn cache_lru_touches_reopened_indexes_and_evicts_oldest_idle() {
    let root = Folder::new();
    for n in 1..=3 {
        let lease = ready(&root.0, n, 10, 4096);
        let path = lease.path().to_path_buf();
        drop(lease);
        File::options().write(true).open(lease_path(&path)).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(n as u64)).unwrap();
    }
    let hot = pin_index(&root.0, &key(1), 4096).unwrap().unwrap();
    drop(hot);
    let stats = maintain(&root.0, 20).unwrap();
    assert_eq!(stats.remaining_bytes, 20);
    assert_eq!(stats.removed, 1);
    assert!(index_path(&root.0, &key(1)).unwrap().exists());
    assert!(!index_path(&root.0, &key(2)).unwrap().exists());
    assert!(index_path(&root.0, &key(3)).unwrap().exists());
}

#[test]
fn cache_shared_readers_and_oversized_indexes_are_protected_until_release() {
    let root = Folder::new();
    let first = ready(&root.0, 1, 10, 4096);
    let second = pin_index(&root.0, &key(1), 4096).unwrap().unwrap();
    let path = first.path().to_path_buf();
    assert_eq!(maintain(&root.0, 0).unwrap().pinned_bytes, 10);
    drop(first);
    assert!(path.exists());
    assert_eq!(maintain(&root.0, 0).unwrap().remaining_bytes, 10);
    drop(second);
    assert_eq!(maintain(&root.0, 0).unwrap().remaining_bytes, 0);
    assert!(!path.exists());
    let oversized = ready(&root.0, 2, 40, 20);
    let path = oversized.path().to_path_buf();
    assert_eq!(maintain(&root.0, 20).unwrap().pinned_bytes, 40);
    assert!(path.exists());
    drop(oversized);
    assert!(!path.exists(), "oversized indexes must remain session-only");
}

#[test]
fn cache_release_racing_a_sweep_is_retried_without_another_checkout() {
    let root = Folder::new();
    let (lease, file) = temporary_index(&root.0);
    drop(file);
    let path = lease.path().to_path_buf();
    let sweep = directory_lock(&root.0).unwrap();
    drop(lease);
    assert!(path.exists(), "a busy sweep owns directory mutations");
    drop(sweep);
    let deadline = Instant::now() + Duration::from_secs(10);
    while path.exists() {
        assert!(Instant::now() < deadline, "release cleanup was never retried");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cache_concurrent_builder_uses_private_index_without_replacing_reader() {
    let root = Folder::new();
    let reader = ready(&root.0, 1, 10, 4096);
    let mut private = reserve_index(&root.0, &key(1), 4096).unwrap();
    assert_ne!(reader.path(), private.path());
    File::options().write(true).open(private.path()).unwrap().set_len(30).unwrap();
    private.commit().unwrap();
    assert_eq!(std::fs::metadata(reader.path()).unwrap().len(), 10);
    let path = private.path().to_path_buf();
    drop(private);
    assert!(!path.exists());
    assert!(reader.path().exists());
}

#[test]
fn cache_cleanup_is_bounded_and_preserves_unknown_files_and_orphans_in_use() {
    let root = Folder::new();
    std::fs::write(root.0.join("customer.sqlite"), b"keep").unwrap();
    let unleased = root.0.join("rg-index-work-v1-1-2-4.sqlite");
    std::fs::write(&unleased, b"unverified").unwrap();
    assert!(!owned("rg-index-work-v1-abc.sqlite"));
    std::fs::write(root.0.join("0000000000000000.sqlite"), b"legacy index: ownership uncertain").unwrap();
    for n in 0..CANDIDATES + 3 {
        let path = index_path(&root.0, &key(n)).unwrap();
        std::fs::write(&path, [1]).unwrap();
        std::fs::write(lease_path(&path), b"C").unwrap();
    }
    let orphan = root.0.join("rg-index-work-v1-1-2-3.lease");
    let lock = open_lock(&orphan).unwrap();
    lock.lock().unwrap();
    let stats = maintain(&root.0, 2).unwrap();
    assert_eq!(stats.remaining_bytes, 10);
    assert_eq!(stats.removed, (CANDIDATES + 3) as u64);
    assert_eq!(std::fs::read(unleased).unwrap(), b"unverified");
    assert!(orphan.exists());
    drop(lock);
    maintain(&root.0, 2).unwrap();
    assert!(!orphan.exists());
    assert_eq!(std::fs::read(root.0.join("customer.sqlite")).unwrap(), b"keep");
    assert!(root.0.join("0000000000000000.sqlite").exists());
    assert!(reserve_index(&root.0, "../customer", 2).is_err());
}

struct Process(Child, PathBuf);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(root: &Path, mode: &str) -> Process {
    let marker = root.join(format!("ready-{}", unique()));
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "source::shp::cache::tests::cache_child_fixture", "--nocapture"])
        .env("RUSTGIS_CACHE_TEST_READY", &marker)
        .env("RUSTGIS_CACHE_TEST_ROOT", root)
        .env("RUSTGIS_CACHE_TEST_MODE", mode)
        .env("RUSTGIS_SHP_CACHE", if matches!(mode, "fallback" | "scan") { root.join("blocked") } else { root.to_path_buf() })
        .env("TMP", root)
        .env("TEMP", root)
        .env("TMPDIR", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    Process(child, marker)
}
fn wait_ready(child: &mut Process, root: &Path) -> PathBuf {
    let marker = &child.1;
    assert!(marker.starts_with(root));
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        assert!(child.0.try_wait().unwrap().is_none(), "cache child exited before holding its lease");
        assert!(Instant::now() < deadline, "cache child did not become ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    PathBuf::from(std::fs::read_to_string(marker).unwrap())
}

/// Real process death releases OS leases without running any Rust destructors.
#[test]
fn cache_killed_process_is_reclaimed_at_checkout_but_live_instance_is_preserved() {
    let root = Folder::new();
    let mut live = spawn(&root.0, "temporary");
    let live_path = wait_ready(&mut live, &root.0);
    for mode in ["temporary", "building", "index"] {
        let mut dead = spawn(&root.0, mode);
        let dead_path = wait_ready(&mut dead, &root.0);
        assert!(maintain(&root.0, 0).unwrap().pinned_bytes >= 2048);
        if mode == "index" {
            let reader = pin_index(&root.0, &key(dead.0.id() as usize), 4096).unwrap().unwrap();
            assert_eq!(reader.path(), dead_path);
        }
        dead.0.kill().unwrap();
        dead.0.wait().unwrap();
        let mut cleaner = spawn(&root.0, "sweep");
        assert!(cleaner.0.wait().unwrap().success());
        if mode == "index" {
            assert!(dead_path.exists(), "a ready index below budget must survive restart");
            maintain(&root.0, 0).unwrap();
        }
        assert!(!dead_path.exists());
        assert!(live_path.exists(), "checkout deleted another running instance's temporary index");
    }
    live.0.kill().unwrap();
    live.0.wait().unwrap();
    let mut cleaner = spawn(&root.0, "sweep");
    assert!(cleaner.0.wait().unwrap().success());
    assert!(!live_path.exists());
}

#[test]
fn cache_child_fixture() {
    let Some(root) = std::env::var_os("RUSTGIS_CACHE_TEST_ROOT").map(PathBuf::from) else { return };
    let mode = std::env::var("RUSTGIS_CACHE_TEST_MODE").unwrap();
    if mode == "sweep" {
        checkout().unwrap();
        return;
    }
    if mode == "scan" {
        std::fs::write(root.join("rustgis-shp-index"), b"keep fallback").unwrap();
        let mut ds = crate::Dataset::new("scan", crate::GeometryType::Point, vec![]);
        ds.push(Some(crate::Geometry::Point(crate::Coord::new(1.0, 2.0))), vec![]);
        let path = root.join("source.shp");
        crate::io::write_path(&ds, &path).unwrap();
        let source = super::super::ShpSource::open_path(&path, "scan").unwrap();
        assert!(!source.info().indexed);
        assert!(source.info().notes.iter().any(|n| n.contains("scanning record headers")));
        for limit in [0, 1, 100] {
            let batch = source.read(&crate::source::Request { extent: Some(crate::Extent::new(0.0, 0.0, 3.0, 3.0)), limit }).unwrap();
            assert_eq!(batch.matched, 1);
            assert_eq!(batch.features.len(), limit.min(1));
        }
        assert_eq!(std::fs::read(root.join("blocked")).unwrap(), b"keep");
        assert_eq!(std::fs::read(root.join("rustgis-shp-index")).unwrap(), b"keep fallback");
        return;
    }
    if mode == "fallback" {
        let mut ds = crate::Dataset::new("fallback", crate::GeometryType::Point, vec![]);
        ds.push(Some(crate::Geometry::Point(crate::Coord::new(1.0, 2.0))), vec![]);
        let path = root.join("source.shp");
        crate::io::write_path(&ds, &path).unwrap();
        let source = crate::source::open_path(&path, None).unwrap();
        assert_eq!(source.read(&crate::source::Request::all()).unwrap().features.len(), 1);
        drop(source);
        let fallback = index_fallback_dir();
        let index = std::fs::read_dir(&fallback)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rg-index-v1-"))
                    && p.extension().is_some_and(|e| e == "sqlite")
            })
            .unwrap();
        let db = rusqlite::Connection::open(&index).unwrap();
        db.execute_batch("CREATE TABLE reuse_probe(value INTEGER)").unwrap();
        drop(db);
        drop(crate::source::open_path(&path, None).unwrap());
        let db = rusqlite::Connection::open(&index).unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM sqlite_master WHERE name='reuse_probe'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        drop(db);
        let abandoned = fallback.join("rg-index-work-v1-1-2-3.sqlite");
        std::fs::write(&abandoned, b"partial").unwrap();
        std::fs::write(lease_path(&abandoned), b"T").unwrap();
        checkout().unwrap();
        assert!(!abandoned.exists(), "checkout must sweep the fallback cache too");
        assert_eq!(std::fs::read(root.join("blocked")).unwrap(), b"keep");
        return;
    }
    let lease = if mode == "temporary" {
        let (lease, file) = temporary_index(&root);
        file.set_len(1024).unwrap();
        lease
    } else if mode == "index" {
        ready(&root, std::process::id() as usize, 1024, 4096)
    } else {
        let lease = reserve_index(&root, &key(std::process::id() as usize), 4096).unwrap();
        File::options().write(true).open(lease.path()).unwrap().set_len(1024).unwrap();
        lease
    };
    let marker = PathBuf::from(std::env::var_os("RUSTGIS_CACHE_TEST_READY").unwrap());
    let scratch = marker.with_extension("tmp");
    std::fs::write(&scratch, lease.path().to_string_lossy().as_bytes()).unwrap();
    std::fs::rename(scratch, marker).unwrap();
    std::thread::sleep(Duration::from_secs(60));
    drop(lease);
}

#[test]
fn cache_symlinks_never_authorize_deleting_or_modifying_their_targets() {
    let root = Folder::new();
    let other = Folder::new();
    let sentinel = other.0.join("sentinel");
    std::fs::write(&sentinel, b"private").unwrap();
    let link = root.0.join("rg-index-work-v1-1-2-3.sqlite");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&sentinel, &link).is_ok();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&sentinel, &link).is_ok();
    #[cfg(not(any(windows, unix)))]
    let linked = false;
    if linked {
        maintain(&root.0, 0).unwrap();
        assert!(std::fs::symlink_metadata(link).is_ok());
    }
    assert_eq!(std::fs::read(sentinel).unwrap(), b"private");
}

#[test]
fn cache_unwritable_primary_reuses_and_cleans_the_fallback_directory() {
    let root = Folder::new();
    std::fs::write(root.0.join("blocked"), b"keep").unwrap();
    let mut child = spawn(&root.0, "fallback");
    assert!(child.0.wait().unwrap().success(), "fallback cache was not reused or cleaned");
}

fn temporary_index(root: &Path) -> (Lease, File) {
    let _directory = directory_lock(root).unwrap();
    create_locked(root.join(format!("rg-index-work-v1-{}.sqlite", unique())), true, 4096).unwrap()
}
fn checkout() -> io::Result<()> {
    for root in [index_dir(), index_fallback_dir()] {
        if root.is_dir() {
            maintain(&root, index_budget())?;
        }
    }
    Ok(())
}

#[test]
fn cache_unwritable_locations_keep_read_only_shapefile_loading_available() {
    let root = Folder::new();
    std::fs::write(root.0.join("blocked"), b"keep").unwrap();
    let mut child = spawn(&root.0, "scan");
    assert!(child.0.wait().unwrap().success(), "scan fallback failed");
}

#[test]
fn cache_reservation_preserves_unverified_payload_even_at_the_requested_key() {
    let root = Folder::new();
    let path = index_path(&root.0, &key(17)).unwrap();
    std::fs::write(&path, b"unrelated").unwrap();
    let lease = reserve_index(&root.0, &key(17), 4096).unwrap();
    assert_ne!(lease.path(), path);
    assert_eq!(std::fs::read(&path).unwrap(), b"unrelated");
    drop(lease);
    maintain(&root.0, 0).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"unrelated");
}
