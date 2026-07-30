//! Status line assembly for dbar.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::io::{self, ErrorKind};

use camino::Utf8PathBuf;
use mockable::Clock;

use crate::cache;
use crate::command::CommandRunner;
use crate::config::StatusArgs;
use crate::error::DbarError;
use crate::git;
use crate::github::GitHubClient;
use crate::render;
use crate::tmux::{self, TmuxContext};
use crate::types::PrNumber;

/// Build a full tmux status line for the provided arguments.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::command::RealCommandRunner;
/// use dbar::config::StatusArgs;
/// use dbar::github::GhCliClient;
/// use dbar::status::build_status_line;
/// use mockable::DefaultClock;
///
/// let args = StatusArgs::default();
/// let runner = RealCommandRunner::default();
/// let github = GhCliClient::new(&runner);
/// let clock = DefaultClock;
/// let line = build_status_line(&args, &runner, &clock, &github)?;
/// assert!(!line.is_empty());
/// # Ok::<(), dbar::DbarError>(())
/// ```
pub fn build_status_line(
    args: &StatusArgs,
    runner: &dyn CommandRunner,
    clock: &dyn Clock,
    github: &dyn GitHubClient,
) -> Result<String, DbarError> {
    let project_dir = resolve_project_dir(args)?;
    let project = git::project_name(runner, &project_dir);
    let git_status = git::git_status(runner, &project_dir);

    let show_pr = args.show_pr.unwrap_or(true);
    let pr_number = if show_pr {
        git_status.as_ref().and_then(|status| {
            pr_number(&PrLookup {
                args,
                clock,
                github,
                project_dir: &project_dir,
                branch: status.branch.as_ref(),
            })
        })
    } else {
        None
    };

    let tmux_context = tmux::resolve_context(
        runner,
        TmuxContext {
            session: args.session.clone(),
            window: args.window.clone(),
            pane: args.pane.clone(),
            socket: args.socket.clone(),
        },
    );
    let clock_label = render_clock(args, clock)?;

    let render_context = render::RenderContext {
        project: &project,
        git_status: git_status.as_ref(),
        pr_number: pr_number.as_ref(),
        tmux: Some(&tmux_context),
        clock: clock_label.as_deref(),
        client_width: args.client_width.map(usize::from),
    };

    Ok(render::render_status_line(&render_context))
}

struct PrLookup<'a> {
    args: &'a StatusArgs,
    clock: &'a dyn Clock,
    github: &'a dyn GitHubClient,
    project_dir: &'a Utf8PathBuf,
    branch: &'a str,
}

fn resolve_project_dir(args: &StatusArgs) -> Result<Utf8PathBuf, DbarError> {
    if let Some(path) = args.project_dir.clone() {
        return Ok(path);
    }
    let current = std::env::current_dir()?;
    let path = Utf8PathBuf::from_path_buf(current)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "current directory is not UTF-8"))?;
    Ok(path)
}

fn render_clock(args: &StatusArgs, clock: &dyn Clock) -> Result<Option<String>, DbarError> {
    if !args.show_clock.unwrap_or(false) {
        return Ok(None);
    }
    // `DelayedFormat::fmt` returns an error for invalid strftime directives, so
    // render via `write!` and surface that as a typed error rather than letting
    // `ToString::to_string` panic on a user-supplied `clock_format`.
    let mut label = String::new();
    write!(label, "{}", clock.local().format(&args.clock_format))
        .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "invalid clock_format"))?;
    Ok(Some(label))
}

fn pr_number(context: &PrLookup<'_>) -> Option<PrNumber> {
    let cache_dir = cache::resolve_cache_dir(context.args.cache_dir.clone()).ok();
    let cache_path = cache_dir
        .as_ref()
        .map(|dir| pr_cache_path(dir, context.branch, context.project_dir));

    if let Some(path) = cache_path.as_ref()
        && let Ok(Some(value)) =
            cache::load_cached_value(path, context.clock, context.args.pr_cache_ttl_seconds)
    {
        if !value.is_empty() {
            return Some(PrNumber::new(value));
        }
        return None;
    }

    let pr = match context
        .github
        .pr_number(context.project_dir, context.branch)
    {
        Ok(Some(value)) => Some(value),
        Ok(None) => pr_from_branch(context.branch),
        // A failed lookup (network/rate-limit) must not poison the cache with a
        // fallback value for the whole TTL; return without writing the cache.
        Err(_err) => return pr_from_branch(context.branch),
    };

    if let Some(path) = cache_path.as_ref() {
        let cache_value = pr.as_ref().map(ToString::to_string).unwrap_or_default();
        if let Err(_err) = cache::store_cached_value(path, context.clock, cache_value) {}
    }

    pr
}

fn pr_from_branch(branch: &str) -> Option<PrNumber> {
    let trimmed = branch.trim();
    let stripped = trimmed
        .strip_prefix("pr/")
        .or_else(|| trimmed.strip_prefix("pr-"))
        .or_else(|| trimmed.strip_prefix("pull/"))
        .or_else(|| trimmed.strip_prefix("pull-"))?;
    if stripped.is_empty() || !stripped.chars().all(|ch| ch.is_ascii_digit()) {
        None
    } else {
        Some(PrNumber::new(stripped.to_owned()))
    }
}

