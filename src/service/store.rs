//! The SQLite store. Cache key is `(slug, commit_sha)` -- docs/ARCHITECTURE.md's
//! "the repository holds intent, the store holds derivation" -- and this is
//! also what makes finding 4's warm start possible outside an eval script:
//! `warm_start_source` is how a job finds the previous commit's membership
//! to seed the partitioner with.
//!
//! **The map document itself is stored as a content-addressed file next to
//! the database, not a BLOB column.** Two reasons: `geometry::build_from_graph_warm`
//! already writes it to disk as part of running the pipeline (this is how
//! the reference CLI works too, and reproducing that write path rather than
//! also serialising into SQLite avoids doing the ~megabyte JSON encode
//! twice); and serving `GET /api/maps/{owner}/{repo}` can then stream the
//! file straight back without going through the database or an extra
//! deserialise/reserialise round trip. The database row is the index into
//! that file, not a duplicate of its content -- `map_path` is the join key.
//! The path is deterministic (`<cache_dir>/maps/<owner>/<repo>/<commit>.json`),
//! so nothing but the row's existence is actually load-bearing; the column
//! is there for the same reason a lockfile records paths rather than making
//! the reader reconstruct them: fewer places that have to agree on the
//! layout convention.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use crate::naming::{CacheEntry, NameCache};
use crate::schema::MapDocument;
use crate::service::eta::TimingRow;

#[derive(Clone, Debug)]
pub struct MapRow {
    pub slug: String,
    pub owner: String,
    pub repo: String,
    pub commit: String,
    pub branch: Option<String>,
    pub lang: String,
    pub files: i64,
    pub districts: i64,
    pub modularity: f64,
    pub map_path: PathBuf,
    pub indexed_at: String,
}

// Schema lives in one place (here) and migrates forward via `user_version`,
// per CLAUDE.md's "migrate forward" convention -- there is one migration
// today, but the mechanism is the point: a second one appends to this slice
// rather than editing the first.
const MIGRATIONS: &[&str] = &[
    r#"
    CREATE TABLE maps (
        slug        TEXT NOT NULL,
        owner       TEXT NOT NULL,
        repo        TEXT NOT NULL,
        commit_sha  TEXT NOT NULL,
        branch      TEXT,
        lang        TEXT NOT NULL,
        files       INTEGER NOT NULL,
        districts   INTEGER NOT NULL,
        modularity  REAL NOT NULL,
        map_path    TEXT NOT NULL,
        indexed_at  TEXT NOT NULL,
        PRIMARY KEY (slug, commit_sha)
    );
    CREATE INDEX maps_slug_indexed_at ON maps(slug, indexed_at DESC);
"#,
    r#"
    CREATE TABLE district_names (
        slug TEXT NOT NULL,
        fingerprint TEXT NOT NULL,
        entry_json TEXT NOT NULL,
        PRIMARY KEY (slug, fingerprint)
    );
"#,
    r#"
    CREATE TABLE job_timings (
        job_id TEXT PRIMARY KEY,
        features_json TEXT NOT NULL,
        stage_s_json TEXT NOT NULL,
        elapsed_s REAL NOT NULL,
        completed_at TEXT NOT NULL
    );
    CREATE INDEX job_timings_completed ON job_timings(completed_at DESC);
"#,
    // #97 phase 0: the job child's own peak RSS (`jobs::wait_with_peak`),
    // next to its stage durations, so `MemoryModel` can fit from the same
    // rows `EtaModel` does. Nullable: a row from before this migration, and
    // a row from a platform `wait4` does not cover, both read back `None`
    // (CLAUDE.md "migrate forward" -- never edit `job_timings`' first
    // migration above).
    "ALTER TABLE job_timings ADD COLUMN peak_rss_bytes INTEGER;",
    // #97 phase 2 (docs/WORKER_TIER.md §2.2, §6): durable jobs and leases,
    // written only in worker modes (`TOLMAP_WORKERS=loopback:N`); local
    // mode never reads or writes this table. One row per job:
    // - `status` is the master's internal state, not the public
    //   `JobSnapshot.status`: `queued`, `leased`, `running`, `done`,
    //   `failed`. A row never leaves `done` or `failed`: every update below
    //   is guarded on it, which is what keeps a cancel terminal whatever
    //   races it.
    // - `attempt` starts at 1 and counts lost workers (§10.6); a re-queue
    //   for a master restart or a graceful stop leaves it alone.
    // - `epoch` is the epoch of the job's current (or pending) assignment,
    //   0 before the first. It is raised and written when a runner starts
    //   looking for an agent, *before* the `assign` carrying it is sent, so
    //   no epoch number is ever given out twice, even across a crash
    //   between the two.
    // - `queue_order` is the admission sequence. Queues are kept in this
    //   order, and a re-queued job keeps its number, which puts it at the
    //   head of its class queue: every job still queued was admitted after
    //   it started.
    // - `spec_json` is the job's `JobSpec`, which is all a restarted master
    //   needs to rebuild the job; `snapshot_json` the last persisted
    //   `JobSnapshot`, which `GET /api/jobs/{id}` serves after a restart.
    r#"
    CREATE TABLE jobs (
        job_id            TEXT PRIMARY KEY,
        slug              TEXT NOT NULL,
        commit_sha        TEXT NOT NULL,
        spec_json         TEXT NOT NULL,
        class             INTEGER NOT NULL,
        status            TEXT NOT NULL,
        attempt           INTEGER NOT NULL,
        epoch             INTEGER NOT NULL,
        lease_holder      TEXT,
        lease_deadline_ms INTEGER,
        queue_order       INTEGER NOT NULL,
        snapshot_json     TEXT NOT NULL,
        created_at        TEXT NOT NULL,
        updated_at        TEXT NOT NULL
    );
    CREATE INDEX jobs_status_order ON jobs(status, queue_order);
"#,
];

