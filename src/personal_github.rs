//! Personal GitHub acquisition. The public source adapter stays anonymous.
use anyhow::{ensure, Context, Result};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
};

const ACCESS_ERROR: &str = "GitHub access failed; configure a git credential helper or run gh auth login externally, check repository access, then retry";

/// Permit only the session settings needed by existing credential providers.
/// Git config/trace overrides are excluded; indexing keeps its original environment.
fn helper_environment(command: &mut Command) {
    let paths = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let paths = std::env::split_paths(&paths)
        .filter(|path| path.is_absolute())
        .collect::<Vec<_>>();
    command.env(
        "PATH",
        std::env::join_paths(paths).expect("PATH entries remain valid"),
    );
    for name in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "SSH_AUTH_SOCK",
        "GH_CONFIG_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .env("GCM_INTERACTIVE", "never")
        .env("GH_PROMPT_DISABLED", "1");
}

fn helper_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    helper_environment(&mut command);
    command
}

fn exchange(mut command: Command, input: &[u8]) -> Result<Output> {
    use std::os::{fd::AsRawFd, unix::process::CommandExt};
    let mut child = command
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("credential provider unavailable")?;
    let pid = child.id();
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take().unwrap();
    let result = (|| -> Result<Output> {
        for fd in [stdin.as_ref().unwrap().as_raw_fd(), stdout.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            ensure!(
                flags >= 0
                    && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
                "credential pipe unavailable"
            );
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut written = 0;
        let mut bytes = Vec::new();
        let mut eof = false;
        loop {
            ensure!(
                std::time::Instant::now() < deadline,
                "credential provider timed out"
            );
            if let Some(pipe) = stdin.as_mut() {
                match pipe.write(&input[written..]) {
                    Ok(n) => written += n,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
                if written == input.len() {
                    stdin.take();
                }
            }
            if !eof {
                let mut buffer = [0; 4096];
                match stdout.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(n) => {
                        bytes.extend_from_slice(&buffer[..n]);
                        ensure!(bytes.len() <= 64 * 1024, "credential response too large");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            // Do not reap before EOF: retaining the direct child also reserves
            // its process-group ID until timeout cleanup has killed that group.
            if eof {
                if let Some(status) = child.try_wait()? {
                    return Ok(Output {
                        status,
                        stdout: bytes,
                        stderr: Vec::new(),
                    });
                }
            }
            let mut fds = [
                libc::pollfd {
                    fd: if eof { -1 } else { stdout.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: stdin.as_ref().map_or(-1, |p| p.as_raw_fd()),
                    events: libc::POLLOUT,
                    revents: 0,
                },
            ];
            unsafe {
                libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 50);
            }
        }
    })();
    if result.is_err() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    // Nonblocking pipes are dropped here even if an escaped daemon owns copies.
    result
}

/// Git's helper protocol is the only secret channel. Ignore store/erase so even
/// a configured credential-store helper cannot persist a credential via tolmap.
pub fn credential(operation: &str) -> Result<()> {
    if operation != "get" {
        return Ok(());
    }
    let mut input = Vec::new();
    std::io::stdin().take(16 * 1024).read_to_end(&mut input)?;
    std::io::stdout().write_all(&provider_response(&input)?)?;
    Ok(())
}

fn provider_response(input: &[u8]) -> Result<Vec<u8>> {
    let request = std::str::from_utf8(input).context("invalid credential request")?;
    ensure!(
        request.lines().any(|line| line == "protocol=https")
            && request.lines().any(|line| line == "host=github.com"),
        "GitHub HTTPS credentials required"
    );
    let mut git = helper_command("git");
    git.args(["-c", "credential.interactive=false", "credential", "fill"]);
    if let Ok(output) = exchange(git, &input) {
        if output.status.success() {
            return Ok(output.stdout);
        }
    }
    let mut gh = helper_command("gh");
    for name in ["GH_TOKEN", "GITHUB_TOKEN", "GH_HOST"] {
        if let Some(value) = std::env::var_os(name) {
            gh.env(name, value);
        }
    }
    gh.args(["auth", "git-credential", "get"]);
    if let Ok(output) = exchange(gh, &input) {
        if output.status.success() {
            return Ok(output.stdout);
        }
    }
    Ok(Vec::new())
}

fn command(root: &Path, authenticated: bool) -> Result<Command> {
    let mut command = crate::source::git(root);
    command.args(["-c", "credential.helper=", "-c", "core.askPass="]);
    if authenticated {
        helper_environment(&mut command);
        for name in ["GH_TOKEN", "GITHUB_TOKEN", "GH_HOST"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let executable = std::env::current_exe().context("tolmap executable unavailable")?;
        // Git interprets !helpers as shell commands; quote the executable path.
        let executable = executable
            .to_str()
            .context("tolmap executable path must be UTF-8")?
            .replace('\'', "'\\''");
        command.args([
            "-c",
            &format!(
                "credential.helper=!'{}' personal-github-credential",
                executable
            ),
        ]);
    }
    command.stdin(Stdio::null());
    Ok(command)
}

fn refused(output: &Output) -> bool {
    let error = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    [
        "authentication failed",
        "could not read username",
        "could not read password",
        "repository not found",
        "requested url returned error: 401",
        "requested url returned error: 403",
        "requested url returned error: 404",
    ]
    .iter()
    .any(|text| error.contains(text))
}

fn attempt(mut run: impl FnMut(bool) -> Result<Output>) -> Result<(Output, bool)> {
    let anonymous = run(false).context("GitHub network operation unavailable")?;
    if anonymous.status.success() {
        return Ok((anonymous, false));
    }
    ensure!(
        refused(&anonymous),
        "GitHub network operation failed; check connectivity and repository URL, then retry"
    );
    let authenticated = run(true).context(ACCESS_ERROR)?;
    ensure!(authenticated.status.success(), ACCESS_ERROR);
    Ok((authenticated, true))
}

pub fn acquire(root: &Path, path: &Path, location: &str) -> Result<()> {
    let url = format!("https://{location}.git");
    let (head, _) = attempt(|authenticated| {
        let mut git = command(root, authenticated)?;
        Ok(git
            .args(["ls-remote", "--symref", "--", &url, "HEAD"])
            .output()?)
    })?;
    // Never include provider-controlled bytes in errors or store metadata.
    let advertised = std::str::from_utf8(&head.stdout).context("invalid GitHub HEAD response")?;
    let branch = advertised
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: ")
                .and_then(|s| s.strip_suffix("\tHEAD"))
        })
        .context("GitHub default branch is unavailable")?;
    ensure!(
        branch.starts_with("refs/heads/") && !branch.contains(char::is_whitespace),
        "invalid GitHub default branch"
    );
    let remote = format!(
        "refs/remotes/origin/{}",
        branch.strip_prefix("refs/heads/").unwrap()
    );
    let refspec = format!("+{branch}:{remote}");
    attempt(|authenticated| {
        let mut git = command(root, authenticated)?;
        if path.exists() {
            // Use the validated URL, never a repository-controlled origin URL.
            git.arg("-C")
                .arg(path)
                .args(["fetch", "--no-tags", "--", &url, &refspec]);
        } else {
            git.args(["clone", "--no-checkout", "--"])
                .arg(&url)
                .arg(path);
        }
        Ok(git.output()?)
    })?;
    let status = crate::source::git(path)
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", &remote])
        .output()?;
    ensure!(
        status.status.success(),
        "GitHub default branch ref unavailable"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    fn output(success: bool, stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
        }
    }
    #[test]
    fn anonymous_environment_excludes_provider_tokens() {
        let dir = tempfile::tempdir().unwrap();
        for authenticated in [false, true] {
            let git = command(dir.path(), authenticated).unwrap();
            let environment = git.get_envs().collect::<std::collections::BTreeMap<_, _>>();
            for name in ["GH_TOKEN", "GITHUB_TOKEN", "GH_HOST"] {
                let value = environment
                    .get(std::ffi::OsStr::new(name))
                    .copied()
                    .flatten();
                let inherited = std::env::var_os(name);
                assert_eq!(
                    value,
                    if authenticated {
                        inherited.as_deref()
                    } else {
                        None
                    }
                );
                if let Some(secret) = inherited.filter(|value| !value.is_empty()) {
                    assert!(!git.get_args().any(|arg| arg
                        .as_encoded_bytes()
                        .windows(secret.as_encoded_bytes().len())
                        .any(|part| part == secret.as_encoded_bytes())));
                }
            }
            if !authenticated {
                let anonymous = crate::source::git(dir.path());
                assert_eq!(
                    git.get_envs().collect::<Vec<_>>(),
                    anonymous.get_envs().collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn anonymous_first_and_only_auth_refusals_retry() {
        let mut calls = vec![];
        assert!(
            !attempt(|authenticated| {
                calls.push(authenticated);
                Ok(output(true, ""))
            })
            .unwrap()
            .1
        );
        assert_eq!(calls, [false]);
        calls.clear();
        assert!(
            attempt(|authenticated| {
                calls.push(authenticated);
                Ok(output(authenticated, "fatal: repository not found"))
            })
            .unwrap()
            .1
        );
        assert_eq!(calls, [false, true]);
        calls.clear();
        assert!(attempt(|authenticated| {
            calls.push(authenticated);
            Ok(output(false, "TLS certificate error"))
        })
        .is_err());
        assert_eq!(calls, [false]);
    }
    #[test]
    fn anonymous_success_skips_synthetic_helper_and_refusal_invokes_it() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("helper-calls");
        for anonymous_success in [true, false] {
            let (_, used) = attempt(|authenticated| {
                if !authenticated { return Ok(output(anonymous_success, "repository not found")); }
                let mut helper = Command::new("/bin/sh");
                helper.args(["-c", "cat >/dev/null; printf get >> \"$1\"; printf 'username=fixture\\npassword=synthetic-token\\n'", "fixture"]).arg(&marker);
                let credential = exchange(helper, b"protocol=https\nhost=github.com\n\n")?;
                assert!(String::from_utf8_lossy(&credential.stdout).contains("synthetic-token"));
                Ok(output(credential.status.success(), ""))
            }).unwrap();
            assert_eq!(used, !anonymous_success);
            assert_eq!(marker.exists(), !anonymous_success);
        }
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "get");
    }

    #[test]
    fn errors_discard_all_provider_diagnostics() {
        use crate::service::store::{JobRow, PersonalSnapshot, Repository, Store};
        let token = "secret-fixture-token";
        let error = attempt(|_| Ok(output(false, &format!("authentication failed {token}"))))
            .unwrap_err()
            .to_string();
        assert!(!error.contains(token));
        assert!(error.contains("gh auth login"));
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let store = Store::open(&db).unwrap();
        store
            .register(&Repository {
                id: "repo".into(),
                name: "private".into(),
                kind: "github".into(),
                location: "github.com/owner/private".into(),
                observed_head: None,
                observed_at: None,
            })
            .unwrap();
        let snapshot = PersonalSnapshot {
            id: "prior".into(),
            repository: "repo".into(),
            commit: "a".repeat(40),
            map_path: dir.path().join("map"),
            source_path: dir.path().join("source"),
            indexed_at: String::new(),
        };
        store.personal_begin("repo", "prior").unwrap();
        store.personal_finish("prior", Ok(&snapshot)).unwrap();
        store.personal_begin("repo", "revoked").unwrap();
        store.personal_finish("revoked", Err(&error)).unwrap();
        let json = serde_json::json!({"error":error}).to_string();
        std::fs::create_dir_all(dir.path().join("logs/jobs")).unwrap();
        std::fs::write(dir.path().join("logs/jobs/failure.json"), &json).unwrap();
        store
            .insert_job(&JobRow {
                job_id: "job".into(),
                slug: "personal-web/batch".into(),
                commit: String::new(),
                spec_json: "{\"repository_id\":\"repo\"}".into(),
                class: 0,
                status: "queued".into(),
                attempt: 0,
                epoch: 0,
                lease_holder: None,
                lease_deadline_ms: None,
                queue_order: 0,
                snapshot_json: "{}".into(),
            })
            .unwrap();
        store.finish_job("job", "failed", &json).unwrap();
        assert_eq!(store.personal_history("repo").unwrap()[0].id, "prior");
        assert!(!store
            .job("job")
            .unwrap()
            .unwrap()
            .snapshot_json
            .contains(token));
        fn scan(path: &Path, token: &str) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    scan(&path, token);
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    assert!(
                        !bytes
                            .windows(token.len())
                            .any(|window| window == token.as_bytes()),
                        "{}",
                        path.display()
                    );
                }
            }
        }
        assert!(db.with_file_name("db-wal").exists());
        assert!(db.with_file_name("db-shm").exists());
        scan(dir.path(), token);
        drop(store);
    }
}
