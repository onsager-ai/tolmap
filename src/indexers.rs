//! SCIP indexer invocation (issue #110, P1a) and the dependency installs
//! that may precede it (P1c), and nothing else.
//!
//! Every binary, argument and environment variable tolmap hands an indexer
//! or a package manager lives in this module, so the worker image (P1b) and
//! the install sandbox (P1c) change one place. The indexer methods are the
//! ones P0 measured (`eval/scip_index.sh`, finding 41):
//!
//! - **Only TypeScript workspaces are ever installed, and only inside the
//!   sandbox** (P1c, `docs/SCIP_SANDBOX.md`, owner decisions on #117). An
//!   install runs the indexed repository's own dependency graph, so it
//!   happens only when the caller allows it (`extract::InstallMode`), only
//!   for a workspace with a pnpm or npm lockfile ([`install_plan`]), and
//!   only in nsjail as an unprivileged uid with lifecycle scripts and
//!   pnpmfiles off and network to `registry.npmjs.org` alone ([`install`]).
//!   If the sandbox cannot start, or the install fails, runs past 20
//!   minutes or grows past 20 GB, nothing it wrote is kept and the indexer
//!   runs as if installs were off; the map's `coverage.references` records
//!   which. There is no code path that runs a package manager outside the
//!   sandbox. Python and Go are never installed: finding 41 measured their
//!   installs adding 33 and 2 in-repo pairs for up to 10.5 GB. Go runs with
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
//! The indexers themselves still run unsandboxed, as the worker's uid, with
//! no network restriction; `docs/SCIP_SANDBOX.md` §4.1 puts them in the
//! jail too, and that is a separate change.
//!
//! Binaries come from `PATH`, or from `TOLMAP_SCIP_PYTHON`,
//! `TOLMAP_SCIP_GO` and `TOLMAP_SCIP_TYPESCRIPT`. There is no timeout on
//! indexing: the no-caps policy (finding 38) applies to indexing as to
//! every other stage, and the ETA model carries the expectation instead.
//! The install's 20-minute bound is not an admission cap: exceeding it
//! never fails a job, it only falls back (owner decision on #117).

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

// ---------------------------------------------------------------------------
// Dependency installs (issue #110 P1c, docs/SCIP_SANDBOX.md)
// ---------------------------------------------------------------------------

/// The one host the install sandbox can reach (owner decision on #117:
/// `registry.npmjs.org` only). Matched exactly: no wildcard, no suffix, no IP
/// literal. `registry.yarnpkg.com` is not an alias here because yarn is not
/// installed at all (see [`install_plan`]).
pub const REGISTRY_HOST: &str = "registry.npmjs.org";
const REGISTRY_URL: &str = "https://registry.npmjs.org/";

/// Wall-time bound on one install (owner decision on #117). Not an
/// admission cap: exceeding it falls back to indexing without installs.
pub const INSTALL_TIME_LIMIT: Duration = Duration::from_secs(20 * 60);

/// Disk bound on one install, in bytes newly allocated inside the checkout
/// and the sandbox's home (owner decision on #117: 20 GB). Exceeding it
/// falls back, as the time bound does.
pub const INSTALL_DISK_BUDGET: u64 = 20_000_000_000;

/// The package managers an install may run. Both come from the image
/// (`/opt/node/bin`), never from the repository: `packageManager` and
/// pnpm's own version management are switched off (`pm_on_fail=ignore`),
/// so a repository cannot choose which package-manager build executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageManager {
    Pnpm,
    Npm,
}

impl PackageManager {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pnpm => "pnpm",
            Self::Npm => "npm",
        }
    }

    /// The install command, run inside the jail. The same switches are also
    /// set in the jail's environment (`jail_env`), because repository
    /// configuration (`.npmrc`, `pnpm-workspace.yaml`) ranks below both.
    /// `--frozen-lockfile`/`ci`: exactly the lockfile, never a resolution the
    /// repository did not commit. `--ignore-scripts`: no lifecycle script
    /// of the repository or any dependency. `--ignore-pnpmfile`: pnpm runs
    /// `.pnpmfile` hooks even with scripts off (docs/SCIP_SANDBOX.md §2).
    fn command(self) -> Vec<String> {
        let words: &[&str] = match self {
            Self::Pnpm => &[
                "pnpm",
                "install",
                "--frozen-lockfile",
                "--ignore-scripts",
                "--ignore-pnpmfile",
                "--reporter=append-only",
            ],
            Self::Npm => &[
                "npm",
                "ci",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--registry=https://registry.npmjs.org/",
            ],
        };
        words.iter().map(|word| (*word).to_owned()).collect()
    }
}

/// What the install policy decides for a checkout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallPlan {
    Install(PackageManager),
    Skip {
        reason: &'static str,
        manager: Option<PackageManager>,
    },
}

/// The install policy (docs/SCIP_SANDBOX.md §4.1 point 1), from files at
/// the repository root alone:
///
/// - a workspace, because that is where a no-install index loses edges
///   (finding 41: dify's tsconfigs `extends` an uninstalled workspace
///   package and index nothing; n8n keeps 85 of 11,690 cross-package
///   pairs). A single package gains only external types, which tolmap
///   drops. pnpm's workspace is `pnpm-workspace.yaml`; npm's is
///   `package.json` `workspaces`;
/// - a lockfile the image's package manager installs frozen: pnpm's or
///   npm's. yarn is not installed at all: berry's `yarnPath` and classic's
///   `yarn-path` make yarn execute a file the repository commits, before any
///   flag of ours applies. bun is not in the image;
/// - no `node_modules` at the root already. A fresh clone never has one; a
///   developer's checkout with one keeps it, and it is not ours to replace
///   or, on a fallback, to delete.
///
/// Entries are read with `symlink_metadata` and a symlinked manifest or
/// lockfile is refused: the job service evaluates this as root on a
/// checkout the repository controls, and a committed
/// `package.json -> /etc/shadow` must not make it read outside the checkout.
pub fn install_plan(repo: &Path) -> InstallPlan {
    // None: absent. Some(true): a regular file. Some(false): anything else.
    let kind = |name: &str| {
        fs::symlink_metadata(repo.join(name))
            .ok()
            .map(|meta| meta.file_type().is_file())
    };
    let skip = |reason, manager| InstallPlan::Skip { reason, manager };
    match kind("package.json") {
        None => return skip("no_package_json", None),
        Some(false) => return skip("unsafe_manifest", None),
        Some(true) => {}
    }
    if fs::symlink_metadata(repo.join("node_modules")).is_ok() {
        return skip("node_modules_present", None);
    }
    let pnpm = kind("pnpm-lock.yaml");
    let npm = kind("package-lock.json").or_else(|| kind("npm-shrinkwrap.json"));
    if pnpm == Some(false) || npm == Some(false) {
        return skip("unsafe_manifest", None);
    }
    if pnpm == Some(true) {
        return if kind("pnpm-workspace.yaml") == Some(true) {
            InstallPlan::Install(PackageManager::Pnpm)
        } else {
            skip("not_a_workspace", Some(PackageManager::Pnpm))
        };
    }
    if npm == Some(true) {
        return if manifest_declares_workspaces(&repo.join("package.json")) {
            InstallPlan::Install(PackageManager::Npm)
        } else {
            skip("not_a_workspace", Some(PackageManager::Npm))
        };
    }
    if ["yarn.lock", "bun.lock", "bun.lockb", ".yarnrc.yml"]
        .iter()
        .any(|name| kind(name).is_some())
    {
        return skip("unsupported_lockfile", None);
    }
    skip("no_lockfile", None)
}

/// `package.json` `workspaces` as npm reads it: a non-empty array, or an
/// object with a non-empty `packages` array. Read with `O_NOFOLLOW` and at
/// most 4 MiB, for the reason [`install_plan`] gives.
fn manifest_declares_workspaces(path: &Path) -> bool {
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(file) = options.open(path) else {
        return false;
    };
    let mut text = Vec::new();
    if file.take(4 << 20).read_to_end(&mut text).is_err() {
        return false;
    }
    let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return false;
    };
    let non_empty = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_array)
            .is_some_and(|entries| !entries.is_empty())
    };
    let workspaces = manifest.get("workspaces");
    non_empty(workspaces) || non_empty(workspaces.and_then(|value| value.get("packages")))
}

fn install_coverage(
    status: &str,
    reason: &str,
    manager: Option<PackageManager>,
) -> crate::schema::InstallCoverage {
    crate::schema::InstallCoverage {
        status: status.to_owned(),
        reason: reason.to_owned(),
        manager: manager.map(|manager| manager.as_str().to_owned()),
    }
}