/// The two statuses a `jobs` row never leaves.
const TERMINAL_JOB: &str = "status NOT IN ('done', 'failed')";

/// One `jobs` row (#97 phase 2) -- see the migration above for what each
/// column means. JSON columns stay strings here: the store does not need
/// to know the shapes it keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRow {
    pub job_id: String,
    pub slug: String,
    pub commit: String,
    pub spec_json: String,
    pub class: i64,
    pub status: String,
    pub attempt: i64,
    pub epoch: i64,
    pub lease_holder: Option<String>,
    pub lease_deadline_ms: Option<i64>,
    pub queue_order: i64,
    pub snapshot_json: String,
}

/// What [`Store::requeue_job`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requeued {
    /// Back to `queued`, at this attempt.
    Queued { attempt: i64 },
    /// A counted loss past the retry bound: nothing was written, and the
    /// caller fails the job having lost this many workers.
    Exhausted { lost: i64 },
    /// The row is terminal already (a cancel won), or at another epoch.
    Unchanged,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn save_timing(&self, job_id: &str, row: &TimingRow) -> Result<()> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.execute(
            "INSERT OR REPLACE INTO job_timings (job_id, features_json, stage_s_json, elapsed_s, peak_rss_bytes, completed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job_id,
                serde_json::to_string(&row.features)?,
                serde_json::to_string(&row.stage_s)?,
                row.elapsed_s,
                row.peak_rss_bytes.map(|bytes| bytes as i64),
                crate::service::time::now_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn recent_timings(&self) -> Result<Vec<TimingRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let mut query = conn.prepare("SELECT features_json, stage_s_json, elapsed_s, peak_rss_bytes FROM job_timings ORDER BY completed_at DESC, job_id DESC LIMIT 256")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (features, stage_s, elapsed_s, peak_rss_bytes) = row?;
            Ok(TimingRow {
                features: serde_json::from_str(&features)?,
                stage_s: crate::service::eta::upgrade_stage_layout(serde_json::from_str(&stage_s)?),
                elapsed_s,
                // Negative is impossible from `wait_with_peak`, but a
                // malformed or hand-edited row must not resurrect a bogus
                // peak rather than falling back to "unknown".
                peak_rss_bytes: peak_rss_bytes.and_then(|bytes| u64::try_from(bytes).ok()),
            })
        })
        .collect()
    }

    /// Durable derivation cache, independent of the disposable work directory.
    pub fn load_names(&self, slug: &str) -> Result<NameCache> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let mut statement = conn.prepare("SELECT fingerprint, entry_json FROM district_names WHERE slug = ?1 ORDER BY fingerprint")?;
        let rows = statement.query_map([slug], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut cache = NameCache::new();
        for row in rows {
            let (fingerprint, json) = row?;
            cache.insert(fingerprint, serde_json::from_str::<CacheEntry>(&json)?);
        }
        Ok(cache)
    }

    pub fn save_names(&self, slug: &str, cache: &NameCache) -> Result<()> {
        let mut conn = self.conn.lock().expect("store connection mutex poisoned");
        let tx = conn.transaction()?;
        for (fingerprint, entry) in cache {
            tx.execute("INSERT OR REPLACE INTO district_names (slug, fingerprint, entry_json) VALUES (?1, ?2, ?3)",
                params![slug, fingerprint, serde_json::to_string(entry)?])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create store directory {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite store {}", path.display()))?;
        // Worker hardening (docs/SCIP_SANDBOX.md #4.1 point 4): lock the
        // file itself down to the service's own uid, not its *directory*.
        // The directory is deliberately left alone here -- in today's
        // production deployment's layout the store's directory (`/data`, from
        // `TOLMAP_DB_PATH=/data/tolmap.sqlite3`) is an *ancestor* of
        // `TOLMAP_CACHE_DIR=/data/cache`, and a `0700`-root-owned `/data`
        // would deny the dropped-uid worker child even search permission
        // into `/data/cache/work/.../job_dir`, breaking every job outright
        // -- see `jobs::harden_persistent_dir`'s doc comment for the fuller
        // version of this. Locking the file (not an ancestor of anything
        // the worker needs to reach) gets the same "even a bug that points
        // the worker here can't read it" guarantee without that collateral
        // breakage. Does not cover the `-wal`/`-shm` sidecar files WAL mode
        // below creates lazily on first write -- they inherit the
        // process's umask instead; a real gap, but a narrow one (they hold
        // only recent, not-yet-checkpointed writes, not the whole store).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("harden store file {}", path.display()))?;
        }
        // WAL so a long-running index build's occasional store write does
        // not block concurrent GET /api/maps reads behind it.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        for (offset, migration) in MIGRATIONS.iter().enumerate() {
            let target = offset as i64 + 1;
            if version < target {
                conn.execute_batch(migration)
                    .with_context(|| format!("apply migration {target}"))?;
                conn.pragma_update(None, "user_version", target)?;
            }
        }
        Ok(())
    }

    pub fn get(&self, slug: &str, commit: &str) -> Result<Option<MapRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.query_row(
            "SELECT slug, owner, repo, commit_sha, branch, lang, files, districts, modularity, map_path, indexed_at
             FROM maps WHERE slug = ?1 AND commit_sha = ?2",
            params![slug, commit],
            row_to_map,
        )
        .optional_context()
    }

    /// The most recent indexed commit for `slug`, regardless of branch --
    /// backs `GET /api/maps/{owner}/{repo}` with no `?commit=`.
    pub fn latest(&self, slug: &str) -> Result<Option<MapRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.query_row(
            "SELECT slug, owner, repo, commit_sha, branch, lang, files, districts, modularity, map_path, indexed_at
             FROM maps WHERE slug = ?1 ORDER BY indexed_at DESC LIMIT 1",
            params![slug],
            row_to_map,
        )
        .optional_context()
    }

    /// Where a warm start reads its previous membership from: same branch
    /// preferred (docs/ARCHITECTURE.md: "warm-start from the most recent
    /// prior commit on the same branch"), falling back to the most recent
    /// commit on any branch when there is no same-branch history yet (e.g.
    /// the first index of a new branch) -- a cold start on day one and a
    /// stale-but-related seed on day two both beat no seed at all, and
    /// finding 4 measured warm-starting from an unrelated-but-recent
    /// commit as still a net win, not just same-branch history.
    pub fn warm_start_source(&self, slug: &str, branch: Option<&str>) -> Result<Option<MapRow>> {
        if let Some(branch) = branch {
            let conn = self.conn.lock().expect("store connection mutex poisoned");
            let by_branch = conn
                .query_row(
                    "SELECT slug, owner, repo, commit_sha, branch, lang, files, districts, modularity, map_path, indexed_at
                     FROM maps WHERE slug = ?1 AND branch = ?2 ORDER BY indexed_at DESC LIMIT 1",
                    params![slug, branch],
                    row_to_map,
                )
                .optional_context()?;
            drop(conn);
            if by_branch.is_some() {
                return Ok(by_branch);
            }
        }
        self.latest(slug)
    }

    /// Sent to the child in newest-first order so it can choose the same
    /// branch after clone without giving the child database access.
    pub fn warm_start_candidates(&self, slug: &str) -> Result<Vec<MapRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let mut statement = conn.prepare(
            "SELECT slug, owner, repo, commit_sha, branch, lang, files, districts, modularity, map_path, indexed_at
             FROM maps WHERE slug = ?1 ORDER BY indexed_at DESC"
        )?;
        let rows = statement
            .query_map(params![slug], row_to_map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One row per slug (its most recently indexed commit) -- `GET /api/maps`.
    pub fn list_latest(&self) -> Result<Vec<MapRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let mut statement = conn.prepare(
            "SELECT m.slug, m.owner, m.repo, m.commit_sha, m.branch, m.lang, m.files, m.districts, m.modularity, m.map_path, m.indexed_at
             FROM maps m
             JOIN (SELECT slug, MAX(indexed_at) AS max_at FROM maps GROUP BY slug) latest
               ON m.slug = latest.slug AND m.indexed_at = latest.max_at
             ORDER BY m.indexed_at DESC",
        )?;
        let rows = statement
            .query_map([], row_to_map)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Deletes indexed rows for `slug` beyond the `keep` most-recently
    /// indexed (by `indexed_at`), and removes their map files from disk --
    /// issue #23 gap 2: every `(repo, commit_sha)` ever indexed used to
    /// keep its row and its map file forever, so a service indexing one
    /// repository repeatedly grows without bound.
    ///
    /// `keep` is floored at 1: the single newest row for `slug` is always
    /// kept no matter what is passed, because it is exactly what
    /// `warm_start_source`'s branch-less fallback (and a first-ever index
    /// of a new branch) reads. Evicting it would not just lose a row, it
    /// would cost district retention on the *next* index of this repo
    /// (docs/FINDINGS.md finding 4: warm-starting Leiden from the previous
    /// membership took retention from 46% to 88% on django, at no
    /// modularity cost -- the highest-leverage result in the project).
    /// Callers that want a stricter cap should still pass 1, not 0.
    ///
    /// A commits-per-repo count, not an age or a total-bytes budget, is
    /// the policy chosen here: it is the one that makes "the newest row
    /// survives" true by construction (rank 1 of an `indexed_at DESC`
    /// ordering is always kept for any `keep >= 1`), where an age or byte
    /// budget would need a special case for "unless it is the newest" to
    /// get the same guarantee -- and it maps directly onto the growth this
    /// issue actually describes ("a service indexing a repository per
    /// commit grows without bound"), which is per-repo, not global.
    ///
    /// Called once per slug, right after that slug's `insert`
    /// (`jobs.rs::run_blocking`) -- so the bound holds continuously rather
    /// than needing a separate sweep/cron.
    pub fn prune(&self, slug: &str, keep: usize) -> Result<usize> {
        let keep = keep.max(1);
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let stale: Vec<(String, String)> = {
            let mut statement = conn.prepare(
                "SELECT commit_sha, map_path FROM maps WHERE slug = ?1 ORDER BY indexed_at DESC",
            )?;
            let rows = statement
                .query_map(params![slug], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().skip(keep).collect()
        };
        for (commit, map_path) in &stale {
            conn.execute(
                "DELETE FROM maps WHERE slug = ?1 AND commit_sha = ?2",
                params![slug, commit],
            )?;
            // Best-effort: the row is the source of truth for what is
            // "indexed" (docs/API.md), so a file already missing for
            // whatever reason should not stop the row from being pruned.
            if let Err(err) = std::fs::remove_file(map_path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("prune: could not remove map file {map_path}: {err}");
                }
            }
            let symbols_path = Path::new(map_path).with_extension("symbols.json");
            if let Err(err) = std::fs::remove_file(&symbols_path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "prune: could not remove symbols file {}: {err}",
                        symbols_path.display()
                    );
                }
            }
            let district_path = Path::new(map_path).with_extension("symbols");
            if let Err(err) = std::fs::remove_dir_all(&district_path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "prune: could not remove symbols directory {}: {err}",
                        district_path.display()
                    );
                }
            }
        }
        Ok(stale.len())
    }

    // ---- durable jobs (#97 phase 2), worker modes only ------------------

    /// Admission: the job's first row, before `POST /api/index` answers.
    pub fn insert_job(&self, row: &JobRow) -> Result<()> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let now = crate::service::time::now_rfc3339();
        conn.execute(
            "INSERT INTO jobs (job_id, slug, commit_sha, spec_json, class, status, attempt, epoch,
                               lease_holder, lease_deadline_ms, queue_order, snapshot_json,
                               created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
            params![
                row.job_id,
                row.slug,
                row.commit,
                row.spec_json,
                row.class,
                row.status,
                row.attempt,
                row.epoch,
                row.lease_holder,
                row.lease_deadline_ms,
                row.queue_order,
                row.snapshot_json,
                now,
            ],
        )?;
        Ok(())
    }

    /// Raises a live job's epoch for its next assignment and returns it,
    /// with the job's admission order; `None` when the row is terminal (or
    /// missing), so the job must not be assigned. Written before `assign`
    /// is sent -- see the migration.
    pub fn begin_attempt(&self, job_id: &str) -> Result<Option<(u64, i64)>> {
        let mut conn = self.conn.lock().expect("store connection mutex poisoned");
        let tx = conn.transaction()?;
        let changed = tx.execute(
            &format!(
                "UPDATE jobs SET epoch = epoch + 1, updated_at = ?2 WHERE job_id = ?1 AND {TERMINAL_JOB}"
            ),
            params![job_id, crate::service::time::now_rfc3339()],
        )?;
        let attempt = if changed == 1 {
            Some(tx.query_row(
                "SELECT epoch, queue_order FROM jobs WHERE job_id = ?1",
                params![job_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?)
        } else {
            None
        };
        tx.commit()?;
        Ok(
            attempt
                .and_then(|(epoch, order)| u64::try_from(epoch).ok().map(|epoch| (epoch, order))),
        )
    }

    /// A live job at `epoch` moves to `status` (`leased` or `running`)
    /// with its latest snapshot and lease holder. Guarded on the epoch, so
    /// a runner can only ever write the lease it holds.
    pub fn save_job_state(
        &self,
        job_id: &str,
        epoch: u64,
        status: &str,
        lease_holder: Option<&str>,
        lease_deadline_ms: Option<i64>,
        snapshot_json: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.execute(
            &format!(
                "UPDATE jobs SET status = ?3, lease_holder = COALESCE(?4, lease_holder),
                     lease_deadline_ms = COALESCE(?5, lease_deadline_ms), snapshot_json = ?6,
                     updated_at = ?7
                 WHERE job_id = ?1 AND epoch = ?2 AND {TERMINAL_JOB}"
            ),
            params![
                job_id,
                epoch as i64,
                status,
                lease_holder,
                lease_deadline_ms,
                snapshot_json,
                crate::service::time::now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Puts a live job at `epoch` back to `queued` with `snapshot_json`
    /// (its stages reset) and no lease. A `counted` re-queue is a lost
    /// worker (§10.6): it raises `attempt`, unless `retries` lost-worker
    /// retries are used up already, in which case nothing is written and
    /// the caller fails the job. One transaction, so the bound is decided
    /// on the value it updates.
    pub fn requeue_job(
        &self,
        job_id: &str,
        epoch: u64,
        counted: bool,
        retries: u32,
        snapshot_json: &str,
    ) -> Result<Requeued> {
        let mut conn = self.conn.lock().expect("store connection mutex poisoned");
        let tx = conn.transaction()?;
        let attempt: Option<i64> = tx
            .query_row(
                &format!(
                    "SELECT attempt FROM jobs WHERE job_id = ?1 AND epoch = ?2 AND {TERMINAL_JOB}"
                ),
                params![job_id, epoch as i64],
                |row| row.get(0),
            )
            .optional_context()?;
        let Some(attempt) = attempt else {
            return Ok(Requeued::Unchanged);
        };
        // `attempt` counts the workers the job has been on; the retries
        // used so far are one fewer, so the bound is reached once
        // `attempt - 1 >= retries`.
        if counted && attempt > i64::from(retries) {
            return Ok(Requeued::Exhausted { lost: attempt });
        }
        let attempt = attempt + i64::from(counted);
        tx.execute(
            "UPDATE jobs SET status = 'queued', attempt = ?3, lease_holder = NULL,
                 lease_deadline_ms = NULL, snapshot_json = ?4, updated_at = ?5
             WHERE job_id = ?1 AND epoch = ?2",
            params![
                job_id,
                epoch as i64,
                attempt,
                snapshot_json,
                crate::service::time::now_rfc3339()
            ],
        )?;
        tx.commit()?;
        Ok(Requeued::Queued { attempt })
    }

    /// A job's terminal snapshot. The first terminal write wins: callers
    /// write the in-memory snapshot, which is itself terminal once and for
    /// all (`jobs::finish_failed`, `jobs::finish_done`), so every writer of
    /// one job writes the same terminal state.
    pub fn finish_job(&self, job_id: &str, status: &str, snapshot_json: &str) -> Result<()> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.execute(
            &format!(
                "UPDATE jobs SET status = ?2, lease_holder = NULL, lease_deadline_ms = NULL,
                     snapshot_json = ?3, updated_at = ?4
                 WHERE job_id = ?1 AND {TERMINAL_JOB}"
            ),
            params![
                job_id,
                status,
                snapshot_json,
                crate::service::time::now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Every job not yet `done` or `failed`, in admission order: what a
    /// restarted master reloads.
    pub fn live_jobs(&self) -> Result<Vec<JobRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        let mut statement = conn.prepare(&format!(
            "SELECT job_id, slug, commit_sha, spec_json, class, status, attempt, epoch,
                    lease_holder, lease_deadline_ms, queue_order, snapshot_json
             FROM jobs WHERE {TERMINAL_JOB} ORDER BY queue_order, job_id"
        ))?;
        let rows = statement
            .query_map([], row_to_job)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn job(&self, job_id: &str) -> Result<Option<JobRow>> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.query_row(
            "SELECT job_id, slug, commit_sha, spec_json, class, status, attempt, epoch,
                    lease_holder, lease_deadline_ms, queue_order, snapshot_json
             FROM jobs WHERE job_id = ?1",
            params![job_id],
            row_to_job,
        )
        .optional_context()
    }

    /// The largest admission number ever given out, so a restarted master
    /// continues after it.
    pub fn max_job_order(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        Ok(conn.query_row(
            "SELECT COALESCE(MAX(queue_order), 0) FROM jobs",
            [],
            |row| row.get(0),
        )?)
    }

    /// Keeps the newest `keep` terminal rows, so `GET /api/jobs/{id}` still
    /// answers for recent jobs after a restart without the table growing
    /// forever. Live rows are never touched.
    pub fn prune_finished_jobs(&self, keep: usize) -> Result<usize> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        Ok(conn.execute(
            "DELETE FROM jobs WHERE status IN ('done', 'failed') AND job_id NOT IN (
                 SELECT job_id FROM jobs WHERE status IN ('done', 'failed')
                 ORDER BY updated_at DESC, job_id DESC LIMIT ?1)",
            params![keep as i64],
        )?)
    }

    pub fn insert(&self, row: &MapRow) -> Result<()> {
        let conn = self.conn.lock().expect("store connection mutex poisoned");
        conn.execute(
            "INSERT INTO maps (slug, owner, repo, commit_sha, branch, lang, files, districts, modularity, map_path, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (slug, commit_sha) DO UPDATE SET
               branch = excluded.branch, lang = excluded.lang, files = excluded.files,
               districts = excluded.districts, modularity = excluded.modularity,
               map_path = excluded.map_path, indexed_at = excluded.indexed_at",
            params![
                row.slug,
                row.owner,
                row.repo,
                row.commit,
                row.branch,
                row.lang,
                row.files,
                row.districts,
                row.modularity,
                row.map_path.to_string_lossy(),
                row.indexed_at,
            ],
        )?;
        Ok(())
    }
}

fn row_to_map(row: &rusqlite::Row) -> rusqlite::Result<MapRow> {
    Ok(MapRow {
        slug: row.get(0)?,
        owner: row.get(1)?,
        repo: row.get(2)?,
        commit: row.get(3)?,
        branch: row.get(4)?,
        lang: row.get(5)?,
        files: row.get(6)?,
        districts: row.get(7)?,
        modularity: row.get(8)?,
        map_path: PathBuf::from(row.get::<_, String>(9)?),
        indexed_at: row.get(10)?,
    })
}

fn row_to_job(row: &rusqlite::Row) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        job_id: row.get(0)?,
        slug: row.get(1)?,
        commit: row.get(2)?,
        spec_json: row.get(3)?,
        class: row.get(4)?,
        status: row.get(5)?,
        attempt: row.get(6)?,
        epoch: row.get(7)?,
        lease_holder: row.get(8)?,
        lease_deadline_ms: row.get(9)?,
        queue_order: row.get(10)?,
        snapshot_json: row.get(11)?,
    })
}

