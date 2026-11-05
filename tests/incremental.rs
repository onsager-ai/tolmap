use std::path::Path;
use std::sync::{Arc, Mutex};
use tolmap::extract::{self, LanguageKind};
use tolmap::geometry::{self, BuildFeatures};
use tolmap::incremental::Cache;
use tolmap::progress::Progress;
use tolmap::schema::MapDocument;
use tolmap::worker::WorkerEvent;

fn git(repo: &Path, args: &[&str]) {
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .unwrap()
        .status
        .success());
}

fn build(repo: &Path, out: &Path, progress: &Progress) -> Vec<Vec<u8>> {
    build_with_prior(repo, out, progress, None)
}

fn build_with_prior(
    repo: &Path,
    out: &Path,
    progress: &Progress,
    previous: Option<&MapDocument>,
) -> Vec<Vec<u8>> {
    let path = geometry::build_multi_warm_with_progress(
        repo,
        &[("pkg".into(), LanguageKind::Python)],
        Some("fixture"),
        out,
        1.1,
        BuildFeatures {
            parcels: true,
            prune_variant: Default::default(),
            namer: tolmap::naming::NamerKind::Idf,
            namer_model: String::new(),
            refs: extract::RefsMode::Hand,
            install: extract::InstallMode::Off,
        },
        previous,
        progress,
    )
    .unwrap();
    vec![
        std::fs::read(&path).unwrap(),
        std::fs::read(path.with_extension("symbols.json")).unwrap(),
    ]
}

#[test]
fn full_and_reused_map_and_symbols_agree_after_dependency_and_file_changes() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(repo.join("pkg")).unwrap();
    git(&repo, &["init", "-q"]);
    for i in 0..8 {
        std::fs::write(
            repo.join(format!("pkg/f{i}.py")),
            format!(
                "from .f{} import value\ndef value():\n    return {}\n",
                (i + 1) % 8,
                i
            ),
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "initial"]);
    let logs = Arc::new(Mutex::new(Vec::new()));
    let capture = logs.clone();
    let progress = Progress::new(move |event| {
        if let WorkerEvent::Log { message, .. } = event {
            capture.lock().unwrap().push(message);
        }
    })
    .with_cache(Cache::local(
        root.path().join("cache"),
        repo.to_string_lossy().into_owned(),
    ));
    let mut n = 0;
    let mut compare = |reasons: &[&str], reused: usize, previous: Option<&MapDocument>| {
        logs.lock().unwrap().clear();
        let warm = build_with_prior(
            &repo,
            &root.path().join(format!("warm-{n}")),
            &progress,
            previous,
        );
        let full = build_with_prior(
            &repo,
            &root.path().join(format!("full-{n}")),
            &Progress::silent(),
            previous,
        );
        assert_eq!(warm, full, "{reasons:?}");
        let rows = logs.lock().unwrap();
        let report: serde_json::Value = rows
            .iter()
            .find_map(|s| {
                s.strip_prefix("index_reuse ")
                    .map(|v| serde_json::from_str(v).unwrap())
            })
            .unwrap();
        assert_eq!(report["reused"], reused, "{report}");
        assert_eq!(
            report["history"], "recompute_at_requested_revision_4000_commit_window",
            "history is recomputed for every build"
        );
        for reason in reasons {
            assert!(
                report["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r == *reason),
                "{report}"
            );
        }
        n += 1;
    };
    compare(&["cold_cache"], 0, None);
    compare(&[], 8, None);
    std::fs::write(
        repo.join("pkg/f0.py"),
        "from .f3 import value\ndef value():\n    return 99\n",
    )
    .unwrap();
    compare(&["content_changed"], 7, None);
    std::fs::write(repo.join("pkg/added.py"), "from .f0 import value\n").unwrap();
    compare(&["file_added"], 8, None);
    std::fs::remove_file(repo.join("pkg/added.py")).unwrap();
    compare(&["file_deleted"], 8, None);
    std::fs::rename(repo.join("pkg/f0.py"), repo.join("pkg/renamed.py")).unwrap();
    compare(&["file_deleted", "file_added", "file_renamed"], 7, None);
    std::fs::create_dir_all(repo.join(".tolmap")).unwrap();
    std::fs::write(
        repo.join(".tolmap/config.toml"),
        "exclude = [\"unused/**\"]\n",
    )
    .unwrap();
    compare(&["config_or_exclusions_changed"], 0, None);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "history advances"]);
    compare(&["history_window_advanced"], 8, None);

    // The renamed consumer imports value from f1. Changing only the provider
    // then changes the unchanged consumer's resolution target; every
    // downstream stage must agree with full extraction while it is reused.
    std::fs::write(
        repo.join("pkg/renamed.py"),
        "from .f1 import value\ndef caller():\n    return value()\n",
    )
    .unwrap();
    std::fs::write(repo.join("pkg/f1.py"), "def value():\n    return 1\n").unwrap();
    compare(&["content_changed"], 6, None);
    std::fs::write(repo.join("pkg/f1.py"), "def replacement():\n    return 2\n").unwrap();
    compare(&["content_changed"], 7, None);

    let prior_bytes = build(&repo, &root.path().join("prior-seed"), &Progress::silent());
    let prior: MapDocument = serde_json::from_slice(&prior_bytes[0]).unwrap();
    logs.lock().unwrap().clear();
    let warm_prior = build_with_prior(
        &repo,
        &root.path().join("warm-prior"),
        &progress,
        Some(&prior),
    );
    let full_prior = build_with_prior(
        &repo,
        &root.path().join("full-prior"),
        &Progress::silent(),
        Some(&prior),
    );
    assert_eq!(warm_prior, full_prior, "compatible prior map");
    assert_eq!(
        serde_json::from_slice::<MapDocument>(&warm_prior[0])
            .unwrap()
            .names,
        prior.names,
        "reusing extraction must preserve district names from an identical prior"
    );
    let rows = logs.lock().unwrap();
    let report: serde_json::Value = rows
        .iter()
        .find_map(|s| {
            s.strip_prefix("index_reuse ")
                .map(|v| serde_json::from_str(v).unwrap())
        })
        .unwrap();
    assert_eq!(report["reused"], 8, "{report}");

    // The prior map is a downstream partitioning seed, not extraction
    // provenance. A changed repository label leaves raw-file reuse intact.
    let mut changed_prior = prior.clone();
    changed_prior.repo = "other-repository".into();
    drop(rows);
    logs.lock().unwrap().clear();
    let warm_changed_prior = build_with_prior(
        &repo,
        &root.path().join("warm-changed-prior"),
        &progress,
        Some(&changed_prior),
    );
    let full_changed_prior = build_with_prior(
        &repo,
        &root.path().join("full-changed-prior"),
        &Progress::silent(),
        Some(&changed_prior),
    );
    assert_eq!(warm_changed_prior, full_changed_prior, "changed prior map");
    let rows = logs.lock().unwrap();
    let report: serde_json::Value = rows
        .iter()
        .find_map(|s| {
            s.strip_prefix("index_reuse ")
                .map(|v| serde_json::from_str(v).unwrap())
        })
        .unwrap();
    assert_eq!(report["reused"], 8, "{report}");
}

