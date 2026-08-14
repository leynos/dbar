//! Property-based tests for [`pr_cache_path`].
//!
//! The example-based tests in the parent module pin specific inputs. These
//! properties generalise them over a bounded, deliberately hostile domain:
//! Unicode, path separators, framing punctuation, and empty-ish fields.
//!
//! # What is proved, and what is only made overwhelmingly likely
//!
//! **Stability** is a theorem: the digest is a pure function of its inputs, so
//! equal inputs must give equal paths. The property below simply exercises it
//! across the generated domain, including repeated calls within one run.
//!
//! **Distinctness is not a theorem.** The digest is 64 bits wide, so by
//! pigeonhole some pair of inputs must collide; a test asserting universal
//! distinctness would be asserting something false in general. The property is
//! therefore scoped to the generated domain, and the domain is kept small
//! enough that observing a collision would be evidence of a broken digest
//! rather than bad luck:
//!
//! - [`DISTINCTNESS_CASES`] cases each compare one pair of inputs, so a sound
//!   64-bit digest fails with probability at most `256 / 2^64`, about
//!   `1.4e-17`.
//! - The split-family property draws at most [`MAX_CONTENT_CHARS`] characters
//!   and compares two splits, so its exposure is the same order. Even reading
//!   every generated split of every case as one birthday problem — fewer than
//!   `256 * 11` values, so under `2^12` — the collision bound
//!   `k^2 / 2^65 < 2^24 / 2^65 = 2^-41` is still negligible.
//!
//! In short: at these case counts a failure is roughly `10^13` times more
//! likely to mean the framing or the digest regressed than to mean the test
//! got unlucky. That is what makes the property meaningful rather than merely
//! lucky.
//!
//! # Accepted collision model
//!
//! Should a real collision ever occur in production, two `(project, branch)`
//! pairs share one cache file, so one branch renders a stale or wrong PR
//! number until the entry's TTL expires and the next lookup rewrites it.
//! Nothing else is corrupted: the cache file is derived data, and the PR
//! number is cosmetic status-line decoration. This matches the collision model
//! documented on the parent module.

use camino::{Utf8Path, Utf8PathBuf};
use proptest::prelude::*;

use super::pr_cache_path;

/// Cases run for each distinctness property.
///
/// See the module documentation for the collision arithmetic this number
/// feeds.
const DISTINCTNESS_CASES: u32 = 256;

/// Upper bound on the characters drawn for one generated field.
const MAX_CONTENT_CHARS: usize = 10;

/// Punctuation that shifts meaning when it moves between fields, including
/// the path and framing separators the digest must not confuse.
const FRAMING_CHARS: &[char] = &['/', '\\', ':', '-', '_', '.', '#', ' '];

/// Characters outside ASCII, so multi-byte fields exercise byte-length
/// framing rather than character-count framing.
const UNICODE_CHARS: &[char] = &['é', 'ß', '漢', '字', 'ы', '→', '\u{301}'];

/// A bounded generator of characters plausible in a branch name or path.
fn field_char() -> impl Strategy<Value = char> {
    prop_oneof![
        5 => proptest::char::range('a', 'z'),
        4 => proptest::sample::select(FRAMING_CHARS),
        2 => proptest::sample::select(UNICODE_CHARS),
        1 => proptest::char::range('0', '9'),
    ]
}

/// A bounded generator of one field, from empty up to
/// [`MAX_CONTENT_CHARS`] characters.
fn field() -> impl Strategy<Value = String> {
    proptest::collection::vec(field_char(), 0..=MAX_CONTENT_CHARS)
        .prop_map(|characters| characters.into_iter().collect())
}

/// The fixed cache directory; the digest never depends on it.
fn cache_dir() -> Utf8PathBuf {
    Utf8PathBuf::from("/cache")
}

/// Build the cache path for a `(project directory, branch)` pair.
fn path_for(project_dir: &str, branch: &str) -> Utf8PathBuf {
    pr_cache_path(cache_dir().as_path(), branch, Utf8Path::new(project_dir))
}

/// Split `content` after `count` characters, without slicing a `str`.
fn split_at_char(content: &[char], count: usize) -> (String, String) {
    let head = content.iter().take(count).collect();
    let tail = content.iter().skip(count).collect();
    (head, tail)
}

/// One string plus two split points, both generated in range.
///
/// The split points are drawn from the string's own length rather than
/// filtered afterwards, so no case is rejected for being out of range and
/// shrinking stays well behaved.
fn content_and_two_splits() -> impl Strategy<Value = (Vec<char>, usize, usize)> {
    proptest::collection::vec(field_char(), 1..=MAX_CONTENT_CHARS).prop_flat_map(|content| {
        let length = content.len();
        (Just(content), 0..=length, 0..=length)
    })
}

proptest! {
    // Bounded and deterministic for continuous integration; regression files
    // are disabled because the repository tracks none.
    #![proptest_config(ProptestConfig {
        cases: DISTINCTNESS_CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Equal inputs always yield equal paths, including on repeated calls and
    /// when the inputs are separately constructed rather than shared.
    #[test]
    fn identical_inputs_produce_an_equal_cache_path(
        project_dir in field(),
        branch in field(),
    ) {
        let first = path_for(&project_dir, &branch);
        let second = path_for(&project_dir, &branch);
        let rebuilt_dir = project_dir.clone();
        let rebuilt_branch = branch.clone();
        let third = path_for(&rebuilt_dir, &rebuilt_branch);
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(&first, &third);
    }

    /// Within the generated domain, differing inputs yield differing paths.
    ///
    /// This is the probabilistic property described in the module
    /// documentation: it is bounded to the generated domain because a 64-bit
    /// digest cannot promise universal distinctness.
    #[test]
    fn differing_inputs_produce_differing_cache_paths(
        left_dir in field(),
        left_branch in field(),
        right_dir in field(),
        right_branch in field(),
    ) {
        prop_assume!((&left_dir, &left_branch) != (&right_dir, &right_branch));
        let left = path_for(&left_dir, &left_branch);
        let right = path_for(&right_dir, &right_branch);
        prop_assert_ne!(
            &left,
            &right,
            "collision for ({:?}, {:?}) and ({:?}, {:?})",
            left_dir, left_branch, right_dir, right_branch
        );
    }

    /// Two different splits of the same string yield different paths.
    ///
    /// Every split of one string has the same concatenation, so this family is
    /// exactly what an unframed digest cannot separate: it is the generalised
    /// form of the `("ab", "c")` versus `("a", "bc")` example. A failure here
    /// indicts the length framing specifically, not the digest's mixing.
    #[test]
    fn distinct_splits_of_one_string_produce_distinct_cache_paths(
        (content, left_split, right_split) in content_and_two_splits(),
    ) {
        prop_assume!(left_split != right_split);
        let (left_dir, left_branch) = split_at_char(&content, left_split);
        let (right_dir, right_branch) = split_at_char(&content, right_split);
        let left = path_for(&left_dir, &left_branch);
        let right = path_for(&right_dir, &right_branch);
        prop_assert_ne!(
            &left,
            &right,
            "framing failed to separate splits {} and {} of {:?}",
            left_split, right_split, content
        );
    }
}
