//! Behavioural tests for tmux snippet installation, idempotence, and layout.
use super::fs::{open_parent_for_read, split_parent};
use super::snippet::{MARKER_END, MARKER_START, quoted_format};
use super::*;
use camino::Utf8Path;
use camino::Utf8PathBuf;
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use rstest::{fixture, rstest};
use tempfile::TempDir;

/// A temporary directory and the `tmux.conf` path within it.
type Workspace = Result<(TempDir, Utf8PathBuf), InstallError>;

/// Create a temporary directory and the `tmux.conf` path within it.
#[fixture]
fn workspace() -> Workspace {
    let temp_dir = TempDir::new().map_err(InstallError::Io)?;
    let path = Utf8PathBuf::from_path_buf(temp_dir.path().join("tmux.conf"))
        .map_err(|_| InstallError::MissingFileName)?;
    Ok((temp_dir, path))
}

#[rstest]
fn install_writes_snippet(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let initial = "set -g status on\n";
    write(&path, initial).expect("write config");

    let outcome = install(
        Some(path.clone()),
        StatusPosition::Right,
        RunMode::Write,
        Width::Plain,
    )
    .expect("install snippet");
    assert!(outcome.updated);
    assert!(outcome.backup_path.is_some());

    let contents = read_to_string(&path).expect("read config");
    assert!(contents.contains(MARKER_START));
    assert!(contents.contains(MARKER_END));
}

#[rstest]
fn install_is_idempotent(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let _ = install(
        Some(path.clone()),
        StatusPosition::Right,
        RunMode::Write,
        Width::Plain,
    )
    .expect("install snippet");
    let second = install(
        Some(path.clone()),
        StatusPosition::Right,
        RunMode::Write,
        Width::Plain,
    )
    .expect("install snippet");
    assert!(!second.updated);
}

/// Each request must yield its own snippet variant.
///
/// `install` returns `build_snippet`'s output verbatim, so the variant matrix is
/// asserted against `build_snippet` directly. The end-to-end install path keeps
/// its own coverage: `install_writes_snippet` and `install_is_idempotent` drive
/// it here, the property suite drives it across both positions and both widths,
/// and `tests/e2e/install_cli.rs` drives `--full --position right` through the
/// binary.
#[rstest]
#[case::full_left(StatusPosition::Left, Width::Full, &["status-left-length 999"], &["--show-clock true"])]
#[case::right_plain(StatusPosition::Right, Width::Plain, &["--show-clock true", "status-right "], &["-length 999"])]
#[case::left_plain(StatusPosition::Left, Width::Plain, &["status-left "], &["--show-clock true", "-length 999"])]
fn snippet_variants_match_the_request(
    #[case] position: StatusPosition,
    #[case] width: Width,
    #[case] expected: &[&str],
    #[case] rejected: &[&str],
) {
    let snippet = build_snippet(position, width);
    // The width slot appears exactly when the full variant was requested.
    let width_slot = format!("--client-width {}", quoted_format("client_width"));
    assert_eq!(
        snippet.contains(&width_slot),
        width.is_full(),
        "`--client-width` must appear only for the full variant: {snippet}"
    );
    for token in expected {
        assert!(
            snippet.contains(token),
            "snippet missing {token}: {snippet}"
        );
    }
    for token in rejected {
        assert!(
            !snippet.contains(token),
            "snippet must not contain {token}: {snippet}"
        );
    }
}

#[rstest]
fn install_dry_run_leaves_missing_parent_absent(workspace: Workspace) {
    let (temp_dir, _) = workspace.expect("workspace");
    let missing_parent =
        Utf8PathBuf::from_path_buf(temp_dir.path().join("missing")).expect("missing parent path");
    let config = missing_parent.join("tmux.conf");
    let outcome = install(
        Some(config),
        StatusPosition::Left,
        RunMode::DryRun,
        Width::Plain,
    )
    .expect("dry run install");
    assert!(outcome.dry_run);
    // The parent directory must not have been created by the dry run.
    assert!(Dir::open_ambient_dir(missing_parent.as_path(), ambient_authority()).is_err());
}

#[rstest]
#[cfg(unix)]
fn install_preserves_restrictive_permissions(workspace: Workspace) {
    use cap_std::fs_utf8::{Permissions, PermissionsExt as _};

    let (_temp_dir, path) = workspace.expect("workspace");
    write(&path, "set -g status on\n").expect("write config");

    let (dir, file_name) = open_parent_for_read(&path).expect("open parent");
    dir.set_permissions(file_name, Permissions::from_mode(0o600))
        .expect("restrict config to 0600");

    let outcome = install(
        Some(path.clone()),
        StatusPosition::Right,
        RunMode::Write,
        Width::Plain,
    )
    .expect("install snippet");
    assert!(outcome.updated);

    let mode = dir
        .metadata(file_name)
        .expect("stat config")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "install must not widen existing permissions");

    // The backup holds the same restricted content, so it must not be written
    // with the default umask permissions.
    let backup = outcome.backup_path.expect("backup written");
    let (backup_dir, backup_name) = open_parent_for_read(&backup).expect("open backup parent");
    let backup_mode = backup_dir
        .metadata(backup_name)
        .expect("stat backup")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        backup_mode, 0o600,
        "the backup must inherit the config's permissions"
    );
}

