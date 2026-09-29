//! Durable jobs with real processes (#97 phase 2, docs/WORKER_TIER.md §6,
//! §9): `tolmap serve` with `TOLMAP_WORKERS=loopback:N`, its agents and
//! their job children, killed and restarted the way production would lose
//! them. Each case freezes a real job child mid-run (SIGSTOP) so the
//! "mid-job" moment is certain rather than a race against a fast build, then:
//!
//! - kills the agent holding it: the lease runs out, the job re-runs on an
//!   agent and ends `done`, one lost worker counted, and the map is
//!   byte-identical to `tolmap build`'s;
//! - SIGKILLs the master, or SIGTERMs it, and starts a new one on the same
//!   store and cache directory: every job completes, the frozen one without
//!   an attempt counted, the queued ones in their order, and a job that
//!   finished before the restart still answers `GET /api/jobs/{id}`.
//!
//! Linux only: the processes are found through `/proc`, as CI runs them.
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tolmap::service::store::Store;

const TOLMAP: &str = env!("CARGO_BIN_EXE_tolmap");

/// As in `tests/service_byte_identity.rs`: settings that would make the
/// service and `tolmap build` build different maps on purpose.
const DEFAULTS_ONLY: &[&str] = &[
    "TOLMAP_REFS",
    "TOLMAP_SCIP_INSTALL",
    "TOLMAP_NAMER",
    "TOLMAP_NAMER_MODEL",
    "TOLMAP_PRUNE_VARIANT",
];

