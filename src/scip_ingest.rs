//! Reads one `index.scip` into tolmap's reference data (issue #110, P1a).
//!
//! This is the product port of P0's `eval/scip_ingest.py`, which stays the
//! oracle: on the same index and the same file set, the file pairs, their
//! distinct-symbol weights and the credited symbol references must match
//! it (finding 43 measures that). The derivation is the same:
//!
//! - **File -> file**: a non-definition occurrence in file A of a
//!   non-local symbol whose definition occurrence is in file B, A != B.
//!   Weighted by the number of distinct symbols (P0's primary weighting).
//!   A pair supported only by namespace/module symbols (a descriptor ending
//!   in `/` or `:`: a Go package clause, a Python module object) is flagged
//!   as not a `use`, for measurement.
//! - **Symbol references**: every such occurrence, as `(file, line)` of the
//!   reference and the `(file, line)` of the symbol's canonical definition
//!   site. `symbols.rs` credits both ends to the innermost enclosing symbol
//!   span once the map's symbol order is final.
//! - **Implementation relationships**: `SymbolInformation.relationships`
//!   rows with `is_implementation`, as canonical definition sites.
//!
//! **Only in-repository definitions count**: a symbol is in scope only if
//! one of its definition occurrences is in a file of `scope`, the
//! language's mapped files. A symbol's canonical site is the first of its
//! in-scope definition sites in (path, line) order.
//!
//! **Everything is sorted.** scip-go's output order is not byte-stable
//! (finding 41), so nothing here depends on document or occurrence order,
//! with one exception carried over from P0 on purpose: when two documents
//! share a path (overlapping TypeScript projects), the first one read wins,
//! because `indexers::typescript_projects` orders projects so that the first
//! is the file's nearest `tsconfig.json`.
//!
//! The top-level `Index` message is walked on the wire and each `Document`
//! decoded on its own, twice (definitions, then references), so an index of
//! n8n's size (583 MB) is never one object tree in memory.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufReader, ErrorKind, Read};
use std::path::Path;

use anyhow::{bail, Context, Result};
use protobuf::Message;
use scip::types::occurrence::Typed_range;
use scip::types::{Document, Metadata, Occurrence};

/// `scip.SymbolRole.Definition`.
const DEFINITION: i32 = 0x1;

/// The descriptor suffix of a symbol, read from its final character as P0
/// does. Only `Namespace` and `Meta` change a result here (the `uses`
/// flag); the rest are kept for parity with the oracle's categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Callable,
    Type,
    Term,
    Namespace,
    Meta,
    Parameter,
    TypeParameter,
    Other,
}

pub fn category(symbol: &str) -> Category {
    if symbol.ends_with(").") {
        return Category::Callable;
    }
    match symbol.as_bytes().last() {
        Some(b'#') => Category::Type,
        Some(b'.') => Category::Term,
        Some(b'/') => Category::Namespace,
        Some(b':') => Category::Meta,
        Some(b')') => Category::Parameter,
        Some(b']') => Category::TypeParameter,
        _ => Category::Other,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEdge {
    /// Distinct in-scope symbols referenced from the source file and defined
    /// in the target file: the static-signal weight.
    pub symbols: usize,
    /// Reference occurrences behind the pair.
    pub occurrences: usize,
    /// At least one referenced symbol is not a namespace/module symbol.
    pub uses: bool,
}

#[derive(Debug, Default)]
pub struct Ingested {
    /// `tool_info.name` and `.version` from the index metadata.
    pub tool: String,
    pub documents: usize,
    pub duplicate_documents: usize,
    /// In-scope files the index has a document for.
    pub indexed_files: usize,
    /// Directed `(from, to)` file pairs, keyed by `scope`'s ids.
    pub file_edges: BTreeMap<(u32, u32), FileEdge>,
    /// `[from file, from line, to file, to line] -> occurrences`, sorted,
    /// lines 1-based. `to` is the referenced symbol's canonical site.
    pub refs: Vec<([u32; 4], u32)>,
    /// `[source file, source line, target file, target line]` canonical
    /// sites of `is_implementation` relationships, sorted and unique.
    pub implementations: Vec<[u32; 4]>,
}

fn read_varint(reader: &mut impl Read) -> Result<Option<u64>> {
    let mut result = 0u64;
    let mut shift = 0;
    let mut byte = [0u8; 1];
    loop {
        match reader.read_exact(&mut byte) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::UnexpectedEof && shift == 0 => {
                return Ok(None);
            }
            Err(error) => return Err(error).context("truncated varint in SCIP index"),
        }
        if shift >= 64 {
            bail!("varint longer than 64 bits in SCIP index");
        }
        result |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(Some(result));
        }
        shift += 7;
    }
}

