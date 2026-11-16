//! Personal CLI exercises the production worker and retained snapshot protocol.
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
fn fixture(repo: &Path) -> String {
    fs::create_dir_all(repo.join("pkg")).unwrap();
    git(repo, &["init", "-q"]);
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
    git(repo, &["add", "."]);
    git(repo, &["commit", "-qm", "initial"]);
    git(repo, &["rev-parse", "HEAD"])
}
fn cli(cwd: &Path, data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .current_dir(cwd)
        .env("XDG_CONFIG_HOME", data.join("config-home"))
        .arg("--data")
        .arg(data)
        .args(args)
        .output()
        .unwrap()
}
fn response(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn add(cwd: &Path, data: &Path, repo: &Path, name: Option<&str>) -> Value {
    let mut args = vec![
        "add",
        repo.to_str().unwrap(),
        "--pkg",
        "pkg",
        "--lang",
        "py",
    ];
    if let Some(name) = name {
        args.extend(["--name", name]);
    }
    response(cli(cwd, data, &args))
}

#[test]
fn refresh_queries_history_failure_and_removal_share_one_store() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let data = root.path().join("data");
    let head = fixture(&repo);
    fs::write(repo.join("pkg/dirty.py"), "x=1\n").unwrap();
    let registration = add(root.path(), &data, &repo, Some("fixture"));
    let first = response(cli(&repo, &data, &["status"]));
    assert_eq!(first["snapshot"]["commit"], head);
    assert_eq!(first["dirty"], true);
    assert_eq!(first["freshness"], "fresh");
    let first_id = first["snapshot"]["id"].as_str().unwrap().to_string();
    let first_path = first["snapshot"]["map_path"].as_str().unwrap().to_string();
    let source = first["snapshot"]["source_path"]
        .as_str()
        .unwrap()
        .to_string();
    let map: Value = serde_json::from_slice(&fs::read(&first_path).unwrap()).unwrap();
    assert!(!map["F"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "pkg/dirty.py"));
    for args in [
        vec!["overview"],
        vec!["where", "pkg/f0.py"],
        vec!["where", "pkg/f0.py::value"],
        vec!["deps", "pkg/f0.py"],
        vec!["route", "pkg/f0.py", "pkg/f1.py"],
    ] {
        let inferred = response(cli(&repo.join("pkg"), &data, &args));
        let mut named = args.clone();
        named.extend(["--repo", "fixture"]);
        assert_eq!(inferred, response(cli(root.path(), &data, &named)));
        assert_eq!(inferred["map"]["commit"], head);
    }
    let explicit = response(cli(root.path(), &data, &["overview", "--map", &first_path]));
    assert_eq!(explicit, response(cli(&repo, &data, &["overview"])));
    assert!(!cli(
        &repo,
        &data,
        &["overview", "--map", &first_path, "--repo", "fixture"]
    )
    .status
    .success());
    assert!(!cli(root.path(), &data, &["overview"]).status.success());
    response(cli(
        root.path(),
        &data,
        &["debt", "--repo", "fixture", "--format", "json"],
    ));
    let check = response(cli(
        &repo,
        &data,
        &[
            "check",
            "--repo",
            "fixture",
            "--report-only",
            "--format",
            "json",
        ],
    ));
    assert!(check.is_object());
    let detected = cli(root.path(), &data, &["detect", "--repo", "fixture"]);
    assert!(
        detected.status.success(),
        "{}",
        String::from_utf8_lossy(&detected.stderr)
    );
    let lens_out = root.path().join("lenses");
    let suggest = cli(
        root.path(),
        &data,
        &[
            "lens",
            "suggest",
            "--repo",
            "fixture",
            "--out",
            lens_out.to_str().unwrap(),
        ],
    );
    assert!(
        suggest.status.success(),
        "{}",
        String::from_utf8_lossy(&suggest.stderr)
    );
    let historical = cli(
        root.path(),
        &data,
        &[
            "refresh", "fixture", "--commit", &head, "--pkg", "pkg", "--lang", "py",
        ],
    );
    response(historical);
    let second = response(cli(
        &repo,
        &data,
        &["refresh", "fixture", "--pkg", "pkg", "--lang", "py"],
    ));
    assert_ne!(second["snapshot"]["id"], first_id);
    let second_path = second["snapshot"]["map_path"].as_str().unwrap().to_owned();
    let index: Value = serde_json::from_slice(
        &fs::read(Path::new(&second_path).with_file_name("index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["extraction"][0]["parsed"], 0);
    assert_eq!(index["extraction"][0]["reused"], 8);
    assert!(
        !cli(&repo, &data, &["overview", "--commit", &head])
            .status
            .success(),
        "same-commit histories require snapshot selector"
    );
    response(cli(&repo, &data, &["overview", "--snapshot", &first_id]));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "next"]);
    let next_head = git(&repo, &["rev-parse", "HEAD"]);
    assert!(!cli(
        &repo,
        &data,
        &[
            "check",
            "--repo",
            "fixture",
            "--base",
            &next_head,
            "--report-only"
        ]
    )
    .status
    .success());
    let stale = response(cli(&repo, &data, &["status"]));
    assert_eq!(stale["freshness"], "stale");
    let failed = cli(
        &repo,
        &data,
        &[
            "refresh", "--repo", "fixture", "--pkg", "missing", "--lang", "py",
        ],
    );
    assert!(!failed.status.success());
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(failed["snapshot"], second["snapshot"]);
    assert_eq!(failed["attempt"][0], "failed");
    assert!(Path::new(&first_path).exists());
    assert!(
        !cli(&repo, &data, &["refresh", "fixture", "--repo", "fixture"])
            .status
            .success()
    );
    assert!(!cli(&repo, &data, &["overview", "--commit", "deadbeef"])
        .status
        .success());
    // Retained objects remain readable after removal of the original checkout.
    fs::remove_dir_all(&repo).unwrap();
    assert_eq!(
        git(Path::new(&source), &["show", &format!("{head}:pkg/f0.py")]),
        "from .f1 import value\ndef value():\n    return 0"
    );
    let missing = response(cli(root.path(), &data, &["status", "--repo", "fixture"]));
    assert_eq!(missing["freshness"], "missing");
    response(cli(root.path(), &data, &["overview", "--repo", "fixture"]));
    let removed = cli(root.path(), &data, &["rm", "fixture"]);
    assert!(removed.status.success());
    assert!(!Path::new(&first_path).exists());
    assert!(!Path::new(&source).exists());
    assert_eq!(
        response(cli(root.path(), &data, &["repos"])),
        serde_json::json!([])
    );
    assert!(!registration["id"].as_str().unwrap().is_empty());
}

#[test]
fn collisions_relink_verification_and_scan_are_deterministic() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a/repo");
    let b = root.path().join("b/repo");
    let data = root.path().join("data");
    let head = fixture(&a);
    fixture(&b);
    let first = add(root.path(), &data, &a, None);
    let second = add(root.path(), &data, &b, None);
    assert_eq!(first["name"], "repo");
    assert!(second["name"].as_str().unwrap().starts_with("repo-"));
    assert_ne!(first["id"], second["id"]);
    symlink(root.path().join("a"), root.path().join("symlink")).unwrap();
    assert_eq!(
        tolmap::personal::scan(&root.path().join("a")).unwrap(),
        vec![a.canonicalize().unwrap()]
    );
    assert!(tolmap::personal::scan(&root.path().join("symlink"))
        .unwrap()
        .is_empty());
    let moved = root.path().join("moved");
    fs::rename(&a, &moved).unwrap();
    let relink = add(root.path(), &data, &moved, Some("repo"));
    assert_eq!(relink["id"], first["id"]);
    assert_eq!(
        response(cli(&moved, &data, &["status"]))["snapshot"]["commit"],
        head
    );
    // A different history cannot steal the missing registration.
    fs::rename(&moved, root.path().join("hidden")).unwrap();
    fs::write(b.join("unique.py"), "unique=1\n").unwrap();
    git(&b, &["add", "."]);
    git(&b, &["commit", "--amend", "-qm", "unrelated"]);
    // b is already registered, so use a separate independent checkout.
    let wrong = root.path().join("wrong");
    fixture(&wrong);
    fs::write(wrong.join("unique.py"), "x=2\n").unwrap();
    git(&wrong, &["add", "."]);
    git(&wrong, &["commit", "--amend", "-qm", "different root"]);
    assert!(!cli(
        root.path(),
        &data,
        &[
            "add",
            wrong.to_str().unwrap(),
            "--name",
            "repo",
            "--pkg",
            "pkg",
            "--lang",
            "py"
        ]
    )
    .status
    .success());
    assert!(cli(root.path(), &data, &["rm", "repo"]).status.success());
    assert!(root.path().join("hidden").exists());
}

