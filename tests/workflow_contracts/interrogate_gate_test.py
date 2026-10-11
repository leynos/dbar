"""Contract tests for the Python docstring-coverage gate in the Makefile.

`make lint` must reach an `interrogate` run that covers every Python file the
repository owns, at a threshold of 100, with a guard that fails on any `MISSED`
row (interrogate rounds its percentage to one decimal, so the threshold alone
passes a lone gap in a large repository), and CI must have uv before it runs
`make lint`. These tests read the recipe and the workflow, and also run the
target against a stub tool, so a deleted flag, a dropped prerequisite or a
guard that stops failing breaks a test rather than silently restoring the gap.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[2]
MAKEFILE = ROOT / "Makefile"
CI = ROOT / ".github" / "workflows" / "ci.yml"


def _makefile_text() -> str:
    """Return the Makefile source."""
    return MAKEFILE.read_text(encoding="utf-8")


def _variable(name: str) -> str:
    """Return the value a `NAME ?=` or `NAME =` assignment gives, joined."""
    pattern = re.compile(rf"^{name}\s*\??=\s*((?:.*\\\n)*.*)$", re.MULTILINE)
    match = pattern.search(_makefile_text())
    assert match, f"the Makefile must assign {name}"
    return re.sub(r"\\\n\s*", " ", match.group(1)).strip()


def _prerequisites(target: str) -> list[str]:
    """Return the prerequisites declared for *target*."""
    pattern = re.compile(rf"^{re.escape(target)}:([^=\n]*)$", re.MULTILINE)
    match = pattern.search(_makefile_text())
    assert match, f"the Makefile must declare a {target} target"
    return match.group(1).split("##")[0].split()


def _tracked(*patterns: str) -> list[str]:
    """Return the Git-tracked files matching *patterns*.

    Git is the authority on what the repository owns: a walk would also see
    the virtual environments CI creates beside the sources.
    """
    return subprocess.run(  # noqa: S603 - fixed argv, no shell
        ["git", "-C", str(ROOT), "ls-files", *patterns],  # noqa: S607 - git from PATH
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()


def _python_directories() -> set[str]:
    """Return the paths that hold Python source or stubs Git tracks."""
    return {
        str(Path(name).parent) if "/" in name else name
        for name in _tracked("*.py", "*.pyi")
    }


def test_lint_depends_on_interrogate() -> None:
    """Dropping the prerequisite would remove the gate from `make lint`."""
    assert "interrogate" in _prerequisites("lint")


def test_the_tool_is_pinned_and_run_through_uv() -> None:
    """An unpinned tool could change the verdict with no repository change."""
    assert _variable("INTERROGATE_VERSION") == "1.7.0"
    command = _variable("INTERROGATE")
    assert "tool run" in command
    assert "--from 'interrogate==$(INTERROGATE_VERSION)'" in command


def test_the_flags_demand_a_full_exact_report() -> None:
    """Threshold 100, every definition printed, and stubs included."""
    flags = _variable("INTERROGATE_FLAGS").split()
    assert "--fail-under" in flags
    assert flags[flags.index("--fail-under") + 1] == "100"
    assert "-vv" in flags
    if _tracked("*.pyi"):
        assert "--ext" in flags, "stubs need --ext pyi"
        assert flags[flags.index("--ext") + 1] == "pyi", "stubs need --ext pyi"
    relaxing = [f for f in flags if f.startswith(("--ignore", "--exclude", "-e"))]
    assert not relaxing, f"no ignore or exclude flag may relax the gate: {relaxing}"


def test_every_python_directory_is_a_target() -> None:
    """A Python directory outside `INTERROGATE_TARGETS` is undocumented unchecked."""
    targets = _variable("INTERROGATE_TARGETS").split()
    uncovered = [
        directory
        for directory in sorted(_python_directories())
        if not any(directory == t or directory.startswith(f"{t}/") for t in targets)
    ]
    assert not uncovered, f"add these to INTERROGATE_TARGETS: {uncovered}"


def test_ci_installs_uv_before_it_runs_make_lint() -> None:
    """The target runs interrogate through uv, so the lint job needs uv first."""
    workflow = yaml.safe_load(CI.read_text(encoding="utf-8"))
    for job in workflow["jobs"].values():
        steps = job.get("steps", [])
        lint = [i for i, s in enumerate(steps) if str(s.get("run", "")).strip() == "make lint"]
        if not lint:
            continue
        uv = [i for i, s in enumerate(steps) if "setup-uv" in str(s.get("uses", ""))]
        assert uv
        assert min(uv) < lint[0]
        return
    pytest.fail("no CI job runs `make lint`")


def _run_target(tmp_path: Path, script: str) -> subprocess.CompletedProcess[str]:
    """Run `make interrogate` with the tool replaced by a stub script."""
    stub = tmp_path / "stub.sh"
    stub.write_text(
        '#!/bin/sh\nprintf "%s\\n" "$@" > "$(dirname "$0")/args.txt"\n'
        + textwrap.dedent(script),
        encoding="utf-8",
    )
    stub.chmod(0o755)
    make = shutil.which("make")
    assert make
    return subprocess.run(  # noqa: S603 - fixed argv, no shell
        [make, "-C", str(ROOT), "interrogate", f"INTERROGATE={stub}"],
        capture_output=True,
        text=True,
        check=False,
    )


def test_the_target_passes_when_the_tool_reports_no_gap(tmp_path: Path) -> None:
    """A clean report and a zero status pass."""
    result = _run_target(tmp_path, "echo '| f.py | COVERED |'\nexit 0\n")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "| f.py | COVERED |" in result.stdout


def test_the_target_passes_the_configured_flags_and_targets(tmp_path: Path) -> None:
    """The tool receives exactly the configured flags, then the targets."""
    _run_target(tmp_path, "exit 0\n")
    received = (tmp_path / "args.txt").read_text(encoding="utf-8").split()
    expected = _variable("INTERROGATE_FLAGS").split() + _variable("INTERROGATE_TARGETS").split()
    assert received == expected


def test_the_target_fails_when_the_tool_fails(tmp_path: Path) -> None:
    """The tool's own non-zero status is kept."""
    result = _run_target(tmp_path, "echo 'FAILED'\nexit 1\n")
    assert result.returncode != 0


def test_the_target_fails_on_a_missed_row_the_percentage_hid(tmp_path: Path) -> None:
    """Rounded to 100.0% and exit 0, a MISSED row still fails the target."""
    result = _run_target(
        tmp_path, "echo '| f.main (L3) |  MISSED |'\necho 'RESULT: PASSED'\nexit 0\n"
    )
    assert result.returncode != 0
    assert "undocumented definitions remain" in result.stderr
