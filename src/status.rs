//! Status line assembly for dbar.

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
    let clock_label = render_clock(args, clock);

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

fn render_clock(args: &StatusArgs, clock: &dyn Clock) -> Option<String> {
    args.show_clock
        .unwrap_or(false)
        .then(|| clock.local().format(&args.clock_format).to_string())
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
        Err(_err) => pr_from_branch(context.branch),
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
    let key = format!(
        "pr_{}_{}",
        sanitize_key(project_dir.as_str()),
        sanitize_key(branch),
    );
    cache_dir.join(format!("{key}.json"))
}

fn sanitize_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}
