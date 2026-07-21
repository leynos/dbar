//! Git probing utilities for dbar.

use camino::Utf8Path;

use crate::command::{CommandRunner, CommandSpec};
use crate::types::{AheadCount, BehindCount, BranchName, ProjectName};

#[derive(Debug, Clone)]
/// Snapshot of git status metadata for rendering.
pub struct GitStatus {
    /// The current branch name.
    pub branch: BranchName,
    /// Whether the worktree has unstaged changes.
    pub dirty: bool,
    /// Whether the index contains staged changes.
    pub staged: bool,
    /// Ahead count relative to upstream.
    pub ahead: AheadCount,
    /// Behind count relative to upstream.
    pub behind: BehindCount,
    /// Whether this path looks like a worktree.
    pub is_worktree: bool,
}

/// Resolve the project name using git metadata and directory heuristics.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::command::RealCommandRunner;
/// use dbar::git::project_name;
///
/// let runner = RealCommandRunner::default();
/// let name = project_name(&runner, Utf8Path::new("."));
/// println!("{name}");
/// ```
pub fn project_name(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> ProjectName {
    let origin = CommandSpec::new("git")
        .args(["remote", "get-url", "origin"])
        .cwd(project_dir.to_path_buf());
    if let Ok(output) = runner.run(&origin)
        && let Some(name) = parse_origin_name(&output.stdout)
    {
        return name;
    }

    if let Some(name) = name_from_worktree_path(project_dir) {
        return name;
    }

    ProjectName::new(project_dir.file_name().unwrap_or_default())
}

/// Load git status information for the given project directory.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::command::RealCommandRunner;
/// use dbar::git::git_status;
///
/// let runner = RealCommandRunner::default();
/// let status = git_status(&runner, Utf8Path::new("."));
/// ```
pub fn git_status(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> Option<GitStatus> {
    if !is_git_repo(runner, project_dir) {
        return None;
    }

    let branch = git_branch(runner, project_dir);
    let (dirty, staged) = git_worktree_status(runner, project_dir);
    let (ahead, behind) = upstream_counts(runner, project_dir);
    let is_worktree = is_worktree_path(project_dir);

    Some(GitStatus {
        branch,
        dirty,
        staged,
        ahead,
        behind,
        is_worktree,
    })
}

fn parse_origin_name(origin: &str) -> Option<ProjectName> {
    let trimmed = origin.trim();
    let name = trimmed.rsplit(&['/', ':'][..]).next()?;
    let cleaned = name.trim_end_matches(".git");
    if cleaned.is_empty() {
        None
    } else {
        Some(ProjectName::new(cleaned.to_owned()))
    }
}

fn name_from_worktree_path(path: &Utf8Path) -> Option<ProjectName> {
    let value = path.as_str();
    let marker = ".worktrees";
    let (before, _) = value.split_once(marker)?;
    let name = before.rsplit('/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(ProjectName::new(name.to_owned()))
    }
}

fn is_worktree_path(path: &Utf8Path) -> bool {
    let value = path.as_str();
    value.contains(".worktrees") || value.contains("/.git/worktrees/")
}

fn is_git_repo(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> bool {
    let spec = CommandSpec::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .cwd(project_dir.to_path_buf());
    match runner.run(&spec) {
        Ok(output) => output.stdout.trim() == "true",
        Err(_) => false,
    }
}

fn git_branch(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> BranchName {
    let spec = CommandSpec::new("git")
        .args(["branch", "--show-current"])
        .cwd(project_dir.to_path_buf());
    match runner.run(&spec) {
        Ok(output) if !output.stdout.is_empty() => BranchName::new(output.stdout),
        _ => BranchName::new("detached"),
    }
}

fn git_worktree_status(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> (bool, bool) {
    let spec = CommandSpec::new("git")
        .args(["status", "--porcelain"])
        .cwd(project_dir.to_path_buf());
    let Ok(output) = runner.run(&spec) else {
        return (false, false);
    };

    let mut dirty = false;
    let mut staged = false;

    for line in output.stdout.lines() {
        let mut chars = line.chars();
        let index = chars.next().unwrap_or(' ');
        let worktree = chars.next().unwrap_or(' ');
        if matches!(worktree, 'M' | 'A' | 'D' | 'R' | 'C' | 'U' | '?') {
            dirty = true;
        }
        if matches!(index, 'M' | 'A' | 'D' | 'R' | 'C') {
            staged = true;
        }
    }

    (dirty, staged)
}

fn upstream_counts(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
) -> (AheadCount, BehindCount) {
    let spec = CommandSpec::new("git")
        .args(["rev-list", "--left-right", "--count", "@{upstream}...HEAD"])
        .cwd(project_dir.to_path_buf());
    let Ok(output) = runner.run(&spec) else {
        return (AheadCount::new(0), BehindCount::new(0));
    };

    let mut parts = output.stdout.split_whitespace();
    let behind = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    let ahead = parts
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);

    (AheadCount::new(ahead), BehindCount::new(behind))
}

#[cfg(test)]
mod tests {
    //! Tests for git branch, dirty/staged/ahead/behind, and worktree probing.
    use super::*;
    use crate::command::{CommandError, CommandOutput, CommandRunner, CommandSpec};
    use camino::Utf8PathBuf;
    use rstest::rstest;
    use std::collections::HashMap;

    #[derive(Default)]
    struct StubRunner {
        outputs: HashMap<CommandSpec, CommandOutput>,
    }

    impl StubRunner {
        fn with_output(mut self, spec: CommandSpec, stdout: &str) -> Self {
            self.outputs.insert(
                spec,
                CommandOutput {
                    stdout: stdout.to_owned(),
                },
            );
            self
        }
    }

    impl CommandRunner for StubRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.outputs
                .get(spec)
                .cloned()
                .ok_or(CommandError::NonZero {
                    status: Some(1),
                    stderr: String::new(),
                })
        }
    }

    #[rstest]
    #[case("git@github.com:owner/dbar.git", "dbar")]
    #[case("https://github.com/owner/alpha", "alpha")]
    fn project_name_prefers_origin(#[case] origin: &str, #[case] expected: &str) {
        let runner = StubRunner::default().with_output(
            CommandSpec::new("git")
                .args(["remote", "get-url", "origin"])
                .cwd(Utf8PathBuf::from("/tmp/demo")),
            origin,
        );
        let name = project_name(&runner, Utf8Path::new("/tmp/demo"));
        assert_eq!(name.as_ref(), expected);
    }

    #[test]
    fn project_name_falls_back_to_worktree_path() {
        let runner = StubRunner::default();
        let name = project_name(&runner, Utf8Path::new("/tmp/repo.worktrees/feat"));
        assert_eq!(name.as_ref(), "repo");
    }

    #[test]
    fn git_status_parses_porcelain_and_counts() {
        let runner = StubRunner::default()
            .with_output(
                CommandSpec::new("git")
                    .args(["rev-parse", "--is-inside-work-tree"])
                    .cwd(Utf8PathBuf::from("/tmp/repo")),
                "true",
            )
            .with_output(
                CommandSpec::new("git")
                    .args(["branch", "--show-current"])
                    .cwd(Utf8PathBuf::from("/tmp/repo")),
                "main",
            )
            .with_output(
                CommandSpec::new("git")
                    .args(["status", "--porcelain"])
                    .cwd(Utf8PathBuf::from("/tmp/repo")),
                "MM file.txt\n",
            )
            .with_output(
                CommandSpec::new("git")
                    .args(["rev-list", "--left-right", "--count", "@{upstream}...HEAD"])
                    .cwd(Utf8PathBuf::from("/tmp/repo")),
                "1\t2",
            );
        let status = git_status(&runner, Utf8Path::new("/tmp/repo")).expect("status");
        assert!(status.dirty);
        assert!(status.staged);
        assert_eq!(status.ahead.value(), 2);
        assert_eq!(status.behind.value(), 1);
    }
}
