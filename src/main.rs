use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "tolmap",
    about = "Turn a source repository into a deterministic map"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Build {
        // Exactly one of `repo` or `--graph` is required -- validated in
        // `main`, since clap's derive can't express "either, not both"
        // across a positional and a flag as cleanly as a manual check does.
        repo: Option<PathBuf>,
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
        /// A pre-extracted graph (from `dump-graph`) to run the pipeline on
        /// instead of parsing `repo`. `pkg`/`lang` are ignored with this
        /// (the graph already carries its language), and `name` is required,
        /// since there is no repository directory to name the map after.
        #[arg(long)]
        graph: Option<PathBuf>,
    },
    DumpBlend {
        repo: PathBuf,
        #[arg(long, default_value = ".")]
        pkg: String,
        #[arg(long, default_value = "py", value_parser = ["py", "go", "ts"])]
        lang: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Extraction only: parse a repository and dump the resulting graph as
    /// JSON, without running blend/prune/partition/layout. Exists so a small
    /// fixture's graph can be pre-extracted once and checked in for CI to
    /// consume via `build --graph`, since CI cannot clone the fixture repos
    /// on every push.
    DumpGraph {
        repo: PathBuf,
        #[arg(long, default_value = ".")]
        pkg: String,
        #[arg(long, default_value = "py", value_parser = ["py", "go", "ts"])]
        lang: String,
        #[arg(long)]
        out: PathBuf,
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
            graph,
        } => match (repo, graph) {
            (Some(_), Some(_)) => {
                anyhow::bail!("pass either a repository or --graph, not both")
            }
            (None, None) => anyhow::bail!("pass a repository, or a pre-extracted --graph"),
            (Some(repo), None) => tolmap::geometry::build(
                &repo,
                &pkg,
                &lang,
                name.as_deref(),
                &out,
                resolution,
                !no_parcels,
            )
            .map(|_| ()),
            (None, Some(graph_path)) => {
                let name = name.ok_or_else(|| {
                    anyhow::anyhow!("--graph requires --name (no repository to name the map after)")
                })?;
                let raw = std::fs::read_to_string(&graph_path)
                    .with_context(|| format!("read {}", graph_path.display()))?;
                let data: tolmap::schema::GraphData = serde_json::from_str(&raw)
                    .with_context(|| format!("parse graph {}", graph_path.display()))?;
                tolmap::geometry::build_from_graph(data, name, &out, resolution, !no_parcels)
                    .map(|_| ())
            }
        },
        Command::DumpBlend {
            repo,
            pkg,
            lang,
            out,
        } => tolmap::blenddump::dump(&repo, &pkg, &lang, &out),
        Command::DumpGraph {
            repo,
            pkg,
            lang,
            out,
        } => {
            let language = tolmap::extract::LanguageKind::parse(&lang)?;
            let graph = tolmap::extract::build(&repo, &pkg, language)?;
            let bytes = serde_json::to_vec(&graph)?;
            std::fs::write(&out, bytes).with_context(|| format!("write {}", out.display()))?;
            println!(
                "{}: {} nodes, {} candidate edges -> {}",
                graph.repo,
                graph.nodes.len(),
                graph.edges.len(),
                out.display()
            );
            Ok(())
        }
        Command::Parity {
            reference,
            candidate,
            idf_names,
        } => {
            let report =
                tolmap::parity::compare_files(&reference, &candidate, idf_names.as_deref())?;
            println!("{report}");
            if report.passed() {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
    }
}
