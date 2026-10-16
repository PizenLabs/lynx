//! Integration tests for the installed `lx` binary.
//!
//! These exercise the binary as a user would run it (a real process, real
//! stdout/stderr, real exit codes) — not the library. `doctor` and
//! `mcp --check` must never load the embedding model or index a repository, so
//! every test here is fast and hermetic.

use std::path::Path;
use std::process::{Command, Output};

/// Path to the compiled `lx` binary under test.
fn lx() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lx"))
}

/// Runs `lx` in `dir` with `args` and returns the captured output.
fn run(dir: &Path, args: &[&str]) -> Output {
    lx().current_dir(dir)
        .args(args)
        .output()
        .expect("failed to spawn lx")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_is_machine_readable() {
    let out = lx().arg("--version").output().expect("spawn lx --version");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(
        text.trim(),
        format!("lx {}", env!("CARGO_PKG_VERSION")),
        "unexpected version output: {text:?}"
    );
}

#[test]
fn help_lists_the_distribution_commands() {
    let out = lx().arg("--help").output().expect("spawn lx --help");
    assert!(out.status.success());
    let text = stdout(&out);
    for expected in ["doctor", "mcp", "index", "search", "resolve", "context"] {
        assert!(text.contains(expected), "help missing `{expected}`: {text}");
    }
}

#[test]
fn doctor_runs_and_reports_a_verdict() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = run(dir.path(), &["doctor"]);
    // Diagnosis always completes; the verdict is carried in the output.
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Lynx Doctor"), "missing header: {text}");
    assert!(
        text.contains("Version      "),
        "missing version line: {text}"
    );
    assert!(
        text.contains("Platform     "),
        "missing platform line: {text}"
    );
    assert!(
        text.contains("MCP          OK"),
        "MCP check should pass: {text}"
    );
    assert!(
        text.contains("Status       READY") || text.contains("Status       NOT READY"),
        "missing status verdict: {text}"
    );
}

#[test]
fn doctor_reports_not_ready_without_an_index() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = run(dir.path(), &["doctor"]);
    let text = stdout(&out);
    // An unindexed, non-git directory must be reported, not hidden.
    assert!(
        text.contains("NOT READY"),
        "expected NOT READY in an empty dir: {text}"
    );
    assert!(text.contains("missing"), "expected missing index: {text}");
}

#[test]
fn mcp_check_validates_the_protocol_contract() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = run(dir.path(), &["mcp", "--check"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Lynx MCP check"), "missing header: {text}");
    assert!(text.contains("2024-11-05"), "missing protocol: {text}");
    for tool in [
        "search",
        "resolve",
        "resolve_all",
        "inspect",
        "relations",
        "similar",
        "context",
        "index_status",
    ] {
        assert!(text.contains(tool), "tools list missing `{tool}`: {text}");
    }
    assert!(text.contains("Status       OK"), "missing status: {text}");
}

#[test]
fn status_fails_clearly_when_not_indexed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = run(dir.path(), &["status"]);
    assert!(
        !out.status.success(),
        "status should fail on an unindexed workspace"
    );
    let text = stderr(&out);
    assert!(
        text.contains("not indexed") || text.contains("lx index"),
        "error should be actionable: {text}"
    );
}
