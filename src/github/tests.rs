//! Tests for `gh` PR parsing, failure propagation, and the mock client.
use super::*;
use crate::command::{CommandError, CommandFailure, CommandOutput, MockCommandRunner};
use mockall::predicate::eq;
use rstest::rstest;

/// The project directory every lookup under test is rooted at.
const PROJECT_DIR: &str = "/projects/demo";

/// The spec `pr_number` is expected to build for `branch`.
fn expected_spec(project_dir: &Utf8Path, branch: &str) -> CommandSpec {
    // The working directory scopes `gh` to the right repository, `--head`
    // overrides the current checkout, `--json` suppresses the no-results
    // error so an absent PR is not a failure, and the timeout keeps a
    // stalled network call off the status path. Spelled out rather than
    // reusing `pr_list_args`, so a change to the real command has to be
    // restated here deliberately.
    CommandSpec::new("gh")
        .args([
            "pr".to_owned(),
            "list".to_owned(),
            format!("--head={branch}"),
            "--state".to_owned(),
            "open".to_owned(),
            "--limit".to_owned(),
            "1".to_owned(),
            "--json".to_owned(),
            "number".to_owned(),
            "--jq".to_owned(),
            r#".[0].number // """#.to_owned(),
        ])
        .cwd(project_dir.to_path_buf())
        .timeout(GH_TIMEOUT)
        .max_output_bytes(GH_MAX_OUTPUT_BYTES)
}

/// A runner that answers the one expected lookup with `stdout`.
fn runner_answering(stdout: &str) -> MockCommandRunner {
    let answer = stdout.to_owned();
    let mut runner = MockCommandRunner::new();
    runner
        .expect_run()
        .with(eq(expected_spec(Utf8Path::new(PROJECT_DIR), "main")))
        .times(1)
        .returning(move |_| Ok(CommandOutput::from_stdout(answer.clone())));
    runner
}

#[rstest]
#[case::plain("42", "42")]
#[case::trailing_newline("42\n", "42")]
#[case::surrounding_space("  7  ", "7")]
fn pr_number_parses_gh_output(#[case] stdout: &str, #[case] expected: &str) {
    let runner = runner_answering(stdout);
    let client = GhCliClient::new(&runner);
    let pr = client
        .pr_number(Utf8Path::new(PROJECT_DIR), "main")
        .expect("lookup succeeds");
    assert_eq!(pr.map(|value| value.to_string()).as_deref(), Some(expected));
}

#[rstest]
#[case::empty("")]
#[case::whitespace_only("   \n")]
fn pr_number_reports_no_pr_for_empty_output(#[case] stdout: &str) {
    let runner = runner_answering(stdout);
    let client = GhCliClient::new(&runner);
    let pr = client
        .pr_number(Utf8Path::new(PROJECT_DIR), "main")
        .expect("lookup succeeds");
    assert!(pr.is_none(), "empty gh output means no open PR");
}

/// A realistic I/O failure can name a credential-bearing remote URL.
const LEAKY_STDERR: &str = "fatal: could not read from remote repository \
     https://x-access-token:ghp_supersecret0123456789@github.com/acme/widgets.git";

/// Fragments that would betray a credential if any rendering exposed them.
const TOKEN_MARKERS: [&str; 6] = [
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "x-access-token",
    "@github.com",
];

/// Every string an unwary caller could get out of `error`.
///
/// The `source` chain is walked rather than probed at a fixed depth, so a
/// future variant that gains a source is covered without editing this helper.
fn every_rendering(error: &GitHubError) -> Vec<String> {
    let mut renderings = vec![error.to_string(), format!("{error:?}")];
    let mut next: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(current) = next {
        renderings.push(current.to_string());
        renderings.push(format!("{current:?}"));
        next = current.source();
    }
    renderings
}

/// The renderings of `error` that expose token-shaped data, if any.
fn token_shaped_renderings(error: &GitHubError) -> Vec<String> {
    every_rendering(error)
        .into_iter()
        .filter(|rendering| {
            TOKEN_MARKERS
                .iter()
                .any(|marker| rendering.contains(marker))
        })
        .collect()
}

