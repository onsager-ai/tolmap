//! SCIP indexer invocation (issue #110, P1a), and nothing else.
//!
//! Every binary, argument and environment variable tolmap hands an indexer
//! lives in this module, so the worker image (P1b) and the install sandbox
//! (P1c) change one place. The methods are the ones P0 measured
//! (`eval/scip_index.sh`, finding 41), in its default mode:
//!
//! - **Nothing is installed.** No npm/pnpm/pip/uv install and no module
//!   download. Installing runs the indexed repository's own code, and the
//!   owner's decision (issue #110, P1 decisions) is that installs happen
//!   only inside a sandbox that P1c designs first. Go runs with
//!   `GOPROXY=off`, `GOTOOLCHAIN=local` and an empty module cache, exactly
//!   as P0's no-install variant did, so a host's warm module cache cannot
//!   make one machine's map differ from another's.
//! - **Python** runs `scip-python` once at the repository root. Finding 41
//!   measured a nested project root (dify's `api/`) raising recall from
//!   0.894 to 0.970; rooting per project is a later change, not this one.
//! - **Go** runs `scip-go` at the repository root.
//! - **TypeScript** runs `scip-typescript` once over every tracked
//!   `tsconfig.json`, deepest first. Not `--infer-tsconfig`: P0's first run
//!   used it and an inferred per-package config displaced vue's root one,
//!   keeping 6 of 259 cross-package edges (finding 41).
//!
//! Binaries come from `PATH`, or from `TOLMAP_SCIP_PYTHON`,
//! `TOLMAP_SCIP_GO` and `TOLMAP_SCIP_TYPESCRIPT`. There is no timeout:
//! the no-caps policy (finding 38) applies to indexing as to every other
//! stage, and the ETA model carries the expectation instead.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::extract::LanguageKind;
use crate::progress::StageCounter;

/// Why an indexer produced no usable index. Every variant is a fallback to
/// the hand-written graph for that language, recorded in the map's
/// `coverage.references`, never a failed build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexFailure {
    /// The binary is not on `PATH` (or its override does not exist).
    NotFound { binary: String },
    /// TypeScript without a tracked `tsconfig.json`: nothing to index with
    /// the repository's own configuration.
    NoProjects,
    /// The process could not be started or waited on for another reason.
    Spawn { message: String },
    /// Non-zero exit. A partial index written before the failure is not
    /// trusted: dify's TypeScript exits 1 with "no files got indexed".
    Exit { code: Option<i32> },
    /// Exited 0 without writing an index.
    NoIndex,
}

impl IndexFailure {
    /// The stable reason code the coverage report records. Never a message
    /// with paths or indexer output in it: the map must stay byte-identical
    /// across machines.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "indexer_not_found",
            Self::NoProjects => "no_tsconfig",
            Self::Spawn { .. } => "indexer_spawn_failed",
            Self::Exit { .. } => "indexer_failed",
            Self::NoIndex => "no_index_written",
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Exit { code } => *code,
            _ => None,
        }
    }
}

/// The indexer binary for `language`: its override variable when set,
/// otherwise the plain name for a `PATH` lookup.
pub fn binary(language: LanguageKind) -> String {
    let (variable, name) = match language {
        LanguageKind::Python => ("TOLMAP_SCIP_PYTHON", "scip-python"),
        LanguageKind::Go => ("TOLMAP_SCIP_GO", "scip-go"),
        LanguageKind::TypeScript => ("TOLMAP_SCIP_TYPESCRIPT", "scip-typescript"),
    };
    std::env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| name.to_owned())
}

/// The checkout's commit SHA for scip-python's `--project-version`, read
/// with `safe.directory=*` so a clone owned by another uid still answers;
/// `unknown` when there is no commit to read.
pub fn project_version(repo: &Path) -> String {
    Command::new("git")
        .args(["-c", "safe.directory=*", "-C"])
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|sha| sha.trim().to_owned())
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Every tracked `tsconfig.json` directory, deepest first and then by path,
/// as P0's `scip_index.sh` ordered them (`git ls-files`, drop anything under
/// `node_modules`, sort by component count descending then path). A file
/// covered by two projects is read under the first, its nearest config;
/// `scip_ingest` keeps the first document for a path.
pub fn typescript_projects(repo: &Path) -> Result<Vec<String>, IndexFailure> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "safe.directory=*"])
        .args(["ls-files", "-z", "--", "tsconfig.json", "*/tsconfig.json"])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| IndexFailure::Spawn {
            message: format!("git ls-files: {error}"),
        })?;
    if !output.status.success() {
        return Err(IndexFailure::Spawn {
            message: format!("git ls-files exited {:?}", output.status.code()),
        });
    }
    let mut projects = output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| !path.contains("node_modules"))
        .map(|path| {
            let depth = path.split('/').count();
            let directory = path.rsplit_once('/').map_or(".", |(dir, _)| dir);
            (depth, directory.to_owned())
        })
        .collect::<Vec<_>>();
    projects.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    projects.dedup();
    if projects.is_empty() {
        return Err(IndexFailure::NoProjects);
    }
    Ok(projects
        .into_iter()
        .map(|(_, directory)| directory)
        .collect())
}

