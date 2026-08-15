//! Deterministic cache-file naming for PR lookups.
//!
//! The cache file for a `(project directory, branch)` pair is named after a
//! digest of that pair, so the name must be stable for as long as cache
//! entries are expected to be readable — across process invocations, across
//! dbar releases, and across Rust releases.
//!
//! # Why not `DefaultHasher`
//!
//! `std::collections::hash_map::DefaultHasher` is explicitly documented as
//! *not* guaranteed to be stable: its algorithm and seeding may change in any
//! Rust release. A digest that changes under the reader's feet silently
//! orphans every existing cache entry. This module therefore implements the
//! hash itself.
//!
//! # What this module guarantees
//!
//! - The algorithm is **FNV-1a, 64-bit** ([`FNV_OFFSET_BASIS`],
//!   [`FNV_PRIME`]), implemented here and owned by this crate, so it cannot
//!   drift with the toolchain.
//! - Each input field is **framed explicitly** before it is absorbed: the
//!   field's byte length is written in ASCII decimal, then a `:` separator,
//!   then the field's bytes. This is netstring-style framing, so it is
//!   self-delimiting and `("ab", "c")` cannot hash to the same value as
//!   `("a", "bc")`. Nothing here relies on `str`'s `Hash` implementation,
//!   which does *not* length-prefix its input in general.
//!
//! # Collision model
//!
//! A fixed-width hash cannot guarantee zero collisions, and 64 bits is no
//! exception. The accepted consequence is bounded and deliberate: two
//! colliding `(project, branch)` pairs share one cache file, so one of them
//! renders a stale or wrong PR number until the entry's TTL expires and the
//! next lookup rewrites it. The PR number is cosmetic status-line decoration
//! with a short TTL, so this is preferred to the cost and path-length risk of
//! naming files after the raw inputs.

use camino::{Utf8Path, Utf8PathBuf};

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Absorb `bytes` into an FNV-1a digest.
fn absorb(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |digest, byte| {
        (digest ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Absorb one length-framed field into an FNV-1a digest.
///
/// The frame is the field's byte length in ASCII decimal, a `:`, then the
/// field itself, so field boundaries cannot be forged by moving characters
/// from one field to the next.
fn absorb_field(hash: u64, field: &str) -> u64 {
    let frame = format!("{}:", field.len());
    let framed = absorb(hash, frame.as_bytes());
    absorb(framed, field.as_bytes())
}

/// Digest a `(project directory, branch)` pair.
fn digest(project_dir: &str, branch: &str) -> u64 {
    [project_dir, branch]
        .into_iter()
        .fold(FNV_OFFSET_BASIS, absorb_field)
}

/// Build the cache-file path for one project directory and branch.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::status::cache_key::pr_cache_path;
///
/// let path = pr_cache_path(
///     Utf8Path::new("/cache"),
///     Utf8Path::new("/projects/demo"),
///     "main",
/// );
/// assert!(path.as_str().ends_with(".json"));
/// ```
///
/// The project directory precedes the branch, matching the order in which the
/// two fields are absorbed into the digest, so the signature cannot invite a
/// transposition that would silently change the key.
pub fn pr_cache_path(cache_dir: &Utf8Path, project_dir: &Utf8Path, branch: &str) -> Utf8PathBuf {
    let value = digest(project_dir.as_str(), branch);
    cache_dir.join(format!("pr_{value:016x}.json"))
}

// Sibling file rather than a `cache_key/` subdirectory, so the path is given
// explicitly; it resolves relative to this file's directory.
#[cfg(test)]
#[path = "cache_key_properties.rs"]
mod cache_key_properties;

#[cfg(test)]
mod tests {
    //! Tests for cache-key stability, field framing, and the documented
    //! FNV-1a digest values.
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::adjacent_punctuation("/projects/demo", "feature/a", "/projects/demo", "feature-a")]
    #[case::shifted_boundary("/projects/a_b", "c", "/projects/a", "b_c")]
    #[case::framing_ab_c("/ab", "c", "/a", "bc")]
    #[case::distinct_projects("/projects/one", "main", "/projects/two", "main")]
    fn distinct_inputs_produce_distinct_cache_paths(
        #[case] left_dir: &str,
        #[case] left_branch: &str,
        #[case] right_dir: &str,
        #[case] right_branch: &str,
    ) {
        let cache_dir = Utf8PathBuf::from("/cache");
        let left = pr_cache_path(&cache_dir, &Utf8PathBuf::from(left_dir), left_branch);
        let right = pr_cache_path(&cache_dir, &Utf8PathBuf::from(right_dir), right_branch);
        assert_ne!(left, right);
    }

    #[rstest]
    fn identical_inputs_produce_a_stable_cache_path() {
        let cache_dir = Utf8PathBuf::from("/cache");
        let project_dir = Utf8PathBuf::from("/projects/demo");
        let first = pr_cache_path(&cache_dir, &project_dir, "main");
        let second = pr_cache_path(&cache_dir, &project_dir, "main");
        assert_eq!(first, second);
    }

    #[rstest]
    fn digest_matches_the_documented_algorithm() {
        // FNV-1a of the framed input `3:/ab1:c`, computed independently of the
        // implementation below. Pinning the value here is what makes an
        // accidental change of algorithm — the very failure `DefaultHasher`
        // would have introduced — a test failure rather than a silent cache
        // wipe.
        let expected = "3:/ab1:c"
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        assert_eq!(digest("/ab", "c"), expected);
    }

    #[rstest]
    fn absorb_matches_the_published_fnv_1a_64_vector() {
        // The published FNV-1a 64-bit test vector for the single byte `a`,
        // taken from the FNV reference test suite rather than from this
        // implementation. `digest_matches_the_documented_algorithm` re-derives
        // the framed digest with the same arithmetic as the code, so it would
        // survive a change to the offset basis or the prime; this one would
        // not. Together they pin both the mixing function and the framing.
        assert_eq!(absorb(FNV_OFFSET_BASIS, b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[rstest]
    fn cache_path_is_named_after_the_digest() {
        let cache_dir = Utf8PathBuf::from("/cache");
        let path = pr_cache_path(&cache_dir, &Utf8PathBuf::from("/ab"), "c");
        let expected = format!("/cache/pr_{:016x}.json", digest("/ab", "c"));
        assert_eq!(path.as_str(), expected);
    }
}