/// A policy decision not to install.
pub fn skipped(reason: &str, manager: Option<PackageManager>) -> crate::schema::InstallCoverage {
    install_coverage("skipped", reason, manager)
}

/// An install that was wanted and did not happen: the indexer runs without
/// it, exactly as with installs off.
pub fn fell_back(reason: &str, manager: Option<PackageManager>) -> crate::schema::InstallCoverage {
    install_coverage("fell_back", reason, manager)
}

/// Where and as whom the sandbox runs, and its bounds. Operator settings,
/// never read from the repository.
#[derive(Clone, Debug)]
pub struct InstallSettings {
    /// `TOLMAP_NSJAIL`, default `nsjail` on `PATH`.
    pub nsjail: String,
    /// The Node.js prefix mounted read-only in the jail, whose `bin` holds
    /// `node`, `npm` and `pnpm`: `TOLMAP_INSTALL_NODE_PREFIX`, default the
    /// prefix of `node` on `PATH` (`/opt/node` in the runtime image).
    pub node_prefix: Option<PathBuf>,
    /// The unprivileged uid/gid the install runs as: the worker's.
    pub uid: u32,
    pub gid: u32,
    /// `TOLMAP_INSTALL_TIME_LIMIT_S`, default [`INSTALL_TIME_LIMIT`].
    pub time_limit: Duration,
    /// `TOLMAP_INSTALL_DISK_BUDGET_BYTES`, default [`INSTALL_DISK_BUDGET`].
    pub disk_budget: u64,
    /// `TOLMAP_INSTALL_MEMORY_MAX`, in bytes: a memory cgroup (and a 4,096
    /// pids limit) on the jail. Off by default: whether nsjail's cgroups work
    /// on the production host's cgroup v1 is unverified (#117), and a
    /// cgroup that cannot be created would make every install fall back.
    pub memory_max: Option<u64>,
}

impl InstallSettings {
    pub fn from_env(uid: u32, gid: u32) -> Self {
        let number = |key: &str| {
            std::env::var(key)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
        };
        Self {
            nsjail: std::env::var("TOLMAP_NSJAIL")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "nsjail".to_owned()),
            node_prefix: std::env::var_os("TOLMAP_INSTALL_NODE_PREFIX")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            uid,
            gid,
            // Seconds, fractions allowed: CI proves the timeout fallback
            // with a bound no install can meet.
            time_limit: std::env::var("TOLMAP_INSTALL_TIME_LIMIT_S")
                .ok()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                .map(Duration::from_secs_f64)
                .unwrap_or(INSTALL_TIME_LIMIT),
            disk_budget: number("TOLMAP_INSTALL_DISK_BUDGET_BYTES").unwrap_or(INSTALL_DISK_BUDGET),
            memory_max: number("TOLMAP_INSTALL_MEMORY_MAX"),
        }
    }

    /// For `tolmap build --install sandbox` and `tolmap sandbox-exec`: the
    /// uid/gid in `TOLMAP_WORKER_UID`/`TOLMAP_WORKER_GID` when set (the
    /// runtime image sets both), otherwise the owner of `repo`, so the
    /// install can write `node_modules` into a checkout the caller owns.
    /// A root-owned checkout without those variables resolves to uid 0,
    /// which the sandbox refuses.
    pub fn for_repository(repo: &Path) -> Self {
        #[cfg(unix)]
        let owner = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(repo)
                .map(|meta| (meta.uid(), meta.gid()))
                .unwrap_or((0, 0))
        };
        #[cfg(not(unix))]
        let owner = {
            let _ = repo;
            (0, 0)
        };
        let id = |key: &str, fallback: u32| {
            std::env::var(key)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(fallback)
        };
        Self::from_env(
            id("TOLMAP_WORKER_UID", owner.0),
            id("TOLMAP_WORKER_GID", owner.1),
        )
    }
}

/// Installs `repo`'s TypeScript workspace dependencies in the sandbox, if
/// the policy wants to, and says what happened. `scratch` must not exist
/// inside `repo` and is removed afterwards; `tick` is called about once a
/// second while the install runs; `cancelled` is polled and ends the
/// install when it turns true. `log` gets the sandbox's own diagnostics
/// and, on failure, the package manager's last lines: never the map.
///
/// Fails safe (owner decision on #117): anything short of a finished
/// install inside its bounds returns a `fell_back` coverage, having removed
/// every `node_modules` directory the attempt created, so the indexer runs
/// on the checkout exactly as with installs off. The package manager only
/// ever runs inside nsjail; when the jail cannot be set up or its
/// self-test fails, nothing runs at all.
pub fn install(
    repo: &Path,
    scratch: &Path,
    settings: &InstallSettings,
    tick: &dyn Fn(),
    cancelled: &dyn Fn() -> bool,
    log: &dyn Fn(String),
) -> crate::schema::InstallCoverage {
    let manager = match install_plan(repo) {
        InstallPlan::Skip { reason, manager } => return skipped(reason, manager),
        InstallPlan::Install(manager) => manager,
    };
    #[cfg(unix)]
    let coverage = jail::install(repo, scratch, settings, manager, tick, cancelled, log);
    #[cfg(not(unix))]
    let coverage = {
        let _ = (settings, tick, cancelled);
        log("install sandbox unavailable: nsjail needs Linux".to_owned());
        fell_back("sandbox_unavailable", Some(manager))
    };
    let _ = fs::remove_dir_all(scratch);
    coverage
}

/// Runs `command` in the install sandbox with `repo` as its working
/// directory: same
/// mounts, uid, environment and egress proxy as an install, after the same
/// self-test. For `tolmap sandbox-exec`, which CI and operators use to show
/// what the jail can and cannot reach. Returns the command's exit status,
/// or 125 when the sandbox cannot start.
pub fn sandbox_exec(
    repo: &Path,
    scratch: &Path,
    settings: &InstallSettings,
    command: &[String],
    log: &dyn Fn(String),
) -> i32 {
    #[cfg(unix)]
    let code = jail::exec(repo, scratch, settings, command, log);
    #[cfg(not(unix))]
    let code = {
        let _ = (repo, settings, command);
        log("sandbox unavailable: nsjail needs Linux".to_owned());
        125
    };
    let _ = fs::remove_dir_all(scratch);
    code
}

/// The jail: nsjail started by root, dropping straight to the worker's
/// uid, with a tmpfs root, read-only system trees, the checkout at its own path
/// and a network namespace whose only way out is the egress proxy.
#[cfg(unix)]
mod jail {
    use std::ffi::OsString;
    use std::fs::{self, File};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    use super::egress;
    use super::{fell_back, install_coverage, InstallSettings, PackageManager, REGISTRY_HOST};

    const JAIL_HOME: &str = "/home/tolmap";
    const JAIL_RUN: &str = "/run/tolmap";
    const PROXY_PORT: u16 = 3128;

