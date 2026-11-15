//! Integration tests for the `atomanja` binary (plan task cli-01).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_atomanja");

/// A fresh, empty directory under the system temp dir, unique per test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomanja-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The binary with a complete `ATOMANJA_*` environment rooted at `dir`.
fn command(dir: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env_clear()
        .env("ATOMANJA_DB", dir.join("ledger.db"))
        .env("ATOMANJA_DEV_DB", dir.join("dev.db"))
        .env("ATOMANJA_VIKUNJA_URL", "http://127.0.0.1:9")
        .env("ATOMANJA_VIKUNJA_TOKEN", "token")
        .env("ATOMANJA_BASE_URL", "http://localhost")
        .env("ATOMANJA_SMTP_HOST", "localhost")
        .env("ATOMANJA_SMTP_USER", "user")
        .env("ATOMANJA_SMTP_PASSWORD", "secret")
        .env("ATOMANJA_SMTP_FROM", "atomanja@localhost")
        .env("ATOMANJA_VAPID_KEY", dir.join("vapid.key"));
    cmd
}

/// A command with an empty environment.
fn bare() -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env_clear();
    cmd
}

/// Run `cmd` with `args`, feeding `stdin`.
fn run(mut cmd: Command, args: &[&str], stdin: &str) -> Output {
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn binary");
    // The binary may exit before reading stdin; a broken pipe is fine.
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes());
    child.wait_with_output().expect("wait for binary")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cli_unknown_subcommand_exits_nonzero() {
    let out = run(bare(), &["frobnicate"], "");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("frobnicate"), "names the bad subcommand: {err}");
    assert!(err.contains("usage"), "prints usage: {err}");
    assert!(err.contains("create-worker"), "lists subcommands: {err}");
}

#[test]
fn cli_no_subcommand_prints_usage_and_exits_nonzero() {
    let out = run(bare(), &[], "");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("usage"));
}

#[test]
fn cli_help_prints_usage_and_succeeds() {
    let out = run(bare(), &["--help"], "");
    assert!(out.status.success());
    assert!(stdout(&out).contains("seed-dev"));
}

#[test]
fn cli_missing_config_names_the_variable() {
    for args in [&["create-worker", "w@example.org"][..], &["serve"][..]] {
        let out = run(bare(), args, "correct-horse\n");
        assert!(!out.status.success());
        let err = stderr(&out);
        assert!(err.contains("ATOMANJA_DB"), "{err}");
        assert!(!err.contains("panicked"), "{err}");
    }
}

#[test]
fn cli_create_worker_only_once() {
    let dir = temp_dir("worker-once");
    let first = run(command(&dir), &["create-worker", "w@example.org"], "correct-horse\n");
    assert!(first.status.success(), "{}", stderr(&first));
    assert!(!stdout(&first).contains("correct-horse"), "never echoes the password");
    assert!(!stderr(&first).contains("correct-horse"), "never echoes the password");

    let second = run(command(&dir), &["create-worker", "x@example.org"], "correct-horse\n");
    assert!(!second.status.success());
    let err = stderr(&second);
    assert!(err.contains("worker"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn cli_create_worker_rejects_short_password() {
    let dir = temp_dir("short-pw");
    let out = run(command(&dir), &["create-worker", "w@example.org"], "short\n");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("password"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn cli_create_worker_requires_an_email() {
    let dir = temp_dir("no-email");
    let out = run(command(&dir), &["create-worker"], "correct-horse\n");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("usage"));
    let out = run(command(&dir), &["create-worker", "not-an-email"], "correct-horse\n");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("email"));
}

#[test]
fn cli_create_admin_goes_to_dev_db_and_rejects_duplicate_email() {
    let dir = temp_dir("admin");
    let out = run(command(&dir), &["create-admin", "a@example.org"], "admin-password\n");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(dir.join("dev.db").exists());

    let again = run(command(&dir), &["create-admin", "a@example.org"], "admin-password\n");
    assert!(!again.status.success());
    assert!(!stderr(&again).contains("panicked"));

    // DEV-3: the admin lives in the dev file, so the ledger file has no
    // account with that email and no worker yet.
    let worker = run(command(&dir), &["create-worker", "w@example.org"], "worker-password\n");
    assert!(worker.status.success(), "{}", stderr(&worker));
}

#[test]
fn cli_admin_email_may_not_shadow_a_ledger_account() {
    // Login checks the ledger file first (DEV-3): the same email in both
    // files would make the admin unreachable.
    let dir = temp_dir("shadow");
    let w = run(command(&dir), &["create-worker", "same@example.org"], "worker-password\n");
    assert!(w.status.success(), "{}", stderr(&w));
    let a = run(command(&dir), &["create-admin", "same@example.org"], "admin-password\n");
    assert!(!a.status.success());
    assert!(stderr(&a).contains("already exists"), "{}", stderr(&a));
}

#[test]
fn cli_gen_vapid_writes_key_and_refuses_to_overwrite() {
    let dir = temp_dir("vapid");
    let out = run(command(&dir), &["gen-vapid"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    let key = std::fs::read_to_string(dir.join("vapid.key")).expect("key file written");
    assert!(!key.trim().is_empty());
    assert!(!stdout(&out).contains(key.trim()), "private key not printed");

    let again = run(command(&dir), &["gen-vapid"], "");
    assert!(!again.status.success());
    let after = std::fs::read_to_string(dir.join("vapid.key")).expect("key file kept");
    assert_eq!(key, after);
}

#[test]
fn cli_seed_dev_seeds_once() {
    let dir = temp_dir("seed");
    let out = run(command(&dir), &["seed-dev"], "");
    assert!(out.status.success(), "{}", stderr(&out));
    let again = run(command(&dir), &["seed-dev"], "");
    assert!(!again.status.success());
    assert!(!stderr(&again).contains("panicked"));
}

#[test]
fn cli_serve_fails_cleanly_without_vapid_key() {
    let dir = temp_dir("serve-nokey");
    let out = run(command(&dir), &["serve", "127.0.0.1:0"], "");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("gen-vapid"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}
