//! Shared personal CLI/web settings. Repository intent is a separate file.
use super::config::ServeConfig;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ApplicationConfig {
    pub data: PathBuf,
    pub service: ServeConfig,
    /// None retains every personal snapshot.
    pub retain_commits_per_repo: Option<usize>,
}

impl ApplicationConfig {
    pub fn load(data: Option<&Path>) -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|p| p.join(".config")))
            .context("set HOME or XDG_CONFIG_HOME")?
            .join("tolmap/config.toml");
        let text = match std::fs::read_to_string(&config) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).context("read application config"),
        };
        let table: toml::Table = toml::from_str(&text).context("parse application config")?;
        Self::from_settings(
            data,
            &table,
            |key| std::env::var(key).ok(),
            home.as_deref(),
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .as_deref(),
        )
    }

    fn from_settings(
        flag: Option<&Path>,
        file: &toml::Table,
        env: impl Fn(&str) -> Option<String>,
        home: Option<&Path>,
        xdg: Option<&Path>,
    ) -> Result<Self> {
        let setting = |key: &str| -> Option<String> {
            env(&format!("TOLMAP_{}", key.to_ascii_uppercase())).or_else(|| {
                file.get(key).map(|v| match v {
                    toml::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                })
            })
        };
        let env_root = env("TOLMAP_DATA_DIR");
        let file_root = file.get("data_dir").and_then(|v| v.as_str());
        let data = flag
            .map(PathBuf::from)
            .or_else(|| env_root.as_ref().map(PathBuf::from))
            .or_else(|| file_root.map(PathBuf::from))
            .or_else(|| xdg.map(|p| p.join("tolmap")))
            .or_else(|| home.map(|p| p.join(".local/share/tolmap")))
            .context("set HOME, XDG_DATA_HOME, TOLMAP_DATA_DIR or --data")?;
        let root_priority = if flag.is_some() {
            3
        } else if env_root.is_some() {
            2
        } else if file_root.is_some() {
            1
        } else {
            0
        };
        let individual = |key: &str, default: PathBuf| {
            if root_priority <= 2 {
                if let Some(v) = env(&format!("TOLMAP_{}", key.to_ascii_uppercase())) {
                    return PathBuf::from(v);
                }
            }
            if root_priority <= 1 {
                if let Some(v) = file.get(key).and_then(|v| v.as_str()) {
                    return PathBuf::from(v);
                }
            }
            default
        };
        let mut service = ServeConfig::from_env();
        service.bind = "127.0.0.1:8787".parse().unwrap();
        service.static_dir = None;
        service.prune_variant = crate::pipeline::PruneVariant::default();
        service.namer = crate::naming::NamerKind::Idf;
        service.namer_model = crate::naming::DEFAULT_MODEL.into();
        service.refs = crate::extract::RefsMode::default();
        service.scip_install = false;
        service.limits = super::config::Limits::default();

        service.db_path = individual("db_path", data.join("tolmap.sqlite3"));
        service.cache_dir = individual("cache_dir", data.join("cache"));
        if let Some(v) = setting("bind_addr") {
            if let Ok(address) = v.parse::<std::net::SocketAddr>() {
                service.bind = address;
            } else {
                service.bind.set_ip(v.parse().context("bind_addr")?);
            }
        }
        if let Some(v) = setting("port") {
            service.bind.set_port(v.parse().context("port")?);
        }
        if let Some(v) = setting("static_dir") {
            service.static_dir = Some(v.into());
        }
        if let Some(v) = setting("prune_variant") {
            service.prune_variant = v.parse().map_err(anyhow::Error::msg)?;
        }
        if let Some(v) = setting("namer") {
            service.namer = v.parse().map_err(anyhow::Error::msg)?;
        }
        if let Some(v) = setting("namer_model") {
            service.namer_model = v;
        }
        if let Some(v) = setting("refs") {
            service.refs = v.parse().map_err(anyhow::Error::msg)?;
        }
        if let Some(v) = setting("scip_install") {
            anyhow::ensure!(
                matches!(v.as_str(), "off" | "sandbox"),
                "scip_install must be off or sandbox"
            );
            service.scip_install = v == "sandbox";
        }
        if let Some(v) = setting("clone_cache_bytes") {
            service.limits.clone_cache_bytes = v.parse().context("clone_cache_bytes")?;
        }
        if let Some(v) = setting("max_concurrent_jobs") {
            service.limits.max_concurrent_jobs = v.parse().context("max_concurrent_jobs")?;
        }
        anyhow::ensure!(
            service.limits.max_concurrent_jobs > 0,
            "max_concurrent_jobs must be positive"
        );
        if let Some(v) = setting("max_queued_jobs") {
            service.limits.max_queued_jobs = v.parse().context("max_queued_jobs")?;
        }
        let retain_commits_per_repo = setting("retain_commits_per_repo")
            .map(|v| v.parse().context("retain_commits_per_repo"))
            .transpose()?;
        Ok(Self {
            data,
            service,
            retain_commits_per_repo,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bind_accepts_socket_address_and_explicit_port_wins() {
        let file: toml::Table = toml::from_str("bind_addr='0.0.0.0:8988'").unwrap();
        let config = ApplicationConfig::from_settings(
            None,
            &file,
            |_| None,
            Some(Path::new("/home/fixture")),
            None,
        )
        .unwrap();
        assert_eq!(config.service.bind, "0.0.0.0:8988".parse().unwrap());
        let env = |key: &str| (key == "TOLMAP_PORT").then(|| "9000".into());
        let config = ApplicationConfig::from_settings(
            None,
            &file,
            env,
            Some(Path::new("/home/fixture")),
            None,
        )
        .unwrap();
        assert_eq!(config.service.bind, "0.0.0.0:9000".parse().unwrap());
    }

    #[test]
    fn root_and_individual_priority_and_personal_retention() {
        let file: toml::Table = toml::from_str(
            "data_dir='/file'\ndb_path='/file-db'\nrefs='hand'\nmax_concurrent_jobs=2",
        )
        .unwrap();
        let env = |key: &str| match key {
            "TOLMAP_DATA_DIR" => Some("/env".into()),
            "TOLMAP_CACHE_DIR" => Some("/env-cache".into()),
            _ => None,
        };
        let c = ApplicationConfig::from_settings(None, &file, env, None, None).unwrap();
        assert_eq!(c.service.db_path, Path::new("/env/tolmap.sqlite3"));
        assert_eq!(c.service.cache_dir, Path::new("/env-cache"));
        assert_eq!(c.service.limits.max_concurrent_jobs, 2);
        assert!(c.retain_commits_per_repo.is_none());
        let c = ApplicationConfig::from_settings(Some(Path::new("/flag")), &file, env, None, None)
            .unwrap();
        assert_eq!(c.service.cache_dir, Path::new("/flag/cache"));
        let c = ApplicationConfig::from_settings(None, &file, |_| None, None, None).unwrap();
        assert_eq!(c.service.db_path, Path::new("/file-db"));
    }

    #[test]
    fn malformed_numeric_settings_name_the_setting_in_file_and_environment() {
        for (key, value) in [
            ("clone_cache_bytes", "invalid"),
            ("max_concurrent_jobs", "invalid"),
            ("max_queued_jobs", "invalid"),
            ("retain_commits_per_repo", "invalid"),
        ] {
            let file: toml::Table = format!("{key}='{value}'").parse().unwrap();
            let error = ApplicationConfig::from_settings(
                None,
                &file,
                |_| None,
                Some(Path::new("/home/fixture")),
                None,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(key));

            let env_key = format!("TOLMAP_{}", key.to_ascii_uppercase());
            let env = |candidate: &str| (candidate == env_key).then(|| value.to_string());
            let error = ApplicationConfig::from_settings(
                None,
                &toml::Table::new(),
                env,
                Some(Path::new("/home/fixture")),
                None,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(key));
        }
    }
}