    /// Inside the jail, `socat` turns the proxy's Unix socket (bind-mounted
    /// from the trusted side) into `127.0.0.1:3128`, the only listening
    /// address in a network namespace that has nothing but loopback. It waits
    /// until the bridge accepts before handing over, so the package
    /// manager's first request cannot race it. Exit 97 is "the bridge never
    /// came up", a setup failure rather than the install's.
    const BRIDGE: &str =
        "socat TCP-LISTEN:3128,bind=127.0.0.1,reuseaddr,fork UNIX-CONNECT:/run/tolmap/proxy.sock &
tries=0
until socat -u OPEN:/dev/null TCP:127.0.0.1:3128 2>/dev/null; do
  tries=$((tries + 1))
  if [ \"$tries\" -gt 200 ]; then
    echo 'tolmap sandbox: the egress bridge did not start' >&2
    exit 97
  fi
  sleep 0.05
done
exec \"$@\"
";

    /// The self-test run inside the jail before every install and every
    /// `sandbox-exec` (docs/SCIP_SANDBOX.md §4.1 point 3): not root, none of
    /// the service's paths visible, no credential-like variable, no direct
    /// egress, and the proxy refusing a host off the allowlist. Any failure
    /// means no install at all.
    const PROBE_JS: &str = r#"
const net = require('net');
const fs = require('fs');
let finished = false;
const done = (ok, message) => {
  if (finished) return;
  finished = true;
  console.log('sandbox self-test: ' + message);
  process.exit(ok ? 0 : 1);
};
if (process.getuid() === 0 || process.getgid() === 0) done(false, 'running as root');
for (const path of ['/app', '/root', '/opt/go', '/var/run/docker.sock', '/.fly']) {
  if (fs.existsSync(path)) done(false, path + ' is visible');
}
// The checkout is mounted at its own path; its parent must hold nothing
// else: not the job's output or names cache, not another job.
const parent = require('path').dirname(process.cwd());
const siblings = parent === '/' ? [] : fs.readdirSync(parent);
if (siblings.length > 1) done(false, 'the checkout has visible siblings: ' + siblings.join(', '));
for (const name of Object.keys(process.env)) {
  if (/KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|^FLY_|^AWS_|^OPENROUTER/i.test(name)) {
    done(false, 'credential-like variable ' + name);
  }
}
let asked = false;
const askProxy = () => {
  if (asked) return;
  asked = true;
  const proxy = net.connect({ host: '127.0.0.1', port: 3128 }, () => {
    proxy.write('CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n');
  });
  let reply = '';
  proxy.setTimeout(5000);
  proxy.on('data', (chunk) => {
    reply += chunk;
    if (!reply.includes('\r\n')) return;
    const status = reply.split('\r\n')[0];
    const refused = /^HTTP\/1\.[01] 403 /.test(status);
    done(refused, refused ? 'ok' : 'the proxy did not refuse example.com: ' + status);
  });
  proxy.on('timeout', () => done(false, 'the egress proxy did not answer'));
  proxy.on('error', (error) => done(false, 'the egress proxy is unreachable: ' + error.message));
};
const direct = net.connect({ host: '1.1.1.1', port: 443 });
direct.setTimeout(3000);
direct.on('connect', () => done(false, 'direct egress to 1.1.1.1:443 connected'));
direct.on('timeout', () => { direct.destroy(); askProxy(); });
direct.on('error', () => askProxy());
"#;

    /// One top-level entry of the jail's root: a read-only bind of a host
    /// directory, or a symlink (a merged-/usr `/bin -> usr/bin`).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) enum RootEntry {
        Dir(PathBuf),
        Link { target: PathBuf, path: PathBuf },
    }

