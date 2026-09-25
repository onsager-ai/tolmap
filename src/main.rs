use std::io::{IsTerminal, Write};
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
        ///
        /// Repeatable, paired positionally with `--lang` by occurrence order
        /// (the Nth `--pkg` pairs with the Nth `--lang`): `--pkg . --lang go
        /// --pkg web/ui --lang ts` unions two sources (see
        /// `extract::build_multi_source`) instead of building one. With 0 or
        /// 1 occurrences, behaves exactly as before (single source, detected
        /// when omitted).
        #[arg(long)]
        pkg: Vec<String>,
        /// Required with `--graph`; detected when omitted otherwise. See
        /// `--pkg` for the repeatable/paired form.
        #[arg(long, value_parser = ["py", "go", "ts"])]
        lang: Vec<String>,
        /// Union every source `tolmap detect` finds that clears the
        /// `--all-sources` floor (`detect::ALL_SOURCES_MIN_FILES` files and
        /// `detect::ALL_SOURCES_MIN_SHARE` of detected source files) instead
        /// of building one language. Mutually exclusive with `--pkg`/`--lang`.
        #[arg(long)]
        all_sources: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "out")]
        out: PathBuf,
        #[arg(long, default_value_t = 1.1)]
        resolution: f64,
        #[arg(long)]
        no_parcels: bool,
        /// Blend/prune route to run. Defaults to `node-relative`; pass
        /// `--prune-variant absolute` to reproduce the original fixed floor.
        #[arg(long, default_value_t = tolmap::pipeline::PruneVariant::NodeRelative)]
        prune_variant: tolmap::pipeline::PruneVariant,
        /// Prior map document used to warm-start Leiden membership. Intended
        /// for reproducible two-commit stability measurements; ordinary CLI
        /// builds remain cold when omitted.
        #[arg(long)]
        previous_map: Option<PathBuf>,
        /// District namer. Model calls require an environment key and budget.
        #[arg(long, default_value_t = tolmap::naming::NamerKind::Idf)]
        namer: tolmap::naming::NamerKind,
        /// OpenRouter model id used when --namer model is selected.
        #[arg(long)]
        namer_model: Option<String>,
        /// A pre-extracted graph (from `dump-graph`) to run the pipeline on
        /// instead of parsing `repo`. `pkg`/`lang`/`--all-sources` are
        /// ignored with this (the graph already carries its source(s)), and
        /// `name` is required, since there is no repository directory to
        /// name the map after.
        #[arg(long)]
        graph: Option<PathBuf>,
        /// Reference graph source (issue #110): `hand` (the default), the
        /// tree-sitter resolver, or `scip`, which runs each language's SCIP
        /// indexer (scip-python, scip-go, scip-typescript on PATH, or
        /// TOLMAP_SCIP_PYTHON/_GO/_TYPESCRIPT) without installing any
        /// dependency, and uses its references wherever the index keeps at
        /// least 80% of the hand-written graph. Other languages, including
        /// any whose indexer is not installed, fall back to `hand`; the
        /// map's `coverage.references` records which and why. Ignored with
        /// `--graph`.
        #[arg(long, default_value_t = tolmap::extract::RefsMode::default())]
        refs: tolmap::extract::RefsMode,
    },
    DumpBlend {
        repo: PathBuf,
        /// Defaults to a single "." / "py" source when neither `--pkg`,
        /// `--lang` nor `--all-sources` is given. See `build`'s `--pkg` for
        /// the repeatable/paired multi-source form.
        #[arg(long)]
        pkg: Vec<String>,
        #[arg(long, value_parser = ["py", "go", "ts"])]
        lang: Vec<String>,
        #[arg(long)]
        all_sources: bool,
        #[arg(long)]
        out: PathBuf,
        /// Blend/prune route to report. Defaults to `node-relative`.
        #[arg(long, default_value_t = tolmap::pipeline::PruneVariant::NodeRelative)]
        prune_variant: tolmap::pipeline::PruneVariant,
    },
    /// Extraction only: parse a repository and dump the resulting graph as
    /// JSON, without running blend/prune/partition/layout. Exists so a small
    /// fixture's graph can be pre-extracted once and checked in for CI to
    /// consume via `build --graph`, since CI cannot clone the fixture repos
    /// on every push.
    DumpGraph {
        repo: PathBuf,
        /// Defaults to a single "." / "py" source when neither `--pkg`,
        /// `--lang` nor `--all-sources` is given. See `build`'s `--pkg` for
        /// the repeatable/paired multi-source form.
        #[arg(long)]
        pkg: Vec<String>,
        #[arg(long, value_parser = ["py", "go", "ts"])]
        lang: Vec<String>,
        #[arg(long)]
        all_sources: bool,
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
    /// Step 2 of the polyglot union-extraction work (docs/FINDINGS.md
    /// finding 13): measures what `--all-sources` (or an explicit
    /// `--pkg`/`--lang` set) actually produces on a repository -- candidate
    /// and kept edges split intra/cross-language, per-signal cross-language
    /// edge mass, below-prune-floor share per language, NMI/adjusted-Rand
    /// between district membership and language, and projection drift
    /// against each language's own single-source map. Writes the full
    /// measurement to `--out` as JSON and prints a human summary. Does not
    /// change the map pipeline itself -- see `src/polyglot.rs`'s module doc.
    PolyglotReport {
        repo: PathBuf,
        #[arg(long)]
        pkg: Vec<String>,
        #[arg(long, value_parser = ["py", "go", "ts"])]
        lang: Vec<String>,
        #[arg(long)]
        all_sources: bool,
        #[arg(long, default_value_t = 1.1)]
        resolution: f64,
        #[arg(long)]
        out: PathBuf,
        /// Blend/prune route to measure. Defaults to `node-relative`.
        #[arg(long, default_value_t = tolmap::pipeline::PruneVariant::NodeRelative)]
        prune_variant: tolmap::pipeline::PruneVariant,
    },
    /// The job service (milestone 3, issue #5): clones/indexes repositories
    /// on demand and serves the results over HTTP. Binds 127.0.0.1 only --
    /// see docs/API.md and `service::config::ServeConfig`'s doc comment.
    /// Not a tokio::main binary: only this subcommand needs a runtime, so
    /// it builds one itself rather than paying async overhead on every
    /// other `tolmap` invocation.
    Serve,
    /// One JSON job spec on stdin; versioned JSON events on stdout.
    Worker,
    #[command(hide = true)]
    EtaReplay { timeline: PathBuf },
}