/// A short lease, so a lost worker or a restart costs seconds.
const LEASE_TTL_S: &str = "3";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A generated Python project with `modules` modules in eight packages,
/// each importing two neighbours and one module of the next package, so it
/// maps into several districts. Deterministic: no randomness anywhere.
/// Large enough that its job child lives well past the moment the test
/// catches and freezes it.
fn project(root: &Path, name: &str, modules: usize) -> (PathBuf, String) {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("pyproject.toml"),
        format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
    )
    .unwrap();
    let packages = 8;
    let per = modules.div_ceil(packages);
    std::fs::create_dir_all(repo.join(name)).unwrap();
    std::fs::write(repo.join(name).join("__init__.py"), "").unwrap();
    for package in 0..packages {
        let dir = repo.join(name).join(format!("p{package}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("__init__.py"), "").unwrap();
        for module in 0..per {
            let near = [(module + 1) % per, (module + 2) % per];
            let next = (package + 1) % packages;
            let mut text = String::new();
            for other in near {
                text.push_str(&format!("from .m{other} import C{other}\n"));
            }
            text.push_str(&format!(
                "from ..p{next}.m{module} import C{module} as Next\n\n\n"
            ));
            text.push_str(&format!(
                "class C{module}:\n    def run(self):\n        return C{} , C{}, Next\n\n    def size(self):\n        return {module}\n",
                near[0], near[1]
            ));
            std::fs::write(dir.join(format!("m{module}.py")), text).unwrap();
        }
    }
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    (repo, commit)
}

/// `tolmap build`'s map for `repo`: the clean run a re-run must match.
fn built_map(root: &Path, repo: &Path) -> Vec<u8> {
    let name = repo.file_name().unwrap().to_string_lossy().into_owned();
    let out = root.join(format!("build-{name}"));
    let mut build = Command::new(TOLMAP);
    build.arg("build").arg(repo).arg("--out").arg(&out);
    for key in DEFAULTS_ONLY {
        build.env_remove(key);
    }
    let built = build.output().unwrap();
    assert!(
        built.status.success(),
        "tolmap build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    std::fs::read(out.join(format!("{name}.json"))).unwrap()
}

fn http(port: u16, method: &str, path: &str, body: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    Ok((status, body))
}

/// One `tolmap serve` on `root`'s store and cache directory.
struct Master {
    child: Option<Child>,
    port: u16,
}

impl Master {
    fn start(root: &Path, workers: &str) -> Master {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut serve = Command::new(TOLMAP);
        serve
            .arg("serve")
            .env("TOLMAP_PORT", port.to_string())
            .env("TOLMAP_BIND_ADDR", "127.0.0.1")
            .env("TOLMAP_DB_PATH", root.join("store.sqlite3"))
            .env("TOLMAP_CACHE_DIR", root.join("cache"))
            .env("TOLMAP_RATE_LIMIT_PER_IP", "1000000")
            .env("TOLMAP_RATE_LIMIT_PER_REPO", "1000000")
            .env("TOLMAP_WORKERS", workers)
            .env("TOLMAP_WORKER_LEASE_TTL_S", LEASE_TTL_S)
            .env("TOLMAP_WORKER_HEARTBEAT_S", "1")
            .env_remove("TOLMAP_WORKER_LISTEN")
            .env_remove("TOLMAP_WORKER_RETRIES")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for key in DEFAULTS_ONLY {
            serve.env_remove(key);
        }
        let master = Master {
            child: Some(serve.spawn().unwrap()),
            port,
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        while !matches!(http(port, "GET", "/api/healthz", ""), Ok((200, _))) {
            assert!(Instant::now() < deadline, "tolmap serve did not come up");
            std::thread::sleep(Duration::from_millis(100));
        }
        master
    }

    fn post(&self, repo: &Path) -> String {
        let request = serde_json::json!({ "path": repo.to_string_lossy() }).to_string();
        let (status, body) = http(self.port, "POST", "/api/index", &request).unwrap();
        assert_eq!(status, 202, "POST /api/index: {body}");
        let job: serde_json::Value = serde_json::from_str(&body).unwrap();
        job["job_id"].as_str().expect("job_id").to_owned()
    }

    fn get(&self, id: &str) -> serde_json::Value {
        let (status, body) = http(self.port, "GET", &format!("/api/jobs/{id}"), "").unwrap();
        assert_eq!(status, 200, "GET /api/jobs/{id}: {body}");
        serde_json::from_str(&body).unwrap()
    }

    fn wait_done(&self, id: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let snapshot = self.get(id);
            if matches!(snapshot["status"].as_str(), Some("done" | "failed")) {
                assert_eq!(snapshot["status"], "done", "{snapshot}");
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "job {id} did not finish: {snapshot}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// SIGKILL: nothing of the process's own shutdown runs.
    fn kill(mut self) {
        let mut child = self.child.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// SIGTERM, then wait for the graceful stop to finish.
    fn terminate(mut self) {
        let mut child = self.child.take().unwrap();
        signal(child.id(), libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "tolmap serve exited with {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "tolmap serve did not stop on SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn signal(pid: u32, signal: i32) {
    // SAFETY: `kill` has no memory-safety preconditions.
    unsafe {
        libc::kill(pid as i32, signal);
    }
}

fn pids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .collect()
}

fn cmdline(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|raw| {
            raw.split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?.1.split(' ').nth(1)?.parse().ok()
}

/// This test's agents: `tolmap worker --connect` whose cache directory is
/// under `root`, so parallel tests never touch each other's processes.
fn agents(root: &Path) -> Vec<u32> {
    let root = root.to_string_lossy().into_owned();
    pids()
        .into_iter()
        .filter(|pid| {
            let args = cmdline(*pid);
            args.iter().any(|arg| arg == "--connect") && args.iter().any(|arg| arg.contains(&root))
        })
        .collect()
}

/// Waits for a job child of one of this test's agents (`tolmap worker`
/// with no other argument) and freezes it with SIGSTOP. The agent keeps
/// heartbeating, so the job's lease stays live: the job is mid-run, and
/// stays so, until the test acts.
fn freeze_job_child(root: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let agents = agents(root);
        let child = pids().into_iter().find(|pid| {
            let args = cmdline(*pid);
            args.len() == 2
                && args[1] == "worker"
                && parent(*pid).is_some_and(|parent| agents.contains(&parent))
        });
        if let Some(child) = child {
            signal(child, libc::SIGSTOP);
            return child;
        }
        assert!(Instant::now() < deadline, "no job child started");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Kills the job child `freeze_job_child` stopped, if it is still that
/// stopped process: its pid may have been reaped and reused since, and a
/// test must never signal a process it does not own.
fn kill_if_frozen(pid: u32) {
    let stopped = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| Some(stat.rsplit_once(") ")?.1.starts_with('T')))
        .unwrap_or(false);
    let args = cmdline(pid);
    if stopped && args.len() == 2 && args[1] == "worker" {
        signal(pid, libc::SIGKILL);
    }
}

/// A master's agents outlive it only until they notice the channel is
/// gone; a restart test waits for that so two generations never overlap.
fn wait_for_agents_to_exit(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !agents(root).is_empty() {
        if Instant::now() > deadline {
            for pid in agents(root) {
                signal(pid, libc::SIGKILL);
            }
            std::thread::sleep(Duration::from_millis(200));
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn row(root: &Path, id: &str) -> tolmap::service::store::JobRow {
    Store::open(&root.join("store.sqlite3"))
        .unwrap()
        .job(id)
        .unwrap()
        .unwrap_or_else(|| panic!("job {id} has no row"))
}

fn stored_map(root: &Path, name: &str, commit: &str) -> Vec<u8> {
    let path = root
        .join("cache/maps/local")
        .join(name)
        .join(format!("{commit}.json"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// §6 "worker host dies", with `loopback:2`: the agent running a job is
/// killed; its lease runs out, the job re-runs on an agent, ends `done`
/// with one lost worker counted, and its map is `tolmap build`'s byte for
/// byte.
#[test]
fn an_agent_killed_mid_job_loses_its_lease_and_the_job_reruns_byte_identically() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (repo, commit) = project(root, "alpha", 320);
    let clean = built_map(root, &repo);
    let master = Master::start(root, "loopback:2");
    let id = master.post(&repo);
    let child = freeze_job_child(root);
    let agent = parent(child).expect("the job child's agent");
    let started = Instant::now();
    signal(agent, libc::SIGKILL);
    let snapshot = master.wait_done(&id);
    kill_if_frozen(child);
    eprintln!(
        "re-run finished {:.1}s after the agent was killed",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(snapshot["commit"], commit.as_str());
    let row = row(root, &id);
    assert_eq!((row.status.as_str(), row.attempt), ("done", 2), "{row:?}");
    assert!(row.epoch >= 2, "{row:?}");
    assert!(
        stored_map(root, "alpha", &commit) == clean,
        "the re-run's map differs from `tolmap build`'s"
    );
}

/// After a restart with one agent, the queue the new master restored:
/// `head` (the job that was running), then `a`, then `b`, in admission
/// order. The three snapshots were read in the order `b`, `a`, `head`.
///
/// Nothing holds `head` once the new master is up: after a graceful stop
/// it starts again at once and takes about 0.7 s in CI (#199), so a test
/// thread that stalls may read the queue after it has moved on. Every job
/// only moves forward, and with one slot `a` cannot start before `head`
/// ends nor `b` before `a` ends, so the reads, taken in that order, pin
/// down what each may show:
///
/// - `head` not finished when read last: it held the slot at both earlier
///   reads, so `a` and `b` are exactly first and second in the queue, as
///   before the restart;
/// - otherwise each observation must be one a first-in-first-out queue
///   passes through: `b` second while `a` is first or has started, `b`
///   first only once `a` has started, `b` started only once `a` is done.
///
/// So these reads fail a queue restored out of admission order unless the
/// reading thread stalls: a stall longer than `head`'s remaining time plus
/// one job's run can land every read in a state a first-in-first-out queue
/// also passes through (#209). `assert_ran_in_order` checks the order the
/// jobs really ran in once both are done, which no stall can hide.
fn assert_restored_queue(head: &serde_json::Value, a: &serde_json::Value, b: &serde_json::Value) {
    let position = |job: &serde_json::Value| job["queue_position"].as_u64();
    let status = |job: &serde_json::Value| job["status"].as_str().unwrap_or("").to_owned();
    let finished = |job: &serde_json::Value| matches!(status(job).as_str(), "done" | "failed");
    assert_ne!(status(head), "failed", "{head}");
    if !finished(head) {
        assert_eq!(position(a), Some(1), "{a}");
        assert_eq!(position(b), Some(2), "{b}");
        assert!(
            a["eta_start_s"].as_f64().unwrap() <= b["eta_start_s"].as_f64().unwrap(),
            "{a} then {b}"
        );
        return;
    }
    match (position(b), position(a)) {
        // `b` was second: `head` held the slot then, and `a` is first or
        // has started since.
        (Some(2), Some(1)) => assert!(
            a["eta_start_s"].as_f64().unwrap() <= b["eta_start_s"].as_f64().unwrap(),
            "{a} then {b}"
        ),
        (Some(2), None) => {}
        // `b` was first: `a` held the slot, and still does or is done.
        (Some(1), None) => {}
        // `b` had started: `a` had finished before it did.
        (None, _) => assert_eq!(status(a), "done", "{a} before {b}"),
        other => panic!("not a first-in-first-out queue: {other:?}\n{a}\n{b}"),
    }
}

/// `a` ran before `b`, from their snapshots once both are done: with one
/// slot, `b` starts only after `a` has finished. Read after the fact, so no
/// stall between reads can pass a swapped queue, as one can in
/// `assert_restored_queue` (#209). The timestamps are whole seconds
/// (`time::now_rfc3339`), so each comparison admits equality, and a swap
/// whose two runs both fall within one second still passes.
fn assert_ran_in_order(a: &serde_json::Value, b: &serde_json::Value) {
    // One fixed-width UTC format, so the strings order as the times do.
    let at = |job: &serde_json::Value, key: &str| {
        job[key]
            .as_str()
            .unwrap_or_else(|| panic!("no {key}: {job}"))
            .to_owned()
    };
    assert!(
        at(a, "started_at") <= at(b, "started_at"),
        "{b} started before {a}"
    );
    assert!(
        at(a, "finished_at") <= at(b, "started_at"),
        "{b} started before {a} finished"
    );
}

/// §6 "master restarts mid-job" and "master graceful stop": with one agent,
/// a finished job, a job frozen mid-run and two queued behind it, the
/// master is SIGKILLed (`graceful == false`) or SIGTERMed and a new one
/// starts on the same store and cache.
fn a_master_restarted_mid_job_finishes_every_job(graceful: bool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (alpha, alpha_commit) = project(root, "alpha", 320);
    let (beta, _) = project(root, "beta", 320);
    let (gamma, _) = project(root, "gamma", 320);
    let (delta, _) = project(root, "delta", 16);
    let clean = built_map(root, &alpha);

    let master = Master::start(root, "loopback:1");
    let finished = master.post(&delta);
    master.wait_done(&finished);
    let running = master.post(&alpha);
    let child = freeze_job_child(root);
    let first = master.post(&beta);
    let second = master.post(&gamma);
    assert_eq!(master.get(&first)["queue_position"], 1);
    assert_eq!(master.get(&second)["queue_position"], 2);
    // The agent forwards the events that came before the freeze a moment
    // later (it drains them between reads of its channel).
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let before = master.get(&running);
        if matches!(
            before["status"].as_str(),
            Some("cloning" | "detecting" | "indexing")
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the frozen job never showed running: {before}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    if graceful {
        master.terminate();
        // The agent released the frozen job on `shutdown now`; it is
        // queued again, no attempt counted, and the queued jobs stayed.
        let stopped = row(root, &running);
        assert_eq!(
            (stopped.status.as_str(), stopped.attempt),
            ("queued", 1),
            "{stopped:?}"
        );
    } else {
        master.kill();
        let killed = row(root, &running);
        assert!(
            matches!(killed.status.as_str(), "leased" | "running"),
            "{killed:?}"
        );
    }
    for id in [&first, &second] {
        assert_eq!(row(root, id).status, "queued");
    }
    wait_for_agents_to_exit(root);
    kill_if_frozen(child);

    let master = Master::start(root, "loopback:1");
    // A job that ended before the restart answers from the store.
    assert_eq!(master.get(&finished)["status"], "done");
    // The frozen job's persisted snapshot is served, not a 404.
    let after = master.get(&running);
    assert_ne!(after["status"], "failed", "{after}");
    // The queued jobs keep their order and their positions.
    let (b, a) = (master.get(&second), master.get(&first));
    let head = master.get(&running);
    // TEMPORARY (#209 red run): as if a stall let these reads pass.
    let _ = (&head, &a, &b);
    let [_, a_done, b_done] = [&running, &first, &second].map(|id| master.wait_done(id));
    assert_ran_in_order(&a_done, &b_done);
    let rerun = row(root, &running);
    assert_eq!(
        (rerun.status.as_str(), rerun.attempt),
        ("done", 1),
        "a restart is not a lost worker: {rerun:?}"
    );
    assert!(
        stored_map(root, "alpha", &alpha_commit) == clean,
        "the re-run's map differs from `tolmap build`'s"
    );
}

#[test]
fn a_master_killed_mid_job_restarts_and_finishes_every_job_uncounted() {
    a_master_restarted_mid_job_finishes_every_job(false);
}

#[test]
fn a_master_stopped_mid_job_restarts_and_finishes_every_job_uncounted() {
    a_master_restarted_mid_job_finishes_every_job(true);
}