// rusqlite's QueryReturnedNoRows is its `Option`-shaped case; the rest of
// this service wants `Result<Option<T>>` so a genuine query error is not
// silently swallowed alongside "no such row".
trait OptionalContext<T> {
    fn optional_context(self) -> Result<Option<T>>;
}

impl<T> OptionalContext<T> for rusqlite::Result<T> {
    fn optional_context(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

/// Reads a previously-written `MapDocument` back off disk, for warm-start
/// membership lookup.
pub fn read_map_document(path: &Path) -> Result<MapDocument> {
    let raw =
        std::fs::read(path).with_context(|| format!("read map document {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parse map document {}", path.display()))
}

/// `file -> district` out of a `MapDocument`, for
/// `pipeline::align_initial_membership`. `files[i]` and `nodes[i]` are
/// parallel arrays by construction (`geometry::compact`).
pub fn membership_by_file(document: &MapDocument) -> BTreeMap<String, usize> {
    document
        .files
        .iter()
        .cloned()
        .zip(document.nodes.iter().map(|node| node.district()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_timings_survive_store_reopen_for_online_refit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timings.sqlite3");
        let row = TimingRow {
            features: crate::worker::RepoFeatures {
                clone_bytes: Some(1024),
                commits: Some(42),
                languages: [(
                    "py".to_owned(),
                    crate::worker::LanguageFeatures {
                        files: 12,
                        bytes: 8000,
                    },
                )]
                .into(),
                refs: None,
                install: None,
            },
            elapsed_s: 4.0,
            stage_s: vec![Some(1.0), None, Some(2.0)],
            peak_rss_bytes: Some(456_789_012),
        };
        Store::open(&path)
            .unwrap()
            .save_timing("job-1", &row)
            .unwrap();
        let reopened = Store::open(&path).unwrap();
        let saved = reopened.recent_timings().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].features.languages["py"].files, 12);
        assert_eq!(saved[0].stage_s, row.stage_s);
        assert_eq!(saved[0].peak_rss_bytes, Some(456_789_012));
    }

    #[test]
    fn a_timing_row_with_no_peak_reads_back_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("timings.sqlite3");
        let row = TimingRow {
            features: crate::worker::RepoFeatures::default(),
            elapsed_s: 1.0,
            stage_s: vec![None],
            peak_rss_bytes: None,
        };
        let store = Store::open(&path).unwrap();
        store.save_timing("job-2", &row).unwrap();
        let saved = store.recent_timings().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].peak_rss_bytes, None);
    }

    fn job_row(id: &str, order: i64) -> JobRow {
        JobRow {
            job_id: id.to_owned(),
            slug: format!("o/{id}"),
            commit: "c0".to_owned(),
            spec_json: "{}".to_owned(),
            class: 0,
            status: "queued".to_owned(),
            attempt: 1,
            epoch: 0,
            lease_holder: None,
            lease_deadline_ms: None,
            queue_order: order,
            snapshot_json: "{}".to_owned(),
        }
    }

    /// #97 phase 2: each assignment raises the epoch first; a counted
    /// re-queue raises the attempt until the retry bound, then refuses; an
    /// uncounted one never does; and a terminal row stays terminal whatever
    /// is written after it.
    #[test]
    fn job_rows_fence_epochs_bound_retries_and_stay_terminal() {
        let (_dir, store) = temp_store();
        store.insert_job(&job_row("a", 1)).unwrap();
        assert_eq!(store.begin_attempt("a").unwrap(), Some((1, 1)));
        store
            .save_job_state("a", 1, "leased", Some("agent 0"), Some(5), "{\"s\":1}")
            .unwrap();
        // A write for another epoch is ignored.
        store
            .save_job_state("a", 7, "running", None, None, "{\"s\":7}")
            .unwrap();
        let row = store.job("a").unwrap().unwrap();
        assert_eq!(row.status, "leased");
        assert_eq!(row.lease_holder.as_deref(), Some("agent 0"));
        assert_eq!(row.snapshot_json, "{\"s\":1}");
        // Two lost-worker retries, then the bound.
        assert_eq!(
            store.requeue_job("a", 1, true, 2, "{}").unwrap(),
            Requeued::Queued { attempt: 2 }
        );
        assert_eq!(store.begin_attempt("a").unwrap(), Some((2, 1)));
        assert_eq!(
            store.requeue_job("a", 2, false, 2, "{}").unwrap(),
            Requeued::Queued { attempt: 2 },
            "a restart or a graceful stop is not a lost worker"
        );
        assert_eq!(store.begin_attempt("a").unwrap(), Some((3, 1)));
        assert_eq!(
            store.requeue_job("a", 3, true, 2, "{}").unwrap(),
            Requeued::Queued { attempt: 3 }
        );
        assert_eq!(store.begin_attempt("a").unwrap(), Some((4, 1)));
        assert_eq!(
            store.requeue_job("a", 4, true, 2, "{}").unwrap(),
            Requeued::Exhausted { lost: 3 }
        );
        assert_eq!(
            store.requeue_job("a", 3, true, 2, "{}").unwrap(),
            Requeued::Unchanged,
            "a superseded epoch cannot re-queue"
        );
        store.finish_job("a", "failed", "{\"end\":1}").unwrap();
        store.finish_job("a", "done", "{\"end\":2}").unwrap();
        store
            .save_job_state("a", 4, "running", None, None, "{}")
            .unwrap();
        assert_eq!(store.begin_attempt("a").unwrap(), None);
        assert_eq!(
            store.requeue_job("a", 4, false, 2, "{}").unwrap(),
            Requeued::Unchanged
        );
        let row = store.job("a").unwrap().unwrap();
        assert_eq!(
            (row.status.as_str(), row.snapshot_json.as_str()),
            ("failed", "{\"end\":1}")
        );
        assert!(store.live_jobs().unwrap().is_empty());
    }

    #[test]
    fn live_jobs_come_back_in_admission_order_and_old_finished_ones_are_pruned() {
        let (_dir, store) = temp_store();
        for (id, order) in [("c", 3), ("a", 1), ("b", 2), ("d", 4)] {
            store.insert_job(&job_row(id, order)).unwrap();
        }
        store.finish_job("d", "done", "{}").unwrap();
        let live: Vec<String> = store
            .live_jobs()
            .unwrap()
            .into_iter()
            .map(|row| row.job_id)
            .collect();
        assert_eq!(live, ["a", "b", "c"]);
        assert_eq!(store.max_job_order().unwrap(), 4);
        assert_eq!(store.prune_finished_jobs(0).unwrap(), 1);
        assert!(store.job("d").unwrap().is_none());
        assert_eq!(store.live_jobs().unwrap().len(), 3);
    }

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
        (dir, store)
    }

    fn row(slug: &str, commit: &str, indexed_at: &str, map_path: &Path) -> MapRow {
        MapRow {
            slug: slug.to_owned(),
            owner: "o".to_owned(),
            repo: "r".to_owned(),
            commit: commit.to_owned(),
            branch: Some("main".to_owned()),
            lang: "py".to_owned(),
            files: 1,
            districts: 1,
            modularity: 0.1,
            map_path: map_path.to_owned(),
            indexed_at: indexed_at.to_owned(),
        }
    }

    #[test]
    fn prune_keeps_only_the_newest_n_and_deletes_older_map_files() {
        let (dir, store) = temp_store();
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| dir.path().join(format!("map{i}.json")))
            .collect();
        for path in &paths {
            std::fs::write(path, b"{}").unwrap();
            let district_dir = path.with_extension("symbols");
            std::fs::create_dir(&district_dir).unwrap();
            std::fs::write(district_dir.join("0.json"), b"{}").unwrap();
        }
        store
            .insert(&row("o/r", "c0", "2024-01-01T00:00:00Z", &paths[0]))
            .unwrap();
        store
            .insert(&row("o/r", "c1", "2024-01-02T00:00:00Z", &paths[1]))
            .unwrap();
        store
            .insert(&row("o/r", "c2", "2024-01-03T00:00:00Z", &paths[2]))
            .unwrap();

        let pruned = store.prune("o/r", 2).unwrap();
        assert_eq!(pruned, 1);
        assert!(
            store.get("o/r", "c0").unwrap().is_none(),
            "oldest row should be pruned"
        );
        assert!(store.get("o/r", "c1").unwrap().is_some());
        assert!(store.get("o/r", "c2").unwrap().is_some());
        assert!(
            !paths[0].exists(),
            "pruned row's map file should be removed from disk"
        );
        assert!(
            !paths[0].with_extension("symbols").exists(),
            "pruned row's district symbols should be removed from disk"
        );
        assert!(paths[1].exists());
        assert!(paths[2].exists());
        assert!(paths[1].with_extension("symbols").exists());
        assert!(paths[2].with_extension("symbols").exists());
    }

    #[test]
    fn prune_never_evicts_the_newest_row_even_when_asked_to_keep_zero() {
        let (dir, store) = temp_store();
        let p0 = dir.path().join("m0.json");
        let p1 = dir.path().join("m1.json");
        std::fs::write(&p0, b"{}").unwrap();
        std::fs::write(&p1, b"{}").unwrap();
        store
            .insert(&row("o/r", "c0", "2024-01-01T00:00:00Z", &p0))
            .unwrap();
        store
            .insert(&row("o/r", "c1", "2024-01-02T00:00:00Z", &p1))
            .unwrap();

        store.prune("o/r", 0).unwrap();
        assert!(
            store.get("o/r", "c1").unwrap().is_some(),
            "the newest row must survive even when keep=0 is requested"
        );
    }

    #[test]
    fn prune_does_not_touch_other_slugs() {
        let (dir, store) = temp_store();
        let pa = dir.path().join("a.json");
        let pb = dir.path().join("b.json");
        std::fs::write(&pa, b"{}").unwrap();
        std::fs::write(&pb, b"{}").unwrap();
        store
            .insert(&row("a/a", "c0", "2024-01-01T00:00:00Z", &pa))
            .unwrap();
        store
            .insert(&row("b/b", "c0", "2024-01-01T00:00:00Z", &pb))
            .unwrap();

        store.prune("a/a", 0).unwrap();
        assert!(store.get("a/a", "c0").unwrap().is_some());
        assert!(
            store.get("b/b", "c0").unwrap().is_some(),
            "pruning one slug must not evict another slug's rows"
        );
    }

    /// Finding 4: the warm start reads the *previous* commit's membership.
    /// `jobs.rs::run_blocking` prunes right after inserting each new row --
    /// this reproduces that sequence (insert, prune, insert, prune, ...)
    /// and checks that a warm start launched after each prune still finds
    /// a row, even though the row it finds was not the very first commit
    /// ever indexed for this slug (and was itself later pruned in turn).
    #[test]
    fn warm_start_source_still_finds_a_row_after_prune_evicts_the_one_before_it() {
        let (dir, store) = temp_store();
        let p0 = dir.path().join("m0.json");
        let p1 = dir.path().join("m1.json");
        let p2 = dir.path().join("m2.json");
        for path in [&p0, &p1, &p2] {
            std::fs::write(path, b"{}").unwrap();
        }

        store
            .insert(&row("o/r", "c0", "2024-01-01T00:00:00Z", &p0))
            .unwrap();
        store.prune("o/r", 1).unwrap();

        // A job indexing c1 would warm-start from c0 here, before this
        // prune (keep=1) runs and evicts c0.
        store
            .insert(&row("o/r", "c1", "2024-01-02T00:00:00Z", &p1))
            .unwrap();
        store.prune("o/r", 1).unwrap();
        assert!(
            store.get("o/r", "c0").unwrap().is_none(),
            "c0 should have been pruned once c1 was indexed"
        );
        let source = store
            .warm_start_source("o/r", Some("main"))
            .unwrap()
            .expect("a warm start source must still exist after pruning c0");
        assert_eq!(
            source.commit, "c1",
            "warm start must find c1, the surviving row, not the pruned c0"
        );

        // A job indexing c2 would warm-start from c1 here, before this
        // prune (keep=1) runs and evicts c1 in turn.
        store
            .insert(&row("o/r", "c2", "2024-01-03T00:00:00Z", &p2))
            .unwrap();
        store.prune("o/r", 1).unwrap();
        assert!(store.get("o/r", "c1").unwrap().is_none());
        let source = store
            .warm_start_source("o/r", Some("main"))
            .unwrap()
            .expect("a warm start source must still exist after pruning c1");
        assert_eq!(
            source.commit, "c2",
            "warm start must find c2 after c1 -- its own former warm-start \
             source -- was pruned"
        );
    }
}
