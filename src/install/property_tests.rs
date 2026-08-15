//! Property tests for the idempotence of tmux snippet installation.
//!
//! [`super::install`] must be safe to re-run: a second install over an
//! already-installed configuration has to be a no-op, and a *changed* request
//! has to replace the existing marker block rather than append a second one.
//! These properties are checked over a bounded domain of pre-existing
//! configurations — empty, whitespace-only, ordinary tmux directives, comments
//! and tmux format tokens, CRLF line endings, absent trailing newlines, and
//! files that already carry a marker block that either matches or differs from
//! the request.
//!
//! Each case installs into its own [`TempDir`], because `install` takes an
//! exclusive `flock` on a sibling of the config path; sharing a path across
//! cases would serialize them at best and deadlock the property at worst.

use super::snippet::{MARKER_END, MARKER_START};
use super::*;
use camino::Utf8Path;
use proptest::prelude::*;
use tempfile::TempDir;

/// A generated pre-existing configuration and the request to apply to it.
#[derive(Debug, Clone)]
struct Scenario {
    /// The bytes the config file holds before the first install.
    existing: String,
    /// Whether the config file exists at all; `false` exercises the
    /// `NotFound` path, which `install` treats as empty content.
    present: bool,
    /// The requested status line position.
    position: StatusPosition,
    /// Whether the request asks for the full-width variant.
    full: bool,
}

/// Ordinary lines a user's `tmux.conf` might hold, including comments and
/// tmux format tokens that must survive verbatim.
fn config_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("   ".to_owned()),
        Just("\t".to_owned()),
        Just("set -g status on".to_owned()),
        Just("set -g status-interval 5".to_owned()),
        Just("# a comment containing a # and a #{token}".to_owned()),
        Just("set -g status-left '#{host} #(date +%H:%M)'".to_owned()),
        Just("bind-key r source-file ~/.tmux.conf".to_owned()),
    ]
}

/// Either status position, with equal weight.
fn any_position() -> impl Strategy<Value = StatusPosition> {
    prop_oneof![Just(StatusPosition::Left), Just(StatusPosition::Right)]
}

/// An optional pre-existing marker block: absent, one of the blocks `install`
/// itself writes (so it may match or differ from the request), or a block from
/// an older release whose body no `build_snippet` call can produce.
fn marker_block() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        3 => Just(None),
        3 => (any_position(), any::<bool>())
            .prop_map(|(position, full)| Some(build_snippet(position, Width::from_full(full)))),
        1 => Just(Some(format!(
            "{MARKER_START}\nset -g status-left 'legacy'\n{MARKER_END}\n"
        ))),
    ]
}

/// The generated shape of a pre-existing configuration file.
#[derive(Debug, Clone)]
struct ConfigShape {
    /// The user's own lines, in order.
    lines: Vec<String>,
    /// The line ending separating them.
    eol: &'static str,
    /// An optional marker block to embed.
    block: Option<String>,
    /// How many lines precede the block, clamped to the line count.
    block_at: usize,
    /// Whether the file ends with a newline.
    trailing_newline: bool,
}