fn cli_progress() -> tolmap::progress::Progress {
    let tty = std::io::stderr().is_terminal();
    let width = std::sync::Mutex::new(0usize);
    tolmap::progress::Progress::new(move |event| {
        use tolmap::worker::WorkerEvent;
        let line = match &event {
            WorkerEvent::StageStarted { stage, .. } => format!("{}", stage.label()),
            WorkerEvent::Progress { value, .. } => format!(
                "{}: {}{}{}",
                value.label,
                value.done,
                value
                    .total
                    .map_or(String::new(), |total| format!("/{total}")),
                value.rate_per_s.map_or(String::new(), |rate| format!(
                    " {} {:.1}/s",
                    value.unit, rate
                )),
            ),
            WorkerEvent::StageFinished {
                stage, duration_s, ..
            } => format!("{}: done in {duration_s:.2}s", stage.label()),
            WorkerEvent::Log { message, .. } => message.clone(),
            _ => return,
        };
        if tty {
            let mut old = width.lock().unwrap();
            eprint!("\r{line}{}", " ".repeat(old.saturating_sub(line.len())));
            *old = line.len();
            if matches!(event, WorkerEvent::StageFinished { .. }) {
                eprintln!();
                *old = 0;
            }
            let _ = std::io::stderr().flush();
        } else {
            eprintln!("{line}");
        }
    })
}