#[test]
fn rust_go_and_typescript_raw_records_round_trip_without_changing_graphs() {
    for (language, files) in [
        (
            LanguageKind::Rust,
            vec![
                (
                    "Cargo.toml",
                    "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
                ),
                (
                    "src/lib.rs",
                    "mod helper; pub fn run() { helper::value(); }\n",
                ),
                ("src/helper.rs", "pub fn value() {}\n"),
            ],
        ),
        (
            LanguageKind::Go,
            vec![
                ("go.mod", "module fixture\n\ngo 1.26\n"),
                ("main.go", "package fixture\nfunc Run() { Value() }\n"),
                ("helper.go", "package fixture\nfunc Value() {}\n"),
            ],
        ),
        (
            LanguageKind::TypeScript,
            vec![
                (
                    "main.ts",
                    "import { value } from './helper'; export function run() { return value(); }\n",
                ),
                ("helper.ts", "export function value() { return 1; }\n"),
            ],
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        for (path, text) in files {
            let path = repo.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "fixture"]);
        let progress = Progress::silent()
            .with_cache(Cache::local(root.path().join("cache"), "fixture".into()));
        let sources = vec![(".".into(), language)];
        let full = extract::build_multi_source(&repo, &sources).unwrap();
        let cold = extract::build_multi_source_with_progress(&repo, &sources, &progress).unwrap();
        let warm = extract::build_multi_source_with_progress(&repo, &sources, &progress).unwrap();
        assert_eq!(
            serde_json::to_vec(&full).unwrap(),
            serde_json::to_vec(&cold).unwrap()
        );
        assert_eq!(
            serde_json::to_vec(&full).unwrap(),
            serde_json::to_vec(&warm).unwrap()
        );
    }
}