/// Calls `visit(field number, payload)` for each length-delimited top-level
/// field of an `Index` (1 metadata, 2 a `Document`, 3 an external
/// `SymbolInformation`), skipping any other wire type, streaming from disk.
fn for_each_field(path: &Path, mut visit: impl FnMut(u32, &[u8]) -> Result<()>) -> Result<()> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut buffer = Vec::new();
    while let Some(key) = read_varint(&mut reader)? {
        let field = (key >> 3) as u32;
        match key & 7 {
            2 => {
                let length = read_varint(&mut reader)?.context("truncated length")?;
                let length = usize::try_from(length).context("field too large")?;
                buffer.resize(length, 0);
                reader
                    .read_exact(&mut buffer)
                    .context("truncated field in SCIP index")?;
                visit(field, &buffer)?;
            }
            0 => {
                read_varint(&mut reader)?.context("truncated varint field")?;
            }
            1 => reader.read_exact(&mut [0u8; 8])?,
            5 => reader.read_exact(&mut [0u8; 4])?,
            wire => bail!("{}: unsupported wire type {wire}", path.display()),
        }
    }
    Ok(())
}

/// 0-based start line, from the packed `range` or the typed one.
fn start_line(occurrence: &Occurrence) -> Option<u32> {
    let line = if let Some(&line) = occurrence.range.first() {
        line
    } else {
        match &occurrence.typed_range {
            Some(Typed_range::SingleLineRange(range)) => range.line,
            Some(Typed_range::MultiLineRange(range)) => range.start_line,
            _ => return None,
        }
    };
    u32::try_from(line).ok()
}

fn normalise(relative: &str) -> &str {
    relative.strip_prefix("./").unwrap_or(relative)
}

fn nonlocal(symbol: &str) -> bool {
    !symbol.is_empty() && !symbol.starts_with("local ")
}

