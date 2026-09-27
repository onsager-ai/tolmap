//! Worker classes with real processes (#97 phase 2, step 4;
//! docs/WORKER_TIER.md §2.1 step 3, §9): `tolmap serve` with
//! `TOLMAP_WORKERS=loopback:2` and `TOLMAP_LOOPBACK_CLASSES` starting a
//! "small" and a "large" agent on one runner. The runner's real memory does
//! not matter, only what each agent advertises.
//!
//! A job is admitted small: nothing is known of the repository yet, and the
//! reference-mode prior (`hand`, about 330 MiB with its margin) fits the
//! small class. Once its job child has detected the repository, its
//! `features` report some 1,600 Python files, which the memory model puts
//! above the small class (about 640 MiB on finding 18's curve with the same
//! margin), so the master tells the small agent `cancel` `reroute`, and the
//! job runs again on the large agent: a new epoch, no attempt counted, and
//! a map byte-identical to `tolmap build`'s. The small class is sized
//! between the two predictions with room on both sides (512 MiB).
//!
//! Linux only, as `tests/service_restart.rs`.
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

/// The generated Python project of `tests/service_restart.rs`: `modules`
/// modules in eight packages, each importing two neighbours and one module
/// of the next package. Deterministic: no randomness anywhere.
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

/// `tolmap build`'s map for `repo`: what the rerouted job must match.
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

/// Kills the service when the test ends, pass or fail.
struct Service(Child);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_job_whose_features_outgrow_the_small_class_is_rerouted_and_maps_byte_identically() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (repo, commit) = project(root, "grows", 1600);
    let clean = built_map(root, &repo);

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
        .env("TOLMAP_WORKERS", "loopback:2")
        .env("TOLMAP_LOOPBACK_CLASSES", "512MiB:1,16GiB:1")
        .env_remove("TOLMAP_WORKER_LISTEN")
        .env_remove("TOLMAP_WORKER_RETRIES")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    for key in DEFAULTS_ONLY {
        serve.env_remove(key);
    }
    let _service = Service(serve.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(60);
    while !matches!(http(port, "GET", "/api/healthz", ""), Ok((200, _))) {
        assert!(Instant::now() < deadline, "tolmap serve did not come up");
        std::thread::sleep(Duration::from_millis(100));
    }

    let request = serde_json::json!({ "path": repo.to_string_lossy() }).to_string();
    let (status, body) = http(port, "POST", "/api/index", &request).unwrap();
    assert_eq!(status, 202, "POST /api/index: {body}");
    let job: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = job["job_id"].as_str().expect("job_id").to_owned();
    let store = || Store::open(&root.join("store.sqlite3")).unwrap();
    // Admitted small: the prior fits the 512 MiB class.
    assert_eq!(store().job(&id).unwrap().expect("a row").class, 0);

    let deadline = Instant::now() + Duration::from_secs(600);
    let snapshot = loop {
        let (status, body) = http(port, "GET", &format!("/api/jobs/{id}"), "").unwrap();
        assert_eq!(status, 200, "GET /api/jobs/{id}: {body}");
        let snapshot: serde_json::Value = serde_json::from_str(&body).unwrap();
        if matches!(snapshot["status"].as_str(), Some("done" | "failed")) {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "job did not finish: {snapshot}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(snapshot["status"], "done", "{snapshot}");
    assert_eq!(snapshot["commit"], commit.as_str());
    let row = store().job(&id).unwrap().expect("a row");
    assert_eq!(
        (row.status.as_str(), row.class, row.epoch, row.attempt),
        ("done", 1, 2, 1),
        "rerouted once to the large class, attempt not counted: {row:?}"
    );
    let stored = root
        .join("cache/maps/local/grows")
        .join(format!("{commit}.json"));
    let map =
        std::fs::read(&stored).unwrap_or_else(|error| panic!("read {}: {error}", stored.display()));
    assert!(
        map == clean,
        "the rerouted job's map differs from `tolmap build`'s"
    );
}
