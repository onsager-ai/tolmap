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
    for program in ["git", "sh", "cat", "sleep", "setsid"] {
        std::os::unix::fs::symlink(format!("/usr/bin/{program}"), bin.join(program)).unwrap();
    }
    dir
}
fn run(dir: &tempfile::TempDir, operation: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .args(["personal-github-credential", operation])
        .env_clear()
        .env("HOME", dir.path())
        .env(
            "PATH",
            format!(".:relative::{}", dir.path().join("bin").display()),
        )
        .env("GH_TOKEN", TOKEN)
        .env("GITHUB_TOKEN", "synthetic-github-token")
        .env("GH_HOST", "github.com")
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
    let dir = fixture("printf '%s' \"$$\" > \"$HOME/sleeper-pid\"; exec sleep 60");
    let started = std::time::Instant::now();
    let output = run(&dir, "get");
    assert!(started.elapsed() < std::time::Duration::from_secs(25));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let pid: i32 = std::fs::read_to_string(dir.path().join("sleeper-pid"))
        .unwrap()
        .parse()
        .unwrap();
    let dead = unsafe { libc::kill(pid, 0) } != 0
        || std::fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|status| {
            status
                .lines()
                .any(|line| line.starts_with("State:") && line.contains("Z (zombie)"))
        });
    assert!(dead, "sleeping helper is still running");
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

#[test]
fn escaped_stdout_holder_does_not_extend_timeout() {
    let dir = fixture("setsid sh -c 'echo $$ > \"$HOME/escaped-pid\"; exec sleep 60' &\nexit 0");
    let started = std::time::Instant::now();
    let output = run(&dir, "get");
    let pid: i32 = std::fs::read_to_string(dir.path().join("escaped-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The escaped process is deliberately outside the provider group; clean it
    // up explicitly after proving its open stdout cannot hold exchange open.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(25));
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn only_gh_receives_environment_tokens_and_helper_path_is_absolute() {
    let dir = fixture("printf '%s|%s|%s|%s' \"${GH_TOKEN-unset}\" \"${GITHUB_TOKEN-unset}\" \"${GH_HOST-unset}\" \"$PATH\" > \"$HOME/helper-env\"; exit 1");
    let git = dir.path().join("bin/git");
    std::fs::remove_file(&git).unwrap();
    std::fs::write(&git, "#!/bin/sh\nprintf '%s|%s|%s' \"${GH_TOKEN-unset}\" \"${GITHUB_TOKEN-unset}\" \"${GH_HOST-unset}\" > \"$HOME/git-env\"\nexec /usr/bin/git \"$@\"\n").unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
    let gh = dir.path().join("bin/gh");
    std::fs::write(&gh, "#!/bin/sh\ncat >/dev/null\nprintf '%s|%s|%s' \"$GH_TOKEN\" \"$GITHUB_TOKEN\" \"$GH_HOST\" > \"$HOME/gh-env\"\nprintf 'username=fixture\\npassword=%s\\n' \"$GH_TOKEN\"\n").unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = run(&dir, "get");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("git-env")).unwrap(),
        "unset|unset|unset"
    );
    // Git prepends its own exec-path (e.g. /usr/lib/git-core) before running
    // helpers, so check the filtered user entry and that every entry is absolute.
    let helper = std::fs::read_to_string(dir.path().join("helper-env")).unwrap();
    let (tokens, path) = helper.rsplit_once('|').unwrap();
    assert_eq!(tokens, "unset|unset|unset");
    assert!(
        path.split(':').all(|entry| entry.starts_with('/')),
        "{path}"
    );
    assert!(path
        .split(':')
        .any(|entry| std::path::Path::new(entry) == dir.path().join("bin")));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("gh-env")).unwrap(),
        format!("{TOKEN}|synthetic-github-token|github.com")
    );
}