/// Resolves the `--pkg`/`--lang` `build` actually runs with: an explicit
/// flag always wins, since the repository holds intent and a maintainer's
/// override of a detected value is intent too (docs/ARCHITECTURE.md). Only
/// the pieces that were *not* pinned get detected, and only those are
/// printed -- an explicit `--pkg` with `--lang` omitted, for instance,
/// prints only the detected language, not a detected `pkg` nobody asked for.
///
/// This is the single-source path (0 or 1 `--pkg`/`--lang` occurrences); see
/// `resolve_multi_source` for 2+ or `--all-sources`.
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
                print_other_candidates(&detection.candidates[1..], repo, "  ");
            }
            Ok((
                detection.chosen.pkg.clone(),
                detection.chosen.language.as_str().to_owned(),
            ))
        }
    }
}

/// Which `(language, pkg)` keys `detect::all_sources` would select for
/// `repo` -- shared by `tolmap detect`'s and `resolve_build_source`'s "other
/// sources found" listings so both describe the same threshold the same
/// way. Falls back to an empty set (nothing marked included) rather than
/// propagating a detection error here: both call sites already have their
/// own primary `detect()` result in hand, and a candidate listing that
/// fails to annotate itself is a better failure mode than losing the
/// listing entirely over a problem the caller's own detect() call didn't hit.
fn all_sources_keys(repo: &Path) -> std::collections::BTreeSet<(&'static str, String)> {
    tolmap::detect::all_sources(repo)
        .map(|selected| {
            selected
                .into_iter()
                .map(|c| (c.language.as_str(), c.pkg))
                .collect()
        })
        .unwrap_or_default()
}

