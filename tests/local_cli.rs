//! Headless commands exercise the production child worker and local store.
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn cli(repo: &Path, data: &Path, command: &str, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .arg(command)
        .arg("--repo")
        .arg(repo)
        .arg("--data")
        .arg(data)
        .args(flags)
        .output()
        .unwrap()
}
fn response(output: Output, code: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn explicit_refresh_publishes_committed_head_and_reuses_second_cache() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let data = root.path().join("data");
    fs::create_dir_all(repo.join("pkg")).unwrap();
    git(&repo, &["init", "-q"]);
    for i in 0..8 {
        fs::write(
            repo.join(format!("pkg/f{i}.py")),
            format!(
                "from .f{} import value\ndef value():\n    return {i}\n",
                (i + 1) % 8
            ),
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "initial"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let none = response(cli(&repo, &data, "status", &[]), 0);
    assert_eq!(none["results"][0]["freshness"], "none");
    assert!(!data.exists(), "status must not create a store");
    // This file must not enter committed extraction or its cache.
    fs::write(repo.join("pkg/dirty.py"), "x = 1\n").unwrap();
    let first = response(
        cli(&repo, &data, "refresh", &["--pkg", "pkg", "--lang", "py"]),
        0,
    );
    let first_row = &first["results"][0];
    assert_eq!(first_row["freshness"], "fresh");
    assert_eq!(first_row["dirty"], true);
    assert_eq!(first_row["changed_files"], 1);
    assert_eq!(first_row["indexed_commit"], head);
    assert_eq!(first["map"]["commit"], head);
    assert_eq!(first_row["previous_map_reused"], false);
    let path = Path::new(first_row["map_path"].as_str().unwrap());
    let map: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(!map["F"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "pkg/dirty.py"));
    let query = Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .args(["where", "pkg/f0.py", "--map"])
        .arg(path)
        .output()
        .unwrap();
    assert!(query.status.success());
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(query["map"], first["map"]);
    let symbol = Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .args(["where", "pkg/f0.py::value", "--map"])
        .arg(path)
        .output()
        .unwrap();
    let symbol = response(symbol, 0);
    assert_eq!(symbol["total"], 1);
    assert!(symbol["coverage"]["calls_total"].is_number());
    assert_eq!(symbol["map"], first["map"]);
    let second = response(
        cli(&repo, &data, "refresh", &["--pkg", "pkg", "--lang", "py"]),
        0,
    );
    let row = &second["results"][0];
    assert_eq!(row["previous_map_reused"], true);
    assert_ne!(row["map_path"], first_row["map_path"]);
    let observations = row["extraction_cache"].as_array().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["parsed"], 0);
    assert_eq!(observations[0]["reused"], 8);
    assert!(path.exists(), "refresh preserves history");
    // An incomplete newer directory never displaces the published snapshot.
    let partial = data
        .join("snapshots")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&partial).unwrap();
    fs::write(partial.join("map.json"), "partial").unwrap();
    let after = response(cli(&repo, &data, "status", &[]), 0);
    assert_eq!(after["results"][0]["map_path"], row["map_path"]);
    assert_eq!(after["map"], second["map"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "next"]);
    let stale = response(cli(&repo, &data, "status", &[]), 0);
    assert_eq!(stale["results"][0]["freshness"], "stale");
    assert_eq!(stale["results"][0]["commits_behind"], 1);
    let failed = response(
        cli(
            &repo,
            &data,
            "refresh",
            &["--pkg", "missing", "--lang", "py"],
        ),
        2,
    );
    assert_eq!(failed["results"][0]["freshness"], "failed");
    assert_eq!(failed["results"][0]["map_path"], row["map_path"]);
    assert_eq!(failed["results"][0]["indexed_commit"], head);
    assert_eq!(
        failed["results"][0]["requested_commit"],
        git(&repo, &["rev-parse", "HEAD"])
    );
}

#[test]
fn input_errors_and_failed_attempts_have_explicit_exit_codes() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let data = root.path().join("data");
    fs::create_dir(&repo).unwrap();
    assert_eq!(cli(&repo, &data, "status", &[]).status.code(), Some(2));
    git(&repo, &["init", "-q"]);
    assert_eq!(cli(&repo, &data, "status", &[]).status.code(), Some(2));
    for i in 0..8 {
        fs::write(
            repo.join(format!("module_{i}.py")),
            "def value():\n    return 1\n",
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "unmarked source"]);
    assert_eq!(
        cli(&repo, &data, "refresh", &["--all-sources", "--pkg", "."])
            .status
            .code(),
        Some(2)
    );
    let failed = response(cli(&repo, &data, "refresh", &[]), 3);
    assert_eq!(failed["results"][0]["freshness"], "failed");
    assert!(failed["results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("confidence"));
    assert!(failed["map"].is_null());
    let status = response(cli(&repo, &data, "status", &[]), 0);
    assert_eq!(status["results"][0]["freshness"], "failed");
    // A malformed store must not be treated as a missing snapshot.
    fs::write(data.join("attempt.json"), "partial-json").unwrap();
    assert_eq!(cli(&repo, &data, "status", &[]).status.code(), Some(3));
}

#[test]
fn indexing_and_interruption_are_read_without_mutating_the_store() {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let data = root.path().join("data");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&data).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("module.py"), "x = 1\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "first"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let attempt = serde_json::json!({"indexed_commit":null,"requested_commit":head,"state":"indexing","failure_reason":null,"failure_code":null,"retryable":false,"job_id":"fixture"});
    let bytes = serde_json::to_vec(&attempt).unwrap();
    fs::write(data.join("attempt.json"), &bytes).unwrap();
    let lock = fs::File::create(data.join("writer.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let status = response(cli(&repo, &data, "status", &[]), 0);
    assert_eq!(status["results"][0]["freshness"], "indexing");
    assert_eq!(cli(&repo, &data, "refresh", &[]).status.code(), Some(3));
    drop(lock);
    let interrupted = response(cli(&repo, &data, "status", &[]), 0);
    assert_eq!(interrupted["results"][0]["freshness"], "failed");
    assert!(interrupted["results"][0]["error"]
        .as_str()
        .unwrap()
        .contains("interrupted"));
    assert_eq!(fs::read(data.join("attempt.json")).unwrap(), bytes);
}
