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
];

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(paths[1].exists());
        assert!(paths[2].exists());
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
