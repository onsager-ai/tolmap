use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use tolmap::schema::MapDocument;
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
            bail!("{} is stale; regenerate it", args.output.display());
        }
        Ok(())
    })();
    fs::remove_dir_all(&temporary)
        .with_context(|| format!("remove temporary directory {}", temporary.display()))?;
    result
}
