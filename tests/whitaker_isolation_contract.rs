//! Contract for the Whitaker isolation the development build standard asks for.
//!
//! Whitaker runs under the toolchain its installer manages, so `make lint` starts it outside the
//! checkout (Cargo then never discovers the checkout's `.cargo/config.toml` and its Cranelift
//! profile), names the manifest by absolute path, and assigns the literal `RUSTFLAGS` that
//! displaces every `rustflags` source. These tests read the real `make -n lint` output, so a
//! recipe that drops any of the three fails here, and read the workflow job that builds under the
//! repository's own Cargo configuration, so it stays free of the setup action's exported flags.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The workflow text the bare-build job is read from.
const CI_WORKFLOW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.github/workflows/ci.yml"
));

/// The job whose `setup-rust` step must export no `RUSTFLAGS`.
const BARE_BUILD_JOB: &str = "unix-only-build-contract";

/// Returns the checkout's canonical path, which is what `make` reports as `CURDIR`.
fn checkout() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .map_err(|error| format!("resolving the checkout: {error}"))
}

/// Returns the `make -n lint` line that runs Whitaker.
fn whitaker_line() -> Result<String, String> {
    let output = Command::new("make")
        .args(["-n", "-B", "lint"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map_err(|error| format!("running make: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`make -n lint` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.contains("whitaker"))
        .map(str::to_owned)
        .ok_or_else(|| "`make -n lint` runs no Whitaker command".to_owned())
}

/// Returns the complaints about a Whitaker line that is not isolated from the checkout.
fn isolation_problems(line: &str, checkout: &Path) -> Vec<String> {
    let parent = checkout.parent().unwrap_or(checkout);
    let wanted = [
        (
            format!("cd \"{}/\" && ", parent.display()),
            "start from the checkout's parent directory",
        ),
        (
            "RUSTFLAGS=\"-D warnings\" ".to_owned(),
            "assign the literal `RUSTFLAGS=\"-D warnings\"`",
        ),
        (
            format!("--manifest-path \"{}/Cargo.toml\"", checkout.display()),
            "name the manifest by absolute path",
        ),
    ];
    wanted
        .iter()
        .filter(|(text, _)| !line.contains(text.as_str()))
        .map(|(_, what)| format!("the Whitaker command must {what}: {line}"))
        .collect()
}

/// Scenario: the real `make -n lint` output.
///
/// Invariant: Whitaker starts outside the checkout, assigns the literal `RUSTFLAGS`, and names
/// the manifest by absolute path.
#[test]
fn make_lint_isolates_whitaker_from_the_checkout() -> Result<(), String> {
    let problems = isolation_problems(&whitaker_line()?, &checkout()?);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Scenario: Whitaker commands that each lack one isolation detail.
///
/// Invariant: each missing detail is reported, so a recipe that drops one cannot pass.
#[test]
fn a_whitaker_command_missing_an_isolation_detail_is_reported() -> Result<(), String> {
    let checkout = Path::new("/work/repo");
    let whole = "cd \"/work/\" && RUSTFLAGS=\"-D warnings\" whitaker --manifest-path \"/work/repo/Cargo.toml\" --all";
    let cases = [
        whole.replace("cd \"/work/\" && ", ""),
        whole.replace("RUSTFLAGS=\"-D warnings\" ", ""),
        whole.replace("--manifest-path \"/work/repo/Cargo.toml\" ", ""),
    ];
    if !isolation_problems(whole, checkout).is_empty() {
        return Err("the whole command must pass".to_owned());
    }
    for case in &cases {
        if isolation_problems(case, checkout).len() != 1 {
            return Err(format!("one detail missing must give one problem: {case}"));
        }
    }
    Ok(())
}

/// Returns the lines of one job of a workflow: from its key to the next job.
fn job_lines<'a>(workflow: &'a str, job: &str) -> Vec<&'a str> {
    let key = format!("  {job}:");
    workflow
        .lines()
        .skip_while(|line| *line != key)
        .enumerate()
        .take_while(|(at, line)| *at == 0 || line.trim().is_empty() || line.starts_with("   "))
        .map(|(_, line)| line)
        .collect()
}

/// Returns whether a job passes an empty `rustflags` input to `setup-rust`.
fn exports_no_rustflags(workflow: &str, job: &str) -> bool {
    job_lines(workflow, job).iter().any(|line| {
        let squeezed: String = line
            .chars()
            .filter(|c| !matches!(c, ' ' | '\'' | '"'))
            .collect();
        squeezed == "rustflags:"
    })
}

/// Scenario: the job that checks the repository's own Cargo configuration.
///
/// Invariant: its `setup-rust` step passes `rustflags: ''`, so the check reads the repository's
/// frontend and linker flags instead of the `-D warnings` the setup action exports.
#[test]
fn the_bare_build_job_exports_no_rustflags() -> Result<(), String> {
    if exports_no_rustflags(CI_WORKFLOW, BARE_BUILD_JOB) {
        Ok(())
    } else {
        Err(format!(
            "the `{BARE_BUILD_JOB}` job must pass `rustflags: ''` to setup-rust"
        ))
    }
}

/// Scenario: workflow jobs with and without the empty `rustflags` input.
///
/// Invariant: only an empty value counts, a sibling job's input does not, and a missing job
/// reads as not exporting nothing.
#[test]
fn the_workflow_reader_tells_an_empty_input_from_a_missing_or_filled_one() -> Result<(), String> {
    let workflow = "jobs:\n  a:\n    steps:\n      - with:\n          rustflags: ''\n  b:\n    steps:\n      - with:\n          install-mold: 'true'\n  c:\n    steps:\n      - with:\n          rustflags: -D warnings\n";
    let read = (
        exports_no_rustflags(workflow, "a"),
        exports_no_rustflags(workflow, "b"),
        exports_no_rustflags(workflow, "c"),
        exports_no_rustflags(workflow, "missing"),
    );
    if read == (true, false, false, false) {
        Ok(())
    } else {
        Err(format!("read wrongly: {read:?}"))
    }
}
