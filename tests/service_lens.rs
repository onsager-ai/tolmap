//! `GET /api/maps/{owner}/{repo}/lenses[/{name}]` (issue #241): the lens is
//! read from the pinned commit's git objects and resolved against that
//! commit's stored map.
use std::{fs, path::Path, process::Command, sync::Arc};

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use tolmap::service::config::{Limits, ServeConfig};
use tolmap::service::ratelimit::RateLimiter;
use tolmap::service::store::{MapRow, Store};
use tolmap::service::{http, jobs, AppState};

fn git(repo: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}

const FLOW_V1: &str = r#"
version = 1
[[areas]]
label = "pair"
color = "green"
members = ["a.py", "*.py"]
[[paths]]
name = "ab"
stops = [{ at = "a.py" }, { at = "b.py" }, { at = "c.py", hop = "declared", reason = "queue" }]
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    state: Arc<AppState>,
    repo: std::path::PathBuf,
    a: String,
    b: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("cache/repos/local/example");
    fs::create_dir_all(repo.join(".tolmap/lenses")).unwrap();
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    git(&repo, &["config", "user.name", "fixture"]);
    for f in ["a.py", "b.py", "c.py"] {
        fs::write(repo.join(f), "x = 1\n").unwrap();
    }
    fs::write(repo.join(".tolmap/lenses/flow.toml"), FLOW_V1).unwrap();
    fs::write(repo.join(".tolmap/lenses/broken.toml"), "version = [").unwrap();
    fs::write(repo.join(".tolmap/lenses/Upper.toml"), FLOW_V1).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "A"]);
    let a = git(&repo, &["rev-parse", "HEAD"]);
    // B changes the lens. Requests pinned to A must keep seeing A's.
    fs::write(
        repo.join(".tolmap/lenses/flow.toml"),
        FLOW_V1.replace("\"pair\"", "\"renamed\""),
    )
    .unwrap();
    git(&repo, &["commit", "-am", "B"]);
    let b = git(&repo, &["rev-parse", "HEAD"]);
    // An uncommitted edit that must never be read.
    fs::write(
        repo.join(".tolmap/lenses/flow.toml"),
        FLOW_V1.replace("\"pair\"", "\"worktree\""),
    )
    .unwrap();

    let map_path = dir.path().join("map.json");
    let node = |d: usize| json!([d, 0.0, 0.0, 1, 0, 0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let map = json!({
        "repo": "example", "q": 0.0, "names": {"0": "core"},
        "districts": {"0": {"size": 3, "c": [0.0, 0.0], "blob": []}},
        "F": ["a.py", "b.py", "c.py"],
        "N": [node(0), node(0), node(0)],
        "E": [[0, 1]], "L": [], "S": {}, "U": {}, "roads": [], "lang": "py"
    });
    fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let config = ServeConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allow_local_paths: false,
        db_path: dir.path().join("store.sqlite3"),
        cache_dir: dir.path().join("cache"),
        static_dir: None,
        prune_variant: tolmap::pipeline::PruneVariant::NodeRelative,
        namer: tolmap::naming::NamerKind::Idf,
        namer_model: tolmap::naming::DEFAULT_MODEL.to_owned(),
        refs: tolmap::extract::RefsMode::Hand,
        scip_install: false,
        limits: Limits::default(),
        retain_commits_per_repo: 20,
        worker_uid: 0,
        worker_gid: 0,
    };
    let store = Store::open(&config.db_path).unwrap();
    for commit in [&a, &b] {
        store
            .insert(&MapRow {
                slug: "local/example".to_owned(),
                owner: "local".to_owned(),
                repo: "example".to_owned(),
                commit: commit.clone(),
                branch: None,
                lang: "py".to_owned(),
                files: 3,
                districts: 1,
                modularity: 0.0,
                map_path: map_path.clone(),
                indexed_at: "2026-11-06T00:00:00Z".to_owned(),
            })
            .unwrap();
    }
    Fixture {
        state: Arc::new(AppState {
            store,
            config,
            jobs: jobs::new_registry(),
            rate_limiter: RateLimiter::new(),
        }),
        _dir: dir,
        repo,
        a,
        b,
    }
}