fn pr_cache_path(cache_dir: &Utf8PathBuf, branch: &str, project_dir: &Utf8PathBuf) -> Utf8PathBuf {
    // Hash the raw (project, branch) pair so punctuation-only differences (for
    // example `feature/a` versus `feature-a`) never collapse to the same cache
    // file. `str`'s `Hash` impl length-prefixes each field, so the boundary
    // between the two inputs cannot be forged either. `DefaultHasher` is seeded
    // deterministically, so the filename is stable across CLI invocations.
    let mut hasher = DefaultHasher::new();
    (project_dir.as_str(), branch).hash(&mut hasher);
    let digest = hasher.finish();
    cache_dir.join(format!("pr_{digest:016x}.json"))
}

#[cfg(test)]
mod tests {
    //! Tests for PR lookup caching, cache-key uniqueness, and clock rendering.
    use super::*;
    use crate::command::CommandError;
    use crate::github::GitHubError;
    use crate::types::CacheTtlSeconds;
    use mockable::DefaultClock;
    use rstest::rstest;
    use tempfile::TempDir;

    /// A client whose lookup always fails, standing in for a network error.
    struct FailingGitHubClient;

    impl GitHubClient for FailingGitHubClient {
        fn pr_number(
            &self,
            _project_dir: &camino::Utf8Path,
            _branch: &str,
        ) -> Result<Option<PrNumber>, GitHubError> {
            Err(GitHubError::Command(CommandError::NonZero {
                status: Some(1),
                stderr: "gh failed".to_owned(),
            }))
        }
    }

    #[rstest]
    fn failed_lookup_does_not_write_a_cache_entry() {
        let temp_dir = TempDir::new().expect("temp dir");
        let cache_dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
            .expect("cache dir is not utf8");
        let project_dir = Utf8PathBuf::from("/projects/demo");
        let args = StatusArgs {
            cache_dir: Some(cache_dir.clone()),
            pr_cache_ttl_seconds: CacheTtlSeconds::new(60),
            ..StatusArgs::default()
        };
        let clock = DefaultClock;
        let github = FailingGitHubClient;

        // The branch fallback still applies, so a `pr/7` branch yields 7 ...
        let pr = pr_number(&PrLookup {
            args: &args,
            clock: &clock,
            github: &github,
            project_dir: &project_dir,
            branch: "pr/7",
        });
        assert_eq!(pr.map(|value| value.to_string()).as_deref(), Some("7"));

        // ... but a failed lookup must not be cached for the whole TTL.
        let cache_file = pr_cache_path(&cache_dir, "pr/7", &project_dir);
        assert!(!cache_file.as_std_path().exists());
    }

    #[rstest]
    #[case("/projects/demo", "feature/a", "/projects/demo", "feature-a")]
    #[case("/projects/a_b", "c", "/projects/a", "b_c")]
    #[case("/projects/one", "main", "/projects/two", "main")]
    fn distinct_inputs_produce_distinct_cache_paths(
        #[case] left_dir: &str,
        #[case] left_branch: &str,
        #[case] right_dir: &str,
        #[case] right_branch: &str,
    ) {
        let cache_dir = Utf8PathBuf::from("/cache");
        let left = pr_cache_path(&cache_dir, left_branch, &Utf8PathBuf::from(left_dir));
        let right = pr_cache_path(&cache_dir, right_branch, &Utf8PathBuf::from(right_dir));
        assert_ne!(left, right);
    }

    #[rstest]
    fn identical_inputs_produce_a_stable_cache_path() {
        let cache_dir = Utf8PathBuf::from("/cache");
        let project_dir = Utf8PathBuf::from("/projects/demo");
        let first = pr_cache_path(&cache_dir, "main", &project_dir);
        let second = pr_cache_path(&cache_dir, "main", &project_dir);
        assert_eq!(first, second);
    }

    #[rstest]
    #[case::dangling_percent("%")]
    #[case::unknown_directive("%Q")]
    fn invalid_clock_format_returns_an_error(#[case] clock_format: &str) {
        let args = StatusArgs {
            show_clock: Some(true),
            clock_format: clock_format.to_owned(),
            ..StatusArgs::default()
        };
        let clock = DefaultClock;
        // Must surface a typed error rather than panicking inside `to_string`.
        assert!(render_clock(&args, &clock).is_err());
    }

    #[rstest]
    fn valid_clock_format_renders() {
        let args = StatusArgs {
            show_clock: Some(true),
            clock_format: "%H:%M".to_owned(),
            ..StatusArgs::default()
        };
        let clock = DefaultClock;
        let label = render_clock(&args, &clock).expect("valid format renders");
        assert!(label.is_some_and(|value| value.contains(':')));
    }

    #[rstest]
    fn clock_is_absent_when_disabled() {
        let args = StatusArgs::default();
        let clock = DefaultClock;
        assert_eq!(render_clock(&args, &clock).expect("no clock"), None);
    }
}
