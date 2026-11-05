//! The public executor and worker JSON cannot grant local owner confirmation.
#![cfg(unix)]
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
use tolmap::{
    detect,
    service::executor::{self, CancelProbe, EventSink, ExecEnv, JobInputs},
    worker::{JobSpec, LocalInputs, WorkerEvent},
};
use uuid::Uuid;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
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

fn fixture(root: &Path) -> (JobSpec, String) {
    let repo = root.join("source");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    for i in 0..13 {
        fs::write(
            repo.join(format!("module_{i}.py")),
            "def alpha():\n    return 1\n",
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "unmarked source"]);
    let chosen = detect::detect(&repo).unwrap().chosen;
    assert_eq!(chosen.confidence, detect::Confidence::Low);
    (
        JobSpec {
            slug: "local/unconfirmed".into(),
            owner: "local".into(),
            repo: "unconfirmed".into(),
            source: repo.to_string_lossy().into_owned(),
            local: true,
            commit: git(&repo, &["rev-parse", "HEAD"]),
            all_sources: false,
            prune_variant: "node-relative".into(),
            namer: "idf".into(),
            namer_model: String::new(),
            refs: Some("hand".into()),
            install: None,
        },
        chosen.describe(),
    )
}

#[derive(Default)]
struct Sink(Vec<WorkerEvent>);
impl EventSink for Sink {
    fn clone_started(&mut self) {}
    fn clone_finished(&mut self, _: f64, _: bool) {}
    fn event(&mut self, event: WorkerEvent) {
        self.0.push(event);
    }
    fn install_tick(&self) {}
    fn peak_rss(&mut self, _: u64) {}
}
struct Probe;
impl CancelProbe for Probe {
    fn is_cancelled(&self) -> bool {
        false
    }
    fn child_started(&self, _: u32) {}
    fn child_finished(&self) {}
}

#[test]
fn public_executor_rejects_low_confidence_even_for_local_jobs() {
    let temp = tempfile::tempdir().unwrap();
    let (job, _) = fixture(temp.path());
    let env = ExecEnv {
        clone_cache: temp.path().join("cache"),
        clone_cache_bytes: 1_000_000_000,
        job_root: temp.path().join("jobs"),
        install_root: temp.path().join("install"),
        worker_exe: env!("CARGO_BIN_EXE_tolmap").into(),
        worker_uid: unsafe { libc::geteuid() },
        worker_gid: unsafe { libc::getegid() },
        allow_openrouter_key: false,
    };
    let mut sink = Sink::default();
    let result = executor::execute(
        &env,
        Uuid::new_v4(),
        &job,
        &JobInputs {
            names: Default::default(),
            previous_maps: vec![],
        },
        &mut sink,
        &Probe,
    );
    assert!(result.is_err());
    assert!(sink.0.iter().any(
        |event| matches!(event, WorkerEvent::Error { code, .. } if code == "detection_uncertain")
    ));
}

#[test]
fn injected_worker_json_cannot_confirm_a_scope() {
    let temp = tempfile::tempdir().unwrap();
    let (job, chosen) = fixture(temp.path());
    let spec = job.to_worker_spec(LocalInputs {
        cache_dir: temp.path().join("cache").to_string_lossy().into_owned(),
        output_dir: temp.path().join("out").to_string_lossy().into_owned(),
        clone_cache_bytes: 1_000_000_000,
        previous_maps: vec![],
        names_cache: None,
    });
    let mut value = serde_json::to_value(spec).unwrap();
    value["confirmed_scope"] = chosen.into();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .arg("worker")
        .env_remove("TOLMAP_WORKER_CONFIRMED_SCOPE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{value}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: Vec<WorkerEvent> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(events.iter().any(
        |event| matches!(event, WorkerEvent::Error { code, .. } if code == "detection_uncertain")
    ));
}