    pub(super) struct JailLayout<'a> {
        pub repo: &'a Path,
        pub git: Option<&'a Path>,
        pub home: &'a Path,
        pub run: &'a Path,
        pub node_prefix: &'a Path,
        pub root_entries: Vec<RootEntry>,
        pub uid: u32,
        pub gid: u32,
        pub time_limit: Duration,
        pub memory_max: Option<u64>,
        pub cgroup_v2: bool,
    }

    /// The jail's environment, and all of it: nsjail starts from an empty
    /// one. Every proxy variable points at the in-jail bridge; the registry
    /// and the execution switches are repeated as npm and pnpm settings
    /// because environment settings outrank a repository's `.npmrc` and
    /// `pnpm-workspace.yaml`. `pm_on_fail=ignore` stops pnpm from
    /// downloading and running the pnpm version a repository's
    /// `packageManager` names; `runtime_on_fail=ignore` stops it from
    /// downloading a Node.js or Python runtime a manifest asks for.
    pub(super) fn jail_env(node_prefix: &Path) -> Vec<(String, String)> {
        let proxy = format!("http://127.0.0.1:{PROXY_PORT}");
        let home = JAIL_HOME;
        let mut env = vec![
            ("HOME".to_owned(), home.to_owned()),
            ("TMPDIR".to_owned(), format!("{home}/tmp")),
            ("XDG_CONFIG_HOME".to_owned(), format!("{home}/.config")),
            ("XDG_CACHE_HOME".to_owned(), format!("{home}/.cache")),
            ("XDG_DATA_HOME".to_owned(), format!("{home}/.local/share")),
            ("XDG_STATE_HOME".to_owned(), format!("{home}/.local/state")),
            (
                "PATH".to_owned(),
                format!("{}/bin:/usr/local/bin:/usr/bin:/bin", node_prefix.display()),
            ),
        ];
        let fixed: &[(&str, &str)] = &[
            ("LANG", "C.UTF-8"),
            ("CI", "true"),
            ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
            ("COREPACK_ENABLE_NETWORK", "0"),
            ("COREPACK_ENABLE_STRICT", "0"),
            ("NO_PROXY", ""),
            ("no_proxy", ""),
            ("npm_config_audit", "false"),
            ("npm_config_fund", "false"),
            ("pnpm_config_ignore_pnpmfile", "true"),
            ("pnpm_config_pm_on_fail", "ignore"),
            ("pnpm_config_runtime_on_fail", "ignore"),
            ("pnpm_config_strict_dep_builds", "false"),
            ("pnpm_config_confirm_modules_purge", "false"),
        ];
        env.extend(
            fixed
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
            env.push((key.to_owned(), proxy.clone()));
        }
        for prefix in ["npm", "pnpm"] {
            for (name, value) in [
                ("registry", super::REGISTRY_URL.to_owned()),
                ("https_proxy", proxy.clone()),
                ("proxy", proxy.clone()),
                ("noproxy", String::new()),
                ("ignore_scripts", "true".to_owned()),
                ("update_notifier", "false".to_owned()),
            ] {
                env.push((format!("{prefix}_config_{name}"), value));
            }
        }
        env.push(("npm_config_cache".to_owned(), format!("{home}/.npm")));
        env.push((
            "pnpm_config_store_dir".to_owned(),
            format!("{home}/.pnpm-store"),
        ));
        env
    }

    /// nsjail's arguments up to, not including, `--`. A pure function of
    /// the layout so the policy it encodes is unit-tested.
    pub(super) fn jail_args(layout: &JailLayout) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        let mut push = |values: &[&str]| args.extend(values.iter().map(OsString::from));
        // --disable_clone_newuser: root starts the jail and setresuid()s the
        // child straight to the worker's uid, with every capability dropped
        // and no_new_privs set. No user namespace is created, so this
        // neither needs unprivileged user namespaces (unverified on the
        // production host, #117) nor exposes their kernel surface. A new
        // network namespace is nsjail's default and is kept: loopback only.
        push(&[
            "--mode",
            "o",
            "--quiet",
            "--hostname",
            "tolmap-install",
            "--disable_clone_newuser",
        ]);
        let uid = format!("{0}:{0}:1", layout.uid);
        let gid = format!("{0}:{0}:1", layout.gid);
        push(&["--user", uid.as_str(), "--group", gid.as_str()]);
        // nsjail's own wall clock is a backstop only: `supervise` enforces
        // the install bound and records it. nsjail's default rlimits (4 GiB
        // address space, 1 MiB files, 32 fds, 600 s CPU) would kill any real
        // install; V8 reserves far more address space than it uses, so
        // memory is the cgroup's job, not RLIMIT_AS's.
        let backstop = (layout.time_limit.as_secs() + 60).to_string();
        push(&["--time_limit", backstop.as_str()]);
        push(&[
            "--rlimit_as",
            "inf",
            "--rlimit_cpu",
            "inf",
            "--rlimit_fsize",
            "inf",
            "--rlimit_nofile",
            "max",
        ]);
        let cwd = layout.repo.to_string_lossy().into_owned();
        push(&["--cwd", cwd.as_str()]);
        // A tmpfs root (no --chroot) holding only what is bound below.
        // Nothing of the service's is there: not /app, not the store or the
        // stored maps, not the other jobs' directories, not /.fly.
        for entry in &layout.root_entries {
            match entry {
                RootEntry::Dir(path) => {
                    let path = path.to_string_lossy().into_owned();
                    push(&["-R", path.as_str()]);
                }
                RootEntry::Link { target, path } => {
                    let link = format!("{}:{}", target.display(), path.display());
                    push(&["-s", link.as_str()]);
                }
            }
        }
        push(&["-R", "/usr", "-R", "/etc"]);
        if !layout.node_prefix.starts_with("/usr") {
            let prefix = layout.node_prefix.to_string_lossy().into_owned();
            push(&["-R", prefix.as_str()]);
        }
        push(&[
            "-B",
            "/dev/null",
            "-R",
            "/dev/zero",
            "-R",
            "/dev/random",
            "-R",
            "/dev/urandom",
            "-T",
            "/tmp",
        ]);
        // The checkout at its own path, so any absolute path the package
        // manager writes into it (a symlink, a shim) still resolves once
        // the worker reads it outside the jail. Its parents are empty
        // directories on the tmpfs root: the job's other files and the
        // other jobs are not there.
        let work = layout.repo.to_string_lossy().into_owned();
        push(&["-B", work.as_str()]);
        // The checkout's .git read-only on top of the read-write checkout.
        // After the install, the worker runs `git` in this checkout as the
        // same uid; a writable .git/config (core.fsmonitor, hooksPath) would
        // turn anything that runs in the jail into code the worker runs
        // outside it, with the network.
        if let Some(git) = layout.git {
            let git = git.to_string_lossy().into_owned();
            push(&["-R", git.as_str()]);
        }
        let home = format!("{}:{JAIL_HOME}", layout.home.display());
        let run = format!("{}:{JAIL_RUN}", layout.run.display());
        push(&["-B", home.as_str(), "-B", run.as_str()]);
        for (key, value) in jail_env(layout.node_prefix) {
            let variable = format!("{key}={value}");
            push(&["-E", variable.as_str()]);
        }
        if let Some(bytes) = layout.memory_max {
            let bytes = bytes.to_string();
            push(&[
                "--cgroup_mem_max",
                bytes.as_str(),
                "--cgroup_pids_max",
                "4096",
            ]);
            if layout.cgroup_v2 {
                push(&["--use_cgroupv2"]);
            }
        }
        args
    }

    /// `name` on `PATH`, or `name` itself when it is a path.
    fn find_program(name: &str) -> Option<PathBuf> {
        if name.contains('/') {
            let path = PathBuf::from(name);
            return path.is_file().then_some(path);
        }
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file())
        })
    }

    /// The host's top-level library and binary directories, as merged-/usr
    /// symlinks where they are symlinks.
    fn root_entries() -> Vec<RootEntry> {
        ["/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32"]
            .iter()
            .filter_map(|path| {
                let path = PathBuf::from(path);
                let meta = fs::symlink_metadata(&path).ok()?;
                if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).ok()?;
                    Some(RootEntry::Link { target, path })
                } else if meta.is_dir() {
                    Some(RootEntry::Dir(path))
                } else {
                    None
                }
            })
            .collect()
    }

    fn chown(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
        std::os::unix::fs::chown(path, Some(uid), Some(gid))
            .map_err(|error| format!("chown {}: {error}", path.display()))
    }

    fn private_dir(path: &Path, mode: u32) -> Result<(), String> {
        fs::create_dir_all(path).map_err(|error| format!("create {}: {error}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| format!("chmod {}: {error}", path.display()))
    }

    /// A prepared jail: scratch directories made, proxy listening, nsjail's
    /// arguments built. Nothing has run yet.
    pub(super) struct Jail {
        nsjail: PathBuf,
        args: Vec<OsString>,
        proxy: egress::Proxy,
        home: PathBuf,
        scratch: PathBuf,
    }

    impl Jail {
        pub(super) fn prepare(
            repo: &Path,
            scratch: &Path,
            settings: &InstallSettings,
            manager: Option<PackageManager>,
        ) -> Result<Self, String> {
            let euid = unsafe { libc::geteuid() };
            if euid != 0 {
                return Err(format!(
                    "starting nsjail without user namespaces needs root; this process is uid {euid}"
                ));
            }
            if settings.uid == 0 || settings.gid == 0 {
                return Err("refusing to run an install as uid or gid 0".to_owned());
            }
            let nsjail = find_program(&settings.nsjail)
                .ok_or_else(|| format!("nsjail not found ({})", settings.nsjail))?;
            let node_prefix = match &settings.node_prefix {
                Some(prefix) => prefix.clone(),
                None => find_program("node")
                    .and_then(|node| node.canonicalize().ok())
                    .and_then(|node| Some(node.parent()?.parent()?.to_path_buf()))
                    .ok_or("node not found on PATH")?,
            };
            if !node_prefix.join("bin/node").is_file() {
                return Err(format!("no node in {}/bin", node_prefix.display()));
            }
            if let Some(manager) = manager {
                if !node_prefix.join("bin").join(manager.as_str()).exists() {
                    return Err(format!(
                        "no {} in {}/bin",
                        manager.as_str(),
                        node_prefix.display()
                    ));
                }
            }
            let socat = find_program("socat").ok_or("socat not found on PATH")?;
            if !socat.starts_with("/usr/") {
                return Err(format!(
                    "socat is at {}; the jail mounts only /usr of the host's system trees",
                    socat.display()
                ));
            }
            let repo_meta = fs::symlink_metadata(repo)
                .map_err(|error| format!("{}: {error}", repo.display()))?;
            if !repo_meta.is_dir() {
                return Err(format!("{} is not a directory", repo.display()));
            }
            let git = repo.join(".git");
            let git = match fs::symlink_metadata(&git) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err("the checkout's .git is a symlink".to_owned())
                }
                Ok(_) => Some(git),
                Err(_) => None,
            };
            // nsjail splits mount specifications on ':'.
            for path in [repo, scratch, node_prefix.as_path()] {
                if path.to_string_lossy().contains(':') {
                    return Err(format!("{} contains ':'", path.display()));
                }
            }

            // The scratch tree is root's; only `home` belongs to the jail's
            // uid. The proxy socket's directory stays root-owned so nothing
            // in the jail can replace the socket.
            private_dir(scratch, 0o700)?;
            let home = scratch.join("home");
            private_dir(&home, 0o700)?;
            chown(&home, settings.uid, settings.gid)?;
            let tmp = home.join("tmp");
            private_dir(&tmp, 0o700)?;
            chown(&tmp, settings.uid, settings.gid)?;
            let run = scratch.join("run");
            private_dir(&run, 0o755)?;
            let socket = run.join("proxy.sock");
            let _ = fs::remove_file(&socket);
            let proxy = egress::Proxy::start(&socket, &[REGISTRY_HOST])
                .map_err(|error| format!("egress proxy: {error}"))?;
            chown(&socket, settings.uid, settings.gid)?;
            fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("chmod {}: {error}", socket.display()))?;

            let cgroup_v2 = Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
            if settings.memory_max.is_some() && !cgroup_v2 {
                // nsjail's cgroup v1 support needs its parent groups to exist.
                for controller in ["memory", "pids"] {
                    let _ = fs::create_dir_all(format!("/sys/fs/cgroup/{controller}/NSJAIL"));
                }
            }
            let args = jail_args(&JailLayout {
                repo,
                git: git.as_deref(),
                home: &home,
                run: &run,
                node_prefix: &node_prefix,
                root_entries: root_entries(),
                uid: settings.uid,
                gid: settings.gid,
                time_limit: settings.time_limit,
                memory_max: settings.memory_max,
                cgroup_v2,
            });
            Ok(Self {
                nsjail,
                args,
                proxy,
                home,
                scratch: scratch.to_path_buf(),
            })
        }

        fn command(&self, program: &[String]) -> Command {
            let mut command = Command::new(&self.nsjail);
            command
                .args(&self.args)
                .arg("--")
                .args(["/bin/sh", "-c", BRIDGE, "tolmap-bridge"])
                .args(program)
                .env_clear()
                .stdin(Stdio::null())
                // Its own process group, so nothing aimed at the service's
                // group reaches it and a kill can target it alone.
                .process_group(0);
            command
        }

        fn log_file(&self, name: &str) -> Result<(File, File, PathBuf), String> {
            let path = self.scratch.join(name);
            let file = File::create(&path)
                .map_err(|error| format!("create {}: {error}", path.display()))?;
            let clone = file
                .try_clone()
                .map_err(|error| format!("duplicate {}: {error}", path.display()))?;
            Ok((file, clone, path))
        }

        pub(super) fn self_test(&self, log: &dyn Fn(String)) -> Result<(), String> {
            let (stdout, stderr, path) = self.log_file("self-test.log")?;
            let program = ["node".to_owned(), "-e".to_owned(), PROBE_JS.to_owned()];
            let mut child = self
                .command(&program)
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .map_err(|error| format!("start {}: {error}", self.nsjail.display()))?;
            let ended = supervise(
                &mut child,
                &Bounds {
                    time_limit: Duration::from_secs(60),
                    disk: None,
                },
                &|| {},
                &|| false,
            );
            match ended {
                Ended::Exited(status) if status.success() => Ok(()),
                other => {
                    for line in super::tail(&path, 20) {
                        log(format!("  {}", printable(&line)));
                    }
                    Err(format!("the sandbox self-test failed ({other:?})"))
                }
            }
        }

        /// Stops the proxy and reports what it saw, and frees the jail's home
        /// (npm's cache, pnpm's store): after the install nothing reads it.
        pub(super) fn finish(mut self, log: &dyn Fn(String)) {
            let stats = self.proxy.stop();
            let refused = stats
                .refused
                .iter()
                .map(|(target, count)| format!("{target} x{count}"))
                .collect::<Vec<_>>();
            log(format!(
                "egress: {} tunnel(s) to {REGISTRY_HOST}, {:.1} MB in, {:.1} MB out; refused: {}",
                stats.allowed,
                stats.bytes_in as f64 / 1e6,
                stats.bytes_out as f64 / 1e6,
                if refused.is_empty() {
                    "none".to_owned()
                } else {
                    refused.join(", ")
                },
            ));
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    /// The bounds `supervise` enforces. `disk` is the tree to measure and
    /// its budget, over a baseline taken before the run.
    struct Bounds<'a> {
        time_limit: Duration,
        disk: Option<DiskBound<'a>>,
    }

    struct DiskBound<'a> {
        roots: [&'a Path; 2],
        baseline: u64,
        budget: u64,
    }

    #[derive(Debug)]
    enum Ended {
        Exited(ExitStatus),
        TimedOut,
        OverBudget(u64),
        Cancelled,
        WaitFailed(String),
    }

    /// Waits for the jail, enforcing its bounds. Disk is measured every five
    /// seconds, or ten times as long as the last measurement took, whichever
    /// is longer, so walking a large `node_modules` never takes more than a
    /// tenth of the install's time.
    fn supervise(
        child: &mut Child,
        bounds: &Bounds,
        tick: &dyn Fn(),
        cancelled: &dyn Fn() -> bool,
    ) -> Ended {
        let started = Instant::now();
        let mut next_disk = started + Duration::from_secs(5);
        let mut beats = 0;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ended::Exited(status),
                Ok(None) => {}
                Err(error) => {
                    terminate(child);
                    return Ended::WaitFailed(error.to_string());
                }
            }
            if cancelled() {
                terminate(child);
                return Ended::Cancelled;
            }
            if started.elapsed() >= bounds.time_limit {
                terminate(child);
                return Ended::TimedOut;
            }
            if let Some(disk) = &bounds.disk {
                if Instant::now() >= next_disk {
                    let measuring = Instant::now();
                    let used = disk_usage(&disk.roots).saturating_sub(disk.baseline);
                    if used > disk.budget {
                        terminate(child);
                        return Ended::OverBudget(used);
                    }
                    next_disk =
                        Instant::now() + (measuring.elapsed() * 10).max(Duration::from_secs(5));
                }
            }
            let seconds = started.elapsed().as_secs();
            if seconds > beats {
                beats = seconds;
                tick();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// nsjail answers SIGTERM by SIGKILLing everything in the jail and
    /// exiting; SIGKILL follows if it has not within five seconds, and the
    /// jail's init then dies with it (nsjail sets PR_SET_PDEATHSIG).
    fn terminate(child: &mut Child) {
        if let Ok(pid) = i32::try_from(child.id()) {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Bytes allocated under `roots`, each inode once (pnpm hardlinks its
    /// store into `node_modules`), never following a symlink.
    pub(super) fn disk_usage(roots: &[&Path]) -> u64 {
        let mut seen = std::collections::HashSet::new();
        let mut total = 0u64;
        let mut stack = roots
            .iter()
            .map(|root| root.to_path_buf())
            .collect::<Vec<_>>();
        while let Some(path) = stack.pop() {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
            total = total.saturating_add(meta.blocks().saturating_mul(512));
            if meta.is_dir() {
                if let Ok(entries) = fs::read_dir(&path) {
                    stack.extend(entries.flatten().map(|entry| entry.path()));
                }
            }
        }
        total
    }

    /// Every `node_modules` directory under `repo`, outside `.git`, without
    /// following symlinks.
    pub(super) fn node_modules_dirs(repo: &Path) -> std::collections::BTreeSet<PathBuf> {
        let mut found = std::collections::BTreeSet::new();
        let mut stack = vec![repo.to_path_buf()];
        while let Some(directory) = stack.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                // DirEntry::file_type does not follow symlinks.
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let name = entry.file_name();
                if directory == repo && name == ".git" {
                    continue;
                }
                if name == "node_modules" {
                    found.insert(entry.path());
                } else {
                    stack.push(entry.path());
                }
            }
        }
        found
    }

    /// Removes every `node_modules` the install created, so a fallback
    /// indexes exactly the checkout it would have without installs (a
    /// partial `node_modules` would give scip-typescript half a dependency
    /// graph). `remove_dir_all` does not follow symlinks.
    pub(super) fn remove_new_node_modules(
        repo: &Path,
        before: &std::collections::BTreeSet<PathBuf>,
    ) -> usize {
        node_modules_dirs(repo)
            .into_iter()
            .filter(|path| !before.contains(path))
            .filter(|path| fs::remove_dir_all(path).is_ok())
            .count()
    }

    /// Package-manager output is the repository's text: control characters
    /// are dropped and lines cut before they reach a log.
    fn printable(line: &str) -> String {
        line.chars()
            .filter(|character| !character.is_control())
            .take(300)
            .collect()
    }

    pub(super) fn install(
        repo: &Path,
        scratch: &Path,
        settings: &InstallSettings,
        manager: PackageManager,
        tick: &dyn Fn(),
        cancelled: &dyn Fn() -> bool,
        log: &dyn Fn(String),
    ) -> crate::schema::InstallCoverage {
        let fallback = |reason: &str| fell_back(reason, Some(manager));
        let jail = match Jail::prepare(repo, scratch, settings, Some(manager)) {
            Ok(jail) => jail,
            Err(why) => {
                log(format!("install sandbox unavailable: {why}"));
                return fallback("sandbox_unavailable");
            }
        };
        if let Err(why) = jail.self_test(log) {
            log(format!("install sandbox unavailable: {why}"));
            jail.finish(log);
            return fallback("sandbox_unavailable");
        }
        let existing = node_modules_dirs(repo);
        let baseline = disk_usage(&[repo]);
        let (stdout, stderr, log_path) = match jail.log_file("install.log") {
            Ok(files) => files,
            Err(why) => {
                log(format!("install sandbox unavailable: {why}"));
                jail.finish(log);
                return fallback("sandbox_unavailable");
            }
        };
        let started = Instant::now();
        let mut child = match jail
            .command(&manager.command())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                log(format!(
                    "install sandbox unavailable: start nsjail: {error}"
                ));
                jail.finish(log);
                return fallback("sandbox_unavailable");
            }
        };
        let home = jail.home.clone();
        let ended = supervise(
            &mut child,
            &Bounds {
                time_limit: settings.time_limit,
                disk: Some(DiskBound {
                    roots: [repo, home.as_path()],
                    baseline,
                    budget: settings.disk_budget,
                }),
            },
            tick,
            cancelled,
        );
        let seconds = started.elapsed().as_secs_f64();
        let added = disk_usage(&[repo, home.as_path()]).saturating_sub(baseline);
        let coverage = match ended {
            // The budget also holds for what a finished install leaves:
            // one that outgrew it between measurements does not count.
            Ended::Exited(status) if status.success() && added > settings.disk_budget => {
                fallback("install_disk_budget")
            }
            Ended::Exited(status) if status.success() => {
                install_coverage("installed", "installed", Some(manager))
            }
            Ended::Exited(status) => {
                log(format!(
                    "{} in the sandbox exited with {status}; last output lines:",
                    manager.as_str()
                ));
                for line in super::tail(&log_path, 30) {
                    log(format!("  {}", printable(&line)));
                }
                fallback("install_failed")
            }
            Ended::TimedOut => fallback("install_timeout"),
            Ended::OverBudget(bytes) => {
                log(format!(
                    "the install passed its disk budget: {:.1} GB written",
                    bytes as f64 / 1e9
                ));
                fallback("install_disk_budget")
            }
            Ended::Cancelled => fallback("cancelled"),
            Ended::WaitFailed(error) => {
                log(format!("waiting for the install failed: {error}"));
                fallback("install_failed")
            }
        };
        log(format!(
            "install {}: {} after {seconds:.1}s, {:.1} MB written (bound {:.0} s, {:.1} GB)",
            manager.as_str(),
            coverage.reason,
            added as f64 / 1e6,
            settings.time_limit.as_secs_f64(),
            settings.disk_budget as f64 / 1e9,
        ));
        if coverage.status != "installed" {
            let removed = remove_new_node_modules(repo, &existing);
            log(format!(
                "install fell back: removed {removed} node_modules director{} it created",
                if removed == 1 { "y" } else { "ies" }
            ));
        }
        jail.finish(log);
        coverage
    }

    pub(super) fn exec(
        repo: &Path,
        scratch: &Path,
        settings: &InstallSettings,
        program: &[String],
        log: &dyn Fn(String),
    ) -> i32 {
        let jail = match Jail::prepare(repo, scratch, settings, None) {
            Ok(jail) => jail,
            Err(why) => {
                log(format!("sandbox unavailable: {why}"));
                return 125;
            }
        };
        if let Err(why) = jail.self_test(log) {
            log(format!("sandbox unavailable: {why}"));
            jail.finish(log);
            return 125;
        }
        log("sandbox self-test: ok".to_owned());
        let status = jail
            .command(program)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        jail.finish(log);
        match status {
            Ok(status) => status.code().unwrap_or(128),
            Err(error) => {
                log(format!("sandbox unavailable: start nsjail: {error}"));
                125
            }
        }
    }
}

