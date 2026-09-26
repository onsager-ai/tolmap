//! Byte identity across execution paths (docs/WORKER_TIER.md §9, #97 phase
//! 1): the same repository at the same commit indexed by `tolmap build` and
//! by `tolmap serve` in local mode -- prepare, the executor, the real
//! `tolmap worker` child, register -- must store byte-identical map and
//! symbols documents. This is the determinism rule (CLAUDE.md, finding 9)
//! extended to the service's own seam, which the executor split moved and
//! a remote worker will move again. The loopback path (`TOLMAP_WORKERS=
//! loopback:N`) joins this test when it exists.
//!
//! The service runs as its own process, as in production, because the job
//! child is `std::env::current_exe()`: only the real binary can play it.
//! Nothing here needs a network beyond loopback.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOLMAP: &str = env!("CARGO_BIN_EXE_tolmap");

/// Settings a CI or developer environment might carry that would make the
/// two paths build different maps on purpose. Both runs use the defaults.
const DEFAULTS_ONLY: &[&str] = &[
    "TOLMAP_REFS",
    "TOLMAP_SCIP_INSTALL",
    "TOLMAP_NAMER",
    "TOLMAP_NAMER_MODEL",
    "TOLMAP_PRUNE_VARIANT",
];

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
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

/// Kills the service when the test ends, pass or fail.
struct Service(Child);

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One HTTP/1.1 request with `Connection: close`; returns the status and
/// the body. The service answers JSON with a `Content-Length`, so reading to
/// the end of the stream is the whole response.
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

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Every file in a map's per-district symbols directory, by name.
fn district_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                read(&entry.path()),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn tolmap_build_and_the_local_service_store_byte_identical_maps() {
    let dir = tempfile::tempdir().unwrap();
    // The directory's name is the map's name on both paths: `tolmap build`
    // names the map after the repository directory, and the service names a
    // local-path job `local/<directory>`.
    let repo = dir.path().join("identity");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service_identity"),
        &repo,
    );
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let commit = git(&repo, &["rev-parse", "HEAD"]);

    let out = dir.path().join("build");
    let mut build = Command::new(TOLMAP);
    build.arg("build").arg(&repo).arg("--out").arg(&out);
    for key in DEFAULTS_ONLY {
        build.env_remove(key);
    }
    let built = build.output().unwrap();
    assert!(
        built.status.success(),
        "tolmap build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );

    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cache = dir.path().join("cache");
    let mut serve = Command::new(TOLMAP);
    serve
        .arg("serve")
        .env("TOLMAP_PORT", port.to_string())
        .env("TOLMAP_BIND_ADDR", "127.0.0.1")
        .env("TOLMAP_DB_PATH", dir.path().join("store.sqlite3"))
        .env("TOLMAP_CACHE_DIR", &cache)
        // Polling below would trip the per-IP limit meant for clients.
        .env("TOLMAP_RATE_LIMIT_PER_IP", "1000000")
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
    let job_id = job["job_id"].as_str().expect("job_id").to_owned();
    let deadline = Instant::now() + Duration::from_secs(300);
    let snapshot = loop {
        let (status, body) = http(port, "GET", &format!("/api/jobs/{job_id}"), "").unwrap();
        assert_eq!(status, 200, "GET /api/jobs/{job_id}: {body}");
        let snapshot: serde_json::Value = serde_json::from_str(&body).unwrap();
        if matches!(snapshot["status"].as_str(), Some("done" | "failed")) {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "job did not finish: {snapshot}");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(snapshot["status"], "done", "{snapshot}");
    assert_eq!(snapshot["commit"], commit.as_str(), "{snapshot}");

    let stored = cache
        .join("maps/local/identity")
        .join(format!("{commit}.json"));
    let map = read(&stored);
    let document: serde_json::Value = serde_json::from_slice(&map).unwrap();
    let files = document["F"].as_array().map_or(0, Vec::len);
    assert!(files >= 5, "the fixture should map its modules: {files}");
    assert!(
        map == read(&out.join("identity.json")),
        "the service's map differs from `tolmap build`'s"
    );
    assert!(
        read(&stored.with_extension("symbols.json")) == read(&out.join("identity.symbols.json")),
        "the service's symbols document differs from `tolmap build`'s"
    );
    let service_districts = district_files(&stored.with_extension("symbols"));
    assert!(!service_districts.is_empty());
    assert!(
        service_districts == district_files(&out.join("identity.symbols")),
        "the service's per-district symbol files differ from `tolmap build`'s"
    );
}
