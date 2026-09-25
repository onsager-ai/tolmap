use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Node, Parser};

use crate::schema::{FileId, GraphData, ReferenceCoverage, SignalEdge, SourceNode, SymbolRow};

const ALPHA: f64 = 0.45;
const BETA: f64 = 0.35;
const GAMMA: f64 = 0.08;
const DELTA: f64 = 0.12;

const IDENT_STOP: &[&str] = &[
    "self",
    "cls",
    "args",
    "kwargs",
    "return",
    "None",
    "True",
    "False",
    "str",
    "int",
    "list",
    "dict",
    "set",
    "type",
    "object",
    "Exception",
    "value",
    "name",
    "key",
    "data",
    "result",
    "item",
    "obj",
    "i",
    "e",
];

pub(crate) const PY_SKIP_DIR: &[&str] = &[
    "__pycache__",
    ".git",
    "tests",
    "test",
    "templates",
    "vendor",
    "third_party",
    "migrations",
    "testdata",
    "node_modules",
];

pub(crate) const MULTI_SKIP_DIR: &[&str] = &[
    "vendor",
    "node_modules",
    "dist",
    "testdata",
    "__tests__",
    "tests",
    "test",
    ".git",
    "docs",
    "documentation",
    "examples",
    "example",
    "third_party",
    "generated",
    "fixtures",
    "__pycache__",
];

const BRANCHY: &[&str] = &[
    "if_statement",
    "for_statement",
    "while_statement",
    "switch_statement",
    "type_switch_statement",
    "expression_switch_statement",
    "select_statement",
    "catch_clause",
    "try_statement",
    "case_clause",
    "expression_case",
    "communication_case",
    "ternary_expression",
    "conditional_expression",
    "binary_expression",
];

const KIND_CLASS: usize = 0;
const KIND_FUNC: usize = 1;
const KIND_METHOD: usize = 2;
const KIND_INTERFACE: usize = 3;
const KIND_TYPE: usize = 4;
const KIND_CONST: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum LanguageKind {
    Python,
    Go,
    TypeScript,
}

impl LanguageKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "py" => Ok(Self::Python),
            "go" => Ok(Self::Go),
            "ts" => Ok(Self::TypeScript),
            _ => bail!("unsupported language {value:?}; expected py, go, or ts"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Python => "py",
            Self::Go => "go",
            Self::TypeScript => "ts",
        }
    }

    /// The grammar for `self` in general. A `.tsx` file needs a different
    /// grammar than this for JSX syntax -- see [`grammar_for_file`], which
    /// every call site in this module uses instead of calling this directly
    /// for a file that might be TypeScript.
    fn tree_sitter(self) -> Language {
        match self {
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

/// The tree-sitter grammar to parse `file` with. `.tsx` needs the TSX
/// grammar (`LANGUAGE_TSX`) to accept JSX syntax; `LANGUAGE_TYPESCRIPT`
/// rejects it outright. This is chosen per *file*, not per `LanguageKind`
/// (`LanguageKind::TypeScript` gets no new variant): a `.tsx` file is
/// TypeScript for every other purpose -- detection, the `lang` field,
/// polyglot source selection -- only the grammar differs. Verified against
/// both grammars' `node-types.json`: every node kind the TS walks in this
/// module match on (`variable_declarator`, `class_declaration`,
/// `import_statement`, the `BRANCHY` set, etc.) exists identically in TSX;
/// TSX only adds JSX-specific kinds (`jsx_element` and friends) this module
/// never looks at, and drops `type_assertion` (the `<Type>expr` cast syntax,
/// which is ambiguous with JSX and not something this module walks for
/// either grammar).
fn grammar_for_file(language: LanguageKind, file: &str) -> Language {
    if language == LanguageKind::TypeScript && file.ends_with(".tsx") {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    } else {
        language.tree_sitter()
    }
}

/// Per-file metrics that do not depend on any other file's content -- the
/// only state kept alive for the whole extraction. Deliberately holds
/// neither `source` nor `tree`: retaining those for every file
/// simultaneously was ~95% of peak RSS (measured on n8n's 14,410 TypeScript
/// files: 1,466 MB with source+tree retained per file vs. 71 MB with only
/// `identifiers`, vs. 20 MB with nothing). See [`parse_files`] for where the
/// tree is parsed, walked and dropped, one file at a time.
#[derive(Clone)]
struct ParsedFile {
    loc: usize,
    code_lines: usize,
    complexity: usize,
    identifiers: BTreeMap<String, usize>,
    symbols: Vec<SymbolRow>,
}

/// The raw, single-file output of the second tree walk -- import strings and
/// selector/attribute candidates -- captured in [`parse_files`] while the
/// tree is still alive, so [`parse_python`] and [`parse_multi`] can resolve
/// them against the *global* `known`/`file_of`/`by_directory` maps (which
/// only exist once every file in the source has been parsed) without ever
/// needing the tree back. This is what makes the two passes not require
/// co-resident trees: phase 1 (in `parse_files`) produces `FileRaw` per file
/// and drops the tree; phase 2 (`parse_python`/`parse_multi`) is pure string
/// resolution over `FileRaw` plus the global maps.
///
/// For Go and TypeScript this is nearly free: `go_imports`/`go_selectors`
/// and `typescript_imports`/`typescript_named` never looked at any other
/// file to begin with (`go_selectors`'s "aliases" are the *local* `import
/// name -> path` binding, resolved from the file's own `import_spec`
/// nodes -- resolving a path to an actual target file, via `by_directory`,
/// is a separate step `resolve_multi` already did in a second loop). Moving
/// their call sites into `parse_files` changes nothing about what they
/// compute, only when.
///
/// Python is the awkward case: its aliasing (`import x as y`, `from a import
/// b as c`) has to be checked against the *global* `known` module set before
/// a bare-attribute use like `y.thing()` can be attributed to a target file,
/// so the attribute-node walk cannot be fully resolved at phase-1 time.
/// `attribute_candidates` carries every `object.attribute` pair the walk
/// finds (object identifier text, attribute name), unfiltered, for phase 2
/// to match against the aliases it can only build once `known`/`file_of`
/// exist.
enum FileRaw {
    Python {
        imports: Vec<PythonImport>,
        attribute_candidates: Vec<(String, String)>,
    },
    Multi {
        imports: Vec<String>,
        named_candidates: Vec<(String, String)>,
    },
}

/// Where the static signal and symbol references come from (issue #110).
///
/// `Hand`, the default, is the tree-sitter resolver: a `--refs hand` map is
/// byte-identical to one built before this option existed. `Scip` runs each
/// detected language's SCIP indexer (`indexers`), reads the index
/// (`scip_ingest`) and uses it per language wherever it passes the fallback
/// gate (`scip_ingest::gate`); every other language -- including one whose
/// indexer is not installed -- keeps the hand-written graph, and the map's
/// `coverage.references` records which path each language took and why.
///
/// Hand stays the default by owner decision (#110 P2a, 2026-09-25T16:56Z: "Tune
/// hand, SCIP as oracle"): it has fewer dependencies and costs seconds where
/// indexing costs minutes and gigabytes (finding 45). SCIP is the measuring
/// stick instead, gated against its own fixtures (`data/scip`,
/// `eval/scip_fixtures.py`). Every gate that compares with the frozen
/// Python reference still passes `--refs hand` explicitly, so none of them
/// depends on what this default is.
///
/// Both the CLI (`tolmap build --refs`) and the service (`TOLMAP_REFS`)
/// take their default from here, so the two cannot drift apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RefsMode {
    #[default]
    Hand,
    Scip,
}

impl std::str::FromStr for RefsMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "hand" => Ok(Self::Hand),
            "scip" => Ok(Self::Scip),
            other => Err(format!("unknown --refs {other:?}; expected hand or scip")),
        }
    }
}

impl std::fmt::Display for RefsMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Hand => "hand",
            Self::Scip => "scip",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PythonImport {
    pub(crate) from: bool,
    pub(crate) level: usize,
    pub(crate) module: String,
    pub(crate) names: Vec<(String, Option<String>)>,
}

pub fn build(repo: &Path, pkg: &str, language: LanguageKind) -> Result<GraphData> {
    build_multi_source(repo, &[(pkg.to_owned(), language)])
}

pub fn build_with_progress(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    progress: &crate::progress::Progress,
) -> Result<GraphData> {
    build_multi_source_with_progress(repo, &[(pkg.to_owned(), language)], progress)
}

/// Unions any number of `(pkg, language)` sources into one graph, extracted
/// before `finish_graph` runs co-change, semantic and proximity over the
/// combined file set (see the module comment on [`finish_graph`] for why the
/// merge has to happen there and not by unioning two finished `GraphData`
/// values). `build(repo, pkg, language)` is a one-element call into this, so
/// single-source output is unchanged: with one source there is nothing to
/// merge, no cross-source file collision, and `static_max` (see
/// [`finish_graph`]) is computed over the same one language it always was.
///
/// Sources are sorted by `(language.as_str(), pkg)` before parsing (finding
/// 9: a seeded stage is not deterministic if a set of strings is iterated
/// upstream of it) -- this fixes both the merge order below and,
/// transitively, which source wins a file-path collision.
pub fn build_multi_source(repo: &Path, sources: &[(String, LanguageKind)]) -> Result<GraphData> {
    build_multi_source_with_progress(repo, sources, &crate::progress::Progress::silent())
}

pub fn build_multi_source_with_progress(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    progress: &crate::progress::Progress,
) -> Result<GraphData> {
    Ok(build_multi_source_inner(repo, sources, false, RefsMode::Hand, progress)?.0)
}

pub(crate) fn build_with_symbols(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
) -> Result<(GraphData, crate::symbols::SymbolSpool)> {
    let (graph, spool) = build_multi_source_inner(
        repo,
        &[(pkg.to_owned(), language)],
        true,
        RefsMode::Hand,
        &crate::progress::Progress::silent(),
    )?;
    Ok((graph, spool.expect("symbol collection requested")))
}

pub(crate) fn build_with_symbols_progress(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    refs: RefsMode,
    progress: &crate::progress::Progress,
) -> Result<(GraphData, crate::symbols::SymbolSpool)> {
    let (graph, spool) =
        build_multi_source_inner(repo, &[(pkg.to_owned(), language)], true, refs, progress)?;
    Ok((graph, spool.expect("symbol collection requested")))
}

pub(crate) fn build_multi_source_with_symbols(
    repo: &Path,
    sources: &[(String, LanguageKind)],
) -> Result<(GraphData, crate::symbols::SymbolSpool)> {
    let (graph, spool) = build_multi_source_inner(
        repo,
        sources,
        true,
        RefsMode::Hand,
        &crate::progress::Progress::silent(),
    )?;
    Ok((graph, spool.expect("symbol collection requested")))
}

pub(crate) fn build_multi_source_with_symbols_progress(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    refs: RefsMode,
    progress: &crate::progress::Progress,
) -> Result<(GraphData, crate::symbols::SymbolSpool)> {
    let (graph, spool) = build_multi_source_inner(repo, sources, true, refs, progress)?;
    Ok((graph, spool.expect("symbol collection requested")))
}

fn build_multi_source_inner(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    collect_symbols: bool,
    refs: RefsMode,
    progress: &crate::progress::Progress,
) -> Result<(GraphData, Option<crate::symbols::SymbolSpool>)> {
    let started = Instant::now();
    let mut symbol_collection = Duration::ZERO;
    let mut collect_steps = crate::symbols::CollectTimings::default();
    let mut graph_time = Duration::ZERO;
    ensure!(
        repo.is_dir(),
        "repository {} is not a directory",
        repo.display()
    );
    ensure!(
        !sources.is_empty(),
        "build_multi_source requires at least one (pkg, language) source"
    );
    let mut sorted_sources = sources.to_vec();
    sorted_sources.sort_by(|a, b| a.1.as_str().cmp(b.1.as_str()).then_with(|| a.0.cmp(&b.0)));

    // Go and TypeScript share one repository-metadata index. Build it once
    // for the whole union rather than re-walking the tree for every source.
    let modules = sorted_sources
        .iter()
        .any(|(_, language)| *language != LanguageKind::Python)
        .then(|| module_index(repo))
        .transpose()?;

    let mut intermediates = Vec::with_capacity(sorted_sources.len());
    let mut spool = collect_symbols
        .then(crate::symbols::SymbolSpool::new)
        .transpose()?;
    for (pkg, language) in &sorted_sources {
        let (parsed, raw, collection_time, steps) =
            parse_files_inner(repo, pkg, *language, spool.as_mut(), progress)?;
        symbol_collection += collection_time;
        collect_steps.add(&steps);
        let graph_started = Instant::now();
        let resolve_stage =
            progress.stage(crate::progress::StageId::Resolve, Some(parsed.len() as u64));
        let intermediate = match language {
            LanguageKind::Python => {
                parse_python_with_progress(repo, pkg, parsed, raw, &resolve_stage)?
            }
            LanguageKind::Go | LanguageKind::TypeScript => parse_multi_with_progress(
                repo,
                pkg,
                *language,
                parsed,
                raw,
                modules
                    .as_ref()
                    .expect("multi-language source has an index"),
                &resolve_stage,
            )?,
        };
        resolve_stage.set(intermediate.parsed.len() as u64);
        resolve_stage.finish();
        intermediates.push(intermediate);
        graph_time += graph_started.elapsed();
    }

    let graph_started = Instant::now();
    let mut merged = union_sources(intermediates)?;
    graph_time += graph_started.elapsed();
    // Indexing runs after every source is resolved, because the fallback
    // gate compares each index with the hand-written graph, and before
    // `finish_graph`, because the static signal it replaces feeds the blend
    // there. Its time is its own stage, not `graph`.
    let references = match refs {
        RefsMode::Hand => None,
        RefsMode::Scip => {
            let (report, scip) =
                apply_scip_references(repo, &mut merged, spool.is_some(), progress)?;
            if let Some(spool) = spool.as_mut() {
                spool.scip = scip;
            }
            Some(report)
        }
    };
    let graph_started = Instant::now();
    let mut graph = finish_graph(repo, merged, progress)?;
    graph.references = references;
    graph_time += graph_started.elapsed();
    let total = started.elapsed();
    // These three durations are disjoint so a build log can account for
    // extraction time without double-counting symbol collection or graph
    // resolution. Metadata discovery and the ordinary file walks are the
    // remainder labelled `extract`.
    let extract_time = total.saturating_sub(symbol_collection + graph_time);
    progress.log(format!("phase extract: {:.3}s", extract_time.as_secs_f64()));
    progress.log(format!(
        "phase symbol_collection: {:.3}s",
        symbol_collection.as_secs_f64()
    ));
    if spool.is_some() {
        // Disjoint steps inside `symbol_collection`; they sum to it up to
        // timer overhead. Finding 40 reads these before and after.
        for (label, duration) in collect_steps.rows() {
            progress.log(format!(
                "phase symbol_collection.{label}: {:.3}s",
                duration.as_secs_f64()
            ));
        }
    }
    progress.log(format!("phase graph: {:.3}s", graph_time.as_secs_f64()));
    progress.log(format!("phase extract_total: {:.3}s", total.as_secs_f64()));
    Ok((graph, spool))
}

/// Parses every source file `source_files` finds for `(pkg, language)` under
/// `repo` with tree-sitter, computing both the per-file metrics (`loc`,
/// `complexity`, `identifiers`, `symbols`) that survive for the rest of the
/// extraction and the raw import/reference candidates (`FileRaw`) that
/// `parse_python`/`parse_multi` resolve into cross-file edges once every
/// file's module name is known.
///
/// This is phase 1 of the two-phase extraction: `source`/`tree` are local to
/// one loop iteration and drop at the end of it, so at most one file's parse
/// tree is ever live at a time -- never all of them at once (see the
/// [`ParsedFile`] and [`FileRaw`] doc comments for why that matters and how
/// the split is divided). Both walks that used to run against `parsed_file.
/// tree` in a later pass (import extraction, and the selector/attribute walk
/// behind `uses`) run here instead, against the same tree, before it is
/// dropped.
#[cfg(test)]
fn parse_files(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
) -> Result<(BTreeMap<String, ParsedFile>, BTreeMap<String, FileRaw>)> {
    let (parsed, raw, _, _) = parse_files_inner(
        repo,
        pkg,
        language,
        None,
        &crate::progress::Progress::silent(),
    )?;
    Ok((parsed, raw))
}

fn parse_files_inner(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    mut spool: Option<&mut crate::symbols::SymbolSpool>,
    progress: &crate::progress::Progress,
) -> Result<(
    BTreeMap<String, ParsedFile>,
    BTreeMap<String, FileRaw>,
    Duration,
    crate::symbols::CollectTimings,
)> {
    let files = source_files(repo, pkg, language)?;
    let parse_stage = progress.stage(crate::progress::StageId::Parse, Some(files.len() as u64));
    let mut parser = Parser::new();

    let mut parsed = BTreeMap::new();
    let mut raw = BTreeMap::new();
    let mut symbol_collection = Duration::ZERO;
    let mut collect_steps = crate::symbols::CollectTimings::default();
    for file in &files {
        // Set per file, not once before the loop: a `.tsx` file needs the
        // TSX grammar while a sibling `.ts` file in the same source needs
        // the plain TypeScript one (see `grammar_for_file`). Go and Python
        // sources only ever pick one grammar, so this is a no-op re-set for
        // them beyond the first file.
        parser
            .set_language(&grammar_for_file(language, file))
            .context("initialize tree-sitter parser")?;
        let source = fs::read(repo.join(file)).with_context(|| format!("read {file}"))?;
        let Some(tree) = parser.parse(&source, None) else {
            // Tree-sitter can return None on cancellation. Keep the file's
            // conservative metrics without inventing symbols or edges.
            parsed.insert(
                file.clone(),
                ParsedFile {
                    loc: source.iter().filter(|&&byte| byte == b'\n').count() + 1,
                    code_lines: nonblank_lines(&source),
                    complexity: 0,
                    identifiers: BTreeMap::new(),
                    symbols: Vec::new(),
                },
            );
            raw.insert(
                file.clone(),
                match language {
                    LanguageKind::Python => FileRaw::Python {
                        imports: Vec::new(),
                        attribute_candidates: Vec::new(),
                    },
                    _ => FileRaw::Multi {
                        imports: Vec::new(),
                        named_candidates: Vec::new(),
                    },
                },
            );
            parse_stage.advance(1);
            continue;
        };
        // ast.parse rejects a Python file as a unit. Matching that behavior is
        // important: accepting the valid half would guess edges upward.
        if language == LanguageKind::Python && tree.root_node().has_error() {
            parse_stage.advance(1);
            continue;
        }
        let root = tree.root_node();
        let (complexity, identifiers, symbols) = match language {
            LanguageKind::Python => python_metrics(root, &source),
            LanguageKind::Go => multi_metrics(root, &source, language),
            LanguageKind::TypeScript => multi_metrics(root, &source, language),
        };
        let file_raw = match language {
            LanguageKind::Python => FileRaw::Python {
                imports: python_imports(root, &source),
                attribute_candidates: python_attribute_candidates(root, &source),
            },
            LanguageKind::Go => FileRaw::Multi {
                imports: go_imports(root, &source),
                named_candidates: go_selectors(root, &source),
            },
            LanguageKind::TypeScript => FileRaw::Multi {
                imports: typescript_imports(root, &source),
                named_candidates: typescript_named(root, &source),
            },
        };
        let loc = source.iter().filter(|&&byte| byte == b'\n').count() + 1;
        let code_lines = if let Some(spool) = spool.as_deref_mut() {
            // One syntax-leaf mask feeds both map C and symbol areas. This
            // keeps the two line counts identical without another tree walk.
            let flags = code_line_flags(root, &source, language);
            let count = flags.iter().filter(|&&flag| flag).count();
            let started = Instant::now();
            let record =
                crate::symbols::collect_timed(root, &source, language, &flags, &mut collect_steps);
            spool.insert_timed(file, &record, &mut collect_steps)?;
            symbol_collection += started.elapsed();
            count
        } else {
            count_code_lines(root, &source, language)
        };
        // `source` and `tree` (and `root`, which borrows `tree`) go out of
        // scope at the end of this iteration -- the tree for this file is
        // never retained past the file that produced it.
        parsed.insert(
            file.clone(),
            ParsedFile {
                loc,
                code_lines,
                complexity,
                identifiers,
                symbols,
            },
        );
        raw.insert(file.clone(), file_raw);
        parse_stage.advance(1);
    }
    parse_stage.finish();
    Ok((parsed, raw, symbol_collection, collect_steps))
}

fn nonblank_lines(source: &[u8]) -> usize {
    source
        .split(|&byte| byte == b'\n')
        .filter(|line| line.iter().any(|byte| !byte.is_ascii_whitespace()))
        .count()
}

