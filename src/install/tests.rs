//! Behavioural tests for tmux snippet installation, idempotence, and layout.
use super::*;
use camino::Utf8PathBuf;
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

    let outcome =
        install(Some(path.clone()), StatusPosition::Right, false, false).expect("install snippet");
    assert!(outcome.updated);
    assert!(outcome.backup_path.is_some());

    let contents = read_to_string(&path).expect("read config");
    assert!(contents.contains(MARKER_START));
    assert!(contents.contains(MARKER_END));
}

#[rstest]
fn install_is_idempotent(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let _ =
        install(Some(path.clone()), StatusPosition::Right, false, false).expect("install snippet");
    let second =
        install(Some(path.clone()), StatusPosition::Right, false, false).expect("install snippet");
    assert!(!second.updated);
}

#[rstest]
fn install_full_adds_client_width(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let outcome = install(Some(path), StatusPosition::Left, true, true).expect("install snippet");
    assert!(outcome.snippet.contains("--client-width #{q:client_width}"));
    assert!(outcome.snippet.contains("status-left-length 999"));
}

#[rstest]
fn install_right_enables_clock(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let outcome = install(Some(path), StatusPosition::Right, true, false).expect("install snippet");
    assert!(outcome.snippet.contains("--show-clock true"));
    assert!(outcome.snippet.contains("status-right"));
}

#[rstest]
fn install_left_omits_clock(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    let outcome = install(Some(path), StatusPosition::Left, true, false).expect("install snippet");
    assert!(!outcome.snippet.contains("--show-clock true"));
}

#[rstest]
fn install_dry_run_leaves_missing_parent_absent(workspace: Workspace) {
    let (temp_dir, _) = workspace.expect("workspace");
    let missing_parent =
        Utf8PathBuf::from_path_buf(temp_dir.path().join("missing")).expect("missing parent path");
    let config = missing_parent.join("tmux.conf");
    let outcome =
        install(Some(config), StatusPosition::Left, true, false).expect("dry run install");
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

    let outcome =
        install(Some(path.clone()), StatusPosition::Right, false, false).expect("install snippet");
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
    let left =
        std::thread::spawn(move || install(Some(left_path), StatusPosition::Left, false, false));
    let right =
        std::thread::spawn(move || install(Some(right_path), StatusPosition::Right, false, false));
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
        format!("{unrelated}{}", build_snippet(winner, false)),
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
        format!("{unrelated}{}", build_snippet(loser, false)),
        "the backup must hold the config as it stood immediately before the winning install"
    );

    let repeat = install(Some(path), winner, false, false).expect("re-install the winner");
    assert!(
        !repeat.updated,
        "the winning snippet must already be installed verbatim: {contents}"
    );
}

#[rstest]
fn install_without_path_reports_missing_path() {
    let err = install(None, StatusPosition::Left, true, false).expect_err("no path supplied");
    assert!(matches!(err, InstallError::MissingPath));
}

#[rstest]
fn install_reports_incomplete_markers(workspace: Workspace) {
    let (_temp_dir, path) = workspace.expect("workspace");
    // A start marker with no matching end marker must not be rewritten.
    write(&path, &format!("{MARKER_START}\nset -g status-left ''\n")).expect("seed config");
    let err =
        install(Some(path), StatusPosition::Left, true, false).expect_err("dangling start marker");
    assert!(matches!(err, InstallError::IncompleteMarkers));
}

#[rstest]
fn install_snippet_shell_quotes_tmux_formats() {
    let snippet = build_snippet(StatusPosition::Left, true);
    for token in [
        "#{q:pane_current_path}",
        "#{q:session_name}",
        "#{q:window_index}",
        "#{q:pane_id}",
        "#{q:socket_path}",
        "#{q:client_width}",
    ] {
        assert!(snippet.contains(token), "snippet missing {token}");
    }
    // The unquoted forms that permitted shell injection must be gone.
    assert!(!snippet.contains("\"#{pane_current_path}\""));
    assert!(!snippet.contains("\"#{client_width}\""));
}