/// Ingests `index`, restricted to `scope` (repository-relative path -> id).
/// Paths in the index must be relative to the repository root, which is
/// where `indexers::run` starts every indexer.
pub fn ingest(index: &Path, scope: &BTreeMap<String, u32>) -> Result<Ingested> {
    let mut result = Ingested::default();
    let mut symbol_ids = HashMap::<String, u32>::new();
    let mut intern = |symbol: &str| -> u32 {
        if let Some(&id) = symbol_ids.get(symbol) {
            return id;
        }
        let id = symbol_ids.len() as u32;
        symbol_ids.insert(symbol.to_owned(), id);
        id
    };
    let mut def_sites = BTreeMap::<u32, BTreeSet<(u32, u32)>>::new();
    let mut relationships = BTreeSet::<(u32, u32)>::new();
    let mut seen = BTreeSet::<String>::new();

    // Pass 1: metadata, in-scope definitions, implementation relationships.
    for_each_field(index, |field, bytes| {
        if field == 1 {
            let metadata = Metadata::parse_from_bytes(bytes).context("decode SCIP metadata")?;
            result.tool = format!("{} {}", metadata.tool_info.name, metadata.tool_info.version)
                .trim()
                .to_owned();
            return Ok(());
        }
        if field != 2 {
            return Ok(());
        }
        let document = Document::parse_from_bytes(bytes).context("decode SCIP document")?;
        result.documents += 1;
        let path = normalise(&document.relative_path);
        if !seen.insert(path.to_owned()) {
            result.duplicate_documents += 1;
            return Ok(());
        }
        if let Some(&file) = scope.get(path) {
            result.indexed_files += 1;
            for occurrence in &document.occurrences {
                if occurrence.symbol_roles & DEFINITION == 0 || !nonlocal(&occurrence.symbol) {
                    continue;
                }
                let Some(line) = start_line(occurrence) else {
                    continue;
                };
                def_sites
                    .entry(intern(occurrence.symbol.as_str()))
                    .or_default()
                    .insert((file, line + 1));
            }
        }
        // Relationships are read from every document, mapped or not: a
        // relationship stated in an unmapped file between two in-scope
        // symbols still holds (P0 reads them the same way).
        for information in &document.symbols {
            if !nonlocal(&information.symbol) {
                continue;
            }
            for relationship in &information.relationships {
                if relationship.is_implementation && nonlocal(&relationship.symbol) {
                    let source = intern(information.symbol.as_str());
                    let target = intern(relationship.symbol.as_str());
                    relationships.insert((source, target));
                }
            }
        }
        Ok(())
    })?;

    let canonical = |symbol: u32| def_sites.get(&symbol).and_then(|s| s.first().copied());

    let mut pair_symbols = BTreeMap::<(u32, u32), BTreeSet<u32>>::new();
    let mut pair_occurrences = BTreeMap::<(u32, u32), usize>::new();
    let mut pair_uses = BTreeSet::<(u32, u32)>::new();
    let mut refs = Vec::<[u32; 4]>::new();
    seen.clear();

    // Pass 2: references from in-scope documents.
    for_each_field(index, |field, bytes| {
        if field != 2 {
            return Ok(());
        }
        let document = Document::parse_from_bytes(bytes).context("decode SCIP document")?;
        let path = normalise(&document.relative_path);
        if !seen.insert(path.to_owned()) {
            return Ok(());
        }
        let Some(&from) = scope.get(path) else {
            return Ok(());
        };
        for occurrence in &document.occurrences {
            if occurrence.symbol_roles & DEFINITION != 0 || !nonlocal(&occurrence.symbol) {
                continue;
            }
            let Some(line) = start_line(occurrence) else {
                continue;
            };
            let Some(&symbol) = symbol_ids.get(occurrence.symbol.as_str()) else {
                continue;
            };
            let Some(sites) = def_sites.get(&symbol) else {
                continue;
            };
            let kind = category(&occurrence.symbol);
            let targets = sites.iter().map(|&(file, _)| file).collect::<BTreeSet<_>>();
            for target in targets {
                if target == from {
                    continue;
                }
                pair_symbols
                    .entry((from, target))
                    .or_default()
                    .insert(symbol);
                *pair_occurrences.entry((from, target)).or_default() += 1;
                if !matches!(kind, Category::Namespace | Category::Meta) {
                    pair_uses.insert((from, target));
                }
            }
            let (to_file, to_line) = *sites.first().expect("a site set is never empty");
            refs.push([from, line + 1, to_file, to_line]);
        }
        Ok(())
    })?;

    result.file_edges = pair_symbols
        .into_iter()
        .map(|(pair, symbols)| {
            (
                pair,
                FileEdge {
                    symbols: symbols.len(),
                    occurrences: pair_occurrences[&pair],
                    uses: pair_uses.contains(&pair),
                },
            )
        })
        .collect();
    refs.sort_unstable();
    for key in refs {
        match result.refs.last_mut() {
            Some((last, count)) if *last == key => *count += 1,
            _ => result.refs.push((key, 1)),
        }
    }
    result.implementations = relationships
        .into_iter()
        .filter_map(|(source, target)| {
            let (source_file, source_line) = canonical(source)?;
            let (target_file, target_line) = canonical(target)?;
            Some([source_file, source_line, target_file, target_line])
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(result)
}

/// Finding 41's per-language admission threshold, as a share of the
/// hand-written graph's intra-language pairs that the SCIP graph also has.
///
/// Chosen from finding 41's no-install table. Every configuration whose
/// project loaded kept 0.894 or more at file granularity (dify's Python at
/// the root, the lowest; django 0.990, vue 0.998, prometheus's UI 0.992),
/// and prometheus's Go keeps 0.997 at directory granularity (see
/// [`recall_granularity`]). The one that lost its graph -- n8n's
/// TypeScript, whose `@n8n/*` workspace imports resolve only through
/// `node_modules` -- kept 0.514. 0.80 sits well inside that gap: n8n falls
/// back to the hand-written resolver with a 0.29 margin, dify's root-level
/// Python passes with a 0.09 margin. A graph that loses a fifth of the
/// pairs the lower-bound resolver already proved is a regression in what
/// the map can show, whatever it adds.
pub const MIN_RECALL: f64 = 0.80;

/// The unit a language's recall is measured in. Go's hand-written resolver
/// spreads every import over each file of the imported package
/// (`extract::resolve_multi`), so a file-level comparison counts that
/// fan-out as misses: prometheus's Go keeps 0.869 of hand pairs by file and
/// 0.997 by target directory (finding 41). The directory is the unit the
/// hand-written Go graph actually asserts.
pub fn recall_granularity(language: crate::extract::LanguageKind) -> &'static str {
    match language {
        crate::extract::LanguageKind::Go => "directory",
        _ => "file",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GateOutcome {
    pub hand_pairs: usize,
    pub recall: Option<f64>,
    pub passed: bool,
}

/// Admits the SCIP graph for one language when it keeps at least
/// [`MIN_RECALL`] of the hand-written graph's pairs, compared at
/// [`recall_granularity`]. An empty hand-written graph has nothing to lose,
/// so any index with at least one in-scope document passes.
pub fn gate(
    language: crate::extract::LanguageKind,
    hand: &BTreeSet<(u32, u32)>,
    scip: &BTreeSet<(u32, u32)>,
    files: &[String],
) -> GateOutcome {
    let directory = |id: u32| {
        let file = files[id as usize].as_str();
        file.rsplit_once('/').map_or("", |(dir, _)| dir)
    };
    let (kept, total) = if recall_granularity(language) == "directory" {
        let hand = hand
            .iter()
            .map(|&(a, b)| (a, directory(b)))
            .collect::<BTreeSet<_>>();
        let scip = scip
            .iter()
            .map(|&(a, b)| (a, directory(b)))
            .collect::<BTreeSet<_>>();
        (hand.intersection(&scip).count(), hand.len())
    } else {
        (hand.intersection(scip).count(), hand.len())
    };
    if total == 0 {
        return GateOutcome {
            hand_pairs: 0,
            recall: None,
            passed: true,
        };
    }
    let recall = kept as f64 / total as f64;
    GateOutcome {
        hand_pairs: total,
        recall: Some(recall),
        passed: recall >= MIN_RECALL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::LanguageKind;
    use scip::types::{Index, Relationship, SingleLineRange, SymbolInformation, ToolInfo};

    fn occurrence(line: i32, symbol: &str, roles: i32) -> Occurrence {
        let mut occurrence = Occurrence::new();
        occurrence.range = vec![line, 0, 1];
        occurrence.symbol = symbol.to_owned();
        occurrence.symbol_roles = roles;
        occurrence
    }

    /// P0's `self_test` (eval/scip_ingest.py), in Rust, with the same
    /// answer: a two-file index plus an unindexed mapped file.
    fn self_test_index() -> Vec<u8> {
        let mut index = Index::new();
        let mut metadata = Metadata::new();
        let mut tool = ToolInfo::new();
        tool.name = "synthetic".to_owned();
        tool.version = "1".to_owned();
        metadata.tool_info = protobuf::MessageField::some(tool);
        index.metadata = protobuf::MessageField::some(metadata);

        let mut a = Document::new();
        a.relative_path = "pkg/a.py".to_owned();
        a.occurrences
            .push(occurrence(0, "p `pkg.a`/__init__:", DEFINITION));
        a.occurrences
            .push(occurrence(1, "p `pkg.a`/f().", DEFINITION));
        a.occurrences
            .push(occurrence(4, "p `pkg.a`/C#", DEFINITION));
        let mut information = SymbolInformation::new();
        information.symbol = "p `pkg.a`/C#".to_owned();
        let mut relationship = Relationship::new();
        relationship.symbol = "p `pkg.b`/Base#".to_owned();
        relationship.is_implementation = true;
        information.relationships.push(relationship);
        a.symbols.push(information);
        index.documents.push(a);

        let mut b = Document::new();
        b.relative_path = "./pkg/b.py".to_owned();
        b.occurrences
            .push(occurrence(0, "p `pkg.b`/__init__:", DEFINITION));
        b.occurrences
            .push(occurrence(0, "p `pkg.a`/__init__:", 0x2));
        b.occurrences
            .push(occurrence(2, "p `pkg.b`/Base#", DEFINITION));
        let mut typed = Occurrence::new();
        typed.symbol = "p `pkg.b`/g().".to_owned();
        typed.symbol_roles = DEFINITION;
        typed.typed_range = Some(Typed_range::SingleLineRange(SingleLineRange {
            line: 5,
            start_character: 4,
            end_character: 9,
            ..Default::default()
        }));
        b.occurrences.push(typed);
        b.occurrences.push(occurrence(6, "p `pkg.a`/f().", 0)); // call inside g
        b.occurrences.push(occurrence(6, "p `pkg.a`/f().", 0)); // second call
        b.occurrences.push(occurrence(7, "npm ext 1.0 lib/x().", 0)); // external
        b.occurrences.push(occurrence(7, "local 3", 0));
        b.occurrences.push(occurrence(9, "p `pkg.a`/C#", 0)); // module level
        index.documents.push(b);
        index.write_to_bytes().unwrap()
    }

    fn scope() -> BTreeMap<String, u32> {
        ["pkg/a.py", "pkg/b.py", "pkg/c.py"]
            .into_iter()
            .enumerate()
            .map(|(id, path)| (path.to_owned(), id as u32))
            .collect()
    }

    #[test]
    fn matches_the_p0_oracle_self_test() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("index.scip");
        std::fs::write(&path, self_test_index()).unwrap();
        let result = ingest(&path, &scope()).unwrap();
        assert_eq!(result.tool, "synthetic 1");
        assert_eq!(result.documents, 2);
        assert_eq!(result.indexed_files, 2);
        // P0: [["pkg/b.py", "pkg/a.py", 3, 4, 1]]
        assert_eq!(
            result.file_edges,
            BTreeMap::from([(
                (1, 0),
                FileEdge {
                    symbols: 3,
                    occurrences: 4,
                    uses: true
                }
            )])
        );
        // The import of the module object (line 1 -> its definition on
        // line 1), both calls of f (line 7 -> line 2) and the module-level
        // use of C (line 10 -> line 5). Crediting to spans is symbols.rs's
        // job; P0's crediting test lives there.
        assert_eq!(
            result.refs,
            vec![([1, 1, 0, 1], 1), ([1, 7, 0, 2], 2), ([1, 10, 0, 5], 1)]
        );
        assert_eq!(result.implementations, vec![[0, 5, 1, 3]]);
    }

    #[test]
    fn document_and_occurrence_order_do_not_change_the_result() {
        let bytes = self_test_index();
        let mut index = Index::parse_from_bytes(&bytes).unwrap();
        index.documents.reverse();
        for document in &mut index.documents {
            document.occurrences.reverse();
        }
        let directory = tempfile::tempdir().unwrap();
        let forward = directory.path().join("forward.scip");
        let backward = directory.path().join("backward.scip");
        std::fs::write(&forward, bytes).unwrap();
        std::fs::write(&backward, index.write_to_bytes().unwrap()).unwrap();
        let forward = ingest(&forward, &scope()).unwrap();
        let backward = ingest(&backward, &scope()).unwrap();
        assert_eq!(forward.file_edges, backward.file_edges);
        assert_eq!(forward.refs, backward.refs);
        assert_eq!(forward.implementations, backward.implementations);
    }

    #[test]
    fn the_gate_admits_vue_and_rejects_n8n() {
        let files = ["a/x.ts", "b/y.ts", "b/z.ts", "c/w.ts"]
            .map(str::to_owned)
            .to_vec();
        let hand = BTreeSet::from([(0, 1), (0, 2), (0, 3), (1, 3), (2, 3)]);
        // Four of five: 0.8 passes at the threshold itself.
        let scip = BTreeSet::from([(0, 1), (0, 2), (0, 3), (1, 3), (3, 0)]);
        let outcome = gate(LanguageKind::TypeScript, &hand, &scip, &files);
        assert_eq!(outcome.recall, Some(0.8));
        assert!(outcome.passed);
        // Two of five (n8n's shape): falls back.
        let scip = BTreeSet::from([(0, 1), (1, 3)]);
        assert!(!gate(LanguageKind::TypeScript, &hand, &scip, &files).passed);
        // Go compares target directories: (0 -> b/y) covers (0 -> b/z).
        let hand = BTreeSet::from([(0, 1), (0, 2)]);
        let scip = BTreeSet::from([(0, 1)]);
        let outcome = gate(LanguageKind::Go, &hand, &scip, &files);
        assert_eq!(outcome.recall, Some(1.0));
        assert_eq!(outcome.hand_pairs, 1);
        // Nothing hand-written to lose.
        assert!(gate(LanguageKind::Python, &BTreeSet::new(), &scip, &files).passed);
    }

    #[test]
    fn categories_follow_the_descriptor_suffix() {
        assert_eq!(category("p `m`/f()."), Category::Callable);
        assert_eq!(category("p `m`/C#"), Category::Type);
        assert_eq!(category("p `m`/x."), Category::Term);
        assert_eq!(category("go m v1 `pkg/x`/"), Category::Namespace);
        assert_eq!(category("p `m`/__init__:"), Category::Meta);
    }
}
