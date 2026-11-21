use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tolmap::service::store::{PersonalSnapshot, Repository, Store};

const TOLMAP: &str = env!("CARGO_BIN_EXE_tolmap");

async fn send(stdin: &mut ChildStdin, message: Value) {
    stdin
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();
}

async fn request(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    send(
        stdin,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
    .await;
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(15), stdout.read_line(&mut line))
        .await
        .expect("timed out reading MCP response")
        .unwrap();
    let response: Value =
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line:?}"));
    assert_eq!(response["id"], id);
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn cli(cwd: &Path, data: &Path, tool: &str, args: &[&str]) -> std::process::Output {
    std::process::Command::new(TOLMAP)
        .current_dir(cwd)
        .arg("--data")
        .arg(data)
        .args([tool, "--format=json"])
        .args(args)
        .output()
        .unwrap()
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = tolmap::source::git(repo)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn check_fixture(cwd: &Path) -> (PathBuf, String, String) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service_identity");
    let repo = cwd.join("check-fixture");
    fs::create_dir_all(repo.join("shop")).unwrap();
    fs::copy(source.join("pyproject.toml"), repo.join("pyproject.toml")).unwrap();
    for file in fs::read_dir(source.join("shop")).unwrap() {
        let file = file.unwrap();
        if file.path().extension().is_some_and(|ext| ext == "py") {
            fs::copy(file.path(), repo.join("shop").join(file.file_name())).unwrap();
        }
    }
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base fixture"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let cart = repo.join("shop/cart.py");
    let original = fs::read_to_string(&cart).unwrap();
    fs::write(cart, format!("{original}\n# committed head edit\n")).unwrap();
    git(&repo, &["commit", "-qam", "edit cart"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    (repo, base, head)
}

#[tokio::test]
async fn stdio_tools_return_the_cli_report_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().canonicalize().unwrap();
    let data = cwd.join("data");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/tolmap.json");
    let mut child = Command::new(TOLMAP)
        .current_dir(&cwd)
        .arg("--data")
        .arg(&data)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let info = request(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "tolmap-test", "version": "0"},
        }),
    )
    .await;
    assert_eq!(info["serverInfo"]["name"], "tolmap");
    assert!(info["capabilities"]["tools"].is_object());
    send(
        &mut stdin,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;

    let listing = request(&mut stdin, &mut stdout, 2, "tools/list", json!({})).await;
    let tools = listing["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["status", "repos", "overview", "where", "deps", "route", "debt", "check"]
    );
    assert_eq!(tools[3]["inputSchema"]["required"], json!(["target"]));
    assert_eq!(
        tools[4]["inputSchema"]["properties"]["direction"]["default"],
        "both"
    );
    assert_eq!(
        tools[6]["inputSchema"]["properties"]["kind"]["type"],
        "array"
    );
    assert!(tools[7]["inputSchema"]["properties"]
        .get("max-dq")
        .is_none());
    assert!(tools
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true));

    let empty = request(
        &mut stdin,
        &mut stdout,
        3,
        "tools/call",
        json!({"name": "repos"}),
    )
    .await;
    assert_eq!(empty["content"][0]["text"], "[]\n");
    assert!(!data.exists());

    let store = Store::open(&data.join("tolmap.sqlite3")).unwrap();
    store
        .register(&Repository {
            id: "fixture-id".into(),
            name: "tolmap-fixture".into(),
            kind: "local".into(),
            location: cwd.to_str().unwrap().into(),
            observed_at: None,
            observed_head: None,
        })
        .unwrap();
    store.personal_begin("fixture-id", "snapshot-id").unwrap();
    store
        .personal_finish(
            "snapshot-id",
            Ok(&PersonalSnapshot {
                id: "snapshot-id".into(),
                repository: "fixture-id".into(),
                commit: "a".repeat(40),
                map_path: fixture.clone(),
                source_path: cwd.clone(),
                indexed_at: String::new(),
            }),
        )
        .unwrap();

    let cases: [(&str, Value, &[&str]); 7] = [
        ("status", json!({}), &[]),
        ("repos", json!({}), &[]),
        ("overview", json!({"limit": 0}), &["--limit=0"]),
        (
            "where",
            json!({"target": "main", "snapshot": "snapshot-id", "limit": 1}),
            &["main", "--snapshot=snapshot-id", "--limit=1"],
        ),
        (
            "deps",
            json!({"path": "src/main.rs", "direction": "out", "limit": 1}),
            &["src/main.rs", "--direction=out", "--limit=1"],
        ),
        (
            "route",
            json!({"from": "src/main.rs", "to": "src/schema.rs"}),
            &["src/main.rs", "src/schema.rs"],
        ),
        (
            "debt",
            json!({"map": fixture, "kind": ["cycle", "hotspot"], "top": 1}),
            &[
                "--map",
                fixture.to_str().unwrap(),
                "--kind=cycle",
                "--kind=hotspot",
                "--top=1",
            ],
        ),
    ];
    for (index, (tool, arguments, args)) in cases.into_iter().enumerate() {
        let expected = cli(&cwd, &data, tool, args);
        assert!(
            expected.status.success(),
            "{tool}: {}",
            String::from_utf8_lossy(&expected.stderr)
        );
        let response = request(
            &mut stdin,
            &mut stdout,
            4 + index as u64,
            "tools/call",
            json!({
                "name": tool, "arguments": arguments,
            }),
        )
        .await;
        assert_eq!(response["content"][0]["type"], "text");
        assert_eq!(
            response["content"][0]["text"].as_str().unwrap().as_bytes(),
            expected.stdout,
            "{tool}"
        );
    }

    let expected = cli(&cwd, &data, "check", &[".", "--report-only"]);
    assert_eq!(expected.status.code(), Some(2));
    let response = request(
        &mut stdin,
        &mut stdout,
        11,
        "tools/call",
        json!({
            "name": "check", "arguments": {"checkout": "."},
        }),
    )
    .await;
    assert_eq!(response["isError"], true);
    assert_eq!(
        response["content"][0]["text"].as_str().unwrap().as_bytes(),
        expected.stderr
    );

    let (repo, base, head) = check_fixture(&cwd);
    let checkout = repo.to_str().unwrap();
    let expected = cli(
        &cwd,
        &data,
        "check",
        &[
            checkout,
            "--base",
            &base,
            "--head",
            &head,
            "--pkg",
            ".",
            "--lang",
            "py",
            "--report-only",
        ],
    );
    assert!(
        expected.status.success(),
        "check: {}",
        String::from_utf8_lossy(&expected.stderr)
    );
    let report: Value = serde_json::from_slice(&expected.stdout).unwrap();
    assert_eq!(report["base"], base);
    assert_eq!(report["head"], head);
    assert_eq!(report["thresholds"]["source"], "report_only");
    assert!(report["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "shop/cart.py"));

    let response = request(
        &mut stdin,
        &mut stdout,
        12,
        "tools/call",
        json!({"name": "check", "arguments": {
            "checkout": checkout, "base": base, "head": head, "pkg": ["."], "lang": ["py"],
        }}),
    )
    .await;
    assert_eq!(response["isError"], false, "{response}");
    assert_eq!(response["content"][0]["type"], "text");
    assert_eq!(
        response["content"][0]["text"].as_str().unwrap().as_bytes(),
        expected.stdout
    );

    drop(stdin);
    assert!(tokio::time::timeout(Duration::from_secs(15), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
