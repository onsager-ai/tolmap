//! Commit-pinned inert text. Adapters resolve repository/access-domain identity
//! and current authority BEFORE calling this module (including warm caches).
use anyhow::{bail, ensure, Result};
use serde::Serialize;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};

pub const VERSION: u8 = 1;
pub const MAX_BYTES: u64 = 1024 * 1024;
pub const LONG_LINE: usize = 20_000;

#[derive(Serialize)]
pub struct Source {
    pub version: u8,
    pub repository: String,
    pub commit: String,
    pub path: String,
    pub status: &'static str,
    pub text: Option<String>,
    pub bytes: u64,
    pub long_line: bool,
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', '\0', '\n', '\r'])
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && p != ".git")
}

pub fn git(repo: &Path) -> Command {
    let mut c = Command::new("git");
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["--no-replace-objects", "--literal-pathspecs", "-C"])
        .arg(repo);
    c
}

/// Never opens a repository-controlled filesystem path. Git tree modes reject
/// symlinks/submodules; cat-file reads only a verified blob object, with a bound.
/// No working-tree, branch, default-branch, network or LFS fallback exists.
pub fn read(repo: &Path, repository: &str, commit: &str, path: &str) -> Result<Source> {
    ensure!(valid_path(path), "invalid relative source path");
    ensure!(
        crate::service::worker_result::is_object_id(commit),
        "immutable commit required"
    );
    let mut result = Source {
        version: VERSION,
        repository: repository.into(),
        commit: commit.into(),
        path: path.into(),
        status: "missing",
        text: None,
        bytes: 0,
        long_line: false,
    };
    let revision = format!("{commit}^{{commit}}");
    if !git(repo)
        .args(["cat-file", "-e", &revision])
        .status()?
        .success()
    {
        result.status = "expired";
        return Ok(result);
    }
    let tree = git(repo)
        .args(["ls-tree", "-z", commit, "--", path])
        .output()?;
    if !tree.status.success() || tree.stdout.is_empty() {
        return Ok(result);
    }
    let entry = std::str::from_utf8(&tree.stdout)?;
    let Some((header, name)) = entry.trim_end_matches('\0').split_once('\t') else {
        bail!("invalid tree entry")
    };
    if name != path {
        return Ok(result);
    }
    let fields: Vec<_> = header.split_whitespace().collect();
    if fields.len() != 3 || fields[1] != "blob" || !matches!(fields[0], "100644" | "100755") {
        result.status = "unsupported";
        return Ok(result);
    }
    let object = fields[2];
    ensure!(
        crate::service::worker_result::is_object_id(object),
        "invalid blob"
    );
    let size = git(repo).args(["cat-file", "-s", object]).output()?;
    ensure!(size.status.success(), "blob unavailable");
    result.bytes = std::str::from_utf8(&size.stdout)?.trim().parse()?;
    if result.bytes > MAX_BYTES {
        result.status = "large";
        return Ok(result);
    }
    let mut child = git(repo)
        .args(["cat-file", "blob", object])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes);
    let status = child.wait()?;
    read?;
    ensure!(
        status.success() && bytes.len() as u64 == result.bytes,
        "blob changed or unavailable"
    );
    if bytes.contains(&0) {
        result.status = "binary";
        return Ok(result);
    }
    match String::from_utf8(bytes) {
        Ok(text) => {
            result.long_line = text.lines().any(|l| l.len() > LONG_LINE);
            result.status = "ready";
            result.text = Some(text);
        }
        Err(_) => result.status = "binary",
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_paths_have_no_escape_or_git_metadata_surface() {
        for p in [
            "",
            "/etc/passwd",
            "../a",
            "a/../b",
            "a//b",
            "a\\b",
            ".git/config",
            "a\0b",
            "./a",
        ] {
            assert!(!valid_path(p), "{p:?}");
        }
        for p in ["src/main.rs", "a b.txt", "<script>.txt", ":(glob)*"] {
            assert!(valid_path(p));
        }
    }
}