async fn get(f: &Fixture, uri: &str) -> (StatusCode, Value) {
    let response = http::router(f.state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn resolves_the_lens_at_the_pinned_commit() {
    let f = fixture();
    let (status, body) = get(
        &f,
        &format!("/api/maps/Local/Example/lenses/flow?commit={}", f.a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["commit"], f.a.as_str());
    assert_eq!(body["name"], "flow");
    let report = &body["report"];
    // Commit A's lens, not B's and not the working tree's.
    assert_eq!(report["areas"][0]["label"], "pair");
    assert_eq!(report["areas"][0]["members"].as_array().unwrap().len(), 3);
    assert_eq!(report["unresolved"], json!([]));
    let hops = &report["paths"][0]["hops"];
    assert_eq!(hops[0]["source"], "computed");
    assert_eq!(hops[0]["edge_kind"], "imports");
    assert_eq!(hops[1]["source"], "declared");
    assert_eq!(hops[1]["edge_kind"], Value::Null);

    let (_, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses/flow?commit={}", f.b),
    )
    .await;
    assert_eq!(body["report"]["areas"][0]["label"], "renamed");
    // No commit: the latest indexed snapshot, named in the response.
    let (status, body) = get(&f, "/api/maps/local/example/lenses/flow").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["commit"] == f.a.as_str() || body["commit"] == f.b.as_str());
}

#[tokio::test]
async fn rename_history_reports_the_old_stop_without_substitution() {
    let f = fixture();
    // The lens is last changed at B; the rename happens at C.
    git(&f.repo, &["mv", "a.py", "renamed.py"]);
    git(&f.repo, &["commit", "-m", "rename source"]);
    let c = git(&f.repo, &["rev-parse", "HEAD"]);
    let mut row = f.state.store.get("local/example", &f.b).unwrap().unwrap();
    let bytes = fs::read(&row.map_path).unwrap();
    let mut map: Value = serde_json::from_slice(&bytes).unwrap();
    map["F"][0] = json!("renamed.py");
    row.map_path = f._dir.path().join("renamed-map.json");
    fs::write(&row.map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    row.commit = c.clone();
    f.state.store.insert(&row).unwrap();
    let (status, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses/flow?commit={c}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let unresolved = body["report"]["unresolved"].as_array().unwrap();
    let rename = unresolved
        .iter()
        .find(|u| u["kind"] == "renamed_file")
        .unwrap();
    assert_eq!(
        rename["renamed"],
        json!({"from":"a.py","to":"renamed.py","commit":c})
    );
    assert!(body["report"]["paths"][0]["stops"][0]["target"].is_null());
    let (_, again) = get(
        &f,
        &format!("/api/maps/local/example/lenses/flow?commit={c}"),
    )
    .await;
    assert_eq!(body, again);
}

#[tokio::test]
async fn same_commit_lens_comparison_is_pinned_and_repeatable() {
    let f = fixture();
    let path = format!(
        "/api/compare/local/example/lenses?base={}&head={}&lens=flow",
        f.a, f.a
    );
    let (status, value) = get(&f, &path).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["base"], f.a);
    assert!(value["changes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["status"] == "shared"));
    assert_eq!(get(&f, &path).await.1, value);
    let (status, _) = get(
        &f,
        &format!(
            "/api/compare/local/example/lenses?base={}&head={}&lens=flow&other=broken",
            f.a, f.b
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lists_lens_names_at_the_commit() {
    let f = fixture();
    let (status, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses?commit={}", f.a),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // `Upper.toml` is not a valid lens name and is not offered.
    assert_eq!(body["names"], json!(["broken", "flow"]));
    assert_eq!(body["commit"], f.a.as_str());
}

#[tokio::test]
async fn names_that_could_leave_the_lens_directory_are_rejected() {
    let f = fixture();
    for name in [
        "..%2F..%2Fa",
        "%2e%2e",
        "..",
        "a%2Fb",
        "a%5Cb",
        ".hidden",
        "-x",
        "Upper",
        "flow.toml",
        "flow%00",
    ] {
        let (status, body) = get(
            &f,
            &format!("/api/maps/local/example/lenses/{name}?commit={}", f.a),
        )
        .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
            "{name}: {status} {body}"
        );
        assert_ne!(status, StatusCode::OK, "{name}");
    }
    // The ones that reach the handler are refused by it, with the contract's code.
    let (status, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses/Upper?commit={}", f.a),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
}

#[tokio::test]
async fn explicit_error_states() {
    let f = fixture();
    let (status, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses/nothing?commit={}", f.a),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("lens_not_found"))
    );

    let (status, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses/broken?commit={}", f.a),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("lens_parse_error"))
    );

    // A commit that exists in the clone but was never indexed.
    fs::write(f.repo.join("d.py"), "y = 2\n").unwrap();
    git(&f.repo, &["add", "d.py"]);
    git(&f.repo, &["commit", "-m", "C"]);
    let c = git(&f.repo, &["rev-parse", "HEAD"]);
    let (status, body) = get(
        &f,
        &format!("/api/maps/local/example/lenses/flow?commit={c}"),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("snapshot_not_indexed"))
    );

    // A ref is never accepted in place of a full commit ID.
    let (status, _) = get(&f, "/api/maps/local/example/lenses/flow?commit=HEAD").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = get(&f, "/api/maps/local/nobody/lenses/flow").await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("snapshot_not_indexed"))
    );
}
