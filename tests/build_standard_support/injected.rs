//! The policy checks fed canned `make -n` text through the injected runner, so no
//! process runs. The real-`make` tests live in the contract file; these cover the
//! parsing and policy path, and run a synthetic held-out target so the check fires
//! in a repository that defines none.

use std::fmt::Write as _;

use super::{
    config::{Pin, THREADS_FLAG},
    make::{
        Host, development_problems, held_out_problems, held_out_problems_for, held_out_target_count,
    },
};

/// Renders canned `make -n` text for a fake runner through a fallible writer, so
/// the fakes keep the runner's `Result` shape honestly.
fn canned(text: std::fmt::Arguments) -> Result<String, String> {
    let mut out = String::new();
    out.write_fmt(text).map_err(|error| error.to_string())?;
    Ok(out)
}

/// A fake runner: a compliant `make -n` for any target, with no process behind it.
fn compliant_make(_target: &str, host: Host) -> Result<String, String> {
    let linker = if host.takes_linker_flag() {
        " -Clink-arg=-fuse-ld=mold"
    } else {
        ""
    };
    canned(format_args!(
        "RUSTFLAGS=\"${{RUSTFLAGS:+$RUSTFLAGS }}-D warnings {THREADS_FLAG}{linker}\" cargo test\n"
    ))
}

/// A fake runner whose command loses the caller's `RUSTFLAGS`.
fn dropping_make(_target: &str, _host: Host) -> Result<String, String> {
    canned(format_args!("RUSTFLAGS=\"-D warnings\" cargo test\n"))
}

/// A fake runner whose test command assigns nothing, so the warning policy never reaches it.
fn bare_test_make(_target: &str, _host: Host) -> Result<String, String> {
    canned(format_args!("cargo test\n"))
}

/// A fake runner whose lint commands assign nothing beside a command that does.
fn bare_lint_make(_target: &str, host: Host) -> Result<String, String> {
    let linker = if host.takes_linker_flag() {
        " -Clink-arg=-fuse-ld=mold"
    } else {
        ""
    };
    canned(format_args!(
        "RUSTFLAGS=\"${{RUSTFLAGS:+$RUSTFLAGS }}-D warnings {THREADS_FLAG}{linker}\" cargo test\ncargo clippy --all-targets\nwhitaker --all\n"
    ))
}

/// A fake runner with commands that run no compiled code under test, beside one that assigns.
fn exempt_commands_make(_target: &str, host: Host) -> Result<String, String> {
    let linker = if host.takes_linker_flag() {
        " -Clink-arg=-fuse-ld=mold"
    } else {
        ""
    };
    canned(format_args!(
        "RUSTFLAGS=\"${{RUSTFLAGS:+$RUSTFLAGS }}-D warnings {THREADS_FLAG}{linker}\" cargo test\ncargo fmt --all --check\ncargo metadata --format-version 1\nRUSTDOCFLAGS=\"-D warnings\" cargo doc\n"
    ))
}

/// A fake runner whose held-out target runs an inspection command beside an assigning build.
fn held_out_inspecting_make(_target: &str, _host: Host) -> Result<String, String> {
    canned(format_args!(
        "cargo metadata --format-version 1 --locked\nRUSTFLAGS=\"-D warnings\" cargo build --release\n"
    ))
}

/// A fake runner whose chained recipe leaves its second command bare beside a probe that assigns.
fn chained_bare_make(_target: &str, host: Host) -> Result<String, String> {
    let linker = if host.takes_linker_flag() {
        " -Clink-arg=-fuse-ld=mold"
    } else {
        ""
    };
    canned(format_args!(
        "if RUSTFLAGS=\"${{RUSTFLAGS:+$RUSTFLAGS }}-D warnings {THREADS_FLAG}{linker}\" cargo nextest --version; then cargo nextest run; else echo \"falling back to cargo test\"; RUSTFLAGS=\"${{RUSTFLAGS:+$RUSTFLAGS }}-D warnings {THREADS_FLAG}{linker}\" cargo test; fi\n"
    ))
}