/// One traversal marks lines covered by syntax leaves. A string leaf can span
/// lines, so checking only its start would undercount multiline code.
fn count_code_lines(root: Node<'_>, source: &[u8], language: LanguageKind) -> usize {
    code_line_flags(root, source, language)
        .into_iter()
        .filter(|line| *line)
        .count()
}

/// Shared line mask for file and symbol areas. A symbol's count uses the
/// same syntax-leaf rule as the map's `C` field, so the areas add up.
pub(crate) fn code_line_flags(root: Node<'_>, source: &[u8], language: LanguageKind) -> Vec<bool> {
    let mut marked = vec![false; source.iter().filter(|&&byte| byte == b'\n').count() + 1];
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" || (language == LanguageKind::Python && is_docstring(node)) {
            continue;
        }
        if node.child_count() == 0 {
            if source[node.byte_range()]
                .iter()
                .all(u8::is_ascii_whitespace)
            {
                continue;
            }
            let start = node.start_position().row;
            let end = node.end_position();
            let last = if end.column == 0 && end.row > start {
                end.row - 1
            } else {
                end.row
            };
            for row in start..=last {
                if let Some(line) = marked.get_mut(row) {
                    *line = true;
                }
            }
        } else {
            for index in (0..node.child_count()).rev() {
                if let Some(child) = node.child(index) {
                    stack.push(child);
                }
            }
        }
    }
    marked
}

fn is_docstring(node: Node<'_>) -> bool {
    if node.kind() != "expression_statement" || node.named_child_count() != 1 {
        return false;
    }
    let Some(value) = node.named_child(0) else {
        return false;
    };
    if value.kind() != "string" && value.kind() != "concatenated_string" {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    // Comments are named tree-sitter children, but Python does not count
    // them as statements before a docstring.
    let first_statement = (0..parent.named_child_count())
        .filter_map(|index| parent.named_child(index))
        .find(|child| child.kind() != "comment");
    match parent.kind() {
        "module" => first_statement == Some(node),
        "block" => {
            let Some(owner) = parent.parent() else {
                return false;
            };
            matches!(owner.kind(), "class_definition" | "function_definition")
                && owner.child_by_field_name("body") == Some(parent)
                && first_statement == Some(node)
        }
        _ => false,
    }
}

/// The parse+resolve output of one `(pkg, language)` source: everything
/// [`finish_graph`] needs, before it is unioned with any other source's.
struct SourceIntermediate {
    pkg: String,
    language: LanguageKind,
    parsed: BTreeMap<String, ParsedFile>,
    /// File paths in lexical order. Their indices are the `FileId`s used by
    /// every high-cardinality structure below, so tuple order is identical
    /// to the previous `(String, String)` BTree order.
    files: Vec<String>,
    static_edges: BTreeMap<(FileId, FileId), f64>,
    directed: BTreeMap<(FileId, FileId), f64>,
    fanin: BTreeMap<FileId, f64>,
    uses: BTreeSet<(FileId, FileId, String)>,
    /// File -> the module/display name `finish_graph` puts on `SourceNode`.
    /// Python's is a dotted module name (`module_name`); Go/TypeScript's is
    /// just the file path. Carried as a map instead of a closure so it can
    /// be merged across sources without boxing a per-source `Fn`.
    module_for: BTreeMap<String, String>,
}

/// The union of every [`SourceIntermediate`], with cross-source file
/// collisions resolved (first source in sorted order wins -- see
/// `build_multi_source`) and every signal map filtered down to edges/uses
/// whose files both survived that resolution, so nothing downstream can
/// index a file that got dropped.
struct MergedSources {
    parsed: BTreeMap<String, ParsedFile>,
    files: Vec<String>,
    static_edges: BTreeMap<(FileId, FileId), f64>,
    directed: BTreeMap<(FileId, FileId), f64>,
    fanin: BTreeMap<FileId, f64>,
    uses: BTreeSet<(FileId, FileId, String)>,
    module_for: BTreeMap<String, String>,
    file_language: BTreeMap<String, LanguageKind>,
    /// One `(pkg, lang)` per source, in the same sorted order they were
    /// merged -- `GraphData.sources`.
    sources: Vec<(String, String)>,
    /// The source with the most surviving files, ties broken by sorted
    /// order (earliest wins) -- `GraphData.pkg`/`GraphData.lang`.
    dominant_pkg: String,
    dominant_lang: LanguageKind,
}

fn file_ids(
    paths: impl IntoIterator<Item = String>,
) -> Result<(Vec<String>, BTreeMap<String, FileId>)> {
    let mut files = paths.into_iter().collect::<Vec<_>>();
    files.sort();
    ensure!(
        files.len() <= FileId::MAX as usize,
        "source has more files than u32 can index"
    );
    let ids = files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.clone(), index as FileId))
        .collect();
    Ok((files, ids))
}

fn ordered_file_pair(a: FileId, b: FileId) -> (FileId, FileId) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

fn union_sources(mut intermediates: Vec<SourceIntermediate>) -> Result<MergedSources> {
    // The common path, including both repositories that exposed issue #52,
    // has one detected source. Its lexical FileIds already are the global
    // FileIds, so rebuilding tens of millions of compact entries here would
    // add a second tree with no remapping benefit.
    if intermediates.len() == 1 {
        let intermediate = intermediates.pop().expect("one source");
        let SourceIntermediate {
            pkg,
            language,
            parsed,
            files,
            static_edges,
            directed,
            fanin,
            uses,
            module_for,
        } = intermediate;
        let file_language = files.iter().map(|file| (file.clone(), language)).collect();
        return Ok(MergedSources {
            parsed,
            files,
            static_edges,
            directed,
            fanin,
            uses,
            module_for,
            file_language,
            sources: vec![(pkg.clone(), language.as_str().to_owned())],
            dominant_pkg: pkg,
            dominant_lang: language,
        });
    }

    // First pass: decide which source owns each file path. Iterating
    // `intermediates` in order (already sorted by (lang, pkg) in
    // `build_multi_source`) and taking the first claim with
    // `Entry::or_insert` is exactly "the first source in sorted order wins".
    let mut owner_index = BTreeMap::<String, usize>::new();
    let mut owner_source = BTreeMap::<String, (String, LanguageKind)>::new();
    for (idx, intermediate) in intermediates.iter().enumerate() {
        for file in intermediate.parsed.keys() {
            if owner_index.contains_key(file) {
                let (winner_pkg, winner_lang) = &owner_source[file];
                eprintln!(
                    "warning: {file} is claimed by more than one source ({} {winner_pkg} and {} {}); keeping {} {winner_pkg} (earlier in sorted source order)",
                    winner_lang.as_str(),
                    intermediate.language.as_str(),
                    intermediate.pkg,
                    winner_lang.as_str()
                );
            } else {
                owner_index.insert(file.clone(), idx);
                owner_source.insert(
                    file.clone(),
                    (intermediate.pkg.clone(), intermediate.language),
                );
            }
        }
    }

    let (files, global_ids) = file_ids(owner_index.keys().cloned())?;
    let mut parsed = BTreeMap::new();
    let mut file_language = BTreeMap::new();
    let mut module_for = BTreeMap::new();
    let mut static_edges = BTreeMap::<(FileId, FileId), f64>::new();
    let mut directed = BTreeMap::<(FileId, FileId), f64>::new();
    let mut fanin = BTreeMap::<FileId, f64>::new();
    let mut uses = BTreeSet::new();
    let mut sources = Vec::with_capacity(intermediates.len());
    let mut dominant: Option<(usize, String, LanguageKind)> = None;

    for (idx, intermediate) in intermediates.into_iter().enumerate() {
        let SourceIntermediate {
            pkg,
            language,
            parsed: source_parsed,
            files: source_files,
            static_edges: source_static,
            directed: source_directed,
            fanin: source_fanin,
            uses: source_uses,
            module_for: source_module_for,
        } = intermediate;

        sources.push((pkg.clone(), language.as_str().to_owned()));

        let mut kept = 0usize;
        for (file, value) in source_parsed {
            if owner_index.get(&file) != Some(&idx) {
                continue;
            }
            file_language.insert(file.clone(), language);
            if let Some(module) = source_module_for.get(&file) {
                module_for.insert(file.clone(), module.clone());
            }
            parsed.insert(file, value);
            kept += 1;
        }
        let better = dominant.as_ref().is_none_or(|(best, _, _)| kept > *best);
        if better {
            dominant = Some((kept, pkg, language));
        }

        // Every edge/use below is intra-source by construction (resolution
        // only ever looks a target up in that source's own known-file set),
        // so a dropped file drops exactly the edges that named it, never a
        // partial pair.
        for ((a, b), value) in source_static {
            let a = &source_files[a as usize];
            let b = &source_files[b as usize];
            if owner_index.get(a) == Some(&idx) && owner_index.get(b) == Some(&idx) {
                *static_edges
                    .entry((global_ids[a], global_ids[b]))
                    .or_insert(0.0) += value;
            }
        }
        for ((a, b), value) in source_directed {
            let a = &source_files[a as usize];
            let b = &source_files[b as usize];
            if owner_index.get(a) == Some(&idx) && owner_index.get(b) == Some(&idx) {
                *directed
                    .entry((global_ids[a], global_ids[b]))
                    .or_insert(0.0) += value;
            }
        }
        for (file_id, value) in source_fanin {
            let file = &source_files[file_id as usize];
            if owner_index.get(file) == Some(&idx) {
                *fanin.entry(global_ids[file]).or_insert(0.0) += value;
            }
        }
        for (a_id, b_id, name) in source_uses {
            let a = &source_files[a_id as usize];
            let b = &source_files[b_id as usize];
            if owner_index.get(a) == Some(&idx) && owner_index.get(b) == Some(&idx) {
                uses.insert((global_ids[a], global_ids[b], name));
            }
        }
    }

    let (_, dominant_pkg, dominant_lang) = dominant.expect("at least one source");
    Ok(MergedSources {
        parsed,
        files,
        static_edges,
        directed,
        fanin,
        uses,
        module_for,
        file_language,
        sources,
        dominant_pkg,
        dominant_lang,
    })
}

/// Removes a temporary index directory on every exit path. Go's module
/// cache marks what it writes read-only, so a failed removal is ignored
/// rather than failing a build that already succeeded.
struct IndexWorkDir(Option<PathBuf>);

impl Drop for IndexWorkDir {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

/// `--refs scip` (issue #110, P1a): index every language present in
/// `merged`, and wherever an index passes the fallback gate, replace that
/// language's static edges (`static_edges`, `directed`, `fanin`) with the
/// SCIP file graph. The blend in `finish_graph` is untouched, so the SCIP
/// signal is mass-normalised exactly as the hand-written one was
/// (finding 1). `uses` stays hand-written: it names roads, not the graph.
///
/// Each directed pair weighs the distinct symbols it references (P0's
/// primary weighting, finding 41), and its undirected sum is the static
/// edge, as the hand-written resolver sums import counts.
///
/// Returns the per-language coverage rows and, when symbols are collected,
/// the reference occurrences `symbols.rs` credits once the symbol order is
/// final. A language that falls back contributes neither.
///
/// Indexes go to a temporary directory that is removed afterwards, or to
/// `TOLMAP_SCIP_INDEX_DIR` when set, where they are kept as `<lang>.scip`
/// so a measurement can run P0's oracle on the very index the build read.
fn apply_scip_references(
    repo: &Path,
    merged: &mut MergedSources,
    keep_symbol_refs: bool,
    progress: &crate::progress::Progress,
) -> Result<(
    BTreeMap<String, ReferenceCoverage>,
    Option<crate::symbols::ScipSymbolRefs>,
)> {
    use crate::scip_ingest::{gate, ingest, recall_granularity, MIN_RECALL};

    let languages = merged
        .files
        .iter()
        .map(|file| merged.file_language[file])
        .collect::<Vec<_>>();
    let mut present = languages.clone();
    present.sort_by_key(|language| language.as_str());
    present.dedup();

    let kept = std::env::var_os("TOLMAP_SCIP_INDEX_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let work = kept.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "tolmap-scip-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    });
    fs::create_dir_all(&work).with_context(|| format!("create {}", work.display()))?;
    let _cleanup = IndexWorkDir(kept.is_none().then(|| work.clone()));

    let mut report = BTreeMap::new();
    let mut symbols = keep_symbol_refs.then(|| crate::symbols::ScipSymbolRefs {
        files: merged.files.clone(),
        ..Default::default()
    });
    for language in present {
        let scope = merged
            .files
            .iter()
            .enumerate()
            .filter(|(id, _)| languages[*id] == language)
            .map(|(id, file)| (file.clone(), id as u32))
            .collect::<BTreeMap<_, _>>();
        let hand = merged
            .directed
            .keys()
            .filter(|(a, _)| languages[*a as usize] == language)
            .copied()
            .collect::<BTreeSet<_>>();
        let mut row = ReferenceCoverage {
            path: "hand".to_owned(),
            reason: String::new(),
            indexer: None,
            exit_code: None,
            files: scope.len(),
            files_indexed: None,
            hand_pairs: gate(language, &hand, &BTreeSet::new(), &merged.files).hand_pairs,
            scip_pairs: None,
            recall: None,
            min_recall: MIN_RECALL,
            granularity: recall_granularity(language).to_owned(),
        };

        let stage = progress.stage(
            crate::progress::StageId::index_for(language),
            Some(scope.len() as u64),
        );
        let output = work.join(format!("{}.scip", language.as_str()));
        // A kept directory from an earlier run must never be read as this
        // run's index.
        let _ = fs::remove_file(&output);
        let log = |message: String| progress.log(message);
        let ingested = match crate::indexers::run(repo, language, &output, &stage, &log) {
            Err(failure) => {
                row.reason = failure.reason().to_owned();
                row.exit_code = failure.exit_code();
                None
            }
            Ok(()) => match ingest(&output, &scope) {
                Ok(ingested) => Some(ingested),
                Err(error) => {
                    progress.log(format!(
                        "scip ingest failed for {}: {error:#}",
                        language.as_str()
                    ));
                    row.reason = "ingest_failed".to_owned();
                    None
                }
            },
        };
        stage.set(ingested.as_ref().map_or(0, |i| i.indexed_files as u64));
        stage.finish();

        if let Some(ingested) = ingested {
            row.indexer = Some(ingested.tool.clone()).filter(|tool| !tool.is_empty());
            row.files_indexed = Some(ingested.indexed_files);
            row.scip_pairs = Some(ingested.file_edges.len());
            let scip_pairs = ingested.file_edges.keys().copied().collect::<BTreeSet<_>>();
            let outcome = gate(language, &hand, &scip_pairs, &merged.files);
            row.recall = outcome.recall.map(|recall| round_to(recall, 4));
            if ingested.indexed_files == 0 {
                row.reason = "no_documents".to_owned();
            } else if !outcome.passed {
                row.reason = "below_min_recall".to_owned();
            } else {
                row.path = "scip".to_owned();
                row.reason = "indexed".to_owned();
                let in_language = |id: FileId| languages[id as usize] == language;
                // Static edges are intra-language by construction (see
                // `union_sources`), so the source end decides.
                merged.static_edges.retain(|(a, _), _| !in_language(*a));
                merged.directed.retain(|(a, _), _| !in_language(*a));
                merged.fanin.retain(|file, _| !in_language(*file));
                for (&(a, b), edge) in &ingested.file_edges {
                    let value = edge.symbols as f64;
                    *merged.directed.entry((a, b)).or_default() += value;
                    *merged
                        .static_edges
                        .entry(ordered_file_pair(a, b))
                        .or_default() += value;
                    *merged.fanin.entry(b).or_default() += value;
                }
                if let Some(symbols) = symbols.as_mut() {
                    symbols.languages.insert(language.as_str().to_owned());
                    symbols.refs.extend(ingested.refs);
                    symbols.implementations.extend(ingested.implementations);
                }
            }
        }
        progress.log(format!(
            "references {}: {} ({}; recall {} at {} granularity, min {:.2}; {} hand pairs, {} SCIP pairs)",
            language.as_str(),
            row.path,
            row.reason,
            row.recall
                .map_or_else(|| "n/a".to_owned(), |recall| format!("{recall:.4}")),
            row.granularity,
            MIN_RECALL,
            row.hand_pairs,
            row.scip_pairs
                .map_or_else(|| "no".to_owned(), |pairs| pairs.to_string()),
        ));
        report.insert(language.as_str().to_owned(), row);
    }
    Ok((report, symbols))
}