impl ConfigShape {
    /// Render the shape to the bytes the config file will hold.
    fn render(&self) -> String {
        let at = self.block_at.min(self.lines.len());
        let block = self.block.as_deref().unwrap_or_default();
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index == at {
                out.push_str(block);
            }
            out.push_str(line);
            out.push_str(self.eol);
        }
        if at >= self.lines.len() {
            out.push_str(block);
        }
        if self.trailing_newline {
            return out;
        }
        // Drop the terminating newline in place; copying the string only to
        // return the copy would clone the whole rendered config.
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

/// The bounded generator of pre-existing configuration shapes.
fn config_shape() -> impl Strategy<Value = ConfigShape> {
    (
        proptest::collection::vec(config_line(), 0..4),
        prop_oneof![Just("\n"), Just("\r\n")],
        marker_block(),
        0_usize..5,
        any::<bool>(),
    )
        .prop_map(
            |(lines, eol, block, block_at, trailing_newline)| ConfigShape {
                lines,
                eol,
                block,
                block_at,
                trailing_newline,
            },
        )
}

/// The bounded generator of pre-existing configurations and requests.
fn scenario() -> impl Strategy<Value = Scenario> {
    (config_shape(), any::<bool>(), any_position(), any::<bool>()).prop_map(
        |(shape, present, position, full)| Scenario {
            existing: shape.render(),
            present,
            position,
            full,
        },
    )
}

/// A temporary directory holding a `tmux.conf` seeded with `existing`.
///
/// The directory is returned alongside the path so the caller keeps it alive
/// for the duration of the case.
///
/// Setup failures are reported as `TestCaseError::Fail` rather than unwrapped:
/// a helper called from a `proptest!` body is not itself a test as far as the
/// lint suite is concerned, and a failure here means the case never ran, which
/// is worth distinguishing from a falsified property.
fn seeded_workspace(
    existing: &str,
    present: bool,
) -> Result<(TempDir, Utf8PathBuf), TestCaseError> {
    let temp_dir =
        TempDir::new().map_err(|error| TestCaseError::fail(format!("create temp dir: {error}")))?;
    let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
        .map_err(|path| TestCaseError::fail(format!("temp dir path is not utf-8: {path:?}")))?;
    if present {
        write(&path, existing)
            .map_err(|error| TestCaseError::fail(format!("seed config: {error}")))?;
    }
    Ok((temp_dir, path))
}

/// The content `install` actually starts from: an absent file is empty
/// regardless of what the scenario generated for it.
const fn baseline(existing: &str, present: bool) -> &str {
    if present { existing } else { "" }
}

/// Read the config back, treating an absent file as empty content.
fn read_back(path: &Utf8Path) -> String {
    read_to_string(path).unwrap_or_default()
}

/// Split a config around its marker block, yielding the text before the start
/// marker and the text after the end marker.
fn split_block(contents: &str) -> Option<(String, String)> {
    let (before, rest) = contents.split_once(MARKER_START)?;
    let (_, after) = rest.split_once(MARKER_END)?;
    Some((before.to_owned(), after.to_owned()))
}

/// Drop the single line break that terminates a marker block, so two configs
/// can be compared on the user's own content alone.
fn without_leading_break(text: &str) -> &str {
    text.strip_prefix('\n').unwrap_or(text)
}

/// Assert that exactly one marker pair is present.
fn one_marker_pair(contents: &str) -> Result<(), TestCaseError> {
    prop_assert_eq!(
        contents.matches(MARKER_START).count(),
        1,
        "start markers in: {:?}",
        contents
    );
    prop_assert_eq!(
        contents.matches(MARKER_END).count(),
        1,
        "end markers in: {:?}",
        contents
    );
    Ok(())
}

/// Assert that everything outside the marker block survived the install.
///
/// When the config already held a block the surrounding text must match
/// byte for byte, modulo the single newline that terminates the block. When it
/// did not, the whole prior file must appear ahead of the appended block, with
/// at most a newline added to separate them.
fn content_preserved(existing: &str, installed: &str) -> Result<(), TestCaseError> {
    let Some((before, after)) = split_block(installed) else {
        return Err(TestCaseError::fail(format!(
            "installed config has no marker block: {installed:?}"
        )));
    };
    if let Some((before_existing, after_existing)) = split_block(existing) {
        prop_assert_eq!(&before, &before_existing, "text before the block changed");
        prop_assert_eq!(
            without_leading_break(&after),
            without_leading_break(&after_existing),
            "text after the block changed"
        );
    } else {
        prop_assert_eq!(without_leading_break(&after), "", "text follows the block");
        let separated = format!("{existing}\n");
        prop_assert!(
            before == existing || before == separated,
            "prior content was not preserved: {:?} became {:?}",
            existing,
            before
        );
    }
    Ok(())
}

proptest! {
    // Bounded and deterministic for continuous integration; regression files
    // are disabled because the repository tracks none. Each case performs real
    // filesystem work, so the budget is smaller than the pure-render suites'.
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// A second install over the first's output changes nothing: the file is
    /// byte-identical, the outcome reports no update, exactly one marker pair
    /// survives, and the user's own content is untouched.
    #[test]
    fn install_is_idempotent_across_configurations(scenario in scenario()) {
        let Scenario { existing, present, position, full } = scenario;
        let (_temp_dir, path) = seeded_workspace(&existing, present)?;

        let first = install(Some(path.clone()), position, RunMode::Write, Width::from_full(full))
            .map_err(|err| TestCaseError::fail(format!("first install failed: {err}")))?;
        let after_first = read_back(&path);

        let second = install(Some(path.clone()), position, RunMode::Write, Width::from_full(full))
            .map_err(|err| TestCaseError::fail(format!("second install failed: {err}")))?;
        let after_second = read_back(&path);

        prop_assert_eq!(&after_first, &after_second, "second install rewrote the config");
        prop_assert!(!second.updated, "second install reported an update");
        prop_assert!(
            second.backup_path.is_none(),
            "second install wrote a backup despite changing nothing"
        );
        prop_assert_eq!(&first.snippet, &second.snippet, "snippet differed between runs");
        prop_assert!(after_first.contains(&first.snippet), "snippet missing from config");
        one_marker_pair(&after_first)?;
        content_preserved(baseline(&existing, present), &after_first)?;
    }

    /// Changing the request updates the existing block rather than appending a
    /// second one, converging on exactly the config a direct install of the
    /// second request would have produced.
    #[test]
    fn changed_requests_converge_on_the_latest_snippet(
        scenario in scenario(),
        next_position in any_position(),
        next_full in any::<bool>(),
    ) {
        let Scenario { existing, present, position, full } = scenario;
        let (_sequential_dir, sequential) = seeded_workspace(&existing, present)?;
        let (_direct_dir, direct) = seeded_workspace(&existing, present)?;

        install(Some(sequential.clone()), position, RunMode::Write, Width::from_full(full))
            .map_err(|err| TestCaseError::fail(format!("first install failed: {err}")))?;
        let second = install(Some(sequential.clone()), next_position, RunMode::Write, Width::from_full(next_full))
            .map_err(|err| TestCaseError::fail(format!("second install failed: {err}")))?;
        let direct_outcome = install(Some(direct.clone()), next_position, RunMode::Write, Width::from_full(next_full))
            .map_err(|err| TestCaseError::fail(format!("direct install failed: {err}")))?;

        let sequential_contents = read_back(&sequential);
        let direct_contents = read_back(&direct);

        prop_assert_eq!(
            &sequential_contents,
            &direct_contents,
            "installing {:?} then {:?} diverged from installing {:?} directly",
            (position, full),
            (next_position, next_full),
            (next_position, next_full)
        );
        prop_assert_eq!(
            second.updated,
            build_snippet(position, Width::from_full(full))
                != build_snippet(next_position, Width::from_full(next_full)),
            "update flag did not track whether the snippet changed"
        );
        prop_assert!(
            sequential_contents.contains(&direct_outcome.snippet),
            "converged config lacks the latest snippet"
        );
        one_marker_pair(&sequential_contents)?;
        content_preserved(baseline(&existing, present), &sequential_contents)?;
    }
}