#[test]
fn locks_coordinate_readers_refresh_remove_and_crash_recovery() {
    use std::os::fd::AsRawFd;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let data = root.path().join("data");
    fixture(&repo);
    let registered = add(root.path(), &data, &repo, Some("fixture"));
    let id = registered["id"].as_str().unwrap();
    let store = tolmap::service::store::Store::open(&data.join("tolmap.sqlite3")).unwrap();
    let before = store.personal_history(id).unwrap();
    let guard = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join("tolmap.locks").join(format!("{id}.lock")))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(guard.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    store.personal_begin(id, "interrupted").unwrap();
    let read = response(cli(&repo, &data, &["overview"]));
    assert_eq!(read["map"]["commit"], before[0].commit);
    assert!(
        !cli(&repo, &data, &["refresh", "--pkg", "pkg", "--lang", "py"])
            .status
            .success()
    );
    assert!(!cli(&repo, &data, &["rm", "fixture"]).status.success());
    assert_eq!(
        response(cli(&repo, &data, &["status"]))["attempt"][0],
        "running"
    );
    drop(guard);
    assert_eq!(
        response(cli(&repo, &data, &["status"]))["attempt"][0],
        "failed"
    );
    response(cli(
        &repo,
        &data,
        &["refresh", "--pkg", "pkg", "--lang", "py"],
    ));
    assert_eq!(store.personal_history(id).unwrap().len(), 2);
    let slot = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join("tolmap.locks/slot-0.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(slot.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(!cli(&repo, &data, &["refresh"]).status.success());
    response(cli(&repo, &data, &["overview"]));
}

#[test]
fn failed_registration_remains_listed_and_scan_reports_each_result() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("empty");
    let data = root.path().join("data");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    let failed = cli(root.path(), &data, &["add", repo.to_str().unwrap()]);
    assert!(!failed.status.success());
    // An unborn checkout has no HEAD; list still reports its registration.
    let good = root.path().join("good");
    fixture(&good);
    let scan = cli(
        root.path(),
        &data,
        &[
            "add",
            "--scan",
            root.path().to_str().unwrap(),
            "--pkg",
            "pkg",
            "--lang",
            "py",
        ],
    );
    assert!(!scan.status.success());
    assert!(String::from_utf8_lossy(&scan.stdout).contains("good"));
    let rows = response(cli(root.path(), &data, &["repos"]));
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert!(rows[0]["snapshot"].is_null());
}
