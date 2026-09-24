//! A repository above the former file cap remains eligible for detection.
//! The expensive index build belongs in remote-build, not this unit gate.

use std::process::Command;

use tolmap::detect;
use tolmap::service::clone::{materialize, RepoRef, RepoSource};
use tolmap::service::config::Limits;

#[test]
fn former_file_and_clone_caps_do_not_reject_a_large_local_repo() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("large");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "initial"
        ])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    std::fs::write(
        repo.join("pyproject.toml"),
        "[project]\nname='large'\nversion='0.1'\n",
    )
    .unwrap();
    let pkg = repo.join("large");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("__init__.py"), "").unwrap();
    for i in 0..5_001 {
        std::fs::write(pkg.join(format!("module_{i}.py")), "x = 1\n").unwrap();
    }
    let reference = RepoRef {
        slug: "local/large".to_owned(),
        owner: "local".to_owned(),
        repo: "large".to_owned(),
        source: RepoSource::Local(repo.clone()),
    };
    let materialized = materialize(
        dir.path(),
        &reference,
        &Limits {
            clone_cache_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let found = detect::detect(&materialized.path).unwrap();
    assert!(found.chosen.file_count > 5_000);
}
