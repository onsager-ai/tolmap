//! Remote worker mode with real processes (#97 phase 3, docs/WORKER_TIER.md
//! §5.1, §5.6, §6, §9): `tolmap serve` with `TOLMAP_WORKERS=remote`, its
//! worker listener on `127.0.0.1` with TLS from the committed test-only
//! certificate (`tests/fixtures/tls`), and `tolmap worker --connect wss://`
//! agents holding tokens from `tolmap worker-token new`. Everything stays on
//! loopback; nothing here is reachable from anywhere else.
//!
//! - `worker-token new` prints a token and the line binding its hash, and
//!   nothing else;
//! - startup refuses a plaintext listener off loopback, a key file others
//!   can read, and a missing token file;
//! - an agent that does not trust the private certificate keeps retrying,
//!   and one given it with `--ca-file` builds a map byte-identical to
//!   `tolmap build`'s;
//! - a master killed or stopped mid-job and started again on the same store
//!   hands the job's lease back to its agent, which finishes it at the same
//!   epoch and attempt: no re-run.
//!
//! Linux only: processes are found through `/proc`, as CI runs them.
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tolmap::service::store::Store;

const TOLMAP: &str = env!("CARGO_BIN_EXE_tolmap");

/// As in `tests/service_restart.rs`: settings that would make the service
/// and `tolmap build` build different maps on purpose.
const DEFAULTS_ONLY: &[&str] = &[
    "TOLMAP_REFS",
    "TOLMAP_SCIP_INSTALL",
    "TOLMAP_NAMER",
    "TOLMAP_NAMER_MODEL",
    "TOLMAP_PRUNE_VARIANT",
];

/// Long enough that an agent redialling with backoff finds a restarted
/// master while the adopted lease still runs.
const LEASE_TTL_S: &str = "15";

const WORKER_ID: &str = "w1";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tls")
}

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

/// The generated project of `tests/service_restart.rs`: `modules` modules in
/// eight packages, deterministic, large enough at 320 that its job child
/// lives well past the moment a test catches it.
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

/// `tolmap build`'s map for `repo`.
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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

