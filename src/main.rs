use std::path::{Path, PathBuf};

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
        /// Source root inside the repository. Detected when omitted (see
        /// `tolmap detect`) -- an explicit value always wins over detection,
        /// since the repository holds intent and a maintainer's override is
        /// intent (docs/ARCHITECTURE.md). Required with `--graph`.
        #[arg(long)]
        pkg: Option<String>,
        /// Required with `--graph`; detected when omitted otherwise.
        #[arg(long, value_parser = ["py", "go", "ts"])]
        lang: Option<String>,
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
    /// Auto-detect language and source root from a bare clone, and print
    /// what was found and why -- the thing to run when a map looks wrong
    /// (see docs/FINDINGS.md finding 7: a wrong `--pkg` fails silently, not
    /// loudly, so this exists to make the choice visible and overridable).
    Detect { repo: PathBuf },
}

/// Resolves the `--pkg`/`--lang` `build` actually runs with: an explicit
/// flag always wins, since the repository holds intent and a maintainer's
/// override of a detected value is intent too (docs/ARCHITECTURE.md). Only
/// the pieces that were *not* pinned get detected, and only those are
/// printed -- an explicit `--pkg` with `--lang` omitted, for instance,
/// prints only the detected language, not a detected `pkg` nobody asked for.
fn resolve_build_source(
    repo: &Path,
    pkg: Option<String>,
    lang: Option<String>,
) -> Result<(String, String)> {
    match (pkg, lang) {
        (Some(pkg), Some(lang)) => Ok((pkg, lang)),
        (Some(pkg), None) => {
            let detection = tolmap::detect::detect(repo)
                .with_context(|| format!("detect language for {}", repo.display()))?;
            eprintln!(
                "detected language: {} ({} confidence) -- {}",
                detection.chosen.language.as_str(),
                detection.chosen.confidence.as_str(),
                detection.chosen.evidence
            );
            Ok((pkg, detection.chosen.language.as_str().to_owned()))
        }
        (None, Some(lang)) => {
            let language = tolmap::extract::LanguageKind::parse(&lang)?;
            let candidate = tolmap::detect::detect_language(repo, language)
                .with_context(|| format!("detect source root for {}", repo.display()))?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no {lang} source found in {}; pass --pkg explicitly",
                        repo.display()
                    )
                })?;
            eprintln!("detected pkg: {}", candidate.describe());
            Ok((candidate.pkg, lang))
        }
        (None, None) => {
            let detection = tolmap::detect::detect(repo).with_context(|| {
                format!("detect language and source root for {}", repo.display())
            })?;
            eprintln!("detected: {}", detection.chosen.describe());
            if detection.candidates.len() > 1 {
                let others = detection.candidates[1..]
                    .iter()
                    .map(|c| c.describe())
                    .collect::<Vec<_>>()
                    .join("\n  ");
                eprintln!(
                    "  other sources found (not merged -- see docs/ARCHITECTURE.md):\n  {others}"
                );
            }
            Ok((
                detection.chosen.pkg.clone(),
                detection.chosen.language.as_str().to_owned(),
            ))
        }
    }
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
            (Some(repo), None) => {
                let (pkg, lang) = resolve_build_source(&repo, pkg, lang)?;
                tolmap::geometry::build(
                    &repo,
                    &pkg,
                    &lang,
                    name.as_deref(),
                    &out,
                    resolution,
                    !no_parcels,
                )
                .map(|_| ())
            }
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
        Command::Detect { repo } => {
            let detection = tolmap::detect::detect(&repo)?;
            println!("chosen: {}", detection.chosen.describe());
            if detection.candidates.len() > 1 {
                println!("other sources found (not merged -- see docs/ARCHITECTURE.md):");
                for candidate in &detection.candidates[1..] {
                    println!("  {}", candidate.describe());
                }
            }
            Ok(())
        }
    }
}
