//! Explicit loopback-only private checkout journey. Separate from the public
//! catalogue router: HTTP never accepts a host path, URL, credential or ref.
use crate::{
    service::{
        executor::{self, CancelProbe, EventSink, ExecEnv, JobInputs, PreviousMapInput},
        worker_result,
    },
    source,
    worker::{JobSpec, WorkerEvent},
};
use anyhow::{bail, ensure, Context, Result};
use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Identity {
    checkout: PathBuf,
    repository: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    id: Uuid,
    repository: String,
    commit: String,
    prepared: Uuid,
    sequence: u64,
}
struct Prepared {
    id: Uuid,
    commit: String,
    chosen: String,
    all: Vec<String>,
}
struct Job {
    id: Uuid,
    stage: String,
    cancel: Arc<Probe>,
}
struct Local {
    checkout: PathBuf,
    data: PathBuf,
    assets: PathBuf,
    origin: String,
    repository: String,
    prepared: Mutex<Option<Prepared>>,
    job: Mutex<Option<Job>>,
    notice: Mutex<String>,
}
struct Probe {
    stopped: AtomicBool,
    pid: AtomicU32,
}
impl Probe {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl CancelProbe for Probe {
    fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
    fn child_started(&self, pid: u32) {
        self.pid.store(pid, Ordering::SeqCst);
        if self.is_cancelled() {
            self.stop();
        }
    }
    fn child_finished(&self) {
        self.pid.store(0, Ordering::SeqCst);
    }
}
struct Sink(Arc<Local>);
impl EventSink for Sink {
    fn clone_started(&mut self) {
        self.stage("Preparing committed source");
    }
    fn clone_finished(&mut self, _: f64, _: bool) {}
    fn event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::StageStarted { stage, .. } => self.stage(&format!("{stage:?}")),
            WorkerEvent::Progress { value, .. } => self.stage(&value.label),
            _ => {}
        }
    }
    fn install_tick(&self) {}
    fn peak_rss(&mut self, _: u64) {}
}
impl Sink {
    fn stage(&self, text: &str) {
        if let Some(j) = self.0.job.lock().unwrap().as_mut() {
            j.stage = text.into();
        }
    }
}
fn private(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    ensure!(
        std::fs::symlink_metadata(path)?.is_dir(),
        "real storage directory required"
    );
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn checked(mut command: std::process::Command) -> Result<Vec<u8>> {
    let o = command.output()?;
    ensure!(
        o.status.success(),
        "local Git operation failed; keep the checkout available and retry"
    );
    Ok(o.stdout)
}
fn command(repo: &Path, args: &[&str]) -> std::process::Command {
    let mut c = source::git(repo);
    c.args(args);
    c
}
fn prepare(s: &Local) -> Result<()> {
    ensure!(s.job.lock().unwrap().is_none(), "indexing is still running");
    let commit = String::from_utf8(checked(command(
        &s.checkout,
        &["rev-parse", "HEAD^{commit}"],
    ))?)?
    .trim()
    .to_owned();
    ensure!(
        worker_result::is_object_id(&commit),
        "committed HEAD required"
    );
    let id = Uuid::new_v4();
    let root = s.data.join("prepared").join(id.to_string());
    private(&root)?;
    let bare = root.join("source.git");
    let mut c = source::git(&s.checkout);
    c.args(["clone", "--bare", "--no-local", "--"])
        .arg(&s.checkout)
        .arg(&bare);
    checked(c)?;
    // Independent objects, no alternates/hardlinks: moving/GC'ing the original
    // checkout cannot substitute or expire source belonging to an old map.
    let preview = root.join("preview");
    let mut c = source::git(&root);
    c.args(["clone", "--no-local", "--"])
        .arg(&bare)
        .arg(&preview);
    checked(c)?;
    checked(command(&preview, &["checkout", "--detach", &commit]))?;
    let detected = crate::detect::detect(&preview)?;
    let all = crate::detect::all_sources(&preview)?
        .iter()
        .map(|c| c.describe())
        .collect();
    *s.prepared.lock().unwrap() = Some(Prepared {
        id,
        commit,
        chosen: detected.chosen.describe(),
        all,
    });
    std::fs::remove_dir_all(preview)?;
    Ok(())
}
fn snapshots(s: &Local) -> Result<Vec<Snapshot>> {
    let mut result = Vec::new();
    for e in std::fs::read_dir(s.data.join("snapshots"))? {
        let p = e?.path().join("snapshot.json");
        if p.is_file() {
            result.push(serde_json::from_slice::<Snapshot>(&std::fs::read(p)?)?);
        }
    }
    result.sort_by_key(|x| (x.sequence, x.id));
    Ok(result)
}
fn run(
    s: Arc<Local>,
    prepared: Uuid,
    commit: String,
    all: bool,
    id: Uuid,
    cancel: Arc<Probe>,
) -> Result<()> {
    let source = s
        .data
        .join("prepared")
        .join(prepared.to_string())
        .join("source.git");
    let previous = snapshots(&s)?.last().cloned();
    let mut inputs = JobInputs {
        names: Default::default(),
        previous_maps: vec![],
    };
    let sequence = previous.as_ref().map(|p| p.sequence + 1).unwrap_or(1);
    if let Some(p) = previous {
        let dir = s.data.join("snapshots").join(p.id.to_string());
        inputs.names = crate::naming::load_cache(&dir.join("names.json"));
        inputs.previous_maps.push(PreviousMapInput {
            commit: p.commit,
            branch: None,
            path: dir.join("map.json"),
        });
    }
    let spec = JobSpec {
        slug: "local/repository".into(),
        owner: "local".into(),
        repo: "repository".into(),
        source: source.to_string_lossy().into(),
        local: true,
        commit: commit.clone(),
        all_sources: all,
        prune_variant: "node-relative".into(),
        namer: "idf".into(),
        namer_model: String::new(),
        refs: Some("hand".into()),
        install: None,
    };
    let env = ExecEnv {
        clone_cache: s.data.join("cache"),
        clone_cache_bytes: 2 * 1024 * 1024 * 1024,
        job_root: s.data.join("jobs"),
        install_root: s.data.join("install"),
        worker_exe: std::env::current_exe()?,
        worker_uid: 10001,
        worker_gid: 10001,
        allow_openrouter_key: false,
    };
    let result = executor::execute(
        &env,
        id,
        &spec,
        &inputs,
        &mut Sink(s.clone()),
        cancel.as_ref(),
    )
    .map_err(|e| anyhow::anyhow!("local executor stopped: {}", e.error))?;
    let work = result.job_dir.clone();
    let publish = (|| -> Result<()> {
        ensure!(
            !cancel.is_cancelled()
                && result.checkout.commit == commit
                && result.output.commit == commit,
            "cancelled or commit mismatch"
        );
        let dir = s.data.join("snapshots").join(id.to_string());
        private(&dir)?;
        let o = &result.output;
        let adopted = worker_result::adopt(
            &result.output_dir,
            &dir,
            "repository",
            &worker_result::Reported {
                map_path: &o.map_path,
                symbols_path: &o.symbols_path,
                symbols_dir: &o.symbols_dir,
                names_cache: &o.names_cache,
                files: o.files,
                districts: o.districts,
            },
            None,
        )
        .map_err(|e| anyhow::anyhow!("invalid worker artifacts: {e}"))?;
        crate::naming::save_cache(&dir.join("names.json"), &adopted.names)?;
        // Metadata is the visibility boundary. Cancellation serializes with it.
        let job = s.job.lock().unwrap();
        ensure!(!cancel.is_cancelled(), "cancelled");
        let snap = Snapshot {
            id,
            repository: s.repository.clone(),
            commit,
            prepared,
            sequence,
        };
        std::fs::write(dir.join("snapshot.json.tmp"), serde_json::to_vec(&snap)?)?;
        std::fs::rename(dir.join("snapshot.json.tmp"), dir.join("snapshot.json"))?;
        drop(job);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(work);
    publish
}
fn html(s: &Local, notice: &str) -> Result<Response> {
    let index = std::fs::read_to_string(s.assets.join("index.html"))?;
    let stylesheet = index
        .split('"')
        .find(|value| value.starts_with("/assets/") && value.ends_with(".css"))
        .context("production viewer stylesheet missing")?;
    let saved_notice = s.notice.lock().unwrap().clone();
    let notice = if notice.is_empty() {
        saved_notice.as_str()
    } else {
        notice
    };
    let job = s
        .job
        .lock()
        .unwrap()
        .as_ref()
        .map(|job| crate::local_pages::Job {
            id: job.id.to_string(),
            stage: job.stage.clone(),
        });
    let prepared =
        s.prepared
            .lock()
            .unwrap()
            .as_ref()
            .map(|prepared| crate::local_pages::Prepared {
                id: prepared.id.to_string(),
                commit: prepared.commit.clone(),
                chosen: prepared.chosen.clone(),
                all: prepared.all.clone(),
            });
    let snapshots = snapshots(s)?
        .into_iter()
        .map(|snapshot| crate::local_pages::Snapshot {
            id: snapshot.id.to_string(),
            commit: snapshot.commit,
        })
        .collect::<Vec<_>>();
    let content = crate::local_pages::render(
        stylesheet,
        &s.data.display().to_string(),
        notice,
        job.as_ref(),
        prepared.as_ref(),
        &snapshots,
    );
    Ok((
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        content,
    )
        .into_response())
}
async fn handle(State(s): State<Arc<Local>>, req: Request) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    if host != Some(s.origin.trim_start_matches("http://"))
        || (method == Method::POST
            && req
                .headers()
                .get(header::ORIGIN)
                .and_then(|h| h.to_str().ok())
                != Some(&s.origin))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let result = tokio::task::spawn_blocking(move || dispatch(&s, &method, &uri)).await;
    let mut r=match result { Ok(Ok(r))=>r, _=>(StatusCode::BAD_REQUEST,"Local request unavailable. Return to / to refresh the committed source or retry. No newer source was substituted.").into_response() };
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("content-security-policy","default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; worker-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'".parse().unwrap());
    r
}
fn dispatch(s: &Arc<Local>, method: &Method, uri: &axum::http::Uri) -> Result<Response> {
    let path = uri.path();
    let Query(params) = Query::<HashMap<String, String>>::try_from_uri(uri)?;
    if *method == Method::POST {
        match path {
            "/local/refresh" => {
                let notice=prepare(s).err().map(|_|"Source detection failed. Confirm this checkout has a supported committed source and retry.").unwrap_or("");
                return html(s, notice);
            }
            "/local/cancel" => {
                if let Some(j) = s.job.lock().unwrap().as_ref() {
                    ensure!(params.get("id") == Some(&j.id.to_string()), "job mismatch");
                    j.cancel.stop();
                }
                return html(s, "Cancellation requested; waiting for the worker to stop.");
            }
            "/local/index" => {
                let mut job = s.job.lock().unwrap();
                ensure!(job.is_none(), "already running");
                let p = s.prepared.lock().unwrap();
                let p = p.as_ref().context("detect first")?;
                ensure!(
                    params.get("id") == Some(&p.id.to_string()),
                    "selection expired; confirm again"
                );
                let (prepared, commit, all) = (
                    p.id,
                    p.commit.clone(),
                    params.get("all").is_some_and(|x| x == "true"),
                );
                ensure!(!all || !p.all.is_empty(), "no selected sources");
                let id = Uuid::new_v4();
                let cancel = Arc::new(Probe {
                    stopped: AtomicBool::new(false),
                    pid: AtomicU32::new(0),
                });
                *s.notice.lock().unwrap() = String::new();
                *job = Some(Job {
                    id,
                    stage: "Queued".into(),
                    cancel: cancel.clone(),
                });
                drop(job);
                let state = s.clone();
                std::thread::spawn(move || {
                    let result = run(state.clone(), prepared, commit, all, id, cancel);
                    let mut job = state.job.lock().unwrap();
                    *state.notice.lock().unwrap() = if result.is_err() {
                        "Indexing failed or was cancelled. The committed source selection is retained; retry indexing or refresh from HEAD.".into()
                    } else {
                        "Snapshot saved. Open it from history below.".into()
                    };
                    *job = None;
                });
                return Ok((StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response());
            }
            _ => bail!("no mutation route"),
        }
    }
    ensure!(*method == Method::GET, "method");
    if path == "/" {
        return html(s, "");
    }
    if path == "/local.css" {
        return Ok((
            [(header::CONTENT_TYPE, "text/css")],
            crate::local_pages::CSS,
        )
            .into_response());
    }
    if path == "/api/healthz" {
        return Ok(axum::Json(json!({"status":"ok","mode":"local"})).into_response());
    }
    if path == "/api/maps" || path == "/maps/index.json" {
        return Ok(axum::Json(json!([])).into_response());
    }
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if parts.len() >= 4 && parts[..3] == ["api", "maps", "snapshot"] {
        let id = Uuid::parse_str(parts[3])?;
        let dir = s.data.join("snapshots").join(id.to_string());
        let snap: Snapshot = serde_json::from_slice(&std::fs::read(dir.join("snapshot.json"))?)?;
        ensure!(
            params.get("commit").is_none_or(|x| x == &snap.commit),
            "commit mismatch"
        );
        if parts.len() == 5 && parts[4] == "provenance" {
            return Ok(axum::Json(json!({"repository":snap.repository,"commit":snap.commit,"version":source::VERSION})).into_response());
        }
        if parts.len() == 5 && parts[4] == "source" {
            let text = source::read(
                &s.data
                    .join("prepared")
                    .join(snap.prepared.to_string())
                    .join("source.git"),
                &snap.repository,
                &snap.commit,
                params.get("path").context("path")?,
            )?;
            return Ok(axum::Json(text).into_response());
        }
        let file = if parts.len() == 5 && parts[4] == "symbols" {
            let d = params
                .get("district")
                .context("district")?
                .parse::<usize>()?;
            dir.join("symbols").join(format!("{d}.json"))
        } else {
            ensure!(
                parts.len() == 4 || (parts.len() == 5 && parts[4] == "artifact"),
                "route"
            );
            dir.join("map.json")
        };
        return Ok((
            [(header::CONTENT_TYPE, "application/json")],
            std::fs::read(file)?,
        )
            .into_response());
    }
    if parts.len() == 2 && parts[0] == "snapshot" {
        Uuid::parse_str(parts[1])?;
        return Ok((
            [(header::CONTENT_TYPE, "text/html")],
            std::fs::read(s.assets.join("index.html"))?,
        )
            .into_response());
    }
    ensure!(
        path.starts_with("/assets/") || path.starts_with("/fonts/") || path == "/favicon.svg",
        "route"
    );
    ensure!(source::valid_path(path.trim_start_matches('/')), "path");
    let file = s.assets.join(path.trim_start_matches('/')).canonicalize()?;
    ensure!(file.starts_with(s.assets.canonicalize()?), "asset boundary");
    let ty = match file.extension().and_then(|s| s.to_str()) {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("woff2") => "font/woff2",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, ty)
        .body(Body::from(std::fs::read(file)?))?)
}

pub async fn serve(checkout: PathBuf, data: PathBuf, assets: PathBuf, port: u16) -> Result<()> {
    let checkout = checkout.canonicalize()?;
    private(&data)?;
    let data = data.canonicalize()?;
    ensure!(
        !data.starts_with(&checkout),
        "private storage must be outside the checkout"
    );
    private(&data.join("snapshots"))?;
    let identity = data.join("identity.json");
    let repo = if identity.exists() {
        let i: Identity = serde_json::from_slice(&std::fs::read(identity)?)?;
        ensure!(
            i.checkout == checkout,
            "storage belongs to another repository"
        );
        i.repository
    } else {
        let i = Identity {
            checkout: checkout.clone(),
            repository: Uuid::new_v4().to_string(),
        };
        std::fs::write(identity, serde_json::to_vec(&i)?)?;
        i.repository
    };
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let s = Arc::new(Local {
        checkout,
        data,
        assets,
        origin: origin.clone(),
        repository: repo,
        prepared: Mutex::new(None),
        job: Mutex::new(None),
        notice: Mutex::new(String::new()),
    });
    eprintln!("Tolmap local: {origin} — committed source only; no upload");
    axum::serve(listener, axum::Router::new().fallback(handle).with_state(s)).await?;
    Ok(())
}