/// Runs `language`'s indexer on `repo`, writing `output`. The indexer's own
/// stdout/stderr go to `<output>.log`; on failure its last lines are passed
/// to `log` (the build log, never the map). `stage` gets a heartbeat while
/// the child runs so a job's elapsed time keeps moving through a
/// minutes-long type check.
pub fn run(
    repo: &Path,
    language: LanguageKind,
    output: &Path,
    stage: &StageCounter,
    log: &dyn Fn(String),
) -> Result<(), IndexFailure> {
    let program = binary(language);
    let mut command = Command::new(&program);
    command.current_dir(repo);
    match language {
        LanguageKind::Python => {
            let name = repo
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "repository".to_owned());
            // Without --project-version, scip-python runs `git rev-parse
            // HEAD` itself and dies on an unhandled TypeError when that
            // fails: outside a git checkout, or on git's "dubious
            // ownership" refusal when the worker's uid differs from the
            // clone's owner (the hardened worker's case). The version only
            // enters SCIP symbol strings, never a map, so the fallback
            // value cannot change output.
            let version = project_version(repo);
            command
                .args(["index", "--project-name", &name])
                .args(["--project-version", &version])
                .args(["--quiet", "--output"])
                .arg(output);
        }
        LanguageKind::Go => {
            let cache = output.with_extension("gomodcache");
            let _ = fs::create_dir_all(&cache);
            command
                .args(["--quiet", "--output"])
                .arg(output)
                .env("GOPROXY", "off")
                .env("GOTOOLCHAIN", "local")
                .env("GOMODCACHE", &cache);
        }
        LanguageKind::TypeScript => {
            let projects = typescript_projects(repo)?;
            log(format!(
                "scip-typescript: {} tsconfig project(s)",
                projects.len()
            ));
            command
                .args(["index", "--no-progress-bar", "--output"])
                .arg(output)
                .args(&projects);
        }
    }
    let log_path = PathBuf::from(format!("{}.log", output.display()));
    let log_file = File::create(&log_path).map_err(|error| IndexFailure::Spawn {
        message: format!("create {}: {error}", log_path.display()),
    })?;
    let log_clone = log_file.try_clone().map_err(|error| IndexFailure::Spawn {
        message: format!("duplicate {}: {error}", log_path.display()),
    })?;
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_clone))
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(IndexFailure::NotFound { binary: program });
        }
        Err(error) => {
            return Err(IndexFailure::Spawn {
                message: format!("{program}: {error}"),
            })
        }
    };
    let started = Instant::now();
    let mut beats = 0u64;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                // One heartbeat a second: `set` never lowers `done`, it only
                // lets the throttled sink emit, which carries the elapsed
                // time to the job. Polling faster than that keeps the exit
                // noticed promptly without a log line every 250 ms.
                let due = started.elapsed().as_secs();
                if due > beats {
                    beats = due;
                    stage.set(0);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(IndexFailure::Spawn {
                    message: format!("wait for {program}: {error}"),
                });
            }
        }
    };
    let failure = if !status.success() {
        Some(IndexFailure::Exit {
            code: status.code(),
        })
    } else if !output.is_file() {
        Some(IndexFailure::NoIndex)
    } else {
        None
    };
    match failure {
        Some(failure) => {
            log(format!(
                "{program} failed ({}); last output lines:",
                failure.reason()
            ));
            for line in tail(&log_path, 20) {
                log(format!("  {line}"));
            }
            Err(failure)
        }
        None => Ok(()),
    }
}

fn tail(path: &Path, lines: usize) -> Vec<String> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let all = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .collect::<Vec<_>>();
    all[all.len().saturating_sub(lines)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn typescript_projects_are_tracked_deepest_first_without_node_modules() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        for path in [
            "tsconfig.json",
            "packages/b/tsconfig.json",
            "packages/a/tsconfig.json",
            "packages/a/deep/x/tsconfig.json",
            "node_modules/dep/tsconfig.json",
            "untracked/tsconfig.json",
        ] {
            let file = repo.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "{}").unwrap();
        }
        git(repo, &["init", "--quiet"]);
        git(
            repo,
            &[
                "add",
                "-f",
                "tsconfig.json",
                "packages",
                "node_modules/dep/tsconfig.json",
            ],
        );
        assert_eq!(
            typescript_projects(repo).unwrap(),
            vec!["packages/a/deep/x", "packages/a", "packages/b", "."]
        );
    }

    #[test]
    fn project_version_is_the_commit_or_unknown() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(project_version(directory.path()), "unknown");
        git(directory.path(), &["init", "--quiet"]);
        fs::write(directory.path().join("a.py"), "x = 1\n").unwrap();
        git(directory.path(), &["add", "a.py"]);
        git(
            directory.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t.invalid",
                "commit",
                "--quiet",
                "-m",
                "one",
            ],
        );
        let version = project_version(directory.path());
        assert_eq!(version.len(), 40, "{version}");
    }

    #[test]
    fn a_missing_indexer_is_a_recorded_fallback_not_an_error() {
        let failure = IndexFailure::NotFound {
            binary: "scip-python".to_owned(),
        };
        assert_eq!(failure.reason(), "indexer_not_found");
        assert_eq!(IndexFailure::Exit { code: Some(1) }.exit_code(), Some(1));
        assert_eq!(IndexFailure::NoProjects.reason(), "no_tsconfig");
    }
}