// pub(crate): `detect` re-walks the same tree with the same filters to count
// candidate source roots before a real build runs -- reusing this rather
// than a second file-matching implementation keeps the detector's file
// counts identical to what `build` would actually index.
pub(crate) fn source_files(repo: &Path, pkg: &str, language: LanguageKind) -> Result<Vec<String>> {
    let root = repo.join(pkg);
    ensure!(
        root.is_dir(),
        "source root {} is not a directory",
        root.display()
    );
    let mut result = Vec::new();
    collect_source_files(repo, &root, language, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect_source_files(
    repo: &Path,
    directory: &Path,
    language: LanguageKind,
    result: &mut Vec<String>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("read directory {}", directory.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            let skip = match language {
                LanguageKind::Python => PY_SKIP_DIR.contains(&name.as_str()),
                LanguageKind::Go | LanguageKind::TypeScript => {
                    MULTI_SKIP_DIR.contains(&name.as_str()) || name.starts_with('.')
                }
            };
            if !skip {
                collect_source_files(repo, &path, language, result)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let accepted = match language {
            LanguageKind::Python => name.ends_with(".py"),
            LanguageKind::Go => name.ends_with(".go") && !name.ends_with("_test.go"),
            LanguageKind::TypeScript => {
                // `.tsx` alongside `.ts`: the import resolver
                // (`resolve_multi`) has always listed `{base}.tsx` as a
                // resolution candidate, but until this file collected it too
                // a `.tsx` target could never be a node -- the import into
                // it, and the component it pointed at, were silently
                // dropped (issue #35). `.d.tsx` is not a real declaration
                // suffix (`.d.ts` is TypeScript-only syntax), so no matching
                // exclusion is added for it.
                (name.ends_with(".ts") || name.ends_with(".tsx"))
                    && !name.ends_with(".d.ts")
                    && !name.contains(".test.")
                    && !name.contains(".spec.")
            }
        };
        if accepted {
            result.push(relative_slash(repo, &path)?);
        }
    }
    Ok(())
}

fn relative_slash(base: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(base)?;
    Ok(relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn walk(root: Node<'_>) -> Vec<Node<'_>> {
    let mut stack = vec![root];
    let mut result = Vec::new();
    while let Some(node) = stack.pop() {
        result.push(node);
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        stack.extend(children.into_iter().rev());
    }
    result
}

fn text<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

fn child_text(node: Node<'_>, field: &str, source: &[u8]) -> Option<String> {
    node.child_by_field_name(field)
        .map(|child| text(child, source).to_owned())
}

fn name_of(node: Node<'_>, source: &[u8]) -> Option<String> {
    if let Some(value) = child_text(node, "name", source) {
        return Some(value);
    }
    let mut cursor = node.walk();
    let result = node
        .named_children(&mut cursor)
        .find(|child| {
            matches!(
                child.kind(),
                "identifier" | "type_identifier" | "field_identifier" | "property_identifier"
            )
        })
        .map(|child| text(child, source).to_owned());
    result
}

fn python_metrics(
    root: Node<'_>,
    source: &[u8],
) -> (usize, BTreeMap<String, usize>, Vec<SymbolRow>) {
    let nodes = walk(root);
    let mut complexity = 0;
    let mut identifiers = BTreeMap::<String, usize>::new();
    for node in &nodes {
        match node.kind() {
            "function_definition" => {
                complexity += 1;
                if let Some(name) = name_of(*node, source) {
                    *identifiers.entry(name).or_default() += 3;
                }
            }
            "class_definition" => {
                if let Some(name) = name_of(*node, source) {
                    *identifiers.entry(name).or_default() += 3;
                }
            }
            // `async for` reuses the plain `for_statement` node (an unlabeled
            // leading `async` token, confirmed by dumping the parse tree), but
            // Python's ast puts it on a DIFFERENT node type -- AsyncFor, not
            // For -- and complexity() only tests `isinstance(x, ast.For)`.
            // Counting it here overcounts by one per async-for loop; on
            // httpx/_models.py that is 4 statement-level async-for loops
            // (list-comprehension `async for` uses a distinct `for_in_clause`
            // node already outside this match, so those were never at risk),
            // matching the file's complexity landing at 273 instead of the
            // reference's 269 and, in turn, its "hazard" landmark's detail
            // string reading "cplx 273" against the reference's "cplx 269"
            // (commit 8fbd0c6).
            "for_statement" if is_async(*node) => {}
            "if_statement" | "elif_clause" | "for_statement" | "while_statement"
            | "except_clause" | "with_statement" | "assert_statement" => complexity += 1,
            "boolean_operator" => {
                if boolean_group_root(*node, source) {
                    complexity += 1;
                }
            }
            "attribute" => {
                if let Some(attribute) = child_text(*node, "attribute", source) {
                    *identifiers.entry(attribute).or_default() += 1;
                }
            }
            "identifier" if python_name_identifier(*node) => {
                let value = text(*node, source);
                *identifiers.entry(value.to_owned()).or_default() += 1;
            }
            _ => {}
        }
    }
    identifiers.retain(|word, _| {
        word.len() > 2 && !word.starts_with("__") && !IDENT_STOP.contains(&word.as_str())
    });
    let symbols = python_symbols(root, source);
    (complexity, identifiers, symbols)
}

/// Whether a node's first child is the unlabeled `async` keyword token --
/// `async def`, `async for` and `async with` all parse to their ordinary
/// node kind (`function_definition`, `for_statement`, `with_statement`) with
/// this as the only marker.
fn is_async(node: Node<'_>) -> bool {
    node.child(0).is_some_and(|child| child.kind() == "async")
}

fn boolean_group_root(node: Node<'_>, source: &[u8]) -> bool {
    let Some(parent) = node.parent() else {
        return true;
    };
    if parent.kind() != "boolean_operator" {
        return true;
    }
    boolean_operator(node, source) != boolean_operator(parent, source)
}

fn boolean_operator<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    let mut cursor = node.walk();
    let result = node
        .children(&mut cursor)
        .find(|child| child.kind() == "and" || child.kind() == "or")
        .map_or("", |child| text(child, source));
    result
}

fn python_name_identifier(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "attribute"
        && parent.child_by_field_name("attribute").map(|n| n.id()) == Some(node.id())
    {
        return false;
    }
    // `except X as name:` and `with EXPR as name:` parse to the same shape --
    // an `as_pattern` wrapping an `as_pattern_target` -- but Python's ast does
    // not treat them alike. `ExceptHandler.name` is a plain string (never a
    // Name node), while `withitem.optional_vars` is a real assignment-target
    // expression that ast.walk DOES visit. Confirmed against identifiers() on
    // scrapy: 13 files disagreed, every one of them by exactly the exception
    // alias (`exc`, `exception`, `spider_exc`, `conn`, `params`, ...) counted
    // once too many because this exclusion was missing.
    if parent.kind() == "as_pattern_target" {
        let in_except_binding = parent
            .parent()
            .and_then(|as_pattern| as_pattern.parent())
            .is_some_and(|grandparent| grandparent.kind() == "except_clause");
        if in_except_binding {
            return false;
        }
    }
    // `default_parameter`, `typed_default_parameter` and `keyword_argument` each
    // carry a `name` field (a syntactic label -- Python's ast never turns this
    // into a Name node: `arg.arg` and `keyword.arg` are plain strings) AND a
    // `value`/`type` field (a real expression, which ast.walk DOES descend into
    // and identifiers() DOES count -- a default value, an annotation, a keyword
    // argument's value). Blanket-excluding the whole parent kind, as a `dotted_name`
    // or `parameters` exclusion correctly does, was dropping the value half along
    // with the label half: `def f(x=DEFAULT)` never counted `DEFAULT`, and
    // `f(kwarg=other)` never counted `other`. Confirmed against ast.walk on a
    // snippet exercising all three: reference identifiers() gives DEFAULT: 2,
    // other: 1; the parent-kind-only check gave DEFAULT: 1 (only the assignment
    // target), other: 0.
    if matches!(
        parent.kind(),
        "default_parameter" | "typed_default_parameter" | "keyword_argument"
    ) {
        return parent.child_by_field_name("name").map(|n| n.id()) != Some(node.id());
    }
    // `*rest` and `**rest` parse to the same `list_splat_pattern` /
    // `dictionary_splat_pattern` node whether they unpack a function parameter
    // (`def f(*args, **kwargs)`, where arg.arg is a plain string -- not
    // counted) or a starred assignment/for target (`*module, _ = x.split(".")`,
    // `for *rest, last in pairs`, where it is `ast.Starred(value=Name(...))` --
    // counted). The two are distinguished by the splat pattern's own parent:
    // `parameters`/`lambda_parameters` for the former (directly, or one level
    // down through `typed_parameter` for an annotated `*args: T`), `pattern_list`
    // (or a bare assignment/for target) for the latter. Confirmed on scrapy's
    // `utils/reactor.py`: identifiers() counts `module` twice (the starred
    // target and a later plain use); blanket-excluding the parent gave 1. The
    // `typed_parameter` hop matters too: `utils/signal.py` declares
    // `*arguments: TypingAny, **named: TypingAny` on five functions --
    // checking only the immediate parent left those uncounted-by-Python names
    // slipping through as counted (arguments 6->11, named 12->17).
    if matches!(
        parent.kind(),
        "list_splat_pattern" | "dictionary_splat_pattern"
    ) {
        let grandparent = parent.parent();
        let is_parameter_unpack = grandparent.is_some_and(|pp| match pp.kind() {
            "parameters" | "lambda_parameters" => true,
            "typed_parameter" => pp
                .parent()
                .is_some_and(|ppp| matches!(ppp.kind(), "parameters" | "lambda_parameters")),
            _ => false,
        });
        return !is_parameter_unpack;
    }
    if matches!(
        parent.kind(),
        "function_definition"
            | "class_definition"
            | "parameters"
            | "lambda_parameters"
            | "typed_parameter"
            | "import_statement"
            | "import_from_statement"
            | "aliased_import"
            | "dotted_name"
            // `global x` / `nonlocal x` name the binding as a plain string on
            // Global/Nonlocal -- never a Name node, so ast.walk never sees it.
            | "global_statement"
            | "nonlocal_statement"
    ) {
        return false;
    }
    true
}

fn unwrap_decorated(node: Node<'_>) -> Node<'_> {
    if node.kind() != "decorated_definition" {
        return node;
    }
    let mut cursor = node.walk();
    let result = node
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "class_definition" | "function_definition"))
        .unwrap_or(node);
    result
}

fn unwrap_expression_statement(node: Node<'_>) -> Node<'_> {
    if node.kind() != "expression_statement" {
        return node;
    }
    let mut cursor = node.walk();
    let result = node
        .named_children(&mut cursor)
        .next()
        .filter(|child| child.kind() == "assignment")
        .unwrap_or(node);
    result
}

fn python_symbols(root: Node<'_>, source: &[u8]) -> Vec<SymbolRow> {
    let mut symbols = Vec::new();
    let mut cursor = root.walk();
    for top in root.named_children(&mut cursor) {
        let top = unwrap_expression_statement(unwrap_decorated(top));
        match top.kind() {
            "class_definition" => {
                if let Some(name) = name_of(top, source) {
                    symbols.push(python_symbol_row(name.clone(), KIND_CLASS, top));
                    if let Some(body) = top.child_by_field_name("body") {
                        let mut body_cursor = body.walk();
                        for method in body.named_children(&mut body_cursor) {
                            let method = unwrap_decorated(method);
                            if method.kind() == "function_definition" {
                                if let Some(method_name) = name_of(method, source) {
                                    if !method_name.starts_with("__") {
                                        symbols.push(python_symbol_row(
                                            format!("{name}.{method_name}"),
                                            KIND_METHOD,
                                            method,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "function_definition" => {
                if let Some(name) = name_of(top, source) {
                    symbols.push(python_symbol_row(name, KIND_FUNC, top));
                }
            }
            "assignment" => {
                if top
                    .end_position()
                    .row
                    .saturating_sub(top.start_position().row)
                    >= 2
                {
                    if let Some(left) = top.child_by_field_name("left") {
                        if left.kind() == "identifier" {
                            symbols.push(python_symbol_row(
                                text(left, source).to_owned(),
                                KIND_CONST,
                                top,
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    symbols.sort_by_key(|symbol| symbol.0 .2);
    symbols.truncate(60);
    symbols
}

fn symbol_row(name: String, kind: usize, node: Node<'_>) -> SymbolRow {
    SymbolRow((
        name,
        kind,
        node.start_position().row + 1,
        node.end_position().row + 1,
    ))
}

/// Like `symbol_row`, but the end line is the last line with actual code in
/// it, not the raw node span. Python's ast has no representation for a
/// comment at all -- `FunctionDef.end_lineno` is the end of the last real
/// statement -- but tree-sitter-python's grammar attaches a trailing
/// `comment` as a plain child of the enclosing `block`, so the block's (and
/// therefore the function/class/const's) own span extends to cover it.
/// Confirmed on sqlalchemy: `setinputsizes` in
/// `connectors/aioodbc.py` ends at its `return` on line 30, but the block
/// also holds two trailing comment lines through line 33, and the raw node
/// span reported 30..33 against the reference's 30..30 (19 of 258 files'
/// symbol tables differed, always by this same shape).
fn python_symbol_row(name: String, kind: usize, node: Node<'_>) -> SymbolRow {
    SymbolRow((
        name,
        kind,
        node.start_position().row + 1,
        effective_end_row(node) + 1,
    ))
}

/// The end row of `node` ignoring any trailing `comment` children, applied
/// recursively so a comment trailing the LAST statement of a nested block
/// (an if/for/while/try body, not just a function's own) doesn't leak into
/// the row reported for the whole enclosing symbol either.
fn effective_end_row(node: Node<'_>) -> usize {
    // A parent's span is always the union of its children's, so
    // `node.end_position()` equals its LAST child's end -- named or not. A
    // trailing `comment` only overshoots the real end when nothing
    // (notably no anonymous closing token, like a parenthesized
    // expression's `)`) follows it within this node -- i.e. the comment
    // itself reaches all the way to `node`'s own boundary, which is exactly
    // the case a block ends on via dedent with no explicit terminator.
    // `isinstance(...) or isinstance(...) # comment\n)` is the
    // counter-case: the comment is the last NAMED child, but the `)` after
    // it is real syntax the comment does NOT reach, so the raw span (which
    // already accounts for it) is correct as-is and must not be stripped.
    let mut cursor = node.walk();
    let named = node.named_children(&mut cursor).collect::<Vec<_>>();
    let Some(&last) = named.last() else {
        return node.end_position().row;
    };
    if last.end_byte() != node.end_byte() {
        return node.end_position().row;
    }
    if last.kind() == "comment" {
        return named
            .iter()
            .rev()
            .find(|child| child.kind() != "comment")
            .map(|child| effective_end_row(*child))
            .unwrap_or_else(|| node.start_position().row);
    }
    effective_end_row(last)
}

fn multi_metrics(
    root: Node<'_>,
    source: &[u8],
    language: LanguageKind,
) -> (usize, BTreeMap<String, usize>, Vec<SymbolRow>) {
    let mut complexity = 0;
    let mut identifiers = BTreeMap::new();
    let mut symbols = Vec::new();
    for node in walk(root) {
        if BRANCHY.contains(&node.kind()) {
            complexity += 1;
        } else if matches!(
            node.kind(),
            "identifier" | "type_identifier" | "field_identifier" | "property_identifier"
        ) {
            let value = text(node, source);
            if value.len() > 2 {
                *identifiers.entry(value.to_owned()).or_default() += 1;
            }
        }

        let kind = match language {
            LanguageKind::Go => match node.kind() {
                "function_declaration" => Some(KIND_FUNC),
                "method_declaration" => Some(KIND_METHOD),
                "type_spec" => {
                    let body = node.child_by_field_name("type");
                    Some(match body.map(|value| value.kind()) {
                        Some("interface_type") => KIND_INTERFACE,
                        Some("struct_type") => KIND_CLASS,
                        _ => KIND_TYPE,
                    })
                }
                _ => None,
            },
            LanguageKind::TypeScript => match node.kind() {
                "class_declaration" => Some(KIND_CLASS),
                "function_declaration" => Some(KIND_FUNC),
                "method_definition" => Some(KIND_METHOD),
                "interface_declaration" => Some(KIND_INTERFACE),
                "type_alias_declaration" => Some(KIND_TYPE),
                "variable_declarator" => {
                    let value = node.child_by_field_name("value");
                    if value.is_some_and(|value| {
                        matches!(value.kind(), "arrow_function" | "function_expression")
                    }) {
                        Some(KIND_FUNC)
                    } else if node.parent().is_some_and(|parent| {
                        parent.kind() == "lexical_declaration"
                            && node
                                .end_position()
                                .row
                                .saturating_sub(node.start_position().row)
                                >= 2
                    }) {
                        Some(KIND_CONST)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            LanguageKind::Python => None,
        };
        if let Some(kind) = kind {
            if let Some(name) = name_of(node, source) {
                // `ts_symbols` in the reference drops a leading-underscore
                // name as a privacy convention; `go_symbols` does not apply
                // that filter at all -- a leading underscore is an ordinary
                // exported-from-package-but-not-part-of-the-public-API
                // identifier in Go, not a marker of anything, and
                // `_newJSONEntry` in promql/query_logger.go is exactly that.
                // Gating this to TypeScript only was previously missing,
                // which silently dropped every such Go symbol.
                let excluded = language == LanguageKind::TypeScript && name.starts_with('_');
                if !excluded {
                    symbols.push(symbol_row(name, kind, node));
                }
            }
        }
    }
    symbols.sort_by_key(|symbol| symbol.0 .2);
    symbols.truncate(60);
    (complexity, identifiers, symbols)
}

fn parse_python(
    repo: &Path,
    pkg: &str,
    parsed: BTreeMap<String, ParsedFile>,
    raw: BTreeMap<String, FileRaw>,
) -> Result<SourceIntermediate> {
    let progress = crate::progress::Progress::silent();
    let stage = progress.stage(crate::progress::StageId::Resolve, Some(parsed.len() as u64));
    let result = parse_python_with_progress(repo, pkg, parsed, raw, &stage);
    if result.is_ok() {
        stage.finish();
    }
    result
}

fn parse_python_with_progress(
    repo: &Path,
    pkg: &str,
    parsed: BTreeMap<String, ParsedFile>,
    raw: BTreeMap<String, FileRaw>,
    progress: &crate::progress::StageCounter,
) -> Result<SourceIntermediate> {
    let (files, ids) = file_ids(parsed.keys().cloned())?;
    let mut modules = BTreeMap::<String, String>::new();
    for file in parsed.keys() {
        modules.insert(module_name(file, pkg), file.clone());
    }
    let known = modules.keys().cloned().collect::<BTreeSet<_>>();
    let file_of = modules.clone();
    // A monorepo can contain an independently launched Python project below
    // the selected source root. Python searches sys.path entries for absolute
    // imports; a process launched in that project can import `core.x` from
    // `api/core/x.py`, while the repository-root spelling is `api.core.x`.
    // Keep both exact, parsed-file spellings, scoped by the declaring project
    // directory. See https://docs.python.org/3/reference/import.html#the-module-cache
    // and https://docs.python.org/3/reference/import.html#the-path-based-finder.
    let mut project_for = BTreeMap::<String, String>::new();
    let mut project_modules = BTreeMap::<String, BTreeMap<String, String>>::new();
    if pkg == "." {
        for file in parsed.keys() {
            if let Some(project) = python_project_root(repo, file) {
                project_modules
                    .entry(project.clone())
                    .or_default()
                    .insert(python_project_module(file, &project), file.clone());
                project_for.insert(file.clone(), project);
            }
        }
    }
    let project_known = project_modules
        .iter()
        .map(|(project, files)| {
            (
                project.clone(),
                files.keys().cloned().collect::<BTreeSet<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut static_edges = BTreeMap::<(FileId, FileId), f64>::new();
    let mut directed = BTreeMap::<(FileId, FileId), f64>::new();
    let mut fanin = BTreeMap::<FileId, f64>::new();
    let mut uses = BTreeSet::<(FileId, FileId, String)>::new();
    for (module, file) in &modules {
        let file_id = ids[file];
        let FileRaw::Python {
            imports,
            attribute_candidates,
        } = &raw[file]
        else {
            unreachable!("parse_python only ever stores FileRaw::Python");
        };
        let is_pkg = file.rsplit('/').next() == Some("__init__.py");
        for import in imports {
            let mut target_files = resolve_python(import, module, &known, is_pkg)
                .into_iter()
                .map(|target| file_of[&target].clone())
                .collect::<BTreeSet<_>>();
            if let Some(project) = project_for.get(file) {
                let scoped = &project_modules[project];
                let current = python_project_module(file, project);
                if python_relative_within_package(import, &current, is_pkg) {
                    for target in resolve_python(import, &current, &project_known[project], is_pkg)
                    {
                        target_files.insert(scoped[&target].clone());
                    }
                }
            }
            for target_file in target_files {
                let target_id = ids[&target_file];
                if target_id == file_id {
                    continue;
                }
                *static_edges
                    .entry(ordered_file_pair(file_id, target_id))
                    .or_default() += 1.0;
                *directed.entry((file_id, target_id)).or_default() += 1.0;
                *fanin.entry(target_id).or_default() += 1.0;
            }
        }
        let mut resolved_uses = python_uses_from_raw(
            module,
            file,
            imports,
            attribute_candidates,
            &known,
            &file_of,
        );
        if let Some(project) = project_for.get(file) {
            let scoped = &project_modules[project];
            let current = python_project_module(file, project);
            let scoped_imports = imports
                .iter()
                .filter(|import| python_relative_within_package(import, &current, is_pkg))
                .cloned()
                .collect::<Vec<_>>();
            resolved_uses.extend(python_uses_from_raw(
                &current,
                file,
                &scoped_imports,
                attribute_candidates,
                &project_known[project],
                scoped,
            ));
        }
        for (target_file, name) in resolved_uses {
            uses.insert((file_id, ids[&target_file], name));
        }
        progress.advance(1);
    }

    let module_for = parsed
        .keys()
        .map(|file| (file.clone(), module_name(file, pkg)))
        .collect();

    Ok(SourceIntermediate {
        pkg: pkg.to_owned(),
        language: LanguageKind::Python,
        parsed,
        files,
        static_edges,
        directed,
        fanin,
        uses,
        module_for,
    })
}

fn python_project_root(repo: &Path, file: &str) -> Option<String> {
    let mut directory = Path::new(file).parent()?;
    while !directory.as_os_str().is_empty() {
        if repo.join(directory).join("pyproject.toml").is_file()
            || repo.join(directory).join("setup.py").is_file()
        {
            return Some(directory.to_string_lossy().replace('\\', "/"));
        }
        directory = directory.parent()?;
    }
    None
}

fn python_project_module(file: &str, project: &str) -> String {
    let relative = file
        .strip_prefix(project)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(file);
    let stem = relative.strip_suffix(".py").unwrap_or(relative);
    let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
    let stem = if stem == "__init__" { "" } else { stem };
    stem.replace('/', ".")
}

fn python_relative_within_package(import: &PythonImport, current: &str, is_pkg: bool) -> bool {
    if import.level == 0 {
        return true;
    }
    let segments = current.split('.').filter(|part| !part.is_empty()).count();
    let package_depth = segments.saturating_sub(usize::from(!is_pkg));
    import.level <= package_depth
}

fn module_name(relative: &str, pkg: &str) -> String {
    let without_ext = relative.strip_suffix(".py").unwrap_or(relative);
    let mut parts = without_ext.split('/').collect::<Vec<_>>();
    if parts.last() == Some(&"__init__") {
        parts.pop();
    }
    let root = pkg.trim_end_matches('/').rsplit('/').next().unwrap_or(pkg);
    if let Some(position) = parts.iter().position(|part| *part == root) {
        parts = parts[position..].to_vec();
    }
    parts.join(".")
}

pub(crate) fn python_imports(root: Node<'_>, source: &[u8]) -> Vec<PythonImport> {
    let mut result = Vec::new();
    for node in walk(root) {
        match node.kind() {
            "import_statement" => {
                let mut names = Vec::new();
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if matches!(child.kind(), "dotted_name" | "aliased_import") {
                        if let Some(pair) = import_name(child, source) {
                            names.push(pair);
                        }
                    }
                }
                result.push(PythonImport {
                    from: false,
                    level: 0,
                    module: String::new(),
                    names,
                });
            }
            "import_from_statement" | "future_import_statement" => {
                let module_node = node.child_by_field_name("module_name");
                let raw_module = module_node.map_or("", |value| text(value, source));
                let level = raw_module.chars().take_while(|value| *value == '.').count();
                let module = raw_module[level..].to_owned();
                let mut names = Vec::new();
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if module_node.map(|value| value.id()) == Some(child.id()) {
                        continue;
                    }
                    if matches!(
                        child.kind(),
                        "dotted_name" | "aliased_import" | "wildcard_import"
                    ) {
                        if child.kind() == "wildcard_import" {
                            names.push(("*".to_owned(), None));
                        } else if let Some(pair) = import_name(child, source) {
                            names.push(pair);
                        }
                    }
                }
                result.push(PythonImport {
                    from: true,
                    level,
                    module,
                    names,
                });
            }
            _ => {}
        }
    }
    result
}

fn import_name(node: Node<'_>, source: &[u8]) -> Option<(String, Option<String>)> {
    if node.kind() == "aliased_import" {
        let name = child_text(node, "name", source)?;
        let alias = child_text(node, "alias", source);
        Some((name, alias))
    } else {
        Some((text(node, source).to_owned(), None))
    }
}

/// `is_pkg`: whether `current_module` names the importing file's own
/// package (the file is an `__init__.py`, already stripped of `__init__`
/// by `module_name()`) rather than an ordinary module inside that package.
/// A relative import resolves against the *containing package* -- itself
/// for a package `__init__`, `current_module` minus its last segment
/// otherwise -- then strips `import.level - 1` further segments.
/// Collapsing that distinction into a single `+ 1` (as this used to) is
/// correct only for the `__init__` case and silently drops every other
/// relative import: at level 1 it kept the whole module name, so
/// `from . import x` resolved to a name that was never a known module.
/// See issue #12.
pub(crate) fn python_head(import: &PythonImport, current_module: &str, is_pkg: bool) -> String {
    if import.level == 0 {
        return import.module.clone();
    }
    let mut pkg_parts = current_module.split('.').collect::<Vec<_>>();
    if !is_pkg {
        pkg_parts.pop();
    }
    let strip = import.level - 1;
    let keep = if strip <= pkg_parts.len() {
        pkg_parts.len() - strip
    } else {
        0
    };
    let prefix = pkg_parts[..keep].join(".");
    if import.module.is_empty() {
        prefix
    } else if prefix.is_empty() {
        import.module.clone()
    } else {
        format!("{prefix}.{}", import.module)
    }
}

fn resolve_python(
    import: &PythonImport,
    current_module: &str,
    known: &BTreeSet<String>,
    is_pkg: bool,
) -> BTreeSet<String> {
    let mut hits = Vec::new();
    if import.from {
        let head = python_head(import, current_module, is_pkg);
        hits.push(head.clone());
        for (name, _) in &import.names {
            hits.push(if head.is_empty() {
                name.clone()
            } else {
                format!("{head}.{name}")
            });
        }
    } else {
        hits.extend(import.names.iter().map(|(name, _)| name.clone()));
    }
    let mut result = BTreeSet::new();
    for hit in hits {
        if hit.is_empty() {
            continue;
        }
        if known.contains(&hit) {
            result.insert(hit);
        } else if let Some((parent, _)) = hit.rsplit_once('.') {
            if known.contains(parent) {
                result.insert(parent.to_owned());
            }
        }
    }
    result.remove(current_module);
    result
}

/// Diagnostic for files left with no edge after pruning. This reparses only
/// those files; the known-file set comes from the graph that was actually
/// built, so a syntactic import never counts as a relationship by itself.
pub fn coverage_diagnostics(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    candidate: &BTreeSet<String>,
    static_files: &BTreeSet<String>,
    after: &GraphData,
) -> Result<serde_json::Value> {
    use serde_json::json;
    let mut kept = BTreeSet::new();
    for edge in &after.edges {
        kept.insert(edge.a.as_str());
        kept.insert(edge.b.as_str());
    }
    let modules = sources
        .iter()
        .any(|(_, lang)| *lang != LanguageKind::Python)
        .then(|| module_index(repo))
        .transpose()?;
    let mut rows = Vec::new();
    let mut causes = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for (pkg, lang) in sources {
        let source_nodes = after
            .nodes
            .iter()
            .filter(|node| {
                node.lang == lang.as_str()
                    && (pkg == "."
                        || node
                            .file
                            .starts_with(&format!("{}/", pkg.trim_end_matches('/'))))
            })
            .collect::<Vec<_>>();
        let by_file = source_nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.file.clone(), i as FileId))
            .collect::<BTreeMap<_, _>>();
        let mut projects = BTreeMap::<String, BTreeSet<String>>::new();
        if pkg == "." && *lang == LanguageKind::Python {
            for file in by_file.keys() {
                if let Some(project) = python_project_root(repo, file) {
                    projects
                        .entry(project.clone())
                        .or_default()
                        .insert(python_project_module(file, &project));
                }
            }
        }
        let known = source_nodes
            .iter()
            .map(|node| module_name(&node.file, pkg))
            .collect::<BTreeSet<_>>();
        let mut by_directory = BTreeMap::<String, Vec<FileId>>::new();
        for (file, id) in &by_file {
            by_directory
                .entry(directory_name(file).to_owned())
                .or_default()
                .push(*id);
        }
        let mut parser = Parser::new();
        for node in source_nodes {
            if kept.contains(node.file.as_str()) {
                continue;
            }
            let bytes = fs::read(repo.join(&node.file))?;
            parser.set_language(&grammar_for_file(*lang, &node.file))?;
            let Some(tree) = parser.parse(&bytes, None) else {
                continue;
            };
            let root = tree.root_node();
            let mut imports = Vec::new();
            match lang {
                LanguageKind::Python => {
                    let current = module_name(&node.file, pkg);
                    let is_pkg = node.file.ends_with("/__init__.py");
                    let project = (pkg == ".")
                        .then(|| python_project_root(repo, &node.file))
                        .flatten();
                    let scoped_known = project.as_ref().and_then(|project| projects.get(project));
                    for spec in python_imports(root, &bytes) {
                        let head = python_head(&spec, &current, is_pkg);
                        let mut targets = resolve_python(&spec, &current, &known, is_pkg);
                        if let (Some(project), Some(scoped_known)) =
                            (project.as_ref(), scoped_known)
                        {
                            let scoped_current = python_project_module(&node.file, project);
                            if python_relative_within_package(&spec, &scoped_current, is_pkg) {
                                targets.extend(resolve_python(
                                    &spec,
                                    &scoped_current,
                                    scoped_known,
                                    is_pkg,
                                ));
                            }
                        }
                        let relative_error = spec.level > 0
                            && spec.level > current.split('.').count() - usize::from(!is_pkg);
                        let reason = if relative_error {
                            "relative_level_error"
                        } else if targets.is_empty() {
                            "no_candidate_in_parsed_set"
                        } else if targets.iter().all(|target| target == &current) {
                            "self_only"
                        } else {
                            "resolved"
                        };
                        let display = if spec.from {
                            format!(
                                "from {}{} import {}",
                                ".".repeat(spec.level),
                                spec.module,
                                spec.names
                                    .iter()
                                    .map(|(name, _)| name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        } else {
                            format!(
                                "import {}",
                                spec.names
                                    .iter()
                                    .map(|(name, _)| name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        imports.push(json!({"specifier": display, "head": head, "project_root": project, "reason": reason, "targets": targets}));
                    }
                }
                LanguageKind::Go | LanguageKind::TypeScript => {
                    let specs = if *lang == LanguageKind::Go {
                        go_imports(root, &bytes)
                    } else {
                        typescript_imports(root, &bytes)
                    };
                    for spec in specs {
                        let targets = resolve_multi(
                            repo,
                            *lang,
                            &spec,
                            &node.file,
                            modules.as_ref().expect("module index"),
                            &by_directory,
                            &by_file,
                        );
                        imports.push(json!({"specifier": spec, "reason": if targets.as_slice().is_empty() { "no_candidate_in_parsed_set" } else { "resolved" }, "target_count": targets.as_slice().len()}));
                    }
                }
            }
            let cause = if candidate.contains(node.file.as_str()) {
                "candidate_pruned"
            } else if static_files.contains(node.file.as_str()) {
                "static_below_candidate_threshold"
            } else if imports.is_empty() {
                "no_static_imports"
            } else if imports.iter().any(|item| item["reason"] == "resolved") {
                "resolved_without_kept_edge"
            } else {
                "no_resolved_import"
            };
            *causes
                .entry(lang.as_str().to_owned())
                .or_default()
                .entry(cause.to_owned())
                .or_default() += 1;
            rows.push(json!({"file": node.file, "lang": lang.as_str(), "cause": cause, "imports": imports}));
        }
    }
    rows.sort_by(|a, b| a["file"].as_str().cmp(&b["file"].as_str()));
    Ok(json!({"by_language": causes, "zero_edge_files": rows}))
}

/// Bucket every non-relative TypeScript import specifier that names a
/// declared workspace package (issue #101). Unlike [`coverage_diagnostics`],
/// this scans every TypeScript file, not only ones left with zero kept edges
/// -- an importer with unrelated resolved edges elsewhere would otherwise
/// never be reparsed, and its unresolved workspace imports would never be
/// counted.
///
/// Five buckets, matching what the owner asked issue #101's measurement to
/// report:
/// - `resolved`: the import resolves to a file already in the parsed set.
/// - `resolved_but_excluded`: [`package_entry_candidates`] names a real file
///   on disk that source collection never parses (e.g. under `generated/`,
///   `MULTI_SKIP_DIR`). This is the raw specifier-level classification and is
///   reported unchanged from before issue #101's redirect rule shipped.
/// - `redirected_from_excluded`: the subset of `resolved_but_excluded` where
///   [`redirect_excluded_workspace_import`] actually found a package entry or
///   fallback file to redirect the edge to (issue #101, "count as edge
///   targets only") -- these are the ones that turned into a real edge in
///   the map, not just a diagnostic count. `resolved_but_excluded` minus this
///   is what stayed unresolved because the package had no parsed file at
///   all to redirect to.
/// - `unresolved`: the specifier names a declared workspace package, but no
///   candidate path exists on disk at all (a typo, a missing subpath, an
///   `exports` map that does not cover it).
/// - `external`: the specifier does not match any declared workspace
///   package prefix at all (an npm dependency).
pub fn workspace_import_coverage(
    repo: &Path,
    sources: &[(String, LanguageKind)],
) -> Result<serde_json::Value> {
    use serde_json::json;
    let mut resolved = 0usize;
    let mut resolved_but_excluded = 0usize;
    let mut redirected_from_excluded = 0usize;
    let mut unresolved = 0usize;
    let mut external = 0usize;
    let mut excluded_examples = BTreeSet::new();

    if sources
        .iter()
        .any(|(_, lang)| *lang == LanguageKind::TypeScript)
    {
        let modules = module_index(repo)?;
        let mut packages = modules
            .ts
            .iter()
            .filter(|entry| entry.is_package)
            .collect::<Vec<_>>();
        // Longest prefix first: the same determinism rule as every other
        // prefix table here (finding 9), and it makes the scoped-package
        // case (`@scope/pkg` vs. a shorter unscoped collision) resolve to
        // the more specific entry first.
        packages.sort_by(|a, b| {
            b.prefix
                .len()
                .cmp(&a.prefix.len())
                .then_with(|| a.prefix.cmp(&b.prefix))
        });

        for (pkg, lang) in sources {
            if *lang != LanguageKind::TypeScript {
                continue;
            }
            let files = source_files(repo, pkg, *lang)?;
            let by_file = files
                .iter()
                .enumerate()
                .map(|(index, file)| (file.clone(), index as FileId))
                .collect::<BTreeMap<_, _>>();
            let mut parser = Parser::new();
            for file in &files {
                let Ok(bytes) = fs::read(repo.join(file)) else {
                    continue;
                };
                parser.set_language(&grammar_for_file(*lang, file))?;
                let Some(tree) = parser.parse(&bytes, None) else {
                    continue;
                };
                for spec in typescript_imports(tree.root_node(), &bytes) {
                    if spec.starts_with('.') {
                        continue;
                    }
                    let Some((entry, rest)) = packages.iter().find_map(|entry| {
                        strip_module_prefix(&spec, &entry.prefix).map(|rest| (*entry, rest))
                    }) else {
                        external += 1;
                        continue;
                    };
                    let Some(manifest) = modules.packages.get(&entry.target) else {
                        external += 1;
                        continue;
                    };
                    let subpath = (!rest.is_empty()).then_some(rest);
                    if resolve_package_entry(&entry.target, subpath, manifest, &by_file).is_some() {
                        resolved += 1;
                        continue;
                    }
                    let candidates = package_entry_candidates(&entry.target, subpath, manifest);
                    if let Some(on_disk) = candidates.iter().find(|c| repo.join(c).exists()) {
                        resolved_but_excluded += 1;
                        excluded_examples.insert(on_disk.clone());
                        if redirect_excluded_workspace_import(
                            repo,
                            &entry.target,
                            subpath,
                            manifest,
                            &by_file,
                        )
                        .is_some()
                        {
                            redirected_from_excluded += 1;
                        }
                    } else {
                        unresolved += 1;
                    }
                }
            }
        }
    }

    Ok(json!({
        "resolved": resolved,
        "resolved_but_excluded": resolved_but_excluded,
        "redirected_from_excluded": redirected_from_excluded,
        "unresolved": unresolved,
        "external": external,
        "resolved_but_excluded_examples": excluded_examples,
    }))
}

/// Every `object.attribute` pair in the file whose `object` is a bare
/// identifier -- e.g. `mod.thing()` yields `("mod", "thing")` -- with no
/// filtering against imports at all. This is the raw half of what used to be
/// `python_uses`'s tree walk: phase 1 (`parse_files`) can capture it while
/// the tree is alive, but which of these pairs is actually a use of an
/// imported module can only be decided in phase 2 (`python_uses_from_raw`),
/// once the *global* `known` module set exists to build aliases against.
fn python_attribute_candidates(root: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for node in walk(root) {
        if node.kind() != "attribute" {
            continue;
        }
        let Some(object) = node.child_by_field_name("object") else {
            continue;
        };
        if object.kind() != "identifier" {
            continue;
        }
        if let Some(attribute) = child_text(node, "attribute", source) {
            result.push((text(object, source).to_owned(), attribute));
        }
    }
    result
}

/// Phase 2 of Python's `uses` extraction: resolves `imports` (raw, from
/// phase 1) into direct `from X import name` uses, and filters
/// `attribute_candidates` (also raw, from phase 1) down to the pairs whose
/// object identifier is an alias of a known module -- exactly what
/// `python_uses` used to compute by re-walking the tree here, now done as
/// pure string matching against candidates captured up front. No tree or
/// source needed: `known`/`file_of` are the only things that had to wait for
/// every file to be parsed.
fn python_uses_from_raw(
    current_module: &str,
    current_file: &str,
    imports: &[PythonImport],
    attribute_candidates: &[(String, String)],
    known: &BTreeSet<String>,
    file_of: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let is_pkg = current_file.rsplit('/').next() == Some("__init__.py");
    let mut result = Vec::new();
    let mut aliases = BTreeMap::<String, String>::new();
    for import in imports {
        if import.from {
            let targets = resolve_python(import, current_module, known, is_pkg);
            for target in targets {
                let target_file = &file_of[&target];
                if target_file != current_file {
                    for (name, _) in &import.names {
                        if name != "*" {
                            result.push((target_file.clone(), name.clone()));
                        }
                    }
                }
            }
            let head = python_head(import, current_module, is_pkg);
            for (name, alias) in &import.names {
                let full = if head.is_empty() {
                    name.clone()
                } else {
                    format!("{head}.{name}")
                };
                if known.contains(&full) {
                    aliases.insert(
                        alias.clone().unwrap_or_else(|| name.clone()),
                        file_of[&full].clone(),
                    );
                }
            }
        } else {
            for (name, alias) in &import.names {
                if known.contains(name) {
                    aliases.insert(
                        alias
                            .clone()
                            .unwrap_or_else(|| name.rsplit('.').next().unwrap_or(name).to_owned()),
                        file_of[name].clone(),
                    );
                }
            }
        }
    }
    if !aliases.is_empty() {
        for (object, attribute) in attribute_candidates {
            let Some(target_file) = aliases.get(object) else {
                continue;
            };
            if target_file == current_file {
                continue;
            }
            result.push((target_file.clone(), attribute.clone()));
        }
    }
    result
}

fn parse_multi(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    parsed: BTreeMap<String, ParsedFile>,
    raw: BTreeMap<String, FileRaw>,
    modules: &ModuleIndex,
) -> Result<SourceIntermediate> {
    let progress = crate::progress::Progress::silent();
    let stage = progress.stage(crate::progress::StageId::Resolve, Some(parsed.len() as u64));
    let result = parse_multi_with_progress(repo, pkg, language, parsed, raw, modules, &stage);
    if result.is_ok() {
        stage.finish();
    }
    result
}

fn parse_multi_with_progress(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    parsed: BTreeMap<String, ParsedFile>,
    raw: BTreeMap<String, FileRaw>,
    modules: &ModuleIndex,
    progress: &crate::progress::StageCounter,
) -> Result<SourceIntermediate> {
    let (files, ids) = file_ids(parsed.keys().cloned())?;
    let mut by_directory = BTreeMap::<String, Vec<FileId>>::new();
    for file in &files {
        by_directory
            .entry(directory_name(file).to_owned())
            .or_default()
            .push(ids[file]);
    }
    let mut static_edges = BTreeMap::<(FileId, FileId), f64>::new();
    let mut directed = BTreeMap::<(FileId, FileId), f64>::new();
    let mut fanin = BTreeMap::<FileId, f64>::new();
    let mut uses = BTreeSet::<(FileId, FileId, String)>::new();
    for file in parsed.keys() {
        let source_id = ids[file];
        let FileRaw::Multi {
            imports,
            named_candidates,
        } = &raw[file]
        else {
            unreachable!("parse_multi only ever stores FileRaw::Multi");
        };
        for path in imports {
            let targets = resolve_multi(repo, language, path, file, modules, &by_directory, &ids);
            let targets = targets.as_slice();
            if targets.is_empty() {
                continue;
            }
            let share = 1.0 / targets.len() as f64;
            for &target in targets {
                if target == source_id {
                    continue;
                }
                *static_edges
                    .entry(ordered_file_pair(source_id, target))
                    .or_default() += share;
                *directed.entry((source_id, target)).or_default() += share;
                *fanin.entry(target).or_default() += share;
            }
        }
        for (path, name) in named_candidates {
            let targets = resolve_multi(repo, language, path, file, modules, &by_directory, &ids);
            let targets = targets.as_slice();
            for &target in targets {
                if target != source_id {
                    uses.insert((source_id, target, name.clone()));
                }
            }
        }
        progress.advance(1);
    }

    let module_for = parsed
        .keys()
        .map(|file| (file.clone(), file.clone()))
        .collect();

    Ok(SourceIntermediate {
        pkg: pkg.to_owned(),
        language,
        parsed,
        files,
        static_edges,
        directed,
        fanin,
        uses,
        module_for,
    })
}

/// Every non-relative way a repository names its own files.
///
/// Both halves of this used to be a single `String`: the `module` line of a
/// `go.mod` at the repository root, and nothing at all for TypeScript, whose
/// resolver returned early on any specifier not starting with `.`. That is
/// wrong in the same way for both languages: it assumes a repository has
/// exactly one module and that internal edges are always spelled relatively.
///
/// Everything here is read from files the repository already has to keep
/// correct for its own toolchain to work. Nothing is inferred or tunable.
#[derive(Debug, Default)]
struct ModuleIndex {
    /// `(module path, repo-relative directory)` for every `go.mod` found.
    go: Vec<(String, String)>,
    /// Prefix, target directory, and declaring scope for every tsconfig
    /// `paths` entry and every workspace `package.json` name. Package names
    /// have the empty (global) scope.
    ts: Vec<TsPrefix>,
    /// `package.json` fields relevant to entry-point resolution (`exports`,
    /// `types`/`typings`, `module`, `main`), keyed by the same repo-relative
    /// directory a `TsPrefix { is_package: true, .. }` entry names as its
    /// `target`. Only populated for a `TsPrefix` that came from a
    /// `package.json` name -- a tsconfig `paths` alias has no manifest to
    /// consult and resolves through `target` alone.
    packages: BTreeMap<String, PackageManifest>,
}

#[derive(Debug, Eq, PartialEq)]
struct TsPrefix {
    prefix: String,
    target: String,
    scope: String,
    /// `true` for a workspace `package.json` name, `false` for a tsconfig
    /// `paths` alias. Resolution branches on this: a package name is looked
    /// up through its manifest (`exports`, then `main`/`types`/`module`,
    /// then `src/index.ts`), while an alias is a plain directory
    /// substitution through the existing relative-import probe.
    is_package: bool,
}

/// The subset of `package.json` that decides a workspace package's entry
/// point. Read once per matched package directory in [`module_index`] and
/// consulted by [`resolve_package_entry`] for every bare-specifier import
/// that names this package.
#[derive(Debug, Default, Clone)]
struct PackageManifest {
    exports: Option<serde_json::Value>,
    types: Option<String>,
    module: Option<String>,
    main: Option<String>,
}

impl ModuleIndex {
    /// Longest prefix first, lexicographic on ties. Both orderings are fixed,
    /// so resolution cannot depend on directory-walk order (finding 9).
    fn sorted(mut entries: Vec<(String, String)>) -> Vec<(String, String)> {
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        entries.dedup();
        entries
    }

    fn sorted_ts(mut entries: Vec<TsPrefix>) -> Vec<TsPrefix> {
        entries.sort_by(|a, b| {
            a.prefix
                .cmp(&b.prefix)
                .then_with(|| a.target.cmp(&b.target))
                .then_with(|| a.scope.cmp(&b.scope))
        });
        entries.dedup();
        entries
    }
}

/// Walk `repo` once, collecting `go.mod`, `tsconfig*.json` and
/// `package.json`. The walk skips exactly the directories the Go/TypeScript
/// source walk skips, plus dot-directories, so vendored metadata cannot add
/// a prefix.
///
/// A nested `package.json` only contributes a workspace-package prefix when
/// its directory is a declared workspace member (`workspace_globs_from_root`)
/// or is the repository root itself. Earlier this repository treated *any*
/// nested `package.json` as fair game for bare-specifier resolution; a repo
/// with many unrelated nested manifests (a vendored example, a test fixture,
/// an editor extension) can name-collide with an external npm dependency of
/// the same name, and the lower-bound rule (a wrong local file is worse than
/// no edge) argues for scoping to what the package manager itself would
/// resolve locally.
fn module_index(repo: &Path) -> Result<ModuleIndex> {
    let workspace_globs = workspace_globs_from_root(repo);
    let mut go = Vec::new();
    let mut ts = Vec::new();
    let mut packages = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = entries.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                if !MULTI_SKIP_DIR.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(path);
                }
                continue;
            }
            let Ok(here) = relative_slash(repo, &directory) else {
                continue;
            };
            match name.as_str() {
                "go.mod" => {
                    if let Some(module) = go_module_line(&path) {
                        go.push((module, here));
                    }
                }
                name if name.starts_with("tsconfig") && name.ends_with(".json") => {
                    ts.extend(tsconfig_aliases(&path, &here));
                }
                "package.json" => {
                    if here == "." || is_workspace_member(&workspace_globs, &here) {
                        if let Some((name, manifest)) = package_manifest(&path) {
                            ts.push(TsPrefix {
                                prefix: name,
                                target: here.clone(),
                                scope: String::new(),
                                is_package: true,
                            });
                            packages.insert(here, manifest);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(ModuleIndex {
        go: ModuleIndex::sorted(go),
        ts: ModuleIndex::sorted_ts(ts),
        packages,
    })
}

/// Workspace-member globs from the repository root only: `pnpm-workspace.yaml`
/// `packages:`, root `package.json` `workspaces` (array or `{packages:[]}`),
/// and root `lerna.json` `packages`. Read directly rather than during the
/// tree walk -- these three files only matter at the root, so there is
/// nothing to gain from discovering them mid-walk, and reading them upfront
/// means every subsequent `package.json` can be gated against a complete
/// list.
fn workspace_globs_from_root(repo: &Path) -> Vec<String> {
    let mut patterns = Vec::new();
    if let Ok(text) = fs::read_to_string(repo.join("package.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            match value.get("workspaces") {
                Some(serde_json::Value::Array(items)) => {
                    patterns.extend(items.iter().filter_map(|v| v.as_str()).map(str::to_owned));
                }
                Some(serde_json::Value::Object(obj)) => {
                    if let Some(serde_json::Value::Array(items)) = obj.get("packages") {
                        patterns.extend(items.iter().filter_map(|v| v.as_str()).map(str::to_owned));
                    }
                }
                _ => {}
            }
        }
    }
    if let Ok(text) = fs::read_to_string(repo.join("pnpm-workspace.yaml")) {
        patterns.extend(pnpm_workspace_packages(&text));
    }
    if let Ok(text) = fs::read_to_string(repo.join("lerna.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(serde_json::Value::Array(items)) = value.get("packages") {
                patterns.extend(items.iter().filter_map(|v| v.as_str()).map(str::to_owned));
            }
        }
    }
    patterns
}

/// Narrow, hand-written reader for exactly `pnpm-workspace.yaml`'s top-level
/// `packages:` block-sequence -- not a YAML parser (no dependency change).
/// Tolerates blank lines and `#`-comment lines between list items (n8n's
/// `pnpm-workspace.yaml` has one), and stops at the first line that is
/// neither, which is always either the next top-level key or end of file.
fn pnpm_workspace_packages(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_list = false;
    for line in text.lines() {
        if !in_list {
            if line.trim_end() == "packages:" {
                in_list = true;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix('-') else {
            break;
        };
        let mut item = rest.trim();
        if let Some(hash) = item.find(" #") {
            item = item[..hash].trim();
        }
        let item = item.trim_matches(['\'', '"']);
        if !item.is_empty() {
            result.push(item.to_owned());
        }
    }
    result
}

/// Whether `dir` (repo-relative, no leading/trailing slash) is included by
/// `globs`, applying `!`-negation entries in order the way `.gitignore` and
/// pnpm's own package-filter both do: the last matching entry wins.
fn is_workspace_member(globs: &[String], dir: &str) -> bool {
    let mut included = false;
    for pattern in globs {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob_matches(negated, dir) {
                included = false;
            }
        } else if glob_matches(pattern, dir) {
            included = true;
        }
    }
    included
}

/// A restricted glob: `*` matches one path segment, `**` matches zero or
/// more segments, anything else must match literally. Covers every pattern
/// form seen in the corpus (`packages/*`, `packages/@n8n/*`,
/// `packages/frontend/**`, a literal path with no wildcard at all).
fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    let pattern_segs = pattern.split('/').collect::<Vec<_>>();
    let path_segs = path.split('/').collect::<Vec<_>>();
    glob_match_segments(&pattern_segs, &path_segs)
}

fn glob_match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(&"**") => {
            (0..=path.len()).any(|skip| glob_match_segments(&pattern[1..], &path[skip..]))
        }
        Some(seg) => {
            !path.is_empty()
                && segment_matches(seg, path[0])
                && glob_match_segments(&pattern[1..], &path[1..])
        }
    }
}

fn segment_matches(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return !text.is_empty();
    }
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    let Some(mut rest) = text.strip_prefix(parts[0]) else {
        return false;
    };
    for (index, part) in parts.iter().enumerate().skip(1) {
        if index == parts.len() - 1 {
            return rest.ends_with(part);
        }
        if part.is_empty() {
            continue;
        }
        let Some(pos) = rest.find(part) else {
            return false;
        };
        rest = &rest[pos + part.len()..];
    }
    true
}

fn go_module_line(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    contents
        .lines()
        .find_map(|line| line.strip_prefix("module "))
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .filter(|module| !module.is_empty())
}

fn package_manifest(path: &Path) -> Option<(String, PackageManifest)> {
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let name = value
        .get("name")?
        .as_str()
        .filter(|name| !name.is_empty())?
        .to_owned();
    let string_field = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let manifest = PackageManifest {
        exports: value.get("exports").cloned(),
        types: string_field("types").or_else(|| string_field("typings")),
        module: string_field("module"),
        main: string_field("main"),
    };
    Some((name, manifest))
}

/// Return `compilerOptions.paths` from one tsconfig as repo-relative roots.
///
/// `extends` is deliberately not followed. It can point into `node_modules`,
/// and a map whose aliases depend on whether dependencies happen to be
/// installed is not a map of the commit.
fn tsconfig_aliases(path: &Path, here: &str) -> Vec<TsPrefix> {
    let Some(text) = fs::read_to_string(path).ok() else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text)) else {
        return Vec::new();
    };
    let Some(options) = value.get("compilerOptions") else {
        return Vec::new();
    };
    let base = options
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let root = join_slash(here, base);
    let Some(paths) = options.get("paths").and_then(|v| v.as_object()) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    for (pattern, targets) in paths {
        // "@/*" registers "@/", not "@". Dropping the separator would
        // swallow unrelated packages such as @tanstack/* and @n8n/*.
        let prefix = pattern.strip_suffix('*').unwrap_or(pattern);
        if prefix.is_empty() {
            continue;
        }
        for target in targets.as_array().into_iter().flatten() {
            let Some(target) = target.as_str() else {
                continue;
            };
            let target = target.strip_suffix('*').unwrap_or(target);
            result.push(TsPrefix {
                prefix: prefix.to_owned(),
                target: join_slash(&root, target),
                scope: here.to_owned(),
                is_package: false,
            });
        }
    }
    result
}

/// Strip JSONC comments and trailing commas while preserving string content.
///
/// A regex cannot do this safely: `"@/*": ["./*"]` contains `/*` inside a
/// string, and dify's `"**/*.ts"` gives a dot-matches-newline regex the `*/`
/// it needs to swallow the entire `paths` object.
fn strip_jsonc(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut uncommented = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        if in_string {
            let ch = text[i..].chars().next().expect("valid UTF-8 boundary");
            uncommented.push(ch);
            i += ch.len_utf8();
            if ch == '\\' && i < bytes.len() {
                let escaped = text[i..].chars().next().expect("valid UTF-8 boundary");
                uncommented.push(escaped);
                i += escaped.len_utf8();
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if bytes[i] == b'"' {
            uncommented.push('"');
            in_string = true;
            i += 1;
        } else if bytes[i..].starts_with(b"//") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            let ch = text[i..].chars().next().expect("valid UTF-8 boundary");
            uncommented.push(ch);
            i += ch.len_utf8();
        }
    }

    let bytes = uncommented.as_bytes();
    let mut cleaned = String::with_capacity(uncommented.len());
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let ch = uncommented[i..]
            .chars()
            .next()
            .expect("valid UTF-8 boundary");
        if in_string {
            cleaned.push(ch);
            i += ch.len_utf8();
            if ch == '\\' && i < bytes.len() {
                let escaped = uncommented[i..]
                    .chars()
                    .next()
                    .expect("valid UTF-8 boundary");
                cleaned.push(escaped);
                i += escaped.len_utf8();
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            cleaned.push(ch);
            in_string = true;
            i += 1;
            continue;
        }
        if ch == ',' {
            let next = uncommented[i + 1..]
                .chars()
                .find(|next| !next.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                i += 1;
                continue;
            }
        }
        cleaned.push(ch);
        i += ch.len_utf8();
    }
    cleaned
}

fn join_slash(base: &str, relative: &str) -> String {
    let mut parts = base
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value.to_owned()),
        }
    }
    parts.join("/")
}

fn go_imports(root: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut result = Vec::new();
    for node in walk(root) {
        if node.kind() != "import_spec" {
            continue;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "interpreted_string_literal" {
                result.push(strip_quotes(text(child, source)).to_owned());
            }
        }
    }
    result
}

fn go_selectors(root: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let mut aliases = BTreeMap::new();
    for node in walk(root) {
        if node.kind() != "import_spec" {
            continue;
        }
        let mut path = None;
        let mut name = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "interpreted_string_literal" => {
                    path = Some(strip_quotes(text(child, source)).to_owned())
                }
                "package_identifier" | "identifier" => name = Some(text(child, source).to_owned()),
                _ => {}
            }
        }
        if let Some(path) = path {
            let alias = name.unwrap_or_else(|| {
                path.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path)
                    .to_owned()
            });
            aliases.insert(alias, path);
        }
    }
    let mut result = Vec::new();
    for node in walk(root) {
        if node.kind() != "selector_expression" {
            continue;
        }
        let Some(operand) = node.child_by_field_name("operand") else {
            continue;
        };
        let Some(field) = node.child_by_field_name("field") else {
            continue;
        };
        if operand.kind() == "identifier" {
            if let Some(path) = aliases.get(text(operand, source)) {
                result.push((path.clone(), text(field, source).to_owned()));
            }
        }
    }
    result
}

fn typescript_imports(root: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut result = Vec::new();
    for node in walk(root) {
        match node.kind() {
            "import_statement" | "export_statement" => {
                if let Some(value) = node.child_by_field_name("source") {
                    result.push(strip_quotes(text(value, source)).to_owned());
                }
            }
            "call_expression" => {
                let Some(function) = node.child_by_field_name("function") else {
                    continue;
                };
                if !matches!(text(function, source), "require" | "import") {
                    continue;
                }
                if let Some(arguments) = node.child_by_field_name("arguments") {
                    let mut cursor = arguments.walk();
                    for child in arguments.children(&mut cursor) {
                        if child.kind() == "string" {
                            result.push(strip_quotes(text(child, source)).to_owned());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn typescript_named(root: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for node in walk(root) {
        if node.kind() != "import_statement" {
            continue;
        }
        let Some(source_node) = node.child_by_field_name("source") else {
            continue;
        };
        let path = strip_quotes(text(source_node, source)).to_owned();
        for child in walk(node) {
            if child.kind() == "import_specifier" {
                if let Some(name) = name_of(child, source) {
                    result.push((path.clone(), name));
                }
            }
        }
    }
    result
}

enum ResolvedTargets<'a> {
    Empty,
    One(FileId),
    Many(&'a [FileId]),
}

impl ResolvedTargets<'_> {
    fn as_slice(&self) -> &[FileId] {
        match self {
            Self::Empty => &[],
            Self::One(target) => std::slice::from_ref(target),
            Self::Many(targets) => targets,
        }
    }
}

fn resolve_multi<'a>(
    repo: &Path,
    language: LanguageKind,
    import: &str,
    source_file: &str,
    modules: &ModuleIndex,
    by_directory: &'a BTreeMap<String, Vec<FileId>>,
    by_file: &BTreeMap<String, FileId>,
) -> ResolvedTargets<'a> {
    match language {
        LanguageKind::Go => {
            // Longest module path wins, so a nested module resolves before
            // its parent. Requiring the slash boundary also prevents a
            // module named `example/co` from capturing `example/core`.
            for (module, directory) in &modules.go {
                let Some(rest) = strip_module_prefix(import, module) else {
                    continue;
                };
                let package = join_slash(directory, rest);
                return by_directory
                    .get(&package)
                    .map_or(ResolvedTargets::Empty, |targets| {
                        ResolvedTargets::Many(targets)
                    });
            }
            ResolvedTargets::Empty
        }
        LanguageKind::TypeScript => {
            let base = if import.starts_with('.') {
                normalize_relative(directory_name(source_file), import)
            } else {
                // Keep looking when a longer alias matches syntactically but
                // has no file target. An unresolvable alias contributes no
                // guessed edge; every candidate must exist in the parsed set.
                let source_directory = directory_name(source_file);
                let mut entries = modules.ts.iter().collect::<Vec<_>>();
                // TypeScript aliases are scoped by their declaring tsconfig.
                // The total order is: governing ancestor before non-ancestor;
                // deeper (nearer) scope; longer prefix; prefix; target. Scope
                // is the final tie-break only for otherwise equivalent entries.
                // Rust's stack.pop() walk and Python's os.walk visit directories
                // in opposite orders, so spelling the comparator identically in
                // both implementations is what makes the oracle independent of
                // either walk. Empty package.json scope is a global ancestor at
                // depth zero. Non-ancestors remain last but stay available as a
                // fallback when no nearer entry resolves to a parsed file.
                entries.sort_by(|a, b| ts_prefix_order(a, b, source_directory));
                let target = entries.into_iter().find_map(|entry| {
                    let rest = strip_module_prefix(import, &entry.prefix)?;
                    if entry.is_package {
                        let manifest = modules.packages.get(&entry.target)?;
                        let subpath = (!rest.is_empty()).then_some(rest);
                        resolve_package_entry(&entry.target, subpath, manifest, by_file).or_else(
                            || {
                                redirect_excluded_workspace_import(
                                    repo,
                                    &entry.target,
                                    subpath,
                                    manifest,
                                    by_file,
                                )
                            },
                        )
                    } else {
                        ts_candidate(&join_slash(&entry.target, rest), by_file)
                    }
                });
                return target.map_or(ResolvedTargets::Empty, ResolvedTargets::One);
            };
            ts_candidate(&base, by_file).map_or(ResolvedTargets::Empty, ResolvedTargets::One)
        }
        LanguageKind::Python => ResolvedTargets::Empty,
    }
}

/// Resolve one workspace-package import (bare `@scope/pkg` or `pkg`, plus an
/// optional `/subpath`) against its manifest. Every candidate this tries is
/// repo-relative and only accepted if it names a file the parsed set already
/// has -- the lower-bound rule applies here exactly as it does to a tsconfig
/// alias or a relative import.
fn resolve_package_entry(
    pkg_dir: &str,
    subpath: Option<&str>,
    manifest: &PackageManifest,
    by_file: &BTreeMap<String, FileId>,
) -> Option<FileId> {
    package_entry_candidates(pkg_dir, subpath, manifest)
        .into_iter()
        .find_map(|candidate| by_file.get(&candidate).copied())
}

/// Issue #101's owner decision ("count as edge targets only"): a workspace
/// import that resolves to a real file on disk under an excluded directory
/// (e.g. `generated/`, `MULTI_SKIP_DIR`) -- a file [`resolve_package_entry`]
/// already rejected because it was never parsed -- becomes an edge to the
/// *package's* nearest non-generated file instead of staying unresolved.
/// Generated files still gain no node, footprint or district of their own;
/// only the edge target moves.
///
/// Returns `None` (leave the import unresolved, as before) unless the
/// failed candidate chain first names a real on-disk file. An import that
/// matches no file at all -- a typo, an `exports` subpath the manifest never
/// declares -- is a different, pre-existing kind of miss and must not gain
/// an edge it did not earn.
///
/// Redirect target, in order:
/// 1. The package's own entry, resolved by the same candidate chain as
///    `import "<pkg>"` (subpath `None`) -- used only if that entry itself
///    was parsed.
/// 2. Otherwise, [`shallowest_parsed_file_in_package`]: the lexicographically
///    first parsed `.ts`/`.tsx` file at the shallowest depth in the package
///    directory. Every path in `by_file` already excludes `MULTI_SKIP_DIR`
///    directories -- source collection never walks into them -- so no
///    exclusion check is needed here.
/// 3. If the package has no parsed file at all, `None`: the import stays
///    unresolved rather than pointing at a file outside the package.
fn redirect_excluded_workspace_import(
    repo: &Path,
    pkg_dir: &str,
    subpath: Option<&str>,
    manifest: &PackageManifest,
    by_file: &BTreeMap<String, FileId>,
) -> Option<FileId> {
    package_entry_candidates(pkg_dir, subpath, manifest)
        .iter()
        .any(|candidate| repo.join(candidate).exists())
        .then(|| {
            resolve_package_entry(pkg_dir, None, manifest, by_file)
                .or_else(|| shallowest_parsed_file_in_package(pkg_dir, by_file))
        })
        .flatten()
}

/// The lexicographically first parsed `.ts`/`.tsx` file at the shallowest
/// depth inside `pkg_dir`, used by [`redirect_excluded_workspace_import`]
/// when a package's declared entry point does not itself resolve to a
/// parsed file. Deterministic by construction: `by_file` is a `BTreeMap`
/// (sorted by path already) and the comparison below breaks every tie on
/// the full path, so no `Hash*` iteration or directory-walk order can move
/// the result (finding 9).
fn shallowest_parsed_file_in_package(
    pkg_dir: &str,
    by_file: &BTreeMap<String, FileId>,
) -> Option<FileId> {
    by_file
        .iter()
        .filter_map(|(path, id)| {
            let rest = package_relative_suffix(path, pkg_dir)?;
            let is_ts = rest.ends_with(".ts") || rest.ends_with(".tsx");
            is_ts.then_some((rest.matches('/').count(), path, *id))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, _, id)| id)
}

/// `path` relative to `pkg_dir`, or `None` if `path` is not inside it.
/// Mirrors [`join_slash`]'s treatment of `.` as the repository root, so a
/// root-level workspace package (an uncommon but legal shape) is handled
/// the same way every other prefix in this file treats it.
fn package_relative_suffix<'a>(path: &'a str, pkg_dir: &str) -> Option<&'a str> {
    if pkg_dir.is_empty() || pkg_dir == "." {
        return Some(path);
    }
    path.strip_prefix(pkg_dir)?.strip_prefix('/')
}

/// The ordered candidate paths [`resolve_package_entry`] tries, kept separate
/// and pure (no filesystem or parsed-set access) so a diagnostic can also
/// see what was attempted when nothing resolved.
///
/// Precedence, as one flat chain tried in order: `exports` (every condition
/// present, not only the first), then `types`/`typings`, then `module`, then
/// `main` (with a `main` pointing at built output, e.g. `dist/index.js`,
/// additionally trying the same stem under `src/`), then `src/index.ts(x)`
/// and `index.ts(x)`. A subpath appends the plain `<dir>/<subpath>`
/// extension/index probe used for relative imports, after any `exports`
/// subpath match.
///
/// **This is not Node's own resolution algorithm.** Real Node stops at
/// `exports` once a package declares one at all, and never falls back to
/// `main` or a bare index file. An earlier version of this function did the
/// same -- and vue's own workspace packages broke it: `packages/reactivity`'s
/// `exports` declares `types`/`node`/`module`/`import`/`require`, and *every
/// one* points at `dist/...` (unbuilt in a fresh clone, and `dist` is itself
/// excluded from source collection, `MULTI_SKIP_DIR`) or a root `index.js`
/// stub that isn't TypeScript at all. The real source is `src/index.ts`,
/// reached only through the legacy `main` field's dist-stem fallback below.
/// Node-faithful encapsulation measured as a real regression against the
/// committed vue fixture (E: 1186 -> 934, this PR's PR #113 review) --
/// every candidate in this chain is still accepted only if it names a parsed
/// file, so this remains "keep looking, invent nothing", the same rule
/// tsconfig alias resolution already applies across entries.
fn package_entry_candidates(
    pkg_dir: &str,
    subpath: Option<&str>,
    manifest: &PackageManifest,
) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(exports) = &manifest.exports {
        candidates.extend(exports_candidates(pkg_dir, subpath, exports));
    }
    match subpath {
        None => {
            for field in [manifest.types.as_deref(), manifest.module.as_deref()] {
                if let Some(value) = field {
                    candidates.push(join_slash(pkg_dir, value));
                }
            }
            if let Some(main) = &manifest.main {
                candidates.push(join_slash(pkg_dir, main));
                // A `main` (or, above, `types`/`module`) that points at
                // built output has no source counterpart at that path --
                // try the same stem under `src/` instead, and only that
                // exact file. Not a general rule that a package's source
                // always lives under `src/`, just the one substitution
                // finding 101 (and vue's own packages) measured as worth
                // making.
                if let Some(stem) = Path::new(main).file_stem().and_then(|s| s.to_str()) {
                    candidates.push(join_slash(pkg_dir, &format!("src/{stem}.ts")));
                    candidates.push(join_slash(pkg_dir, &format!("src/{stem}.tsx")));
                }
            }
            candidates.push(join_slash(pkg_dir, "src/index.ts"));
            candidates.push(join_slash(pkg_dir, "src/index.tsx"));
            candidates.push(join_slash(pkg_dir, "index.ts"));
            candidates.push(join_slash(pkg_dir, "index.tsx"));
        }
        Some(sub) => {
            let target = join_slash(pkg_dir, sub);
            candidates.push(format!("{target}.ts"));
            candidates.push(format!("{target}/index.ts"));
            candidates.push(format!("{target}.tsx"));
            candidates.push(format!("{target}/index.tsx"));
            candidates.push(target);
        }
    }
    candidates
}

/// `exports` resolution for the `"."` entry (`subpath` is `None`) or a
/// `"./subpath"` entry, handling the wildcard subpath pattern form
/// (`"./api/*"`) and the condition keys `types`, `import`, `default`,
/// `require`. Returns one candidate per condition *present*, in that fixed
/// order (not just the first) -- `types` is present on nearly every real
/// package and almost never resolves to a parsed file (`.d.ts` is excluded
/// from source collection), so stopping at the first present condition
/// leaves every other condition, and the whole legacy `main` chain after it,
/// unreachable. See [`package_entry_candidates`]'s doc comment.
fn exports_candidates(
    pkg_dir: &str,
    subpath: Option<&str>,
    exports: &serde_json::Value,
) -> Vec<String> {
    let key = subpath.map_or_else(|| ".".to_owned(), |s| format!("./{s}"));
    let mut templates = Vec::new();
    match exports {
        serde_json::Value::String(value) => {
            if key == "." {
                templates.push((value.as_str(), String::new()));
            }
        }
        serde_json::Value::Object(map) => {
            if map.keys().any(|k| k.starts_with('.')) {
                if let Some((value, capture)) = match_export_key(map, &key) {
                    for template in pick_conditions(value) {
                        templates.push((template, capture.clone()));
                    }
                }
            } else if key == "." {
                for template in pick_conditions(exports) {
                    templates.push((template, String::new()));
                }
            }
        }
        _ => {}
    }
    templates
        .into_iter()
        .map(|(template, capture)| join_slash(pkg_dir, &substitute_wildcard(template, &capture)))
        .collect()
}

/// Exact key first (covers both `"."` and a literal `"./subpath"`), else the
/// wildcard subpath pattern (`"./api/*"`) with the longest matched prefix --
/// Node's own tie-break when more than one pattern could apply. Returns the
/// matched value and the text `*` captured (empty for an exact match).
fn match_export_key<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<(&'a serde_json::Value, String)> {
    if let Some(value) = map.get(key) {
        return Some((value, String::new()));
    }
    let mut candidates = Vec::new();
    for (pattern, value) in map {
        let Some(star) = pattern.find('*') else {
            continue;
        };
        let (prefix, suffix) = (&pattern[..star], &pattern[star + 1..]);
        if key.starts_with(prefix)
            && key.ends_with(suffix)
            && key.len() >= prefix.len() + suffix.len()
        {
            candidates.push((prefix, suffix, value));
        }
    }
    candidates.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(b.0)));
    candidates
        .into_iter()
        .next()
        .map(|(prefix, suffix, value)| {
            (
                value,
                key[prefix.len()..key.len() - suffix.len()].to_owned(),
            )
        })
}

/// Every present condition's target, in the fixed order `types`, `import`,
/// `default`, `require` -- not just the first, so a condition that can never
/// resolve (typically `types`, since `.d.ts` is excluded from source
/// collection) does not block a later one that can. A plain string value is
/// itself the one target (no conditions to pick between).
fn pick_conditions(value: &serde_json::Value) -> Vec<&str> {
    match value {
        serde_json::Value::String(s) => vec![s.as_str()],
        serde_json::Value::Object(map) => ["types", "import", "default", "require"]
            .into_iter()
            .filter_map(|cond| map.get(cond))
            .flat_map(pick_conditions)
            .collect(),
        _ => Vec::new(),
    }
}

fn substitute_wildcard(template: &str, capture: &str) -> String {
    if template.contains('*') {
        template.replacen('*', capture, 1)
    } else {
        template.to_owned()
    }
}

fn ts_prefix_order(a: &TsPrefix, b: &TsPrefix, source_directory: &str) -> std::cmp::Ordering {
    let a_ancestor = path_is_ancestor(&a.scope, source_directory);
    let b_ancestor = path_is_ancestor(&b.scope, source_directory);
    b_ancestor
        .cmp(&a_ancestor)
        .then_with(|| path_depth(&b.scope).cmp(&path_depth(&a.scope)))
        .then_with(|| b.prefix.len().cmp(&a.prefix.len()))
        .then_with(|| a.prefix.cmp(&b.prefix))
        .then_with(|| a.target.cmp(&b.target))
        .then_with(|| a.scope.cmp(&b.scope))
}

fn path_is_ancestor(scope: &str, directory: &str) -> bool {
    scope.is_empty()
        || directory == scope
        || directory
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn path_depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

/// Return the path below `module`, requiring either equality or a `/`
/// boundary. Trailing `/` is retained in the prefix table (`@/*` becomes
/// `@/`) and stripped only for this comparison.
fn strip_module_prefix<'a>(import: &'a str, module: &str) -> Option<&'a str> {
    let module = module.trim_end_matches('/');
    if module.is_empty() {
        return None;
    }
    if import == module {
        return Some("");
    }
    import
        .strip_prefix(module)
        .and_then(|rest| rest.strip_prefix('/'))
}

/// Resolve a TypeScript base only when the target is already in the parsed
/// file set. TypeScript's module-resolution reference, under "File extension
/// substitution", documents that import paths ending in `.js` resolve to
/// `.ts` and then `.tsx` sources (including under node16/nodenext):
/// https://www.typescriptlang.org/docs/handbook/modules/reference.html#file-extension-substitution
/// An exact parsed JavaScript path still wins here, as required by the graph's
/// lower-bound rule. `.mjs` -> `.mts` and `.cjs` -> `.cts` are intentionally
/// absent because source collection does not admit `.mts` or `.cts` yet.
///
/// For extensionless bases, the first four probes are the relative resolver's
/// historical candidates; the final two cover TSX directory entries and
/// workspace packages whose source entry point is `src/index.ts`.
fn ts_candidate(base: &str, by_file: &BTreeMap<String, FileId>) -> Option<FileId> {
    // Test the longer suffixes before `.js`: both `.mjs` and `.cjs` also end
    // in those three characters, but their TypeScript counterparts are not
    // collected at this revision.
    if base.ends_with(".mjs") || base.ends_with(".cjs") {
        return by_file.get(base).copied();
    }
    if let Some(stem) = base.strip_suffix(".jsx") {
        return [base.to_owned(), format!("{stem}.tsx")]
            .into_iter()
            .find_map(|candidate| by_file.get(&candidate).copied());
    }
    if let Some(stem) = base.strip_suffix(".js") {
        return [base.to_owned(), format!("{stem}.ts"), format!("{stem}.tsx")]
            .into_iter()
            .find_map(|candidate| by_file.get(&candidate).copied());
    }

    [
        format!("{base}.ts"),
        format!("{base}/index.ts"),
        format!("{base}.tsx"),
        format!("{base}/index.tsx"),
        format!("{base}/src/index.ts"),
        base.to_owned(),
    ]
    .into_iter()
    .find_map(|candidate| by_file.get(&candidate).copied())
}

fn normalize_relative(directory: &str, import: &str) -> String {
    let mut parts = directory
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for part in import.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            value => parts.push(value.to_owned()),
        }
    }
    parts.join("/")
}

/// Computes co-change, semantic and proximity over the union of every
/// source's parsed files, then builds the candidate edge set from static
/// edges + co-change + semantic and blends them into the raw `SignalEdge`
/// list `GraphData.edges` carries (not yet mass-normalised -- that is
/// [`pipeline::blend`]).
///
/// **Why the merge has to happen here and not by combining two finished
/// `GraphData` values.** Co-change keys off the file set seen in git history
/// (any two files that changed together, whatever language), semantic is IDF
/// over an identifier vocabulary that is only meaningful pooled across every
/// file it was built from, and proximity is a path-prefix ratio that doesn't
/// care about language at all. Compute any of those on one language's files
/// and union afterwards, and every cross-language edge is thrown away before
/// it exists -- the two graphs never had a candidate pair spanning them in
/// the first place. `build_multi_source` therefore merges at the *parsed
/// file + resolved static edge* stage (see [`union_sources`]) and calls this
/// function exactly once, over the combined set.
fn finish_graph(
    repo: &Path,
    merged: MergedSources,
    progress: &crate::progress::Progress,
) -> Result<GraphData> {
    let MergedSources {
        parsed,
        files,
        static_edges,
        directed,
        fanin,
        uses,
        module_for,
        file_language,
        sources,
        dominant_pkg,
        dominant_lang,
    } = merged;

    let history_stage = progress.stage(crate::progress::StageId::History, None);
    let history = git_history(repo, &files, 4000, &history_stage)?;
    history_stage.set(history.commits as u64);
    history_stage.finish();
    let semantic = semantic_vectors(&parsed);
    let file_ids = files
        .iter()
        .enumerate()
        .map(|(index, file)| (file.as_str(), index as FileId))
        .collect::<BTreeMap<_, _>>();
    let cochange = history
        .cochange
        .iter()
        .map(|((a, b), &value)| ((file_ids[a.as_str()], file_ids[b.as_str()]), value))
        .collect::<BTreeMap<_, _>>();
    let mut candidates = static_edges.keys().copied().collect::<BTreeSet<_>>();
    candidates.extend(cochange.keys().copied());

    // Above 600 files the semantic sweep below is restricted to same-
    // directory pairs (an O(n^2) full sweep is too slow past that size).
    //
    // An earlier version of this comment drew a further conclusion from
    // that, and finding 14 falsified it. It said a directory "never spans a
    // language in this pipeline (a source's files all live under its own
    // `pkg` root)", and therefore that any polyglot repo above this
    // threshold loses cross-language semantic bridging entirely. The
    // premise only holds when the sources root at different places --
    // prometheus's Go at `.` against a UI under `web/ui`, or the synthetic
    // fixture's deliberate `.`/`src` split. Two sources can share a root:
    // codex and dify both select `py at .` and `ts at .`, so their
    // directories hold Python and TypeScript side by side and this sweep
    // proposes cross-language pairs freely -- measured at 0.3% and 0.2% of
    // semantic mass crossing languages, on 887 and 6,333 files respectively
    // (finding 14).
    //
    // So what survives: `static` never carries a cross-language edge (see
    // `union_sources`), and proximity goes to 0 across a `server/` +
    // `web/`-style split by construction. Whether *semantic* bridges is a
    // property of where the sources root, not of how many files there are.
    // See `docs/FINDINGS.md` findings 13 and 14.
    if files.len() <= 600 {
        for i in 0..files.len() {
            for j in i + 1..files.len() {
                add_semantic_candidate(
                    i as FileId,
                    j as FileId,
                    &files,
                    &semantic,
                    &mut candidates,
                );
            }
        }
    } else {
        let mut by_directory = BTreeMap::<String, Vec<FileId>>::new();
        for (file_id, file) in files.iter().enumerate() {
            by_directory
                .entry(directory_name(file).to_owned())
                .or_default()
                .push(file_id as FileId);
        }
        for directory_files in by_directory.values() {
            for i in 0..directory_files.len() {
                for j in i + 1..directory_files.len() {
                    add_semantic_candidate(
                        directory_files[i],
                        directory_files[j],
                        &files,
                        &semantic,
                        &mut candidates,
                    );
                }
            }
        }
    }

    // `static_max` used to be one max over every static edge in the graph.
    // In a merged graph that punishes a language systematically rather than
    // measuring anything real: a Go import spreads 1/|D| across the package
    // directory (`resolve_multi`'s `share = 1.0 / targets.len()`, well below
    // this function) while Python and TypeScript resolve to a single file at
    // 1.0, so a shared global max makes every Go static edge lighter for a
    // reason that is a language convention, not a signal. Computed per
    // language instead: each edge divides by its own language's largest
    // static edge. A static edge is always intra-language by construction
    // (resolution only looks a target up in its own source's known-file
    // set -- see `union_sources`), so `file_language[&a]` and
    // `file_language[&b]` always agree for a real static edge and this
    // lookup is unambiguous. For a single-source graph this reduces to
    // exactly the old single global max (one language, one bucket) -- see
    // `single_language_static_max_matches_the_old_global_max` in the tests
    // below.
    let languages = files
        .iter()
        .map(|file| file_language[file])
        .collect::<Vec<_>>();
    let static_max_by_language = static_max_by_language(&static_edges, &languages);

    let mut edges = Vec::new();
    for (a_id, b_id) in candidates {
        let a = &files[a_id as usize];
        let b = &files[b_id as usize];
        let static_max = static_max_by_language[&languages[a_id as usize]];
        let static_signal = static_edges.get(&(a_id, b_id)).copied().unwrap_or(0.0) / static_max;
        let cochange = cochange.get(&(a_id, b_id)).copied().unwrap_or(0.0).min(1.0);
        let proximity = proximity(a, b);
        let semantic_signal = cosine(a, b, &semantic);
        let weight =
            ALPHA * static_signal + BETA * cochange + GAMMA * proximity + DELTA * semantic_signal;
        if weight < 0.02 {
            continue;
        }
        edges.push(SignalEdge {
            a: a.clone(),
            b: b.clone(),
            weight: round_to(weight, 5),
            static_signal: round_to(static_signal, 4),
            cochange: round_to(cochange, 4),
            proximity: round_to(proximity, 4),
            semantic: round_to(semantic_signal, 4),
        });
    }
    // These compact lookup structures are no longer needed once every
    // candidate has been blended. Drop them before materialising the final
    // node/import/use vectors so their high-water marks do not overlap.
    drop(static_edges);
    drop(cochange);
    drop(semantic);
    drop(file_ids);

    let nodes = files
        .iter()
        .enumerate()
        .map(|(file_id, file)| {
            let value = &parsed[file];
            let language = languages[file_id];
            SourceNode {
                file: file.clone(),
                loc: value.loc,
                code_lines: Some(value.code_lines),
                complexity: value.complexity,
                churn: history.churn.get(file).copied().unwrap_or(0),
                fanin: if language == LanguageKind::Python {
                    fanin.get(&(file_id as FileId)).copied().unwrap_or(0.0)
                } else {
                    round_to(fanin.get(&(file_id as FileId)).copied().unwrap_or(0.0), 2)
                },
                module: module_for
                    .get(file)
                    .cloned()
                    .unwrap_or_else(|| file.clone()),
                lang: language.as_str().to_owned(),
            }
        })
        .collect();
    let symbols = parsed
        .iter()
        .filter(|(_, value)| !value.symbols.is_empty())
        .map(|(file, value)| (file.clone(), value.symbols.clone()))
        .collect();

    Ok(GraphData {
        repo: repo
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        pkg: dominant_pkg,
        lang: dominant_lang.as_str().to_owned(),
        sources,
        imports: directed
            .into_iter()
            .map(|((a, b), value)| {
                // Always intra-language (see the static_max comment above);
                // `file_language[&a]` alone decides the rounding.
                let value = if languages[a as usize] == LanguageKind::Python {
                    value
                } else {
                    round_to(value, 3)
                };
                (a, b, value)
            })
            .collect(),
        symbols,
        uses: uses.into_iter().collect(),
        commits_scanned: history.commits,
        nodes,
        edges,
        references: None,
    })
}

/// The largest static edge value per language, floored at 1.0 -- see the
/// comment at its call site in `finish_graph`. Pulled out as its own
/// function so it is directly testable against the pre-polyglot single
/// global max without needing a full parsed repository.
fn static_max_by_language(
    static_edges: &BTreeMap<(FileId, FileId), f64>,
    file_language: &[LanguageKind],
) -> BTreeMap<LanguageKind, f64> {
    let mut result = BTreeMap::<LanguageKind, f64>::new();
    for ((a, _), &value) in static_edges {
        let language = file_language[*a as usize];
        let entry = result.entry(language).or_insert(0.0_f64);
        if value > *entry {
            *entry = value;
        }
    }
    // Every language present in the merged graph needs a floor entry even
    // with zero static edges of its own (e.g. a language whose files have no
    // resolvable imports at all), so a per-edge lookup against this map
    // never misses.
    for &language in file_language {
        result.entry(language).or_insert(0.0_f64);
    }
    for value in result.values_mut() {
        *value = value.max(1.0);
    }
    result
}

fn add_semantic_candidate(
    a: FileId,
    b: FileId,
    files: &[String],
    semantic: &BTreeMap<String, BTreeMap<String, f64>>,
    candidates: &mut BTreeSet<(FileId, FileId)>,
) {
    let pair = ordered_file_pair(a, b);
    if !candidates.contains(&pair)
        && cosine(&files[a as usize], &files[b as usize], semantic) > 0.28
    {
        candidates.insert(pair);
    }
}

fn semantic_vectors(
    parsed: &BTreeMap<String, ParsedFile>,
) -> BTreeMap<String, BTreeMap<String, f64>> {
    let mut document_frequency = BTreeMap::<String, usize>::new();
    for value in parsed.values() {
        for word in value.identifiers.keys() {
            *document_frequency.entry(word.clone()).or_default() += 1;
        }
    }
    let count = parsed.len().max(1) as f64;
    parsed
        .iter()
        .map(|(file, value)| {
            let mut vector = BTreeMap::new();
            for (word, occurrences) in &value.identifiers {
                let frequency = document_frequency[word];
                if frequency < 2 || frequency as f64 > count * 0.5 {
                    continue;
                }
                let weight = (1.0 + (*occurrences as f64).ln()) * (count / frequency as f64).ln();
                vector.insert(word.clone(), weight);
            }
            let norm = vector
                .values()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
            let norm = if norm == 0.0 { 1.0 } else { norm };
            for value in vector.values_mut() {
                *value /= norm;
            }
            (file.clone(), vector)
        })
        .collect()
}

fn cosine(a: &str, b: &str, vectors: &BTreeMap<String, BTreeMap<String, f64>>) -> f64 {
    let mut left = &vectors[a];
    let mut right = &vectors[b];
    if left.len() > right.len() {
        std::mem::swap(&mut left, &mut right);
    }
    left.iter()
        .map(|(word, value)| value * right.get(word).copied().unwrap_or(0.0))
        .sum()
}

fn proximity(a: &str, b: &str) -> f64 {
    let left = directory_name(a).split('/').filter(|part| !part.is_empty());
    let right = directory_name(b).split('/').filter(|part| !part.is_empty());
    let left = left.collect::<Vec<_>>();
    let right = right.collect::<Vec<_>>();
    let shared = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    shared as f64 / left.len().max(right.len()).max(1) as f64
}

fn directory_name(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn strip_quotes(value: &str) -> &str {
    value.trim_matches(['\'', '"'])
}

#[derive(Default)]
struct GitHistory {
    cochange: BTreeMap<(String, String), f64>,
    churn: BTreeMap<String, usize>,
    commits: usize,
}

fn git_history(
    repo: &Path,
    files: &[String],
    max_commits: usize,
    progress: &crate::progress::StageCounter,
) -> Result<GitHistory> {
    let mut child = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "log",
            &format!("-n{max_commits}"),
            "--no-merges",
            "--pretty=format:@%H",
            "--name-only",
            // Rename detection needs blob CONTENT, which a `--filter=blob:none`
            // clone does not have -- `service::clone` makes exactly that kind of
            // clone, so every blob git wants here is fetched from the remote one
            // promisor round-trip at a time. Measured on two fresh blobless
            // clones of encode/httpx (23 source files): 74.59s with rename
            // detection, 0.02s without. The cost scales with history, not file
            // count, which is why it never showed up on the fixtures' file
            // counts and why a dify-sized index spent 712s of wall clock on 3s
            // of CPU (issue #32).
            //
            // What this gives up: a renamed file is reported as delete-old +
            // add-new instead of one path, so a rename commit contributes the
            // old path too. Old paths are not in the current file set and are
            // dropped downstream, so the visible effect is confined to commits
            // that renamed a file -- see the parity evidence in #32 for what
            // that does (or does not) change on the nine fixtures.
            "--no-renames",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("run git log for co-change")?;
    let mut stdout = child.stdout.take().expect("piped git log stdout");
    let mut output_bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    let mut at_line_start = true;
    loop {
        let count = stdout.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        let mut commits = 0;
        for &byte in &chunk[..count] {
            commits += usize::from(at_line_start && byte == b'@');
            at_line_start = byte == b'\n';
        }
        if commits > 0 {
            progress.advance(commits as u64);
        }
        output_bytes.extend_from_slice(&chunk[..count]);
    }
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "git log failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file_set = files.iter().cloned().collect::<BTreeSet<_>>();
    let mut commits = Vec::<BTreeSet<String>>::new();
    let mut current = None::<BTreeSet<String>>;
    for line in String::from_utf8_lossy(&output_bytes).lines() {
        if line.starts_with('@') {
            if let Some(previous) = current.take() {
                if !previous.is_empty() {
                    commits.push(previous);
                }
            }
            current = Some(BTreeSet::new());
        } else if !line.trim().is_empty() && file_set.contains(line) {
            if let Some(current) = current.as_mut() {
                current.insert(line.to_owned());
            }
        }
    }
    if let Some(previous) = current {
        if !previous.is_empty() {
            commits.push(previous);
        }
    }

    let mut solo = BTreeMap::<String, usize>::new();
    let mut pairs = BTreeMap::<(String, String), usize>::new();
    for commit in &commits {
        for file in commit {
            *solo.entry(file.clone()).or_default() += 1;
        }
        if !(2..=40).contains(&commit.len()) {
            continue;
        }
        let values = commit.iter().collect::<Vec<_>>();
        for i in 0..values.len() {
            for j in i + 1..values.len() {
                *pairs
                    .entry((values[i].clone(), values[j].clone()))
                    .or_default() += 1;
            }
        }
    }
    let mut cochange = BTreeMap::new();
    for ((a, b), count) in pairs {
        let denominator = solo[&a].min(solo[&b]);
        if denominator >= 3 && count >= 2 {
            cochange.insert((a, b), count as f64 / denominator as f64);
        }
    }
    Ok(GitHistory {
        cochange,
        churn: solo,
        commits: commits.len(),
    })
}

pub(crate) fn round_to(value: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (value * scale).round_ties_even() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_python_package_anchors_on_last_source_segment() {
        assert_eq!(
            module_name("lib/sqlalchemy/orm/session.py", "lib/sqlalchemy"),
            "sqlalchemy.orm.session"
        );
        assert_eq!(module_name("src/flask/__init__.py", "src/flask"), "flask");
    }

    #[test]
    fn relative_import_resolves_against_containing_package_not_cur_mod() {
        // Issue #12: `from . import b` in a non-package module `pkg.a.mod`
        // (file pkg/a/mod.py, not __init__.py) must resolve to `pkg.a.b`
        // -- the *containing* package -- not `pkg.a.mod.b`. Level 1 keeps
        // the whole containing-package name; only `level - 1` further
        // segments get stripped.
        let known = ["pkg.a".to_owned(), "pkg.a.b".to_owned()]
            .into_iter()
            .collect();
        let import = PythonImport {
            from: true,
            level: 1,
            module: String::new(),
            names: vec![("b".to_owned(), None)],
        };
        assert!(resolve_python(&import, "pkg.a.mod", &known, false).contains("pkg.a.b"));
    }

    #[test]
    fn relative_import_level_two_climbs_one_more_from_the_containing_package() {
        // `from .. import x` in pkg/a/mod.py: the containing package is
        // pkg.a, and one further level up is pkg. The old `+ 1` collapse
        // computed this from cur_mod's own segment count rather than the
        // containing package's, which happened to agree at this depth but
        // diverges as soon as is_pkg matters -- see the next test.
        let known = ["pkg".to_owned(), "pkg.x".to_owned()].into_iter().collect();
        let import = PythonImport {
            from: true,
            level: 2,
            module: String::new(),
            names: vec![("x".to_owned(), None)],
        };
        assert!(resolve_python(&import, "pkg.a.mod", &known, false).contains("pkg.x"));
    }

    #[test]
    fn relative_import_from_a_package_init_resolves_against_itself() {
        // pkg/a/__init__.py's module_name() is already "pkg.a" (__init__
        // stripped), and a level-1 import from it resolves against that
        // same name -- is_pkg=true skips the "minus last segment" step a
        // non-package module needs.
        let known = ["pkg.a".to_owned(), "pkg.a.b".to_owned()]
            .into_iter()
            .collect();
        let import = PythonImport {
            from: true,
            level: 1,
            module: String::new(),
            names: vec![("b".to_owned(), None)],
        };
        assert!(resolve_python(&import, "pkg.a", &known, true).contains("pkg.a.b"));
    }

    #[test]
    fn go_package_import_spreads_across_directory() {
        let files = [("x/a.go".to_owned(), 0), ("x/b.go".to_owned(), 1)]
            .into_iter()
            .collect();
        let directories = [("x".to_owned(), vec![0, 1])].into_iter().collect();
        assert_eq!(
            resolve_multi(
                Path::new("."),
                LanguageKind::Go,
                "example/x",
                "main.go",
                &ModuleIndex {
                    go: vec![("example".to_owned(), String::new())],
                    ts: Vec::new(),
                    packages: BTreeMap::new(),
                },
                &directories,
                &files
            )
            .as_slice(),
            &[0, 1],
            "target ids retain lexical file order"
        );
        assert_eq!(
            resolve_multi(
                Path::new("."),
                LanguageKind::Go,
                "example/x",
                "main.go",
                &ModuleIndex {
                    go: vec![("example".to_owned(), String::new())],
                    ts: Vec::new(),
                    packages: BTreeMap::new(),
                },
                &directories,
                &files
            )
            .as_slice()
            .len(),
            2
        );
    }

    // -- TypeScript collection and resolution ---------------------------

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn python_project_imports_stay_in_the_declaring_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        write(root, "api/pyproject.toml", "[project]\nname = 'api'\n");
        write(root, "api/app.py", "from core import helper\n");
        write(root, "api/core/__init__.py", "");
        write(root, "api/core/helper.py", "VALUE = 1\n");
        write(root, "other/pyproject.toml", "[project]\nname = 'other'\n");
        write(root, "other/app.py", "from core import helper\n");
        write(root, "other/core/__init__.py", "");
        write(root, "other/core/helper.py", "VALUE = 2\n");
        write(root, "outside.py", "from core import helper\n");
        let (parsed, raw) = parse_files(root, ".", LanguageKind::Python).unwrap();
        let intermediate = parse_python(root, ".", parsed, raw).unwrap();
        let edges = intermediate
            .directed
            .keys()
            .map(|&(a, b)| {
                (
                    intermediate.files[a as usize].as_str(),
                    intermediate.files[b as usize].as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert!(edges.contains(&("api/app.py", "api/core/helper.py")));
        assert!(edges.contains(&("other/app.py", "other/core/helper.py")));
        assert!(!edges.contains(&("api/app.py", "other/core/helper.py")));
        assert!(!edges.contains(&("outside.py", "api/core/helper.py")));
    }

    fn resolved_import_edge_count(root: &Path, pkg: &str, language: LanguageKind) -> usize {
        resolved_import_edges(root, pkg, language).len()
    }

    fn resolved_import_edges(
        root: &Path,
        pkg: &str,
        language: LanguageKind,
    ) -> BTreeSet<(String, String)> {
        let modules = module_index(root).unwrap();
        let (parsed, raw) = parse_files(root, pkg, language).unwrap();
        let intermediate = parse_multi(root, pkg, language, parsed, raw, &modules).unwrap();
        intermediate
            .directed
            .keys()
            .map(|&(a, b)| {
                (
                    intermediate.files[a as usize].clone(),
                    intermediate.files[b as usize].clone(),
                )
            })
            .collect()
    }

    fn resolve_typescript(import: &str, source_file: &str, files: &[&str]) -> Vec<String> {
        // Lexical FileIds, as `file_ids` assigns them (#59), mapped back to
        // paths so the assertions read as the resolution rule they test.
        let paths = files
            .iter()
            .map(|file| (*file).to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let ids = paths
            .iter()
            .enumerate()
            .map(|(index, path)| (path.clone(), index as FileId))
            .collect::<BTreeMap<_, _>>();
        let directories = BTreeMap::new();
        resolve_multi(
            Path::new("."),
            LanguageKind::TypeScript,
            import,
            source_file,
            &ModuleIndex::default(),
            &directories,
            &ids,
        )
        .as_slice()
        .iter()
        .map(|id| paths[*id as usize].clone())
        .collect()
    }

    // -- TypeScript JavaScript-extension substitution (issue #58) ------

    #[test]
    fn typescript_js_specifier_resolves_to_ts_source() {
        assert_eq!(
            resolve_typescript("./a.js", "main.ts", &["main.ts", "a.ts", "a.tsx"],),
            vec!["a.ts".to_owned()]
        );
    }

    #[test]
    fn typescript_jsx_specifier_resolves_to_tsx_source() {
        assert_eq!(
            resolve_typescript("./c.jsx", "main.ts", &["main.ts", "c.tsx"]),
            vec!["c.tsx".to_owned()]
        );
    }

    #[test]
    fn typescript_js_index_specifier_resolves_to_ts_index_source() {
        assert_eq!(
            resolve_typescript("./dir/index.js", "main.ts", &["main.ts", "dir/index.ts"],),
            vec!["dir/index.ts".to_owned()]
        );
    }

    #[test]
    fn exact_javascript_file_wins_over_typescript_source_substitution() {
        assert_eq!(
            resolve_typescript("../b.js", "src/main.ts", &["src/main.ts", "b.js", "b.ts"],),
            vec!["b.js".to_owned()]
        );
    }

    #[test]
    fn missing_javascript_specifier_adds_no_edge() {
        assert!(resolve_typescript("./missing.js", "main.ts", &["main.ts"]).is_empty());
    }

    #[test]
    fn extensionless_typescript_resolution_keeps_historical_precedence() {
        assert_eq!(
            resolve_typescript(
                "./a",
                "main.ts",
                &["main.ts", "a.ts", "a/index.ts", "a.tsx"],
            ),
            vec!["a.ts".to_owned()]
        );
    }

    #[test]
    fn typescript_paths_alias_adds_one_hand_counted_edge() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        write(dir.path(), "src/main.ts", "import '@/target';\n");
        write(dir.path(), "src/target.ts", "export const target = 1;\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            1
        );
    }

    #[test]
    fn typescript_js_specifier_resolves_through_paths_alias() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/*"]}}}"#,
        );
        write(dir.path(), "src/main.ts", "import '@/x.js';\n");
        write(dir.path(), "src/x.ts", "export const x = 1;\n");

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [("src/main.ts".to_owned(), "src/x.ts".to_owned(),)]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn workspace_package_name_adds_one_hand_counted_edge() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core"}"#,
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const core = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            1
        );
    }

    /// A nested `package.json` outside every discovered workspace glob does
    /// not become a bare-specifier target. Before this, any nested manifest
    /// anywhere in the tree was fair game -- a name collision with an
    /// external npm dependency (e.g. a fixtures directory's own
    /// `package.json` named the same as a real published package) would
    /// silently resolve to the wrong local file instead of staying
    /// unresolved.
    #[test]
    fn package_json_outside_declared_workspace_globs_is_not_a_resolution_target() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "tools/left-pad/package.json",
            r#"{"name":"left-pad"}"#,
        );
        write(
            dir.path(),
            "tools/left-pad/src/index.ts",
            "export const leftPad = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import 'left-pad';\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            0
        );
    }

    #[test]
    fn pnpm_workspace_yaml_discovers_a_package_with_no_root_package_json() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core"}"#,
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const core = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            1
        );
    }

    #[test]
    fn npm_workspaces_object_form_discovers_a_package() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":{"packages":["packages/*"]}}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core"}"#,
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const core = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            1
        );
    }

    #[test]
    fn exports_condition_precedence_prefers_import_over_default() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core","exports":{".":{"import":"./esm.ts","default":"./cjs.ts"}}}"#,
        );
        write(
            dir.path(),
            "packages/core/esm.ts",
            "export const esm = 1;\n",
        );
        write(
            dir.path(),
            "packages/core/cjs.ts",
            "export const cjs = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "packages/core/esm.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn exports_subpath_wildcard_resolves_to_a_parsed_file() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core","exports":{"./api/*":"./src/api/*.ts"}}"#,
        );
        write(
            dir.path(),
            "packages/core/src/api/foo.ts",
            "export const foo = 1;\n",
        );
        write(
            dir.path(),
            "apps/site/main.ts",
            "import '@scope/core/api/foo';\n",
        );

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "packages/core/src/api/foo.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    /// Issue #101's actual dify shape: `exports` maps a subpath to a
    /// generated file that source collection never parses (`generated` is in
    /// `MULTI_SKIP_DIR`), and the package has no other parsed file at all --
    /// not even an entry point -- for the redirect rule to fall back to.
    /// Correct `exports` resolution still adds zero edges here: the
    /// lower-bound rule cares whether some file was parsed, not whether the
    /// manifest technically names one.
    #[test]
    fn exports_subpath_pointing_at_an_excluded_directory_with_no_parsed_file_stays_unresolved() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/contracts/package.json",
            r#"{"name":"@scope/contracts","exports":{"./api/*":"./generated/api/*.ts"}}"#,
        );
        write(
            dir.path(),
            "packages/contracts/generated/api/foo.ts",
            "export const foo = 1;\n",
        );
        write(
            dir.path(),
            "apps/site/main.ts",
            "import '@scope/contracts/api/foo';\n",
        );

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            0
        );
    }

    /// Issue #101's owner decision ("count as edge targets only"), dify's
    /// exact shape: `@dify/contracts`'s `exports` maps `./api/*` to a
    /// `generated/` file source collection never parses, but the package
    /// also has ordinary parsed files (`console.ts`, `marketplace.ts`) and a
    /// real entry point (`main`). The import must redirect to the package's
    /// *entry* file, not to whichever other parsed file happens to sort
    /// first -- `console.ts` sorts before `index.ts` lexicographically, so a
    /// fallback-first implementation would wrongly land here instead.
    #[test]
    fn generated_import_redirects_to_the_package_entry_file() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/contracts/package.json",
            r#"{"name":"@dify/contracts","main":"./index.ts","exports":{"./api/*":"./generated/api/*.ts"}}"#,
        );
        write(
            dir.path(),
            "packages/contracts/generated/api/openapi/types.gen.ts",
            "export type Foo = {};\n",
        );
        write(
            dir.path(),
            "packages/contracts/index.ts",
            "export const entry = 1;\n",
        );
        write(
            dir.path(),
            "packages/contracts/console.ts",
            "export const console_ = 1;\n",
        );
        write(
            dir.path(),
            "packages/contracts/marketplace.ts",
            "export const marketplace = 1;\n",
        );
        write(
            dir.path(),
            "cli/main.ts",
            "import '@dify/contracts/api/openapi/types.gen';\n",
        );

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "cli/main.ts".to_owned(),
                "packages/contracts/index.ts".to_owned(),
            )]
            .into_iter()
            .collect(),
        );
    }

    /// Issue #101's redirect rule, second tier: the package's own entry point
    /// (`main`) also points at a generated, unparsed file, so the redirect
    /// cannot land on a declared entry at all. It falls back to the
    /// lexicographically first parsed `.ts`/`.tsx` file at the shallowest
    /// depth in the package directory -- shallow `a_file.ts` beats both a
    /// lexicographically earlier but deeper `nested/deep.ts` and a
    /// lexicographically later sibling `b_file.ts`.
    #[test]
    fn generated_import_falls_back_to_shallowest_parsed_file_when_entry_is_also_generated() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/contracts/package.json",
            r#"{"name":"@scope/contracts","main":"./generated/index.js","exports":{"./api/*":"./generated/api/*.ts"}}"#,
        );
        write(
            dir.path(),
            "packages/contracts/generated/index.js",
            "module.exports = {};\n",
        );
        write(
            dir.path(),
            "packages/contracts/generated/api/foo.ts",
            "export const foo = 1;\n",
        );
        write(
            dir.path(),
            "packages/contracts/nested/deep.ts",
            "export const deep = 1;\n",
        );
        write(
            dir.path(),
            "packages/contracts/b_file.ts",
            "export const b = 1;\n",
        );
        write(
            dir.path(),
            "packages/contracts/a_file.ts",
            "export const a = 1;\n",
        );
        write(
            dir.path(),
            "apps/site/main.ts",
            "import '@scope/contracts/api/foo';\n",
        );

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "packages/contracts/a_file.ts".to_owned(),
            )]
            .into_iter()
            .collect(),
        );
    }

    #[test]
    fn main_pointing_at_dist_falls_back_to_the_same_stem_under_src() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core","main":"./dist/index.js"}"#,
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const core = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "packages/core/src/index.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    /// Regression for the vue fixture: `packages/reactivity/package.json`
    /// declares `exports` whose `types`/`import`/`require` conditions all
    /// point at unbuilt `dist/` output (or a non-TypeScript `index.js`
    /// stub), while the real source is `src/index.ts`, reached only through
    /// `main`'s dist-stem fallback. An `exports` field must not block that
    /// fallback just by existing -- every one of its own conditions still
    /// has to fail to resolve first.
    #[test]
    fn exports_pointing_at_unbuilt_output_falls_back_to_main_and_then_src_index() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{
                "name":"@scope/core",
                "main":"index.js",
                "exports":{
                    ".": {
                        "types": "./dist/core.d.ts",
                        "node": {"default": "./dist/core.cjs.js"},
                        "import": "./dist/core.esm.js",
                        "require": "./index.js"
                    }
                }
            }"#,
        );
        write(
            dir.path(),
            "packages/core/index.js",
            "module.exports = require('./dist/core.cjs.js');\n",
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const core = 1;\n",
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "packages/core/src/index.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn main_pointing_at_dist_with_no_matching_src_file_is_left_unresolved() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core","main":"./dist/bundle.js"}"#,
        );
        write(dir.path(), "apps/site/main.ts", "import '@scope/core';\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            0
        );
    }

    #[test]
    fn external_package_with_no_workspace_or_alias_match_adds_no_edge() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core"}"#,
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const core = 1;\n",
        );
        write(
            dir.path(),
            "apps/site/main.ts",
            "import '@scope/core';\nimport 'left-pad';\n",
        );

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "packages/core/src/index.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn nested_go_module_spreads_one_import_across_two_files() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "services/core/go.mod",
            "module example.com/core\n\ngo 1.22\n",
        );
        write(
            dir.path(),
            "services/core/cmd/main.go",
            "package main\nimport _ \"example.com/core/lib\"\nfunc main() {}\n",
        );
        write(
            dir.path(),
            "services/core/lib/a.go",
            "package lib\nfunc A() {}\n",
        );
        write(
            dir.path(),
            "services/core/lib/b.go",
            "package lib\nfunc B() {}\n",
        );

        assert_eq!(
            resolved_import_edge_count(dir.path(), "services/core", LanguageKind::Go),
            2
        );
    }

    #[test]
    fn go_package_fanout_keeps_edge_counts_and_weights_with_file_ids() {
        let dir = tempfile::TempDir::new().unwrap();
        write(dir.path(), "go.mod", "module example.com/repo\n\ngo 1.22\n");
        write(
            dir.path(),
            "cmd/a.go",
            "package cmd\nimport _ \"example.com/repo/lib\"\n",
        );
        write(
            dir.path(),
            "cmd/b.go",
            "package cmd\nimport _ \"example.com/repo/lib\"\n",
        );
        write(dir.path(), "lib/a.go", "package lib\n");
        write(dir.path(), "lib/b.go", "package lib\n");

        let modules = module_index(dir.path()).unwrap();
        let (parsed, raw) = parse_files(dir.path(), ".", LanguageKind::Go).unwrap();
        let intermediate =
            parse_multi(dir.path(), ".", LanguageKind::Go, parsed, raw, &modules).unwrap();

        let expected = [((0, 2), 0.5), ((0, 3), 0.5), ((1, 2), 0.5), ((1, 3), 0.5)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            intermediate.files,
            ["cmd/a.go", "cmd/b.go", "lib/a.go", "lib/b.go"]
        );
        assert_eq!(intermediate.static_edges, expected);
        assert_eq!(intermediate.directed.len(), 4);
        assert!(intermediate.directed.values().all(|&weight| weight == 0.5));
        assert_eq!(intermediate.fanin.get(&2), Some(&1.0));
        assert_eq!(intermediate.fanin.get(&3), Some(&1.0));
    }

    #[test]
    fn jsonc_paths_alias_with_comments_and_trailing_commas_adds_one_edge() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "tsconfig.json",
            r#"{
              // A regex must not treat the slash-star inside "@/*" as a comment.
              "compilerOptions": {
                "baseUrl": ".",
                "paths": {
                  "@/*": ["./src/*",],
                },
              },
              /* Nor may it run through to the star-slash inside this string. */
              "include": ["**/*.ts",],
            }"#,
        );
        write(dir.path(), "src/main.ts", "import '@/target';\n");
        write(dir.path(), "src/target.ts", "export const target = 1;\n");

        let modules = module_index(dir.path()).unwrap();
        assert!(modules.ts.contains(&TsPrefix {
            prefix: "@/".to_owned(),
            target: "src".to_owned(),
            scope: String::new(),
            is_package: false,
        }));
        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            1
        );
    }

    #[test]
    fn unresolvable_alias_target_adds_zero_edges() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["missing/*"]}}}"#,
        );
        write(dir.path(), "src/main.ts", "import '@/target';\n");
        write(dir.path(), "src/target.ts", "export const target = 1;\n");

        assert_eq!(
            resolved_import_edge_count(dir.path(), ".", LanguageKind::TypeScript),
            0
        );
    }

    #[test]
    fn duplicate_aliases_resolve_inside_each_declaring_package() {
        let dir = tempfile::TempDir::new().unwrap();
        for package in ["pkgA", "pkgB"] {
            write(
                dir.path(),
                &format!("{package}/tsconfig.json"),
                r#"{"compilerOptions":{"paths":{"@/*":["src/*"]}}}"#,
            );
            write(
                dir.path(),
                &format!("{package}/src/main.ts"),
                "import '@/target';\n",
            );
            write(
                dir.path(),
                &format!("{package}/src/target.ts"),
                "export const target = 1;\n",
            );
        }

        let edges = resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript);
        assert_eq!(
            edges,
            [
                (
                    "pkgA/src/main.ts".to_owned(),
                    "pkgA/src/target.ts".to_owned(),
                ),
                (
                    "pkgB/src/main.ts".to_owned(),
                    "pkgB/src/target.ts".to_owned(),
                ),
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn nearer_alias_beats_a_global_workspace_package_name() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "package.json",
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        );
        write(
            dir.path(),
            "packages/core/package.json",
            r#"{"name":"@scope/core"}"#,
        );
        write(
            dir.path(),
            "packages/core/src/index.ts",
            "export const global = 1;\n",
        );
        write(
            dir.path(),
            "apps/site/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@scope/core":["src/local"]}}}"#,
        );
        write(
            dir.path(),
            "apps/site/src/main.ts",
            "import '@scope/core';\n",
        );
        write(
            dir.path(),
            "apps/site/src/local.ts",
            "export const local = 1;\n",
        );

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/src/main.ts".to_owned(),
                "apps/site/src/local.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn non_ancestor_alias_remains_a_live_fallback() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "config/tsconfig.json",
            r#"{"compilerOptions":{"paths":{"@/*":["../shared/*"]}}}"#,
        );
        write(dir.path(), "apps/site/main.ts", "import '@/target';\n");
        write(dir.path(), "shared/target.ts", "export const target = 1;\n");

        assert_eq!(
            resolved_import_edges(dir.path(), ".", LanguageKind::TypeScript),
            [(
                "apps/site/main.ts".to_owned(),
                "shared/target.ts".to_owned(),
            )]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn tsx_file_is_collected_alongside_ts() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "src/Foo.tsx",
            "export function Foo() { return null; }\n",
        );
        write(dir.path(), "src/bar.ts", "export const bar = 1;\n");
        let files = source_files(dir.path(), "src", LanguageKind::TypeScript).unwrap();
        assert_eq!(
            files,
            vec!["src/Foo.tsx".to_owned(), "src/bar.ts".to_owned()]
        );
    }

    #[test]
    fn tsx_declaration_test_and_spec_files_are_still_excluded() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "src/Foo.tsx",
            "export function Foo() { return null; }\n",
        );
        write(dir.path(), "src/Foo.test.tsx", "test('x', () => {});\n");
        write(dir.path(), "src/Foo.spec.tsx", "test('x', () => {});\n");
        // `.d.tsx` is not a real declaration suffix -- only `.d.ts` is -- so
        // this is not excluded by that rule; it lacks a JSX return so it
        // parses as an ordinary (if odd) TypeScript-flavored file.
        write(
            dir.path(),
            "src/legacy.d.ts",
            "export declare const x: number;\n",
        );
        let files = source_files(dir.path(), "src", LanguageKind::TypeScript).unwrap();
        assert_eq!(files, vec!["src/Foo.tsx".to_owned()]);
    }

    #[test]
    fn import_of_relative_path_resolves_to_tsx_target() {
        let files = [("Foo.tsx".to_owned(), 0), ("a.ts".to_owned(), 1)]
            .into_iter()
            .collect();
        let directories = BTreeMap::new();
        let targets = resolve_multi(
            Path::new("."),
            LanguageKind::TypeScript,
            "./Foo",
            "a.ts",
            &ModuleIndex::default(),
            &directories,
            &files,
        );
        assert_eq!(targets.as_slice(), &[0]);
    }

    #[test]
    fn tsx_file_parses_with_the_tsx_grammar() {
        // LANGUAGE_TYPESCRIPT rejects JSX syntax outright; if a `.tsx` file
        // were still parsed with it (rather than `grammar_for_file`'s
        // per-file choice), the tree would come back with parse errors and
        // this component's `Foo` symbol/identifier would never be seen.
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "src/Foo.tsx",
            "export function Foo() { return <div>hi</div>; }\n",
        );
        let (parsed, _raw) = parse_files(dir.path(), "src", LanguageKind::TypeScript).unwrap();
        let file = parsed.get("src/Foo.tsx").expect("Foo.tsx was parsed");
        assert!(
            file.identifiers.contains_key("Foo"),
            "identifiers: {:?}",
            file.identifiers
        );
    }

    // -- polyglot union extraction (spec step 1) -----------------------

    #[test]
    fn single_language_static_max_matches_the_old_global_max() {
        // Before per-language static_max, finish_graph computed exactly:
        // `static_edges.values().copied().fold(0.0, f64::max).max(1.0)`, one
        // number for the whole graph. With one language present, the new
        // per-language computation must reduce to that same number -- this
        // is the assertion spec item 4 calls for.
        let mut static_edges = BTreeMap::new();
        static_edges.insert((0, 1), 3.0);
        static_edges.insert((1, 2), 7.0);
        let file_language = vec![LanguageKind::Python; 3];

        let by_language = static_max_by_language(&static_edges, &file_language);
        let old_global_max = static_edges
            .values()
            .copied()
            .fold(0.0_f64, f64::max)
            .max(1.0);
        assert_eq!(by_language.len(), 1, "exactly one language present");
        assert_eq!(by_language[&LanguageKind::Python], old_global_max);
        assert_eq!(by_language[&LanguageKind::Python], 7.0);
    }

    #[test]
    fn cross_language_static_max_does_not_let_one_language_drag_the_other() {
        // finding 1's mechanism one level up: a global max would let
        // TypeScript's dense, single-file-resolving static edges (up to
        // 1.0 each) set the denominator for Go's directory-shared edges
        // (share = 1/|D|, often well under 1.0) for a reason that is a
        // language convention, not a signal -- see the comment in
        // finish_graph. Per language, each bucket floors at 1.0
        // independently.
        let mut static_edges = BTreeMap::new();
        static_edges.insert((0, 1), 0.5);
        static_edges.insert((2, 3), 20.0);
        let file_language = vec![
            LanguageKind::Go,
            LanguageKind::Go,
            LanguageKind::TypeScript,
            LanguageKind::TypeScript,
        ];

        let by_language = static_max_by_language(&static_edges, &file_language);
        assert_eq!(
            by_language[&LanguageKind::Go],
            1.0,
            "Go's own max (0.5) floors at 1.0 -- not dragged up by TypeScript's 20.0"
        );
        assert_eq!(by_language[&LanguageKind::TypeScript], 20.0);
    }

    #[test]
    fn union_sources_first_source_wins_a_file_path_collision_and_sums_the_rest() {
        // Two sources claiming the same path: sorted order is (go, .) then
        // (py, .) (LanguageKind::as_str: "go" < "py"), so the Go source
        // wins "shared.txt" and the Python source's file (and everything
        // that referenced it) is dropped, not silently merged into a
        // dangling edge.
        let mut go_parsed = BTreeMap::new();
        go_parsed.insert(
            "shared.txt".to_owned(),
            test_parsed_file(3, 1, &[("shared.txt".to_owned(), 3)]),
        );
        go_parsed.insert("only_go.txt".to_owned(), test_parsed_file(1, 0, &[]));
        let go_source = SourceIntermediate {
            pkg: ".".to_owned(),
            language: LanguageKind::Go,
            parsed: go_parsed,
            files: vec!["only_go.txt".to_owned(), "shared.txt".to_owned()],
            static_edges: [((0, 1), 1.0)].into_iter().collect(),
            directed: BTreeMap::new(),
            fanin: [(1, 1.0)].into_iter().collect(),
            uses: BTreeSet::new(),
            module_for: [
                ("shared.txt".to_owned(), "shared.txt".to_owned()),
                ("only_go.txt".to_owned(), "only_go.txt".to_owned()),
            ]
            .into_iter()
            .collect(),
        };

        let mut py_parsed = BTreeMap::new();
        py_parsed.insert("shared.txt".to_owned(), test_parsed_file(9, 9, &[]));
        let py_source = SourceIntermediate {
            pkg: ".".to_owned(),
            language: LanguageKind::Python,
            parsed: py_parsed,
            files: vec!["shared.txt".to_owned()],
            static_edges: BTreeMap::new(),
            directed: BTreeMap::new(),
            fanin: BTreeMap::new(),
            uses: BTreeSet::new(),
            module_for: [("shared.txt".to_owned(), "shared".to_owned())]
                .into_iter()
                .collect(),
        };

        let merged = union_sources(vec![go_source, py_source]).unwrap();
        assert_eq!(merged.parsed.len(), 2, "shared.txt kept once, from Go");
        assert_eq!(
            merged.parsed["shared.txt"].loc, 3,
            "Go's version of shared.txt wins, not Python's"
        );
        assert_eq!(merged.file_language["shared.txt"], LanguageKind::Go);
        assert_eq!(merged.module_for["shared.txt"], "shared.txt");
        assert_eq!(merged.static_edges[&(0, 1)], 1.0);
        assert_eq!(
            merged.sources,
            vec![
                (".".to_owned(), "go".to_owned()),
                (".".to_owned(), "py".to_owned())
            ]
        );
        assert_eq!(merged.dominant_lang, LanguageKind::Go);
    }

    fn test_parsed_file(
        loc: usize,
        complexity: usize,
        identifiers: &[(String, usize)],
    ) -> ParsedFile {
        ParsedFile {
            loc,
            code_lines: loc,
            complexity,
            identifiers: identifiers.iter().cloned().collect(),
            symbols: Vec::new(),
        }
    }

    fn code_lines_of(source: &str, language: LanguageKind) -> usize {
        let mut parser = Parser::new();
        parser.set_language(&language.tree_sitter()).unwrap();
        let tree = parser.parse(source.as_bytes(), None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "snippet does not parse: {source}"
        );
        count_code_lines(tree.root_node(), source.as_bytes(), language)
    }

    #[test]
    fn python_docstrings_and_multiline_code() {
        let source = "# preface\n\"\"\"module\ntext\"\"\"\nclass C:\n    # preface\n    \"class\" \"doc\"\n    def f(self):\n        # preface\n        \"\"\"function\n        doc\"\"\"\n        value = \"\"\"code\n        still code\"\"\"\n        return value\n";
        assert_eq!(code_lines_of(source, LanguageKind::Python), 5);
    }

    #[test]
    fn comments_blank_lines_and_trailing_comment() {
        assert_eq!(
            code_lines_of("# only\nx = 1 # still code\n\n", LanguageKind::Python),
            1
        );
        assert_eq!(code_lines_of(" \n\t\n", LanguageKind::Python), 0);
        assert_eq!(
            code_lines_of(
                "// only\nvar x = 1 // code\n/* block\ncomment */\n",
                LanguageKind::Go
            ),
            1
        );
        assert_eq!(
            code_lines_of(
                "// only\nconst x = 1; // code\n/* block\ncomment */\n",
                LanguageKind::TypeScript
            ),
            1
        );
    }

    fn identifiers_of(source: &str) -> BTreeMap<String, usize> {
        let mut parser = Parser::new();
        parser
            .set_language(&LanguageKind::Python.tree_sitter())
            .unwrap();
        let bytes = source.as_bytes();
        let tree = parser.parse(bytes, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "snippet does not parse: {source}"
        );
        python_metrics(tree.root_node(), bytes).1
    }

    fn complexity_of(source: &str) -> usize {
        let mut parser = Parser::new();
        parser
            .set_language(&LanguageKind::Python.tree_sitter())
            .unwrap();
        let bytes = source.as_bytes();
        let tree = parser.parse(bytes, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "snippet does not parse: {source}"
        );
        python_metrics(tree.root_node(), bytes).0
    }

    fn symbols_of(source: &str) -> Vec<SymbolRow> {
        let mut parser = Parser::new();
        parser
            .set_language(&LanguageKind::Python.tree_sitter())
            .unwrap();
        let bytes = source.as_bytes();
        let tree = parser.parse(bytes, None).unwrap();
        assert!(
            !tree.root_node().has_error(),
            "snippet does not parse: {source}"
        );
        python_symbols(tree.root_node(), bytes)
    }

    #[test]
    fn symbol_end_line_ignores_a_block_ending_trailing_comment_but_not_a_comment_inside_brackets() {
        // A comment that is the true last thing in a block (nothing follows
        // it before the dedent) is not part of Python's ast at all -- ast.
        // FunctionDef.end_lineno is the last real statement's end, here the
        // `return` on line 2. But a comment sitting INSIDE an open
        // parenthesis, with a real closing `)` still to come after it, does
        // NOT truncate the span: Python's end_lineno reaches the `)`, same
        // as the raw tree-sitter node does. Found on sqlalchemy's
        // `engine/_util_cy.py` (`_is_mapping_or_tuple`, reference end line 57,
        // an unfixed port's 54) and `connectors/aioodbc.py` (`setinputsizes`,
        // reference end line 30, an unfixed port's 33) -- the two cases this
        // test tells apart.
        let block_ends_on_comment = symbols_of(
            "def f():\n    return 1\n\n    # trailing, nothing else in the block\n    # a second comment line\n",
        );
        assert_eq!(
            block_ends_on_comment[0].0 .3, 2,
            "ends at the return, not the comments"
        );

        let comment_inside_parens = symbols_of(
            "def g():\n    return (\n        1\n        or 2\n        # comment before the closing paren\n    )\n",
        );
        assert_eq!(
            comment_inside_parens[0].0 .3, 6,
            "ends at the closing paren on line 6, which the comment does not reach"
        );
    }

    #[test]
    fn async_for_statement_does_not_count_toward_complexity() {
        // Python's complexity() tests `isinstance(x, ast.For)`, and `async for`
        // is a distinct ast.AsyncFor node -- not counted. tree-sitter-python
        // instead reuses the ordinary `for_statement` kind for both, marked
        // only by a leading `async` token, so a kind-only match overcounts.
        // Found on httpx/_models.py: reference complexity 269, unfixed port
        // 273 -- exactly its 4 statement-level `async for` loops (its two
        // `async for` USES INSIDE A COMPREHENSION are a different node kind,
        // `for_in_clause`, and were never at risk). This is also why that
        // file's "hazard" landmark detail read "cplx 273" instead of the
        // reference's "cplx 269" (commit 8fbd0c6).
        let plain = complexity_of("def f():\n    for x in y:\n        pass\n");
        let asynchronous = complexity_of("async def f():\n    async for x in y:\n        pass\n");
        assert_eq!(plain, 2, "one function + one for loop");
        assert_eq!(
            asynchronous, 1,
            "one async function (still counted) + zero for the async-for loop"
        );
        // The comprehension form uses a different node kind entirely and was
        // already correctly excluded; check it stays that way.
        let comprehension = complexity_of("async def f():\n    z = [a async for a in b]\n");
        assert_eq!(comprehension, 1, "only the async function itself");
    }

    // Each of these reproduces one of the divergences found diffing
    // identifiers() against extract.py's ast.walk on scrapy's fixture: a
    // parent-node-kind exclusion that was too broad (dropping a real
    // reference alongside the syntactic label it shares a parent with) or
    // too narrow (missing a binding form ast.walk never turns into a Name).
    // Regressed once already (see the extract() commit history) -- keep
    // these so a future "simplify the exclusion list" pass fails loudly.

    #[test]
    fn bare_decorator_name_is_a_reference() {
        // decorator_list is walked by ast.walk like any other expression;
        // `@property` is Name(id='property'), counted like any other Name.
        let counts =
            identifiers_of("class Foo:\n    @property\n    def bar(self):\n        pass\n");
        assert_eq!(counts.get("property"), Some(&1));
    }

    #[test]
    fn default_and_keyword_argument_values_are_references_but_their_labels_are_not() {
        // `name` fields (arg.arg / keyword.arg) are plain strings in Python's
        // ast, never Name nodes; `value` fields are real expressions that
        // ast.walk descends into.
        let counts = identifiers_of(
            "DEFAULT = object()\n\nclass Foo:\n    def bar(self, x=DEFAULT, y: SomeType = None):\n        call(kwarg=other)\n        return x\n",
        );
        assert_eq!(
            counts.get("DEFAULT"),
            Some(&2),
            "assignment target + default value"
        );
        assert_eq!(
            counts.get("SomeType"),
            Some(&1),
            "annotation is a reference"
        );
        assert_eq!(
            counts.get("other"),
            Some(&1),
            "keyword-argument value is a reference"
        );
        assert_eq!(
            counts.get("kwarg"),
            None,
            "keyword-argument name is a label, not a Name"
        );
    }

    #[test]
    fn except_alias_is_not_a_reference_but_with_alias_is() {
        // ExceptHandler.name is a plain string; withitem.optional_vars is a
        // real assignment-target expression ast.walk does visit.
        let counts = identifiers_of(
            "try:\n    pass\nexcept ValueError as exc:\n    print(exc)\n\nwith open(\"f\") as conn:\n    conn.read()\n",
        );
        assert_eq!(
            counts.get("exc"),
            Some(&1),
            "only the use inside print(), not the alias"
        );
        assert_eq!(
            counts.get("conn"),
            Some(&2),
            "with-target counts, plus the .read() use"
        );
    }

    #[test]
    fn global_and_nonlocal_names_are_not_references() {
        // Global.names / Nonlocal.names are plain strings, never Name nodes.
        let counts = identifiers_of("def f():\n    global tracked\n    tracked = 1\n");
        assert_eq!(
            counts.get("tracked"),
            Some(&1),
            "only the assignment, not the global statement"
        );
    }

    #[test]
    fn lambda_parameter_names_are_not_references() {
        let counts = identifiers_of("f = lambda conn: conn.request(x)\n");
        assert_eq!(
            counts.get("conn"),
            Some(&1),
            "only the body use, not the lambda parameter"
        );
    }

    #[test]
    fn starred_assignment_target_is_a_reference_but_splat_parameter_is_not() {
        // `*module, _ = x` is ast.Starred(value=Name('module', ctx=Store)) --
        // counted. `def f(*args, **kwargs)` binds via plain strings -- not.
        let counts = identifiers_of(
            "def f(*args, **kwargs):\n    pass\n\ndef g():\n    *module, rest = reactor_path.split(\".\")\n    return module\n",
        );
        assert_eq!(counts.get("args"), None);
        assert_eq!(counts.get("kwargs"), None);
        assert_eq!(
            counts.get("module"),
            Some(&2),
            "starred target + the later use"
        );
    }

    #[test]
    fn typed_splat_parameter_names_are_not_references() {
        // `*arguments: T, **named: T` still binds via plain strings even
        // though the identifier now sits one level down, under
        // `typed_parameter`, instead of directly under `parameters`.
        let counts =
            identifiers_of("def f(*arguments: TypingAny, **named: TypingAny) -> None:\n    pass\n");
        assert_eq!(counts.get("arguments"), None);
        assert_eq!(counts.get("named"), None);
        assert_eq!(
            counts.get("TypingAny"),
            Some(&2),
            "both annotations are references"
        );
    }
}
