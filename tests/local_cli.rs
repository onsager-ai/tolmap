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
}
