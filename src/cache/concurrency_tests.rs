//! Concurrency coverage for the cache writer's atomicity.
//!
//! The writer's claim is that a reader never observes a half-written entry.
//! Reading only after every writer has joined cannot test that claim: the
//! final state of an in-place write is just as well formed as the final state
//! of a write-then-rename. The reader here therefore runs *while* the writers
//! do, so every observation is taken from a directory that is actively being
//! rewritten.

use super::*;
use camino::{Utf8Path, Utf8PathBuf};
use mockable::DefaultClock;
use rstest::{fixture, rstest};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A temporary directory and the cache path the writers contend over.
type CachePath = Result<(TempDir, Utf8PathBuf), CacheError>;

#[fixture]
fn contended_path() -> CachePath {
    let temp_dir = TempDir::new().map_err(CacheError::Io)?;
    let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("cache.json"))
        .map_err(|_| CacheError::InvalidUtf8)?;
    Ok((temp_dir, path))
}

/// Threads writing to the one contended path.
const WRITERS: usize = 8;

/// How long the writers keep rewriting the entry.
const WRITE_WINDOW: Duration = Duration::from_millis(300);

/// The ceiling on the reader's loop, so a stalled writer cannot hang the test.
///
/// The loop is bounded by wall clock rather than by an iteration count: a
/// spin budget would end early on a fast machine and late on a loaded one,
/// which is how this kind of test turns flaky.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// A TTL long enough that nothing written during the test can expire.
const LIVE_TTL: CacheTtlSeconds = CacheTtlSeconds::new(600);

/// The value writer `id` stores, chosen so writers differ in payload length.
///
/// Equal-length payloads would let a torn write happen to produce a
/// well-formed entry; varying the length makes an interleaving far more
/// likely to be visibly malformed.
fn writer_value(id: usize) -> String {
    id.to_string().repeat(id + 1)
}

/// What a reader saw across one run of concurrent writes.
#[derive(Debug, Default)]
struct ReadCensus {
    /// Complete entries holding a value some writer actually stored.
    fresh: usize,
    /// Absences, which the rename window legitimately produces.
    missing: usize,
    /// Observations that were neither: each is a torn read, described.
    torn: Vec<String>,
}

/// Classify one observation of the cache file.
///
/// Split from the polling loop so the loop states only *when* to stop and this
/// states *what* was seen; the two clusters were one function before, which
/// read as a single bumpy road.
fn classify(
    observation: Result<CacheLookup, CacheError>,
    expected: &[String],
    census: &mut ReadCensus,
) {
    match observation {
        Ok(CacheLookup::Fresh(value)) if expected.contains(&value) => census.fresh += 1,
        Ok(CacheLookup::Fresh(value)) => census
            .torn
            .push(format!("entry held a value no writer stored: {value:?}")),
        // A reader that arrives inside the rename window sees no file at all,
        // which is a clean absence rather than a partial entry.
        Ok(CacheLookup::Missing) => census.missing += 1,
        Ok(CacheLookup::Expired) => census
            .torn
            .push("entry read as expired despite a live TTL".to_owned()),
        Err(error) => census.torn.push(format!("read failed: {error}")),
    }
}

/// Read `path` repeatedly until the writers finish or `deadline` passes.
///
/// Every observation is classified rather than asserted on, so the caller —
/// which is the test — owns the assertion.
fn observe_while_writing(path: &Utf8Path, finished: &AtomicUsize, deadline: Instant) -> ReadCensus {
    let clock = DefaultClock;
    let expected: Vec<String> = (0..WRITERS).map(writer_value).collect();
    let mut census = ReadCensus::default();
    while finished.load(Ordering::Acquire) < WRITERS && Instant::now() < deadline {
        classify(
            load_cached_value(path, &clock, LIVE_TTL),
            &expected,
            &mut census,
        );
    }
    census
}

#[rstest]
fn concurrent_reads_never_observe_a_partial_entry(contended_path: CachePath) {
    let (_temp_dir, path) = contended_path.expect("cache path");
    let finished = Arc::new(AtomicUsize::new(0));
    let write_deadline = Instant::now() + WRITE_WINDOW;

    let mut handles = Vec::new();
    for id in 0..WRITERS {
        let writer_path = path.clone();
        let writer_finished = Arc::clone(&finished);
        handles.push(std::thread::spawn(move || {
            let clock = DefaultClock;
            let value = writer_value(id);
            let mut writes = 0_usize;
            while Instant::now() < write_deadline {
                match store_cached_value(&writer_path, &clock, value.as_str()) {
                    Ok(()) => writes += 1,
                    Err(error) => {
                        writer_finished.fetch_add(1, Ordering::Release);
                        return Err(error);
                    }
                }
            }
            writer_finished.fetch_add(1, Ordering::Release);
            Ok(writes)
        }));
    }

    let census = observe_while_writing(&path, &finished, Instant::now() + READ_TIMEOUT);

    let mut writes = 0_usize;
    for handle in handles {
        writes += handle.join().expect("writer thread").expect("store cache");
    }

    assert!(
        census.torn.is_empty(),
        "reads taken during {writes} concurrent writes must never see a partial entry, \
         but {} of {} observations were torn: {:?}",
        census.torn.len(),
        census.fresh + census.missing + census.torn.len(),
        census.torn
    );
    assert!(
        census.fresh > 0,
        "the reader observed no complete entry during {writes} writes \
         ({} absences), so it proved nothing",
        census.missing
    );
}
