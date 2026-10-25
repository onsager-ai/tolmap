use std::path::Path;
use std::sync::{Arc, Mutex};
use tolmap::extract::{self, LanguageKind};
use tolmap::geometry::{self, BuildFeatures};
use tolmap::incremental::Cache;
use tolmap::progress::Progress;
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
        None,
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
    let mut compare = |reason: &str, reused: usize| {
        logs.lock().unwrap().clear();
        let warm = build(&repo, &root.path().join(format!("warm-{n}")), &progress);
        let full = build(
            &repo,
            &root.path().join(format!("full-{n}")),
            &Progress::silent(),
        );
        assert_eq!(warm, full, "{reason}");
        let rows = logs.lock().unwrap();
        let report: serde_json::Value = rows
            .iter()
            .find_map(|s| {
                s.strip_prefix("index_reuse ")
                    .map(|v| serde_json::from_str(v).unwrap())
            })
            .unwrap();
        assert_eq!(report["reused"], reused, "{report}");
        if !reason.is_empty() {
            assert!(
                report["reasons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r == reason),
                "{report}"
            );
        }
        n += 1;
    };
    compare("cold_cache", 0);
    compare("", 8);
    std::fs::write(
        repo.join("pkg/f0.py"),
        "from .f3 import value\ndef value():\n    return 99\n",
    )
    .unwrap();
    compare("content_changed", 7);
    std::fs::write(repo.join("pkg/added.py"), "from .f0 import value\n").unwrap();
    compare("file_added", 8);
    std::fs::remove_file(repo.join("pkg/added.py")).unwrap();
    compare("file_deleted", 8);
    std::fs::rename(repo.join("pkg/f0.py"), repo.join("pkg/renamed.py")).unwrap();
    compare("file_renamed", 7);
    std::fs::create_dir_all(repo.join(".tolmap")).unwrap();
    std::fs::write(
        repo.join(".tolmap/config.toml"),
        "exclude = [\"unused/**\"]\n",
    )
    .unwrap();
    compare("config_or_exclusions_changed", 0);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "history advances"]);
    compare("", 8);
}
