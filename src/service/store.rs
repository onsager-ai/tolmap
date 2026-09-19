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

use crate::schema::MapDocument;

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
const MIGRATIONS: &[&str] = &[r#"
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
"#];

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create store directory {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite store {}", path.display()))?;
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