#[rstest]
fn pr_number_reduces_a_command_failure_to_its_category() {
    let mut runner = MockCommandRunner::new();
    runner
        .expect_run()
        .with(eq(expected_spec(Utf8Path::new(PROJECT_DIR), "main")))
        .times(1)
        .returning(|_| Err(CommandError::Io(std::io::Error::other(LEAKY_STDERR))));
    let client = GhCliClient::new(&runner);
    let err = client
        .pr_number(Utf8Path::new(PROJECT_DIR), "main")
        .expect_err("command failure must propagate");
    // The failure still reaches the caller, but as a category and a status
    // rather than as the child's stderr.
    assert_eq!(
        err,
        GitHubError::Command(CommandFailure::NotRun(std::io::ErrorKind::Other))
    );
    let leaks = token_shaped_renderings(&err);
    assert!(
        leaks.is_empty(),
        "the propagated error still exposes token-shaped data: {leaks:?}"
    );
}

#[rstest]
#[case::plain_branch("feature/login")]
// A dash-prefixed name must reach `gh` as the value of `--head` rather than
// being parsed as a flag, which is what the `--head=<branch>` form
// guarantees.
#[case::dash_prefixed_branch("-weird-branch")]
fn pr_number_builds_the_expected_command_spec(#[case] branch: &str) {
    let project_dir = Utf8Path::new(PROJECT_DIR);
    // The expectation itself is the assertion: any other spec fails to
    // match and the mock panics, and `times(1)` fails the test at drop if
    // the lookup never ran.
    let mut runner = MockCommandRunner::new();
    runner
        .expect_run()
        .with(eq(expected_spec(project_dir, branch)))
        .times(1)
        .returning(|_| Ok(CommandOutput::from_stdout("1")));
    let client = GhCliClient::new(&runner);
    let _ = client.pr_number(project_dir, branch).expect("lookup");
}

#[rstest]
#[case::io(
    CommandError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no such file: {LEAKY_STDERR}"),
    )),
    CommandFailure::NotRun(std::io::ErrorKind::NotFound),
    "GitHub CLI command failed: the process could not be run (entity not found)",
)]
#[case::non_zero(
    CommandError::NonZero { status: Some(1) },
    CommandFailure::ExitStatus(1),
    "GitHub CLI command failed: exit status 1",
)]
#[case::signalled(
    CommandError::NonZero { status: None },
    CommandFailure::Signalled,
    "GitHub CLI command failed: terminated by a signal",
)]
#[case::timeout(
    CommandError::Timeout { timeout: GH_TIMEOUT },
    CommandFailure::TimedOut(GH_TIMEOUT),
    "GitHub CLI command failed: timed out after 5s",
)]
#[case::too_large(
    CommandError::OutputTooLarge { limit: 8, stream: "stdout" },
    CommandFailure::OutputTooLarge { limit: 8, stream: "stdout" },
    "GitHub CLI command failed: stdout exceeded the 8-byte output limit",
)]
fn command_failures_redact_to_a_category(
    #[case] source: CommandError,
    #[case] expected_failure: CommandFailure,
    #[case] expected_message: &str,
) {
    let failure = CommandFailure::from(&source);
    assert_eq!(failure, expected_failure);
    let err = GitHubError::Command(failure);
    assert_eq!(err.to_string(), expected_message);
    // `Display` is only one of the ways an error escapes: `{:?}`, a panicking
    // `unwrap`, and `fn main() -> Result<_, _>` all print `Debug`, and
    // diagnostic renderers walk `source`. None of them may reach the stderr.
    let leaks = token_shaped_renderings(&err);
    assert!(
        leaks.is_empty(),
        "a redacted failure still exposes token-shaped data: {leaks:?}"
    );
    assert!(
        std::error::Error::source(&err).is_none(),
        "a source chain would put the captured stderr back within reach"
    );
}

#[rstest]
#[case::number("42", Some("42"))]
#[case::padded_number("  42  ", Some("42"))]
#[case::empty("", None)]
#[case::whitespace("   ", None)]
#[case::none_lowercase("none", None)]
#[case::none_uppercase("NONE", None)]
#[case::none_mixed("NoNe", None)]
fn mock_client_maps_its_configured_value(#[case] configured: &str, #[case] expected: Option<&str>) {
    let client = MockGitHubClient::new(configured);
    let pr = client
        .pr_number(Utf8Path::new(PROJECT_DIR), "main")
        .expect("the mock never fails");
    assert_eq!(pr.map(|value| value.to_string()).as_deref(), expected);
}
