use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "tolmap", about = "Turn a source repository into a deterministic map")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Build {
        repo: PathBuf,
        #[arg(long, default_value = ".")]
        pkg: String,
        #[arg(long, default_value = "py", value_parser = ["py", "go", "ts"])]
        lang: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "out")]
        out: PathBuf,
        #[arg(long, default_value_t = 1.1)]
        resolution: f64,
        #[arg(long)]
        no_parcels: bool,
    },
    Parity {
        reference: PathBuf,
        candidate: PathBuf,
        #[arg(long)]
        idf_names: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            repo,
            pkg,
            lang,
            name,
            out,
            resolution,
            no_parcels,
        } => tolmap::geometry::build(
            &repo,
            &pkg,
            &lang,
            name.as_deref(),
            &out,
            resolution,
            !no_parcels,
        )
        .map(|_| ()),
        Command::Parity {
            reference,
            candidate,
            idf_names,
        } => {
            let report = tolmap::parity::compare_files(&reference, &candidate, idf_names.as_deref())?;
            println!("{report}");
            Ok(())
        }
    }
}