/// The tmux formats interpolated into the `#(...)` command.
const FORMAT_NAMES: [&str; 6] = [
    "pane_current_path",
    "session_name",
    "window_index",
    "pane_id",
    "socket_path",
    "client_width",
];

/// Characters tmux's `q` modifier backslash-escapes (`format_quote_shell`).
const TMUX_Q_SPECIALS: &str = "|&;<>()$`\\\"'*?[# =%";

/// Model of tmux's `#{q:...}` modifier.
///
/// tmux backslash-escapes each special character rather than wrapping the
/// value in quotes, which is why the snippet must interpolate `#{q:...}`
/// *unquoted*. Verified against tmux next-3.4.
fn tmux_q_modifier(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() * 2);
    for ch in value.chars() {
        if TMUX_Q_SPECIALS.contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Extract the command tmux would run from inside `'#(...)'`.
fn command_in_snippet(snippet: &str) -> Option<&str> {
    let (_, rest) = snippet.split_once("'#(")?;
    let (command, _) = rest.split_once(")'")?;
    Some(command)
}

/// Split a command line the way a POSIX shell would.
///
/// Returns `None` if any shell metacharacter survives unescaped, which is
/// precisely the condition that would let a hostile value break out of its
/// argument and be interpreted as syntax.
fn split_shell_words(input: &str) -> Option<Vec<String>> {
    // Quoting is rejected outright, so a word is never legitimately empty:
    // a non-empty buffer is exactly "a word is in progress".
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => current.push(chars.next()?),
            ' ' | '\t' if current.is_empty() => {}
            ' ' | '\t' => words.push(std::mem::take(&mut current)),
            _ if TMUX_Q_SPECIALS.contains(ch) => return None,
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    Some(words)
}

#[rstest]
#[case::whitespace("/tmp/my project dir")]
#[case::single_quote("/tmp/it's mine")]
#[case::double_quote("/tmp/say \"hi\"")]
#[case::command_substitution("$(touch /tmp/dbar-pwned)")]
#[case::backticks("`touch /tmp/dbar-pwned`")]
#[case::separator_and_glob("x; rm -rf / & echo *")]
fn hostile_tmux_values_stay_single_literal_arguments(#[case] hostile: &str) {
    let snippet = build_snippet(StatusPosition::Left, true);
    let command = command_in_snippet(&snippet).expect("snippet embeds a #(...) command");

    // Every format slot must be interpolated bare: tmux escapes rather than
    // quotes, so wrapping a slot in quotes would break the contract.
    for name in FORMAT_NAMES {
        let slot = format!("#{{q:{name}}}");
        assert!(command.contains(&slot), "command missing {slot}");
        assert!(
            !command.contains(&format!("\"{slot}\"")),
            "{slot} is quoted"
        );
        assert!(!command.contains(&format!("'{slot}'")), "{slot} is quoted");
    }

    // Expand every slot with the hostile value exactly as tmux would.
    let mut expanded = command.to_owned();
    for name in FORMAT_NAMES {
        expanded = expanded.replace(&format!("#{{q:{name}}}"), &tmux_q_modifier(hostile));
    }
    assert!(!expanded.contains("#{"), "every format must be substituted");

    let Some(words) = split_shell_words(&expanded) else {
        panic!("hostile value escaped its argument as shell syntax: {hostile}");
    };

    // The value survives verbatim, once per slot, as its own single word.
    let occurrences = words.iter().filter(|word| *word == hostile).count();
    assert_eq!(
        occurrences,
        FORMAT_NAMES.len(),
        "each slot must yield one literal argument, got words: {words:?}"
    );
    // The command and its flags are still separate, unmangled arguments.
    assert_eq!(words.first().map(String::as_str), Some("dbar"));
    assert!(words.iter().any(|word| word == "--project-dir"));
}
