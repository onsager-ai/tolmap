//! Personal registry and retained snapshots. Public catalogue keys stay unchanged.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub location: String,
    pub observed_head: Option<String>,
    pub observed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersonalSnapshot {
    pub id: String,
    pub repository: String,
    pub commit: String,
    pub map_path: PathBuf,
    pub source_path: PathBuf,
    pub indexed_at: String,
}

impl Store {
    pub fn repositories(&self) -> Result<Vec<Repository>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id,name,kind,location,observed_head,observed_at FROM repositories ORDER BY name,id")?;
        let rows = stmt.query_map([], |r| {
            Ok(Repository {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                location: r.get(3)?,
                observed_head: r.get(4)?,
                observed_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn register(&self, repo: &Repository) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO repositories(id,name,kind,location) VALUES (?1,?2,?3,?4)",
            params![repo.id, repo.name, repo.kind, repo.location],
        )?;
        Ok(())
    }

    pub fn relink(&self, id: &str, location: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE repositories SET location=?2 WHERE id=?1",
            params![id, location],
        )?;
        Ok(())
    }

    pub fn observe(&self, id: &str, commit: &str) -> Result<()> {
        self.conn.lock().unwrap().execute("UPDATE repositories SET observed_head=?2,observed_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1", params![id,commit])?;
        Ok(())
    }

    pub fn personal_history(&self, id: &str) -> Result<Vec<PersonalSnapshot>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id,repository,commit_sha,map_path,source_path,indexed_at FROM personal_snapshots WHERE repository=?1 ORDER BY sequence DESC")?;
        let rows = stmt.query_map([id], |r| {
            Ok(PersonalSnapshot {
                id: r.get(0)?,
                repository: r.get(1)?,
                commit: r.get(2)?,
                map_path: PathBuf::from(r.get::<_, String>(3)?),
                source_path: PathBuf::from(r.get::<_, String>(4)?),
                indexed_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Called only while holding the repository's process lock. A dead writer
    /// cannot still publish, so its unfinished attempt can now become terminal.
    pub fn personal_begin(&self, repository: &str, id: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("UPDATE personal_attempts SET status='failed',error='interrupted writer',finished_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE repository=?1 AND status='running'", [repository])?;
        tx.execute(
            "INSERT INTO personal_attempts(id,repository,status) VALUES (?1,?2,'running')",
            params![id, repository],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn personal_finish(
        &self,
        id: &str,
        result: std::result::Result<&PersonalSnapshot, &str>,
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if let Ok(snapshot) = result {
            tx.execute("INSERT INTO personal_snapshots(id,repository,commit_sha,map_path,source_path) VALUES (?1,?2,?3,?4,?5)",params![snapshot.id,snapshot.repository,snapshot.commit,snapshot.map_path.to_string_lossy(),snapshot.source_path.to_string_lossy()])?;
        }
        tx.execute("UPDATE personal_attempts SET status=?2,error=?3,finished_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1 AND status='running'",params![id,if result.is_ok() {"done"} else {"failed"},result.err()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn personal_attempt(&self, repository: &str) -> Result<Option<(String, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT status,error FROM personal_attempts WHERE repository=?1 ORDER BY rowid DESC LIMIT 1", [repository], |r| Ok((r.get(0)?, r.get(1)?))).optional_context()
    }

    pub fn remove_repository(&self, id: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM personal_snapshots WHERE repository=?1", [id])?;
        tx.execute("DELETE FROM personal_attempts WHERE repository=?1", [id])?;
        tx.execute("DELETE FROM repositories WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_commits_keep_ids_and_failed_attempt_keeps_latest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db")).unwrap();
        store
            .register(&Repository {
                id: "r".into(),
                name: "repo".into(),
                kind: "local".into(),
                location: "/repo".into(),
                observed_head: None,
                observed_at: None,
            })
            .unwrap();
        for id in ["first", "second"] {
            store.personal_begin("r", id).unwrap();
            store
                .personal_finish(
                    id,
                    Ok(&PersonalSnapshot {
                        id: id.into(),
                        repository: "r".into(),
                        commit: "a".repeat(40),
                        map_path: dir.path().join(id),
                        source_path: dir.path().join("source"),
                        indexed_at: String::new(),
                    }),
                )
                .unwrap();
        }
        store.personal_begin("r", "crashed").unwrap();
        store.personal_begin("r", "retry").unwrap();
        store.personal_finish("retry", Err("failed")).unwrap();
        assert_eq!(
            store
                .personal_history("r")
                .unwrap()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert_eq!(store.personal_attempt("r").unwrap().unwrap().0, "failed");
        store.remove_repository("r").unwrap();
        assert!(store.repositories().unwrap().is_empty());
    }
}