/// The trusted side of the jail's only network path (docs/SCIP_SANDBOX.md
/// §3.4): an HTTP CONNECT proxy on a Unix socket, bind-mounted into the
/// jail and bridged to `127.0.0.1:3128` there. It accepts `CONNECT
/// <host>:443` for an allowlisted host only, resolves the name itself and
/// connects only to globally routable addresses, so neither a DNS answer
/// nor an IP literal can reach metadata endpoints, private networks
/// (including Fly's `fdaa::/16`) or the host's own services. TLS runs end
/// to end through the tunnel; the proxy never holds a certificate. Plain
/// HTTP and every other method are refused. std threads rather than the
/// service's tokio runtime: the build that calls this is synchronous, and
/// the CLI has no runtime at all.
#[cfg(unix)]
mod egress {
    use std::collections::BTreeMap;
    use std::io::{self, Read, Write};
    use std::net::{IpAddr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    const MAX_HEAD: usize = 8 * 1024;
    const MAX_TUNNELS: usize = 256;
    const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

    #[derive(Debug, Default)]
    pub(super) struct Stats {
        pub allowed: u64,
        pub refused: BTreeMap<String, u64>,
        pub bytes_in: u64,
        pub bytes_out: u64,
    }

    pub(super) struct Proxy {
        stop: Arc<AtomicBool>,
        accept: Option<JoinHandle<()>>,
        stats: Arc<Mutex<Stats>>,
        socket: PathBuf,
    }

    impl Proxy {
        pub(super) fn start(socket: &Path, allow: &'static [&'static str]) -> io::Result<Self> {
            let listener = UnixListener::bind(socket)?;
            listener.set_nonblocking(true)?;
            let stop = Arc::new(AtomicBool::new(false));
            let stats = Arc::new(Mutex::new(Stats::default()));
            let active = Arc::new(AtomicUsize::new(0));
            let accept = std::thread::Builder::new()
                .name("tolmap-egress".to_owned())
                .spawn({
                    let stop = stop.clone();
                    let stats = stats.clone();
                    move || {
                        while !stop.load(Ordering::Relaxed) {
                            match listener.accept() {
                                Ok((mut client, _)) => {
                                    if active.load(Ordering::Relaxed) >= MAX_TUNNELS {
                                        let _ = respond(&mut client, 503, "Service Unavailable");
                                        continue;
                                    }
                                    active.fetch_add(1, Ordering::Relaxed);
                                    let stats = stats.clone();
                                    let active = active.clone();
                                    let spawned = std::thread::Builder::new()
                                        .name("tolmap-egress-tunnel".to_owned())
                                        .spawn(move || {
                                            serve(client, allow, &stats);
                                            active.fetch_sub(1, Ordering::Relaxed);
                                        });
                                    if spawned.is_err() {
                                        active.fetch_sub(1, Ordering::Relaxed);
                                    }
                                }
                                Err(_) => std::thread::sleep(Duration::from_millis(25)),
                            }
                        }
                    }
                })?;
            Ok(Self {
                stop,
                accept: Some(accept),
                stats,
                socket: socket.to_path_buf(),
            })
        }

        /// Stops accepting and returns what was seen. Tunnels still open end
        /// on their own once the jail, their other end, is gone.
        pub(super) fn stop(&mut self) -> Stats {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(accept) = self.accept.take() {
                let _ = accept.join();
            }
            let _ = std::fs::remove_file(&self.socket);
            std::mem::take(
                &mut *self
                    .stats
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()),
            )
        }
    }

    impl Drop for Proxy {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    /// Why a request was refused: the status line to answer with and a
    /// printable key for the log.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct Refusal {
        pub status: u16,
        pub target: String,
    }

    /// The allowlisted host a CONNECT request line names, or why it is
    /// refused. Returns the allowlist's own string, never the request's, so
    /// nothing the jail wrote is resolved.
    pub(super) fn connect_target<'a>(head: &str, allow: &[&'a str]) -> Result<&'a str, Refusal> {
        let line = head.split("\r\n").next().unwrap_or_default();
        let refuse = |status| Refusal {
            status,
            target: printable(line),
        };
        let mut words = line.split(' ');
        let (Some(method), Some(target), Some(version), None) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return Err(refuse(400));
        };
        if version != "HTTP/1.1" && version != "HTTP/1.0" {
            return Err(refuse(400));
        }
        if method != "CONNECT" {
            return Err(refuse(405));
        }
        let Some((host, port)) = target.rsplit_once(':') else {
            return Err(refuse(400));
        };
        if port != "443" {
            return Err(refuse(403));
        }
        allow
            .iter()
            .find(|allowed| allowed.eq_ignore_ascii_case(host))
            .copied()
            .ok_or_else(|| refuse(403))
    }

    /// Globally routable unicast only. IPv4: not 0/8, 10/8, 100.64/10
    /// (shared), 127/8, 169.254/16 (link-local, cloud metadata), 172.16/12,
    /// 192.0.0/24, 192.0.2/24, 192.88.99/24, 192.168/16, 198.18/15,
    /// 198.51.100/24, 203.0.113/24, or 224/3 and up. IPv6: inside 2000::/3,
    /// which excludes loopback, link-local, unique-local fc00::/7 (Fly's
    /// fdaa::/16 6PN) and NAT64's 64:ff9b::/96, and not 2001::/23 (Teredo
    /// and protocol assignments), 2001:db8::/32 or 6to4's 2002::/16, which
    /// embed IPv4 addresses. IPv4-mapped addresses are judged as IPv4.
    pub(super) fn is_global(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                let [a, b, c, _] = v4.octets();
                !(a == 0
                    || a == 10
                    || a == 127
                    || (a == 100 && (64..128).contains(&b))
                    || (a == 169 && b == 254)
                    || (a == 172 && (16..32).contains(&b))
                    || (a == 192 && b == 0 && (c == 0 || c == 2))
                    || (a == 192 && b == 88 && c == 99)
                    || (a == 192 && b == 168)
                    || (a == 198 && (b == 18 || b == 19))
                    || (a == 198 && b == 51 && c == 100)
                    || (a == 203 && b == 0 && c == 113)
                    || a >= 224)
            }
            IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return is_global(IpAddr::V4(v4));
                }
                let [first, second, ..] = v6.segments();
                (first & 0xe000) == 0x2000
                    && !(first == 0x2001 && second < 0x0200)
                    && !(first == 0x2001 && second == 0x0db8)
                    && first != 0x2002
            }
        }
    }

    fn printable(text: &str) -> String {
        text.chars()
            .take(100)
            .map(|character| {
                if character.is_ascii_graphic() || character == ' ' {
                    character
                } else {
                    '?'
                }
            })
            .collect()
    }

    fn respond(client: &mut UnixStream, status: u16, phrase: &str) -> io::Result<()> {
        client.write_all(
            format!("HTTP/1.1 {status} {phrase}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
    }

    fn phrase(status: u16) -> &'static str {
        match status {
            400 => "Bad Request",
            403 => "Forbidden",
            405 => "Method Not Allowed",
            502 => "Bad Gateway",
            _ => "Error",
        }
    }

    /// Reads a request head: everything up to the blank line, at most
    /// `MAX_HEAD` bytes. Returns it and any bytes already read past it.
    fn read_head(client: &mut UnixStream) -> Option<(String, Vec<u8>)> {
        let mut buffer = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        loop {
            if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                let rest = buffer.split_off(end + 4);
                return Some((String::from_utf8_lossy(&buffer).into_owned(), rest));
            }
            if buffer.len() > MAX_HEAD {
                return None;
            }
            match client.read(&mut chunk) {
                Ok(0) | Err(_) => return None,
                Ok(count) => buffer.extend_from_slice(&chunk[..count]),
            }
        }
    }

    fn serve(mut client: UnixStream, allow: &[&str], stats: &Mutex<Stats>) {
        let _ = client.set_nonblocking(false);
        let _ = client.set_read_timeout(Some(HEAD_TIMEOUT));
        // An empty connection is the in-jail bridge's readiness check.
        let Some((head, rest)) = read_head(&mut client) else {
            return;
        };
        let refuse = |client: &mut UnixStream, refusal: Refusal| {
            let _ = respond(client, refusal.status, phrase(refusal.status));
            let mut stats = stats.lock().unwrap_or_else(|poison| poison.into_inner());
            *stats.refused.entry(refusal.target).or_default() += 1;
        };
        let host = match connect_target(&head, allow) {
            Ok(host) => host,
            Err(refusal) => return refuse(&mut client, refusal),
        };
        let addresses = (host, 443)
            .to_socket_addrs()
            .map(|resolved| {
                resolved
                    .filter(|address| is_global(address.ip()))
                    .collect::<Vec<SocketAddr>>()
            })
            .unwrap_or_default();
        let Some(mut upstream) = addresses
            .iter()
            .find_map(|address| TcpStream::connect_timeout(address, CONNECT_TIMEOUT).ok())
        else {
            return refuse(
                &mut client,
                Refusal {
                    status: 502,
                    target: format!("{host}:443 (no reachable global address)"),
                },
            );
        };
        if client
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .is_err()
            || (!rest.is_empty() && upstream.write_all(&rest).is_err())
        {
            return;
        }
        let _ = client.set_read_timeout(Some(IDLE_TIMEOUT));
        let _ = upstream.set_read_timeout(Some(IDLE_TIMEOUT));
        let (Ok(mut upstream_read), Ok(mut client_write)) =
            (upstream.try_clone(), client.try_clone())
        else {
            return;
        };
        let down = std::thread::spawn(move || {
            let count = io::copy(&mut upstream_read, &mut client_write).unwrap_or(0);
            let _ = client_write.shutdown(Shutdown::Write);
            count
        });
        let up = io::copy(&mut client, &mut upstream).unwrap_or(0);
        let _ = upstream.shutdown(Shutdown::Write);
        let down = down.join().unwrap_or(0);
        let mut stats = stats.lock().unwrap_or_else(|poison| poison.into_inner());
        stats.allowed += 1;
        stats.bytes_out += up + rest.len() as u64;
        stats.bytes_in += down;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_connect_to_an_allowlisted_host_on_443_is_allowed() {
            let allow = ["registry.npmjs.org"];
            let head = |line: &str| format!("{line}\r\nHost: x\r\n\r\n");
            assert_eq!(
                connect_target(&head("CONNECT registry.npmjs.org:443 HTTP/1.1"), &allow),
                Ok("registry.npmjs.org")
            );
            assert_eq!(
                connect_target(&head("CONNECT Registry.NPMJS.org:443 HTTP/1.1"), &allow),
                Ok("registry.npmjs.org")
            );
            for (line, status) in [
                ("CONNECT registry.npmjs.org:80 HTTP/1.1", 403),
                ("CONNECT github.com:443 HTTP/1.1", 403),
                ("CONNECT registry.npmjs.org.evil.example:443 HTTP/1.1", 403),
                ("CONNECT evil.registry.npmjs.org:443 HTTP/1.1", 403),
                ("CONNECT registry.npmjs.org.:443 HTTP/1.1", 403),
                ("CONNECT 169.254.169.254:443 HTTP/1.1", 403),
                ("CONNECT 104.16.0.1:443 HTTP/1.1", 403),
                ("CONNECT [fdaa::3]:443 HTTP/1.1", 403),
                ("GET http://169.254.169.254/latest/meta-data HTTP/1.1", 405),
                ("GET http://registry.npmjs.org/ HTTP/1.1", 405),
                ("CONNECT registry.npmjs.org HTTP/1.1", 400),
                ("CONNECT registry.npmjs.org:443 HTTP/2", 400),
                ("CONNECT registry.npmjs.org:443  HTTP/1.1", 400),
                ("", 400),
            ] {
                let refused = connect_target(&head(line), &allow).unwrap_err();
                assert_eq!(refused.status, status, "{line:?}");
            }
        }

        #[test]
        fn only_globally_routable_addresses_are_dialled() {
            for address in [
                "104.16.3.35",
                "8.8.8.8",
                "2606:4700::6810:1",
                "2a04:4e42::223",
                "::ffff:104.16.3.35",
            ] {
                assert!(is_global(address.parse().unwrap()), "{address}");
            }
            for address in [
                "0.0.0.0",
                "10.0.0.1",
                "100.64.0.1",
                "127.0.0.1",
                "169.254.169.254",
                "172.16.0.1",
                "172.31.255.255",
                "192.0.0.170",
                "192.168.1.1",
                "198.18.0.1",
                "224.0.0.1",
                "255.255.255.255",
                "::",
                "::1",
                "fe80::1",
                "fc00::1",
                "fdaa::3",
                "fdaa:0:1::2",
                "::ffff:10.0.0.1",
                "::ffff:169.254.169.254",
                "64:ff9b::a9fe:a9fe",
                "2001:db8::1",
                "2001::1",
                "2002:a9fe:a9fe::1",
                "ff02::1",
            ] {
                assert!(!is_global(address.parse().unwrap()), "{address}");
            }
        }

        #[test]
        fn the_proxy_refuses_off_allowlist_requests_without_dialling() {
            let directory = tempfile::tempdir().unwrap();
            let socket = directory.path().join("proxy.sock");
            let mut proxy = Proxy::start(&socket, &["registry.npmjs.org"]).unwrap();
            let ask = |request: &str| {
                let mut stream = UnixStream::connect(&socket).unwrap();
                stream.write_all(request.as_bytes()).unwrap();
                let mut reply = String::new();
                stream.read_to_string(&mut reply).unwrap();
                reply
            };
            assert!(
                ask("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\n\r\n")
                    .starts_with("HTTP/1.1 403 ")
            );
            assert!(
                ask("GET http://169.254.169.254/ HTTP/1.1\r\nHost: x\r\n\r\n")
                    .starts_with("HTTP/1.1 405 ")
            );
            assert!(
                ask("CONNECT registry.npmjs.org:22 HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 403 ")
            );
            // The bridge's readiness check: connect and close, no request.
            drop(UnixStream::connect(&socket).unwrap());
            std::thread::sleep(Duration::from_millis(100));
            let stats = proxy.stop();
            assert_eq!(stats.allowed, 0);
            assert_eq!(stats.refused.values().sum::<u64>(), 3, "{stats:?}");
            assert!(stats
                .refused
                .keys()
                .any(|target| target == "CONNECT example.com:443 HTTP/1.1"));
            assert!(!socket.exists());
        }
    }
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

    fn write(repo: &Path, path: &str, text: &str) {
        let file = repo.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }

    #[test]
    fn only_a_pnpm_or_npm_workspace_with_its_lockfile_is_installed() {
        let plan = |files: &[(&str, &str)]| {
            let directory = tempfile::tempdir().unwrap();
            for (path, text) in files {
                write(directory.path(), path, text);
            }
            install_plan(directory.path())
        };
        let skip = |reason, manager| InstallPlan::Skip { reason, manager };
        let manifest = ("package.json", r#"{"name":"root","private":true}"#);
        let npm_workspace = ("package.json", r#"{"workspaces":["packages/*"]}"#);
        let pnpm_workspace = ("pnpm-workspace.yaml", "packages:\n  - packages/*\n");
        assert_eq!(plan(&[]), skip("no_package_json", None));
        assert_eq!(plan(&[manifest]), skip("no_lockfile", None));
        assert_eq!(
            plan(&[manifest, ("pnpm-lock.yaml", ""), pnpm_workspace]),
            InstallPlan::Install(PackageManager::Pnpm)
        );
        assert_eq!(
            plan(&[manifest, ("pnpm-lock.yaml", "")]),
            skip("not_a_workspace", Some(PackageManager::Pnpm))
        );
        assert_eq!(
            plan(&[npm_workspace, ("package-lock.json", "{}")]),
            InstallPlan::Install(PackageManager::Npm)
        );
        assert_eq!(
            plan(&[
                ("package.json", r#"{"workspaces":{"packages":["a"]}}"#),
                ("npm-shrinkwrap.json", "{}")
            ]),
            InstallPlan::Install(PackageManager::Npm)
        );
        assert_eq!(
            plan(&[manifest, ("package-lock.json", "{}")]),
            skip("not_a_workspace", Some(PackageManager::Npm))
        );
        assert_eq!(
            plan(&[
                ("package.json", r#"{"workspaces":[]}"#),
                ("package-lock.json", "{}")
            ]),
            skip("not_a_workspace", Some(PackageManager::Npm))
        );
        assert_eq!(
            plan(&[npm_workspace, ("yarn.lock", "")]),
            skip("unsupported_lockfile", None)
        );
        assert_eq!(
            plan(&[
                manifest,
                ("pnpm-lock.yaml", ""),
                pnpm_workspace,
                ("node_modules/x/package.json", "{}")
            ]),
            skip("node_modules_present", None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_manifest_or_lockfile_is_never_read() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        write(repo, "outside.json", r#"{"workspaces":["a"]}"#);
        std::os::unix::fs::symlink(repo.join("outside.json"), repo.join("package.json")).unwrap();
        assert_eq!(
            install_plan(repo),
            InstallPlan::Skip {
                reason: "unsafe_manifest",
                manager: None
            }
        );
        fs::remove_file(repo.join("package.json")).unwrap();
        write(repo, "package.json", r#"{"workspaces":["a"]}"#);
        std::os::unix::fs::symlink(repo.join("outside.json"), repo.join("package-lock.json"))
            .unwrap();
        assert_eq!(
            install_plan(repo),
            InstallPlan::Skip {
                reason: "unsafe_manifest",
                manager: None
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_jail_drops_to_the_worker_uid_with_only_its_own_mounts() {
        let layout = jail::JailLayout {
            repo: Path::new("/data/cache/work/o/r/job/repo"),
            git: Some(Path::new("/data/cache/work/o/r/job/repo/.git")),
            home: Path::new("/data/cache/install/job/home"),
            run: Path::new("/data/cache/install/job/run"),
            node_prefix: Path::new("/opt/node"),
            root_entries: vec![
                jail::RootEntry::Link {
                    target: PathBuf::from("usr/bin"),
                    path: PathBuf::from("/bin"),
                },
                jail::RootEntry::Dir(PathBuf::from("/lib64")),
            ],
            uid: 10001,
            gid: 10001,
            time_limit: INSTALL_TIME_LIMIT,
            memory_max: None,
            cgroup_v2: true,
        };
        let args = jail::jail_args(&layout)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect::<Vec<_>>();
        let pair = |flag: &str, value: &str| {
            args.windows(2)
                .any(|window| window[0] == flag && window[1] == value)
        };
        // Root drops straight to the worker's uid: no user namespace, no
        // shared network namespace.
        assert!(args.contains(&"--disable_clone_newuser".to_owned()));
        assert!(pair("--user", "10001:10001:1"));
        assert!(pair("--group", "10001:10001:1"));
        assert!(!args
            .iter()
            .any(|arg| arg == "-N" || arg == "--disable_clone_newnet"));
        assert!(!args.iter().any(|arg| arg == "--keep_env" || arg == "-e"));
        assert!(!args.iter().any(|arg| arg == "--keep_caps"));
        // The checkout read-write, its .git read-only on top, and nothing
        // else of the service's: the only read-write binds are the
        // checkout, the jail's home, the proxy socket's directory and
        // /dev/null.
        assert!(pair("-B", "/data/cache/work/o/r/job/repo"));
        assert!(pair("-R", "/data/cache/work/o/r/job/repo/.git"));
        assert!(pair("--cwd", "/data/cache/work/o/r/job/repo"));
        let writable = args
            .windows(2)
            .filter(|window| window[0] == "-B")
            .map(|window| window[1].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            writable,
            vec![
                "/dev/null",
                "/data/cache/work/o/r/job/repo",
                "/data/cache/install/job/home:/home/tolmap",
                "/data/cache/install/job/run:/run/tolmap",
            ]
        );
        assert!(pair("-s", "usr/bin:/bin"));
        assert!(pair("-R", "/opt/node"));
        assert!(pair("--time_limit", "1260"));
        assert!(!args.iter().any(|arg| arg.contains("cgroup")));
        // The environment is exactly what the jail is given: proxies at the
        // in-jail bridge, the registry, scripts and pnpmfiles off, pnpm's
        // own version and runtime downloads off.
        let env = args
            .windows(2)
            .filter(|window| window[0] == "-E")
            .map(|window| window[1].clone())
            .collect::<Vec<_>>();
        for expected in [
            "HOME=/home/tolmap",
            "HTTPS_PROXY=http://127.0.0.1:3128",
            "pnpm_config_registry=https://registry.npmjs.org/",
            "npm_config_registry=https://registry.npmjs.org/",
            "npm_config_ignore_scripts=true",
            "pnpm_config_ignore_scripts=true",
            "pnpm_config_ignore_pnpmfile=true",
            "pnpm_config_pm_on_fail=ignore",
            "pnpm_config_runtime_on_fail=ignore",
            "NO_PROXY=",
        ] {
            assert!(env.contains(&expected.to_owned()), "{expected} in {env:?}");
        }
        assert!(
            !env.iter().any(|variable| variable.starts_with("OPENROUTER")
                || variable.starts_with("TOLMAP_")
                || variable.starts_with("NODE_OPTIONS"))
        );

        let bounded = jail::jail_args(&jail::JailLayout {
            memory_max: Some(8 << 30),
            git: None,
            ..layout
        })
        .into_iter()
        .map(|arg| arg.into_string().unwrap())
        .collect::<Vec<_>>();
        assert!(bounded.contains(&"--use_cgroupv2".to_owned()));
        assert!(bounded.contains(&(8u64 << 30).to_string()));
        assert!(!bounded.iter().any(|arg| arg.ends_with("/.git")));
    }

    #[cfg(unix)]
    #[test]
    fn an_install_without_its_sandbox_falls_back_and_leaves_the_checkout_alone() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("repo");
        write(&repo, "package.json", r#"{"name":"root","private":true}"#);
        write(&repo, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
        write(&repo, "pnpm-workspace.yaml", "packages:\n  - packages/*\n");
        let before = fs::read_dir(&repo).unwrap().count();
        let settings = InstallSettings {
            nsjail: "/nonexistent/nsjail".to_owned(),
            node_prefix: None,
            uid: 10001,
            gid: 10001,
            time_limit: INSTALL_TIME_LIMIT,
            disk_budget: INSTALL_DISK_BUDGET,
            memory_max: None,
        };
        let scratch = directory.path().join("scratch");
        let lines = std::cell::RefCell::new(Vec::new());
        let coverage = install(&repo, &scratch, &settings, &|| {}, &|| false, &|line| {
            lines.borrow_mut().push(line)
        });
        assert_eq!(
            coverage,
            fell_back("sandbox_unavailable", Some(PackageManager::Pnpm))
        );
        assert_eq!(coverage.status, "fell_back");
        assert_eq!(coverage.manager.as_deref(), Some("pnpm"));
        // Whether as a non-root test process or as root without nsjail, the
        // sandbox never started, so nothing ran and nothing was written.
        assert_eq!(fs::read_dir(&repo).unwrap().count(), before);
        assert!(!repo.join("node_modules").exists());
        assert!(!scratch.exists());
        assert!(
            lines
                .borrow()
                .iter()
                .any(|line| line.starts_with("install sandbox unavailable: ")),
            "{:?}",
            lines.borrow()
        );
        // A repository the policy skips never reaches the sandbox at all.
        fs::remove_file(repo.join("pnpm-workspace.yaml")).unwrap();
        let coverage = install(&repo, &scratch, &settings, &|| {}, &|| false, &|_| {});
        assert_eq!(
            coverage,
            skipped("not_a_workspace", Some(PackageManager::Pnpm))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fallback_removes_only_the_node_modules_the_install_created() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path();
        write(repo, "vendor/node_modules/kept/index.js", "");
        write(repo, ".git/node_modules/untouched", "");
        let before = jail::node_modules_dirs(repo);
        assert_eq!(before.len(), 1);
        write(repo, "node_modules/.pnpm/x/index.js", "");
        write(repo, "packages/a/node_modules/y/index.js", "");
        std::os::unix::fs::symlink("/", repo.join("packages/link")).unwrap();
        assert_eq!(jail::remove_new_node_modules(repo, &before), 2);
        assert!(repo.join("vendor/node_modules/kept/index.js").exists());
        assert!(repo.join(".git/node_modules/untouched").exists());
        assert!(!repo.join("node_modules").exists());
        assert!(!repo.join("packages/a/node_modules").exists());
    }

    #[cfg(unix)]
    #[test]
    fn disk_usage_counts_a_hardlinked_file_once() {
        let directory = tempfile::tempdir().unwrap();
        let store = directory.path().join("store");
        let tree = directory.path().join("tree");
        write(&store, "blob", &"x".repeat(64 * 1024));
        fs::create_dir_all(&tree).unwrap();
        fs::hard_link(store.join("blob"), tree.join("blob")).unwrap();
        let one = jail::disk_usage(&[store.as_path()]);
        let both = jail::disk_usage(&[store.as_path(), tree.as_path()]);
        assert!(one >= 64 * 1024, "{one}");
        // The second tree adds its directory entry's blocks, not the file's.
        assert!(both < one + 64 * 1024, "{one} {both}");
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
