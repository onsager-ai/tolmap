use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use tolmap::schema::{DistrictSymbols, MapDocument, SymbolsDocument};
use tolmap::worker::{WorkerEvent, WorkerSpec};
use ts_rs::{Config, TS};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    check: bool,
    #[arg(long, default_value = "bindings")]
    output: PathBuf,
}

fn typescript_files(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let mut result = BTreeMap::new();
    if !root.exists() {
        return Ok(result);
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            for (relative, bytes) in typescript_files(&path)? {
                result.insert(PathBuf::from(entry.file_name()).join(relative), bytes);
            }
        } else if path.extension().is_some_and(|extension| extension == "ts") {
            result.insert(
                path.strip_prefix(root)
                    .expect("entry is below root")
                    .to_owned(),
                fs::read(&path).with_context(|| format!("read {}", path.display()))?,
            );
        }
    }
    Ok(result)
}

fn export_to(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory)?;
    MapDocument::export_all(&Config::default().with_out_dir(directory))?;
    SymbolsDocument::export_all(&Config::default().with_out_dir(directory))?;
    DistrictSymbols::export_all(&Config::default().with_out_dir(directory))?;
    WorkerSpec::export_all(&Config::default().with_out_dir(directory))?;
    WorkerEvent::export_all(&Config::default().with_out_dir(directory))?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !args.check {
        export_to(&args.output)?;
        return Ok(());
    }

    let temporary = std::env::temp_dir().join(format!(
        "tolmap-generate-types-check-{}",
        std::process::id()
    ));
    fs::create_dir(&temporary)
        .with_context(|| format!("create temporary directory {}", temporary.display()))?;
    let result = (|| -> Result<()> {
        export_to(&temporary)?;
        let expected = typescript_files(&temporary)?;
        let actual = typescript_files(&args.output)?;
        if expected != actual {
            for (path, bytes) in &expected {
                if actual.get(path) != Some(bytes) {
                    eprintln!(
                        "expected {}:\n{}",
                        path.display(),
                        String::from_utf8_lossy(bytes)
                    );
                }
            }
            bail!("{} is stale; regenerate it", args.output.display());
        }
        Ok(())
    })();
    fs::remove_dir_all(&temporary)
        .with_context(|| format!("remove temporary directory {}", temporary.display()))?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_bindings_match_the_rust_schema() {
        let temporary = tempfile::TempDir::new().expect("temporary binding directory");
        export_to(temporary.path()).expect("generate TypeScript bindings");
        let expected = typescript_files(temporary.path()).expect("read generated bindings");
        let committed = typescript_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings"))
            .expect("read committed bindings");
        assert_eq!(
            expected.keys().collect::<Vec<_>>(),
            committed.keys().collect::<Vec<_>>()
        );
        let mismatches = expected
            .into_iter()
            .filter_map(|(path, expected_bytes)| {
                (expected_bytes != committed[&path]).then(|| {
                    format!(
                        "binding {} differs; expected:\n{}",
                        path.display(),
                        String::from_utf8_lossy(&expected_bytes)
                    )
                })
            })
            .collect::<Vec<_>>();
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
