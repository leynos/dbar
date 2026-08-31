//! Concurrency coverage for the cache directory's shared mutation lock.

use std::sync::mpsc;
use std::time::Duration;

use camino::Utf8PathBuf;
use cap_std::{ambient_authority, fs_utf8::Dir};
use mockable::DefaultClock;
use rstest::rstest;
use tempfile::TempDir;

use super::{
    CacheError, CacheTtlSeconds, acquire_mutation_lock, store_cached_value, sweep_cache_dir,
};

/// The short interval used only to prove a mutation remains blocked by a held lock.
const BLOCKED_FOR: Duration = Duration::from_millis(100);

/// A cache root whose UTF-8 path is retained by the temporary-directory guard.
fn cache_root() -> Result<(TempDir, Utf8PathBuf), CacheError> {
    let root = TempDir::new().map_err(CacheError::Io)?;
    let path = Utf8PathBuf::from_path_buf(root.path().to_path_buf())
        .map_err(|_| CacheError::InvalidUtf8)?;
    Ok((root, path))
}

/// Assert that one worker cannot finish while another process owns the lock.
fn assert_blocked(done: &mpsc::Receiver<Result<(), CacheError>>) {
    assert!(
        matches!(
            done.recv_timeout(BLOCKED_FOR),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "the cache mutation must wait for the directory lock"
    );
}

#[rstest]
fn storing_waits_for_an_in_progress_sweep_lock() {
    let (_guard, root) = cache_root().expect("cache root");
    let dir = Dir::open_ambient_dir(&root, ambient_authority()).expect("open cache root");
    let lock = acquire_mutation_lock(&dir).expect("hold mutation lock");
    let path = root.join("cache.json");
    let (done_send, done) = mpsc::channel();

    std::thread::spawn(move || {
        let clock = DefaultClock;
        drop(done_send.send(store_cached_value(&path, &clock, "42")));
    });
    assert_blocked(&done);
    drop(lock);
    done.recv_timeout(Duration::from_secs(2))
        .expect("writer completes after lock release")
        .expect("writer succeeds");
}

#[rstest]
fn sweeping_waits_for_an_in_progress_write_lock() {
    let (_guard, root) = cache_root().expect("cache root");
    let dir = Dir::open_ambient_dir(&root, ambient_authority()).expect("open cache root");
    let lock = acquire_mutation_lock(&dir).expect("hold mutation lock");
    let (done_send, done) = mpsc::channel();

    std::thread::spawn(move || {
        let clock = DefaultClock;
        drop(done_send.send(sweep_cache_dir(&root, &clock, CacheTtlSeconds::new(60))));
    });
    assert_blocked(&done);
    drop(lock);
    done.recv_timeout(Duration::from_secs(2))
        .expect("sweep completes after lock release")
        .expect("sweep succeeds");
}