/// Two simultaneous installs must serialize, leaving one well-formed block.
///
/// Both writers target the same `# dbar: begin`..`# dbar: end` block, so
/// "both updates survive" is impossible by design: the install that takes the
/// lock second legitimately supersedes the first. What the lock buys is
/// serializability and integrity — exactly one marker pair, unrelated content
/// intact, and a file that is a single valid block rather than an interleaved
/// mixture of two runs.
#[rstest]
fn concurrent_installs_leave_one_well_formed_block(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let unrelated = "# unrelated\nset -g mouse on\n";
    write(&path, unrelated).expect("seed config");

    let left_path = path.clone();
    let right_path = path.clone();
    let left = std::thread::spawn(move || {
        install(
            Some(left_path),
            StatusPosition::Left,
            RunMode::Write,
            Width::Plain,
        )
    });
    let right = std::thread::spawn(move || {
        install(
            Some(right_path),
            StatusPosition::Right,
            RunMode::Write,
            Width::Plain,
        )
    });
    left.join()
        .expect("left install thread")
        .expect("left install succeeds");
    right
        .join()
        .expect("right install thread")
        .expect("right install succeeds");

    let contents = read_to_string(&path).expect("read config");
    assert_eq!(
        contents.matches(MARKER_START).count(),
        1,
        "exactly one start marker: {contents}"
    );
    assert_eq!(
        contents.matches(MARKER_END).count(),
        1,
        "exactly one end marker: {contents}"
    );
    assert!(
        contents.starts_with(unrelated),
        "unrelated content must survive byte for byte: {contents}"
    );

    // Whichever install won, the file must already hold its snippet verbatim:
    // a re-run reports no update only if the block is well formed.
    let (winner, loser) = if contents.contains("set -g status-right ") {
        (StatusPosition::Right, StatusPosition::Left)
    } else {
        (StatusPosition::Left, StatusPosition::Right)
    };
    assert_eq!(
        contents,
        format!("{unrelated}{}", build_snippet(winner, Width::Plain)),
        "the file must equal a serial execution's result"
    );

    // The backup is the sharpest witness that the two runs were serialized:
    // under the lock the loser completes first, so the winner's backup captures
    // the loser's output. Two unsynchronized runs would both back up the seed
    // instead, losing the intermediate state the backup is meant to preserve.
    let backup = backup_path_for(&path);
    let backed_up = read_to_string(&backup).expect("read backup");
    assert_eq!(
        backed_up,
        format!("{unrelated}{}", build_snippet(loser, Width::Plain)),
        "the backup must hold the config as it stood immediately before the winning install"
    );

    let repeat =
        install(Some(path), winner, RunMode::Write, Width::Plain).expect("re-install the winner");
    assert!(
        !repeat.updated,
        "the winning snippet must already be installed verbatim: {contents}"
    );
}

#[rstest]
fn install_without_path_reports_missing_path() {
    let err = install(None, StatusPosition::Left, RunMode::DryRun, Width::Plain)
        .expect_err("no path supplied");
    assert!(matches!(err, InstallError::MissingPath));
}

#[rstest]
fn install_reports_incomplete_markers(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    // A start marker with no matching end marker must not be rewritten.
    write(&path, &format!("{MARKER_START}\nset -g status-left ''\n")).expect("seed config");
    let err = install(
        Some(path),
        StatusPosition::Left,
        RunMode::DryRun,
        Width::Plain,
    )
    .expect_err("dangling start marker");
    assert!(matches!(err, InstallError::IncompleteMarkers));
}

#[rstest]
fn install_reports_duplicate_marker_blocks(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    // Two complete blocks: rewriting around the first would silently strand the
    // second, which would keep fighting over the same option.
    let block = build_snippet(StatusPosition::Left, Width::Plain);
    write(&path, &format!("{block}set -g mouse on\n{block}")).expect("seed config");
    let err = install(
        Some(path),
        StatusPosition::Right,
        RunMode::DryRun,
        Width::Plain,
    )
    .expect_err("duplicated managed block");
    assert!(matches!(err, InstallError::DuplicateMarkers), "got {err:?}");
}

/// A duplicate must be refused even when the *first* block already matches.
///
/// This is the case the old "split on the first marker" logic got wrong in the
/// most dangerous way: it compared the first block to the snippet, found them
/// equal, and reported the config as up to date while a second block remained.
#[rstest]
fn install_reports_duplicates_even_when_the_first_block_matches(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let block = build_snippet(StatusPosition::Left, Width::Plain);
    write(&path, &format!("{block}{block}")).expect("seed config");
    let err = install(
        Some(path),
        StatusPosition::Left,
        RunMode::DryRun,
        Width::Plain,
    )
    .expect_err("duplicated managed block");
    assert!(matches!(err, InstallError::DuplicateMarkers), "got {err:?}");
}

/// A bare relative path has an empty parent, which is not an openable directory.
#[rstest]
fn split_parent_maps_a_bare_path_to_the_current_directory() {
    let (parent, file_name) = split_parent(Utf8Path::new("tmux.conf")).expect("split bare path");
    assert_eq!(parent, Utf8Path::new("."), "an empty parent is unopenable");
    assert_eq!(file_name, "tmux.conf");
}
