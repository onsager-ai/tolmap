//! Extraction reuse only. Resolvers, history and geometry always run again.
//! A cache is derivation, never authority: embedding hosts must check current
//! access on every lookup and publication through their Policy adapter.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Clone)]
pub struct Cache {
    root: PathBuf,
    pub domain: String,
    pub repository: String,
    authority: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Cache {
    /// Local callers rely on filesystem authority. Hosts use `with_authority`
    /// and an identity constructed by a trusted adapter, never request headers.
    pub fn local(root: PathBuf, repository: String) -> Self {
        Self::with_authority(root, "local-filesystem".into(), repository, || true)
    }

    pub fn with_authority(
        root: PathBuf,
        domain: String,
        repository: String,
        authority: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        let namespace = digest(&serde_json::to_vec(&(&domain, &repository)).unwrap());
        Self {
            root: root.join(namespace),
            domain,
            repository,
            authority: Arc::new(authority),
        }
    }

    pub(crate) fn read<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        ensure!((self.authority)(), "cache_authority_revoked");
        let path = self.root.join(format!("{key}.json"));
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return Ok(None);
        };
        if !metadata.file_type().is_file() {
            return Ok(None);
        }
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(None);
        };
        let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) else {
            return Ok(None);
        };
        if digest(envelope.payload.as_bytes()) != envelope.sha256 {
            return Ok(None);
        }
        Ok(serde_json::from_str(&envelope.payload).ok())
    }

    pub(crate) fn write<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        ensure!((self.authority)(), "cache_authority_revoked");
        private_dir(&self.root)?;
        let payload = serde_json::to_string(value)?;
        let bytes = serde_json::to_vec(&Envelope {
            sha256: digest(payload.as_bytes()),
            payload,
        })?;
        atomic_write(&self.root.join(format!("{key}.json")), &bytes)
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    sha256: String,
    payload: String,
}

pub(crate) fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Provenance {
    pub domain: String,
    pub repository: String,
    pub extractor: String,
    pub schema: String,
    pub resolver: String,
    pub indexer: String,
    pub config: String,
    #[serde(default)]
    pub history_revision: String,
    pub files: BTreeMap<String, String>,
}

impl Provenance {
    pub fn reasons(&self, previous: Option<&Self>) -> Vec<&'static str> {
        let Some(old) = previous else {
            return vec!["cold_cache"];
        };
        let mut reasons = vec![];
        for (changed, reason) in [
            (
                self.domain != old.domain || self.repository != old.repository,
                "repository_or_domain_changed",
            ),
            (self.extractor != old.extractor, "extractor_changed"),
            (self.schema != old.schema, "schema_changed"),
            (self.resolver != old.resolver, "resolver_changed"),
            (self.indexer != old.indexer, "indexer_changed"),
            (self.config != old.config, "config_or_exclusions_changed"),
        ] {
            if changed {
                reasons.push(reason);
            }
        }
        if self.history_revision != old.history_revision {
            reasons.push("history_window_advanced");
        }
        let removed: Vec<_> = old
            .files
            .iter()
            .filter(|(p, _)| !self.files.contains_key(*p))
            .collect();
        let added: Vec<_> = self
            .files
            .iter()
            .filter(|(p, _)| !old.files.contains_key(*p))
            .collect();
        if !removed.is_empty() {
            reasons.push("file_deleted");
        }
        if !added.is_empty() {
            reasons.push("file_added");
        }
        if removed
            .iter()
            .any(|(_, hash)| added.iter().any(|(_, new)| hash == new))
        {
            reasons.push("file_renamed");
        }
        if self
            .files
            .iter()
            .any(|(p, h)| old.files.get(p).is_some_and(|old| old != h))
        {
            reasons.push("content_changed");
        }
        reasons
    }

    pub fn compatible(&self, previous: &Self) -> bool {
        self.domain == previous.domain
            && self.repository == previous.repository
            && self.extractor == previous.extractor
            && self.schema == previous.schema
            && self.resolver == previous.resolver
            && self.indexer == previous.indexer
            && self.config == previous.config
    }
}

/// JSON object keys cannot encode scoped Rust syntax paths (Vec<String>).
/// Preserve their BTreeMap order as a sequence of pairs.
pub(crate) mod ordered_pairs {
    use super::*;
    pub fn serialize<K: Serialize, V: Serialize, S: serde::Serializer>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }
    pub fn deserialize<
        'de,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: serde::Deserializer<'de>,
    >(
        deserializer: D,
    ) -> Result<BTreeMap<K, V>, D::Error> {
        Ok(Vec::<(K, V)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[test]
    fn cache_is_owner_scoped_revocable_and_corruption_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let allowed = Arc::new(AtomicBool::new(true));
        let gate = allowed.clone();
        let cache =
            Cache::with_authority(dir.path().into(), "a".into(), "repo".into(), move || {
                gate.load(Ordering::SeqCst)
            });
        cache.write("entry", &vec![1, 2]).unwrap();
        assert_eq!(cache.read::<Vec<u8>>("entry").unwrap(), Some(vec![1, 2]));
        let other = Cache::with_authority(dir.path().into(), "b".into(), "repo".into(), || true);
        assert_eq!(other.read::<Vec<u8>>("entry").unwrap(), None);
        allowed.store(false, Ordering::SeqCst);
        assert!(cache.read::<Vec<u8>>("entry").is_err());
        assert!(cache.write("entry", &vec![3]).is_err());
        allowed.store(true, Ordering::SeqCst);
        std::fs::write(cache.root.join("entry.json"), b"partial").unwrap();
        assert_eq!(cache.read::<Vec<u8>>("entry").unwrap(), None);
    }
}
