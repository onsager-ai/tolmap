use std::path::Path;
use std::sync::{Arc, Mutex};
use tolmap::extract::LanguageKind;
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

fn build(repo: &Path, out: &Path, progress: &Progress) {
    geometry::build_multi_warm_with_progress(
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
            refs: tolmap::extract::RefsMode::Hand,
            install: tolmap::extract::InstallMode::Off,
        },
        None,
        progress,
    )
    .unwrap();
}

#[test]
fn pipeline_cache_is_owner_scoped_and_revoked_authority_fails_closed() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(repo.join("pkg")).unwrap();
    git(&repo, &["init", "-q"]);
    for i in 0..4 {
        std::fs::write(
            repo.join(format!("pkg/f{i}.py")),
            format!(
                "from .f{} import value\ndef value():\n    return {i}\n",
                (i + 1) % 4
            ),
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);

    let owner_a = Cache::with_authority(
        root.path().join("shared-cache"),
        "owner-a".into(),
        "same-repository".into(),
        || true,
    );
    build(
        &repo,
        &root.path().join("owner-a"),
        &Progress::silent().with_cache(owner_a),
    );

    let owner_b_logs = Arc::new(Mutex::new(Vec::new()));
    let capture = owner_b_logs.clone();
    let owner_b_progress = Progress::new(move |event| {
        if let WorkerEvent::Log { message, .. } = event {
            capture.lock().unwrap().push(message);
        }
    })
    .with_cache(Cache::with_authority(
        root.path().join("shared-cache"),
        "owner-b".into(),
        "same-repository".into(),
        || true,
    ));
    build(&repo, &root.path().join("owner-b"), &owner_b_progress);
    let rows = owner_b_logs.lock().unwrap();
    let report: serde_json::Value = rows
        .iter()
        .find_map(|s| {
            s.strip_prefix("index_reuse ")
                .map(|v| serde_json::from_str(v).unwrap())
        })
        .unwrap();
    assert_eq!(report["reused"], 0, "{report}");
    assert!(report["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "cold_cache"));
    drop(rows);

    // Manifest read and first file lookup are allowed. Revocation at the
    // first parsed-record write must fail the build before map publication.
    let checks = Arc::new(AtomicUsize::new(0));
    let gate = checks.clone();
    let mid_build_progress = Progress::silent().with_cache(Cache::with_authority(
        root.path().join("shared-cache"),
        "owner-c".into(),
        "same-repository".into(),
        move || gate.fetch_add(1, Ordering::SeqCst) < 2,
    ));
    let mid_build_output = root.path().join("mid-build-revoked");
    let error = geometry::build_multi_warm_with_progress(
        &repo,
        &[("pkg".into(), LanguageKind::Python)],
        Some("fixture"),
        &mid_build_output,
        1.1,
        BuildFeatures {
            parcels: true,
            prune_variant: Default::default(),
            namer: tolmap::naming::NamerKind::Idf,
            namer_model: String::new(),
            refs: tolmap::extract::RefsMode::Hand,
            install: tolmap::extract::InstallMode::Off,
        },
        None,
        &mid_build_progress,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("cache_authority_revoked"));
    assert!(checks.load(Ordering::SeqCst) >= 3);
    assert!(!mid_build_output.exists());

    let allowed = Arc::new(AtomicBool::new(false));
    let gate = allowed.clone();
    let revoked_progress = Progress::silent().with_cache(Cache::with_authority(
        root.path().join("shared-cache"),
        "owner-a".into(),
        "same-repository".into(),
        move || gate.load(Ordering::SeqCst),
    ));
    let revoked_output = root.path().join("revoked");
    let error = geometry::build_multi_warm_with_progress(
        &repo,
        &[("pkg".into(), LanguageKind::Python)],
        Some("fixture"),
        &revoked_output,
        1.1,
        BuildFeatures {
            parcels: true,
            prune_variant: Default::default(),
            namer: tolmap::naming::NamerKind::Idf,
            namer_model: String::new(),
            refs: tolmap::extract::RefsMode::Hand,
            install: tolmap::extract::InstallMode::Off,
        },
        None,
        &revoked_progress,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("cache_authority_revoked"));
    assert!(!revoked_output.exists());
}