/// A fake runner whose held-out command assigns `RUSTFLAGS` without a standard flag.
fn held_out_assigning_make(_target: &str, _host: Host) -> Result<String, String> {
    canned(format_args!(
        "RUSTFLAGS=\"-D warnings\" cargo build --release\n"
    ))
}

/// A fake runner whose held-out command assigns nothing, so it takes the configuration's flags.
fn held_out_unassigned_make(_target: &str, _host: Host) -> Result<String, String> {
    canned(format_args!("cargo build --release\n"))
}

/// A fake runner for a target that is not defined.
fn undefined_make(target: &str, _host: Host) -> Result<String, String> {
    Err(format!("`make -n {target}` failed, so it is not defined"))
}

/// Turns a failed expectation into the error a test returns.
fn ensure(holds: bool, message: &str) -> Result<(), String> {
    if holds {
        Ok(())
    } else {
        Err(message.to_owned())
    }
}

/// Scenario: the policy checks fed canned `make -n` text through the injected
/// runner, so no process runs.
///
/// Invariant: a compliant command raises no complaint on either host, a command
/// that drops the caller's flags raises one per target, and a runner error
/// reaches the caller instead of being read as an empty output.
#[test]
fn the_policy_checks_run_against_an_injected_runner() -> Result<(), String> {
    let pin = Pin::Nightly;
    for host in [Host::Linux, Host::Darwin] {
        let (problems, read) = development_problems(compliant_make, host, pin)?;
        ensure(
            problems.is_empty(),
            &format!("a compliant fake raised {problems:?}"),
        )?;
        ensure(read > 0, "the fake's commands were not read")?;
    }
    let (dropped, _) = development_problems(dropping_make, Host::Linux, pin)?;
    ensure(
        !dropped.is_empty(),
        "a command that drops the caller's flags passed",
    )?;
    ensure(
        development_problems(undefined_make, Host::Linux, pin).is_err(),
        "a runner error was swallowed",
    )?;
    let (bare_test, _) = development_problems(bare_test_make, Host::Linux, pin)?;
    ensure(
        !bare_test.is_empty(),
        "a test command that assigns no RUSTFLAGS passed",
    )?;
    let (bare_lint, _) = development_problems(bare_lint_make, Host::Linux, pin)?;
    ensure(
        !bare_lint.is_empty(),
        "lint commands that assign no RUSTFLAGS passed",
    )?;
    let (chained, _) = development_problems(chained_bare_make, Host::Linux, pin)?;
    ensure(
        !chained.is_empty()
            && chained
                .iter()
                .all(|problem| problem.contains("nextest run")),
        &format!("a bare command chained beside an assigning probe raised {chained:?}"),
    )?;
    let (exempt, exempt_read) = development_problems(exempt_commands_make, Host::Linux, pin)?;
    ensure(
        exempt.is_empty() && exempt_read > 0,
        &format!("a formatter, probe or doc build raised {exempt:?}"),
    )?;
    // A synthetic held-out target runs the check in every repository, including one that defines none.
    ensure(
        held_out_problems_for(undefined_make, &["synthetic"]).is_err(),
        "a held-out runner error was swallowed",
    )?;
    let (clean, read) = held_out_problems_for(held_out_assigning_make, &["synthetic"])?;
    ensure(
        clean.is_empty() && read == 1,
        &format!("an assigning held-out command raised {clean:?}"),
    )?;
    let (inspecting, _) = held_out_problems_for(held_out_inspecting_make, &["synthetic"])?;
    ensure(
        inspecting.is_empty(),
        &format!("an inspection command in a held-out target raised {inspecting:?}"),
    )?;
    let (unassigned, _) = held_out_problems_for(held_out_unassigned_make, &["synthetic"])?;
    ensure(
        !unassigned.is_empty(),
        "a held-out command that assigns no RUSTFLAGS passed",
    )?;
    ensure(
        held_out_target_count() == 0 || held_out_problems(undefined_make).is_err(),
        "the listed held-out targets were not run",
    )
}