/// `tolmap worker-token new --id <id>`: the token and the token-file line.
fn issue_token(id: &str) -> (String, String) {
    let output = Command::new(TOLMAP)
        .args(["worker-token", "new", "--id", id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "worker-token new: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "exactly two lines: {stdout:?}");
    (lines[0].to_owned(), lines[1].to_owned())
}

fn write_private(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

/// A master's TLS files, token file and the agent's token file under
/// `root`: the key copied to a `0600` file (git keeps no modes).
struct Setup {
    root: PathBuf,
    cert: PathBuf,
    key: PathBuf,
    tokens: PathBuf,
    token_file: PathBuf,
    worker_port: u16,
}

impl Setup {
    fn new(root: &Path) -> Setup {
        let key = root.join("listener.key");
        std::fs::copy(fixtures().join("test-only-server.key"), &key).unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (token, line) = issue_token(WORKER_ID);
        let tokens = root.join("worker-tokens");
        write_private(&tokens, &format!("# test worker\n{line}\n"));
        let token_file = root.join("agent.token");
        write_private(&token_file, &format!("{token}\n"));
        Setup {
            root: root.to_path_buf(),
            cert: fixtures().join("test-only-server.pem"),
            key,
            tokens,
            token_file,
            worker_port: free_port(),
        }
    }

    fn url(&self) -> String {
        format!("wss://127.0.0.1:{}/workers/connect", self.worker_port)
    }

    /// `tolmap serve` in remote mode, as configured here, with `extra` on
    /// top; not waited for.
    fn serve(&self, extra: &[(&str, &str)]) -> (Command, u16) {
        let port = free_port();
        let mut serve = Command::new(TOLMAP);
        serve
            .arg("serve")
            .env("TOLMAP_PORT", port.to_string())
            .env("TOLMAP_BIND_ADDR", "127.0.0.1")
            .env("TOLMAP_DB_PATH", self.root.join("store.sqlite3"))
            .env("TOLMAP_CACHE_DIR", self.root.join("cache"))
            .env("TOLMAP_RATE_LIMIT_PER_IP", "1000000")
            .env("TOLMAP_RATE_LIMIT_PER_REPO", "1000000")
            .env("TOLMAP_WORKERS", "remote")
            .env(
                "TOLMAP_WORKER_LISTEN",
                format!("127.0.0.1:{}", self.worker_port),
            )
            .env("TOLMAP_WORKER_TLS_CERT", &self.cert)
            .env("TOLMAP_WORKER_TLS_KEY", &self.key)
            .env("TOLMAP_WORKER_TOKENS", &self.tokens)
            .env("TOLMAP_WORKER_LEASE_TTL_S", LEASE_TTL_S)
            .env("TOLMAP_WORKER_HEARTBEAT_S", "1")
            .env_remove("TOLMAP_WORKER_RETRIES")
            .env_remove("TOLMAP_LOOPBACK_CLASSES")
            .stdout(Stdio::null());
        for key in DEFAULTS_ONLY {
            serve.env_remove(key);
        }
        for (key, value) in extra {
            serve.env(key, value);
        }
        (serve, port)
    }

    fn start_master(&self) -> Master {
        let (mut serve, port) = self.serve(&[]);
        serve.stderr(Stdio::inherit());
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

    /// Startup with `extra` fails: its stderr.
    fn refused(&self, extra: &[(&str, &str)]) -> String {
        let (mut serve, _) = self.serve(extra);
        let output = serve.stderr(Stdio::piped()).output().unwrap();
        assert!(
            !output.status.success(),
            "tolmap serve started with {extra:?}"
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// `tolmap worker --connect wss://…` as worker `WORKER_ID`, its stderr
    /// in `<root>/<name>.log`. `ca`: pass `--ca-file` with the test CA.
    fn start_agent(&self, name: &str, ca: bool) -> Agent {
        let log = self.root.join(format!("{name}.log"));
        let mut worker = Command::new(TOLMAP);
        worker
            .arg("worker")
            .arg("--connect")
            .arg(self.url())
            .arg("--token-file")
            .arg(&self.token_file)
            .arg("--worker-id")
            .arg(WORKER_ID)
            .arg("--cache-dir")
            .arg(self.root.join(name))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap());
        if ca {
            worker
                .arg("--ca-file")
                .arg(fixtures().join("test-only-ca.pem"));
        }
        for key in DEFAULTS_ONLY {
            worker.env_remove(key);
        }
        Agent {
            child: worker.spawn().unwrap(),
            log,
        }
    }
}

/// One `tolmap serve`.
struct Master {
    child: Option<Child>,
    port: u16,
}

impl Master {
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

/// One `tolmap worker --connect`, killed when dropped.
struct Agent {
    child: Child,
    log: PathBuf,
}

impl Agent {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

/// Waits for `agent`'s job child (`tolmap worker` with no other argument)
/// and freezes it with SIGSTOP; the agent keeps heartbeating meanwhile.
fn freeze_job_child(agent: u32) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let child = pids().into_iter().find(|pid| {
            let args = cmdline(*pid);
            args.len() == 2 && args[1] == "worker" && parent(*pid) == Some(agent)
        });
        if let Some(child) = child {
            signal(child, libc::SIGSTOP);
            return child;
        }
        assert!(Instant::now() < deadline, "no job child started");
        std::thread::sleep(Duration::from_millis(2));
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

/// §5.1: `worker-token new` prints the token and the line binding its
/// SHA-256 to the id, two lines and nothing else, and writes no file.
#[test]
fn worker_token_new_prints_a_token_and_the_line_for_its_hash() {
    let dir = tempfile::tempdir().unwrap();
    let before: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    let output = Command::new(TOLMAP)
        .args(["worker-token", "new", "--id", "gpu-1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty(), "nothing on stderr");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout:?}");
    let token = lines[0];
    assert_eq!(token.len(), 64);
    assert!(token
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
    let (hash, id) = lines[1].split_once(' ').unwrap();
    assert_eq!(id, "gpu-1");
    use sha2::Digest;
    assert_eq!(
        hash,
        format!("{:x}", sha2::Sha256::digest(token.as_bytes()))
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        before.len(),
        "no file written"
    );
    let bad = Command::new(TOLMAP)
        .args(["worker-token", "new", "--id", "not an id"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty(), "no token printed for a refused id");
}

/// §5.1, §5.6: remote mode never starts in plaintext off loopback, with a
/// key file others can read, or without its token file.
#[test]
fn a_remote_master_refuses_to_start_without_what_keeps_it_private() {
    let dir = tempfile::tempdir().unwrap();
    let setup = Setup::new(dir.path());
    let plaintext = setup.refused(&[
        ("TOLMAP_WORKER_LISTEN", "0.0.0.0:0"),
        ("TOLMAP_WORKER_TLS_CERT", ""),
        ("TOLMAP_WORKER_TLS_KEY", ""),
    ]);
    assert!(plaintext.contains("needs TLS"), "{plaintext}");
    let readable = dir.path().join("readable.key");
    std::fs::copy(&setup.key, &readable).unwrap();
    std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o644)).unwrap();
    let key = setup.refused(&[("TOLMAP_WORKER_TLS_KEY", readable.to_str().unwrap())]);
    assert!(key.contains("mode 644"), "{key}");
    let tokens = setup.refused(&[("TOLMAP_WORKER_TOKENS", "")]);
    assert!(tokens.contains("TOLMAP_WORKER_TOKENS"), "{tokens}");
}

/// §5.1 end to end over TLS on loopback: an agent that does not trust the
/// private certificate keeps retrying rather than exiting or falling back,
/// and one given it with `--ca-file` runs the job, whose map is `tolmap
/// build`'s byte for byte.
#[test]
fn a_remote_agent_over_tls_builds_the_same_map_as_tolmap_build() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let setup = Setup::new(root);
    let (repo, commit) = project(root, "alpha", 64);
    let clean = built_map(root, &repo);
    let master = setup.start_master();

    let mut untrusting = setup.start_agent("untrusting", false);
    let deadline = Instant::now() + Duration::from_secs(60);
    while untrusting.log().matches("could not connect").count() < 2 {
        assert!(
            untrusting.running(),
            "an agent that does not trust the certificate exited: {}",
            untrusting.log()
        );
        assert!(
            Instant::now() < deadline,
            "no retries: {}",
            untrusting.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(untrusting.running(), "{}", untrusting.log());
    drop(untrusting);

    let agent = setup.start_agent("agent", true);
    let id = master.post(&repo);
    let snapshot = master.wait_done(&id);
    assert_eq!(snapshot["commit"], commit.as_str());
    let row = row(root, &id);
    assert_eq!((row.attempt, row.epoch), (1, 1), "{row:?}");
    assert!(
        stored_map(root, "alpha", &commit) == clean,
        "the remote agent's map differs from `tolmap build`'s: {}",
        agent.log()
    );
}

/// §6 "master restarts mid-job" and "master graceful stop", remote mode:
/// with the job child frozen mid-run, the master is SIGKILLed or SIGTERMed
/// and a new one starts on the same store, cache and worker port. The
/// agent redials until it answers, resumes the lease the new master
/// adopted, and the job finishes at epoch 1, attempt 1, with its map
/// byte-identical to `tolmap build`'s.
fn a_remote_master_restarted_mid_job_hands_the_lease_back(graceful: bool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let setup = Setup::new(root);
    let (repo, commit) = project(root, "alpha", 320);
    let clean = built_map(root, &repo);
    let master = setup.start_master();
    let mut agent = setup.start_agent("agent", true);
    let id = master.post(&repo);
    let child = freeze_job_child(agent.child.id());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let before = master.get(&id);
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
    } else {
        master.kill();
    }
    // Either way the job's row still holds its lease, by worker id: nothing
    // was released or re-queued.
    let held = row(root, &id);
    assert!(
        matches!(held.status.as_str(), "leased" | "running"),
        "{held:?}"
    );
    assert_eq!(held.lease_holder.as_deref(), Some(WORKER_ID), "{held:?}");
    assert!(
        agent.running(),
        "the agent outlives its master: {}",
        agent.log()
    );

    let master = setup.start_master();
    let deadline = Instant::now() + Duration::from_secs(60);
    while agent.log().matches("reconnected to").count() < 1 {
        assert!(agent.running(), "{}", agent.log());
        assert!(Instant::now() < deadline, "no reconnect: {}", agent.log());
        std::thread::sleep(Duration::from_millis(100));
    }
    signal(child, libc::SIGCONT);
    let snapshot = master.wait_done(&id);
    assert_eq!(snapshot["commit"], commit.as_str());
    let finished = row(root, &id);
    assert_eq!(
        (finished.attempt, finished.epoch),
        (1, 1),
        "the job re-ran instead of resuming: {finished:?}\n{}",
        agent.log()
    );
    assert!(
        agent.log().contains("resumed at epoch 1"),
        "{}",
        agent.log()
    );
    assert!(
        stored_map(root, "alpha", &commit) == clean,
        "the resumed job's map differs from `tolmap build`'s"
    );
}

#[test]
fn a_remote_master_killed_mid_job_hands_the_lease_back_to_its_agent() {
    a_remote_master_restarted_mid_job_hands_the_lease_back(false);
}

#[test]
fn a_remote_master_stopped_mid_job_hands_the_lease_back_to_its_agent() {
    a_remote_master_restarted_mid_job_hands_the_lease_back(true);
}