fn describe_with_all_sources_status(
    candidate: &tolmap::detect::SourceCandidate,
    included: &std::collections::BTreeSet<(&'static str, String)>,
) -> String {
    let key = (candidate.language.as_str(), candidate.pkg.clone());
    let status = if included.contains(&key) {
        "included by --all-sources"
    } else {
        "excluded by --all-sources (below the file-count/share floor)"
    };
    format!("{} -- {status}", candidate.describe())
}

/// Prints every detected candidate beyond the chosen one, noting which ones
/// `--all-sources` would actually include -- the CLI surface for
/// `tolmap detect`'s doc comment's "print which candidates `--all-sources`
/// would include, and stop saying 'not merged'": now that merging is a real
/// flag, the honest statement is which side of the threshold each candidate
/// falls on, not that merging is unavailable.
fn print_other_candidates(
    candidates: &[tolmap::detect::SourceCandidate],
    repo: &Path,
    indent: &str,
) {
    let included = all_sources_keys(repo);
    for candidate in candidates {
        eprintln!(
            "{indent}{}",
            describe_with_all_sources_status(candidate, &included)
        );
    }
}

/// Resolves an explicit 2+-occurrence `--pkg`/`--lang` set, or
/// `--all-sources`, into the `(pkg, LanguageKind)` list
/// `extract::build_multi_source` takes. Not used for the 0/1-occurrence
/// case -- that stays on `resolve_build_source`'s single-source, detect-on-
/// omission path, so `tolmap build repo` with no new flags is unchanged.
fn resolve_multi_source(
    repo: &Path,
    pkg: Vec<String>,
    lang: Vec<String>,
    all_sources: bool,
) -> Result<Vec<(String, tolmap::extract::LanguageKind)>> {
    if all_sources {
        anyhow::ensure!(
            pkg.is_empty() && lang.is_empty(),
            "--all-sources cannot be combined with --pkg/--lang"
        );
        let candidates = tolmap::detect::all_sources(repo)?;
        anyhow::ensure!(
            !candidates.is_empty(),
            "no source in {} cleared the --all-sources floor ({} files or {:.0}% share of detected source files); pass --pkg/--lang explicitly",
            repo.display(),
            tolmap::detect::ALL_SOURCES_MIN_FILES,
            tolmap::detect::ALL_SOURCES_MIN_SHARE * 100.0,
        );
        eprintln!("--all-sources: {} source(s) selected:", candidates.len());
        for candidate in &candidates {
            eprintln!("  {}", candidate.describe());
        }
        return Ok(candidates
            .into_iter()
            .map(|c| (c.pkg, c.language))
            .collect());
    }
    anyhow::ensure!(
        pkg.len() == lang.len(),
        "--pkg and --lang must be given the same number of times ({} --pkg vs {} --lang); pair them positionally: --pkg . --lang go --pkg web/ui --lang ts",
        pkg.len(),
        lang.len()
    );
    pkg.into_iter()
        .zip(lang)
        .map(|(pkg, lang)| Ok((pkg, tolmap::extract::LanguageKind::parse(&lang)?)))
        .collect()
}

/// True when the caller's `--pkg`/`--lang`/`--all-sources` combination names
/// more than one source -- the signal to take the multi-source path instead
/// of the single-source, detect-on-omission one.
fn wants_multi_source(pkg: &[String], lang: &[String], all_sources: bool) -> bool {
    all_sources || pkg.len() > 1 || lang.len() > 1
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            repo,
            pkg,
            lang,
            all_sources,
            name,
            out,
            resolution,
            no_parcels,
            prune_variant,
            previous_map,
            namer,
            namer_model,
            graph,
            refs,
        } => {
            let progress = cli_progress();
            let namer_model = namer_model
                .or_else(|| std::env::var("TOLMAP_NAMER_MODEL").ok())
                .unwrap_or_else(|| tolmap::naming::DEFAULT_MODEL.to_owned());
            let previous_document: Option<tolmap::schema::MapDocument> = previous_map
                .as_ref()
                .map(|path| {
                    let raw = std::fs::read_to_string(path)
                        .with_context(|| format!("read previous map {}", path.display()))?;
                    serde_json::from_str(&raw)
                        .with_context(|| format!("parse previous map {}", path.display()))
                })
                .transpose()?;
            match (repo, graph) {
                (Some(_), Some(_)) => {
                    anyhow::bail!("pass either a repository or --graph, not both")
                }
                (None, None) => anyhow::bail!("pass a repository, or a pre-extracted --graph"),
                (Some(repo), None) => {
                    if wants_multi_source(&pkg, &lang, all_sources) {
                        let sources = resolve_multi_source(&repo, pkg, lang, all_sources)?;
                        tolmap::geometry::build_multi_warm_with_progress(
                            &repo,
                            &sources,
                            name.as_deref(),
                            &out,
                            resolution,
                            tolmap::geometry::BuildFeatures {
                                parcels: !no_parcels,
                                prune_variant,
                                namer,
                                namer_model,
                                refs,
                            },
                            previous_document.as_ref(),
                            &progress,
                        )
                        .map(|_| ())
                    } else {
                        let (pkg, lang) = resolve_build_source(
                            &repo,
                            pkg.into_iter().next(),
                            lang.into_iter().next(),
                        )?;
                        tolmap::geometry::build_warm_with_progress(
                            &repo,
                            &pkg,
                            &lang,
                            name.as_deref(),
                            &out,
                            resolution,
                            tolmap::geometry::BuildFeatures {
                                parcels: !no_parcels,
                                prune_variant,
                                namer,
                                namer_model,
                                refs,
                            },
                            previous_document.as_ref(),
                            &progress,
                        )
                        .map(|_| ())
                    }
                }
                (None, Some(graph_path)) => {
                    let name = name.ok_or_else(|| {
                        anyhow::anyhow!(
                            "--graph requires --name (no repository to name the map after)"
                        )
                    })?;
                    let raw = std::fs::read_to_string(&graph_path)
                        .with_context(|| format!("read {}", graph_path.display()))?;
                    let data: tolmap::schema::GraphData = serde_json::from_str(&raw)
                        .with_context(|| format!("parse graph {}", graph_path.display()))?;
                    tolmap::geometry::build_from_graph_warm_with_progress(
                        data,
                        name,
                        &out,
                        resolution,
                        tolmap::geometry::BuildFeatures {
                            parcels: !no_parcels,
                            prune_variant,
                            namer,
                            namer_model,
                            refs,
                        },
                        previous_document.as_ref(),
                        &progress,
                    )
                    .map(|_| ())
                }
            }
        }
        Command::DumpBlend {
            repo,
            pkg,
            lang,
            all_sources,
            out,
            prune_variant,
        } => {
            if wants_multi_source(&pkg, &lang, all_sources) {
                let sources = resolve_multi_source(&repo, pkg, lang, all_sources)?;
                tolmap::blenddump::dump_multi(&repo, &sources, prune_variant, &out)
            } else {
                let pkg = pkg.into_iter().next().unwrap_or_else(|| ".".to_owned());
                let lang = lang.into_iter().next().unwrap_or_else(|| "py".to_owned());
                tolmap::blenddump::dump(&repo, &pkg, &lang, prune_variant, &out)
            }
        }
        Command::DumpGraph {
            repo,
            pkg,
            lang,
            all_sources,
            out,
        } => {
            let graph = if wants_multi_source(&pkg, &lang, all_sources) {
                let sources = resolve_multi_source(&repo, pkg, lang, all_sources)?;
                tolmap::extract::build_multi_source(&repo, &sources)?
            } else {
                let pkg = pkg.into_iter().next().unwrap_or_else(|| ".".to_owned());
                let lang = lang.into_iter().next().unwrap_or_else(|| "py".to_owned());
                let language = tolmap::extract::LanguageKind::parse(&lang)?;
                tolmap::extract::build(&repo, &pkg, language)?
            };
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
                println!("other sources found:");
                let included = all_sources_keys(&repo);
                for candidate in &detection.candidates[1..] {
                    println!(
                        "  {}",
                        describe_with_all_sources_status(candidate, &included)
                    );
                }
                println!(
                    "pass --all-sources to `tolmap build`/`dump-graph`/`dump-blend`/`polyglot-report` to merge every included candidate, or --pkg/--lang to pick one explicitly"
                );
            }
            Ok(())
        }
        Command::PolyglotReport {
            repo,
            pkg,
            lang,
            all_sources,
            resolution,
            out,
            prune_variant,
        } => {
            let sources = if wants_multi_source(&pkg, &lang, all_sources) {
                resolve_multi_source(&repo, pkg, lang, all_sources)?
            } else {
                // A single explicit source (or none at all) is still a
                // legal call -- it just reports a graph with no
                // cross-language pairs, which is itself a useful sanity
                // check of the tool. Falls back to --all-sources' own
                // detection when nothing was pinned at all, same as build.
                if pkg.is_empty() && lang.is_empty() {
                    tolmap::detect::all_sources(&repo)?
                        .into_iter()
                        .map(|c| (c.pkg, c.language))
                        .collect()
                } else {
                    resolve_multi_source(&repo, pkg, lang, false)?
                }
            };
            anyhow::ensure!(
                !sources.is_empty(),
                "no source to report on: no candidate in {} cleared the --all-sources floor, and none was given explicitly",
                repo.display()
            );
            tolmap::polyglot::run(&repo, &sources, resolution, prune_variant, &out)
        }
        Command::Serve => {
            let config = tolmap::service::config::ServeConfig::from_env();
            let runtime = tokio::runtime::Runtime::new().context("build tokio runtime")?;
            runtime.block_on(tolmap::service::serve(config))
        }
        Command::Worker => tolmap::worker::run_stdio(),
        Command::EtaReplay { timeline } => {
            let report = tolmap::service::eta::replay_timeline(&timeline)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_refs(args: &[&str]) -> tolmap::extract::RefsMode {
        match Cli::try_parse_from(args).expect("parse").command {
            Command::Build { refs, .. } => refs,
            other => panic!("expected build, got {other:?}"),
        }
    }

    // Issue #110 P2a: an unqualified `tolmap build` uses the hand-written
    // resolver (owner decision, 2026-09-25T16:56Z: hand stays the default,
    // SCIP is the oracle), and both modes stay selectable explicitly.
    #[test]
    fn build_defaults_to_hand_refs_and_accepts_both() {
        use tolmap::extract::RefsMode;
        assert_eq!(build_refs(&["tolmap", "build", "repo"]), RefsMode::Hand);
        assert_eq!(
            build_refs(&["tolmap", "build", "repo", "--refs", "hand"]),
            RefsMode::Hand
        );
        assert_eq!(
            build_refs(&["tolmap", "build", "repo", "--refs", "scip"]),
            RefsMode::Scip
        );
    }
}
