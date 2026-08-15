//! Shell-quoting coverage for the generated tmux status command.
//!
//! These cases model the shell's own word splitting so a hostile
//! `pane_current_path` or session name can be shown to arrive as exactly one
//! literal argument. The newline cases pin a real, live-verified injection:
//! tmux's `q:` modifier escapes shell metacharacters but not control
//! characters, so an unguarded snippet lets a directory name containing a
//! newline split the status command and run the second half.

use super::Width;
use super::snippet::{build_snippet, quoted_format};
use crate::types::StatusPosition;
use rstest::rstest;

#[rstest]
fn install_snippet_shell_quotes_tmux_formats() {
    let snippet = build_snippet(StatusPosition::Left, Width::Full);
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

    // Every slot must also be wrapped in the control-character strip, spelled
    // out here rather than built from the production helper so a change to the
    // guard has to be made deliberately in both places.
    for name in FORMAT_NAMES {
        assert!(
            snippet.contains(&format!("#{{s/[\u{1}-\u{1f}\u{7f}]/_/:#{{q:{name}}}}}")),
            "{name} is not stripped of control characters"
        );
    }
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

/// Model of the control-character strip the snippet wraps every slot in.
///
/// tmux's `q` modifier does not escape control characters, so the snippet
/// substitutes them away *after* quoting. This mirrors that substitution.
fn strip_control_characters(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '_' } else { ch })
        .collect()
}

/// Split a command line the way a POSIX shell would.
///
/// Returns `None` if any shell metacharacter survives unescaped, which is
/// precisely the condition that would let a hostile value break out of its
/// argument and be interpreted as syntax. An unescaped newline is exactly such
/// a metacharacter — it terminates the command outright — and a carriage return
/// is rejected with it, because neither is escaped by tmux's `q` modifier and
/// so neither may be allowed to reach the shell.
fn split_shell_words(input: &str) -> Option<Vec<String>> {
    // Quoting is rejected outright, so a word is never legitimately empty:
    // a non-empty buffer is exactly "a word is in progress".
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => current.push(chars.next()?),
            '\n' | '\r' => return None,
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
#[case::newline("/tmp/evil\n/tmp/dbar-pwned")]
#[case::carriage_return("/tmp/evil\r/tmp/dbar-pwned")]
fn hostile_tmux_values_stay_single_literal_arguments(#[case] hostile: &str) {
    let snippet = build_snippet(StatusPosition::Left, Width::Full);
    let command = command_in_snippet(&snippet).expect("snippet embeds a #(...) command");

    // Every format slot must be interpolated bare: tmux escapes rather than
    // quotes, so wrapping a slot in quotes would break the contract.
    for name in FORMAT_NAMES {
        let slot = quoted_format(name);
        assert!(command.contains(&slot), "command missing {slot}");
        assert!(
            !command.contains(&format!("\"{slot}\"")),
            "{slot} is quoted"
        );
        assert!(!command.contains(&format!("'{slot}'")), "{slot} is quoted");
    }

    // Expand every slot with the hostile value exactly as tmux would: the `q`
    // modifier escapes the shell metacharacters, then the surrounding `s`
    // substitution replaces the control characters `q` left alone.
    let expected = strip_control_characters(hostile);
    let mut expanded = command.to_owned();
    for name in FORMAT_NAMES {
        expanded = expanded.replace(
            &quoted_format(name),
            &strip_control_characters(&tmux_q_modifier(hostile)),
        );
    }
    assert!(!expanded.contains("#{"), "every format must be substituted");

    let Some(words) = split_shell_words(&expanded) else {
        panic!("hostile value escaped its argument as shell syntax: {hostile}");
    };

    // The value survives once per slot as its own single word — verbatim unless
    // it carried control characters, which the guard replaces.
    let occurrences = words.iter().filter(|word| **word == expected).count();
    assert_eq!(
        occurrences,
        FORMAT_NAMES.len(),
        "each slot must yield one literal argument, got words: {words:?}"
    );
    // The command and its flags are still separate, unmangled arguments.
    assert_eq!(words.first().map(String::as_str), Some("dbar"));
    assert!(words.iter().any(|word| word == "--project-dir"));
}
