//! Synthetic providers only: no network and no real user credentials.
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

const TOKEN: &str = "synthetic-private-github-token";
fn fixture(helper_body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helper");
    std::fs::write(&helper, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$HOME/argv\"\ncat >> \"$HOME/request\"\n{helper_body}\n")).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        dir.path().join(".gitconfig"),
        format!("[credential]\n helper = {}\n", helper.display()),
    )
    .unwrap();
    // gh is deliberately absent from PATH, so a failed helper cannot fall back
    // to a developer's actual gh login.
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for program in ["git", "sh", "cat", "sleep"] {
        std::os::unix::fs::symlink(format!("/usr/bin/{program}"), bin.join(program)).unwrap();
    }
    dir
}
fn run(dir: &tempfile::TempDir, operation: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .args(["personal-github-credential", operation])
        .env_clear()
        .env("HOME", dir.path())
        .env("PATH", dir.path().join("bin"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=github.com\npath=owner/private.git\n\n")
        .unwrap();
    child.wait_with_output().unwrap()
}
#[test]
fn helper_secret_uses_protocol_only_and_store_erase_are_ignored() {
    let dir = fixture(&format!(
        "printf 'username=fixture\\npassword={TOKEN}\\n'\nprintf '{TOKEN}' >&2"
    ));
    let output = run(&dir, "get");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert!(output.stderr.is_empty());
    for filename in ["argv", "request", ".gitconfig"] {
        let text = std::fs::read_to_string(dir.path().join(filename)).unwrap();
        assert!(!text.contains(TOKEN));
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv")).unwrap(),
        "get\n"
    );
    for operation in ["store", "erase"] {
        let output = run(&dir, operation);
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv")).unwrap(),
        "get\n"
    );
}
#[test]
fn gh_login_is_used_when_configured_helper_has_no_credential() {
    let dir = fixture("exit 1");
    let gh = dir.path().join("bin/gh");
    std::fs::write(&gh, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$HOME/gh-argv\"\ncat > \"$HOME/gh-request\"\nprintf 'username=fixture\\npassword={TOKEN}\\n'\n")).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = run(&dir, "get");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert!(output.stderr.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("gh-argv")).unwrap(),
        "auth\ngit-credential\nget\n"
    );
    assert!(!std::fs::read_to_string(dir.path().join("gh-request"))
        .unwrap()
        .contains(TOKEN));
}

#[test]
fn unresponsive_helper_is_bounded_and_reaped() {
    let dir = fixture("sleep 60");
    let started = std::time::Instant::now();
    let output = run(&dir, "get");
    assert!(started.elapsed() < std::time::Duration::from_secs(25));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_credentials_never_prompt_or_emit_provider_errors() {
    let dir = fixture(&format!("printf '{TOKEN}' >&2\nexit 1"));
    let output = run(&dir, "get");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("argv")).unwrap(),
        "get\n"
    );
}
