//! Complete symbol data lives beside the parity-constrained map document.
//! Extraction collects compact spans and candidate references while each
//! file's parse tree is alive. This pass resolves them after map file order
//! is known. It never changes `GraphData.symbols` or `uses`.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::extract::{self, LanguageKind};
use crate::schema::{
    symbol_edge_kinds, DistrictSymbols, HierSymbolRow, MapDocument, SourceNode, SymbolCoverage,
    SymbolsDocument,
};

const CLASS: usize = 0;
const FUNCTION: usize = 1;
const METHOD: usize = 2;
const NESTED_FUNCTION: usize = 3;
const INTERFACE: usize = 4;
const TYPE: usize = 5;
const CONST: usize = 6;
const CALL: usize = 1;
const EXTENDS: usize = 2;
const IMPLEMENTS: usize = 3;
const OVERRIDES: usize = 4;
const ANNOTATION: usize = 5;
const DECORATOR: usize = 6;
const VALUE: usize = 7;
const POSSIBLE_IMPLEMENTATION: usize = 8;
/// Issue #110 P1a: a SCIP reference whose syntactic kind the hand-written
/// pass did not establish. SCIP knows which symbol a name refers to, not
/// whether the occurrence was a call, an annotation or a value. Appended to
/// a document's `kinds` legend only when a language took the SCIP path; the
/// viewer draws any kind outside extends/implements/overrides as an
/// ordinary reference line.
const REFERENCE: usize = 9;

#[derive(Clone, Serialize, Deserialize)]
struct Span {
    file: usize,
    name: String,
    kind: usize,
    start: usize,
    credit_start: usize,
    end: usize,
    begin_byte: usize,
    credit_begin_byte: usize,
    end_byte: usize,
    code_lines: usize,
    parent: isize,
    abstract_symbol: bool,
    go_signature: Option<[usize; 2]>,
    go_embeds: bool,
    reference_owner: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum Binding {
    Module(usize),
    From(usize, String),
    Prefix(String, String),
    Package(String),
    External,
}

#[derive(Serialize, Deserialize)]
struct Candidate {
    owner: usize,
    chain: Vec<String>,
    call: bool,
    value: bool,
    kind: usize,
    unresolved_reason: Option<UnresolvedReason>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum UnresolvedReason {
    Dynamic,
    ParentClassMethod,
}

impl UnresolvedReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::ParentClassMethod => "parent_class_method",
        }
    }
}

struct FileInfo {
    lang: LanguageKind,
    directory: String,
    imports: BTreeMap<String, Binding>,
    candidates: Vec<Candidate>,
    shadowed: BTreeMap<usize, BTreeSet<String>>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ParsedSymbols {
    spans: Vec<Span>,
    receivers: Vec<(usize, String)>,
    candidates: Vec<Candidate>,
    shadowed: BTreeMap<usize, BTreeSet<String>>,
    imports: RawImports,
    outside: usize,
}

#[derive(Serialize, Deserialize)]
enum RawImports {
    Python(Vec<extract::PythonImport>),
    Go(Vec<(String, String)>),
    TypeScript(Vec<(String, Vec<(String, Option<String>)>)>),
    /// `(local name, path)`: every named `use` leaf in the file's own
    /// module, and every `mod x;` as `(x, [self, x])` (issue #126).
    Rust(Vec<(String, Vec<String>)>),
}

/// Wall time spent inside symbol collection, split by step so a build log
/// can show where the `symbol_collection` phase goes (finding 40). Purely
/// observational: nothing here reaches a map or a symbols document.
#[derive(Clone, Copy, Default)]
pub(crate) struct CollectTimings {
    pub(crate) spans: Duration,
    pub(crate) receivers: Duration,
    pub(crate) lines: Duration,
    pub(crate) imports: Duration,
    pub(crate) candidates: Duration,
    pub(crate) encode: Duration,
    pub(crate) write: Duration,
}

impl CollectTimings {
    pub(crate) fn add(&mut self, other: &Self) {
        self.spans += other.spans;
        self.receivers += other.receivers;
        self.lines += other.lines;
        self.imports += other.imports;
        self.candidates += other.candidates;
        self.encode += other.encode;
        self.write += other.write;
    }

    /// `(label, duration)` rows in the order the steps run.
    pub(crate) fn rows(&self) -> [(&'static str, Duration); 7] {
        [
            ("spans", self.spans),
            ("receivers", self.receivers),
            ("lines", self.lines),
            ("imports", self.imports),
            ("candidates", self.candidates),
            ("encode", self.encode),
            ("write", self.write),
        ]
    }
}

/// A file-backed index keeps collected records out of the graph and geometry
/// high-water marks. Only the current file's record is materialized at either
/// end; Drop removes the temporary stream on success and on error.
pub(crate) struct SymbolSpool {
    path: PathBuf,
    file: File,
    index: BTreeMap<String, (u64, u64)>,
    /// `--refs scip` reference data for the languages that took the SCIP
    /// path, set by extraction once the fallback gate has run.
    pub(crate) scip: Option<ScipSymbolRefs>,
}

/// SCIP reference occurrences and implementation relationships, in
/// `scip_ingest`'s `(file, line)` form. They are credited to symbols only
/// in [`build_with_progress`], after the final symbol order is fixed,
/// because P0's oracle credits against the finished symbols document.
#[derive(Default)]
pub(crate) struct ScipSymbolRefs {
    /// Language codes (`py`, `go`, `ts`) whose references come from SCIP.
    pub(crate) languages: BTreeSet<String>,
    /// File id -> path for the ids below (extraction's lexical order).
    pub(crate) files: Vec<String>,
    pub(crate) refs: Vec<([u32; 4], u32)>,
    pub(crate) implementations: Vec<[u32; 4]>,
}

impl SymbolSpool {
    pub(crate) fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "tolmap-symbols-{}-{}.spool",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("create symbol spool {}", path.display()))?;
        Ok(Self {
            path,
            file,
            index: BTreeMap::new(),
            scip: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn insert(&mut self, name: &str, record: &ParsedSymbols) -> Result<()> {
        self.insert_timed(name, record, &mut CollectTimings::default())
    }

    pub(crate) fn insert_timed(
        &mut self,
        name: &str,
        record: &ParsedSymbols,
        timings: &mut CollectTimings,
    ) -> Result<()> {
        if self.index.contains_key(name) {
            return Ok(()); // First sorted source owns a file-path collision.
        }
        // One write per file avoids a syscall for every JSON token. The
        // temporary byte buffer drops before the next source file is parsed.
        let started = Instant::now();
        let bytes = serde_json::to_vec(record)?;
        timings.encode += started.elapsed();
        let started = Instant::now();
        let start = self.file.stream_position()?;
        self.file.write_all(&bytes)?;
        self.index
            .insert(name.to_owned(), (start, bytes.len() as u64));
        timings.write += started.elapsed();
        Ok(())
    }

    fn take(&mut self, name: &str) -> Result<Option<ParsedSymbols>> {
        let Some((start, len)) = self.index.remove(name) else {
            return Ok(None);
        };
        self.file.seek(SeekFrom::Start(start))?;
        // serde_json's stream parser asks the File for many tiny reads.
        // Reading one indexed record into a short-lived buffer first avoids
        // that cost while keeping the full record set off the heap.
        let mut bytes = vec![0; usize::try_from(len).context("symbol record too large")?];
        self.file.read_exact(&mut bytes)?;
        Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
            format!("decode symbol record for {name}")
        })?))
    }
}

impl Drop for SymbolSpool {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn text<'a>(node: Node<'_>, bytes: &'a [u8]) -> &'a str {
    std::str::from_utf8(&bytes[node.byte_range()]).unwrap_or("")
}

fn name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| text(n, bytes).to_owned())
        .or_else(|| {
            let mut cursor = node.walk();
            let result = node
                .named_children(&mut cursor)
                .find(|n| {
                    matches!(
                        n.kind(),
                        "identifier"
                            | "type_identifier"
                            | "field_identifier"
                            | "property_identifier"
                    )
                })
                .map(|n| text(n, bytes).to_owned());
            result
        })
}

fn kind(node: Node<'_>, parent: Option<Node<'_>>, lang: LanguageKind) -> Option<usize> {
    match lang {
        LanguageKind::Python => match node.kind() {
            "class_definition" => Some(CLASS),
            "function_definition" => Some(FUNCTION),
            _ => None,
        },
        LanguageKind::Go => match node.kind() {
            "function_declaration" => Some(FUNCTION),
            "method_declaration" | "method_elem" => Some(METHOD),
            "type_spec" => Some(match node.child_by_field_name("type").map(|n| n.kind()) {
                Some("struct_type") => CLASS,
                Some("interface_type") => INTERFACE,
                _ => TYPE,
            }),
            _ => None,
        },
        LanguageKind::TypeScript => match node.kind() {
            "class_declaration" | "abstract_class_declaration" | "class" => Some(CLASS),
            "function_declaration" => Some(FUNCTION),
            "method_definition" | "method_signature" | "abstract_method_signature" => Some(METHOD),
            "interface_declaration" => Some(INTERFACE),
            "type_alias_declaration" => Some(TYPE),
            "variable_declarator" => {
                let value = node.child_by_field_name("value");
                if value
                    .is_some_and(|n| matches!(n.kind(), "arrow_function" | "function_expression"))
                {
                    Some(FUNCTION)
                } else if parent.is_some_and(|n| n.kind() == "lexical_declaration")
                    && node
                        .end_position()
                        .row
                        .saturating_sub(node.start_position().row)
                        >= 2
                {
                    Some(CONST)
                } else {
                    None
                }
            }
            _ => None,
        },
        // Rust's kinds depend on more than the parent (`rust_kind`).
        LanguageKind::Rust => None,
    }
}

/// Rust symbol kinds (issue #126), from a node and its named ancestors: a
/// function in an `impl` or `trait` body is a method, struct, enum and
/// union are classes (they carry methods), a trait is an interface, an
/// inline `mod` and a type alias are types, and a `macro_rules!` is a
/// function. The same codes the other languages use; no schema change.
fn rust_kind<S>(node: Node<'_>, ancestors: &[(Node<'_>, S)]) -> Option<usize> {
    let member = ancestors.len() >= 2
        && ancestors[ancestors.len() - 1].0.kind() == "declaration_list"
        && matches!(
            ancestors[ancestors.len() - 2].0.kind(),
            "impl_item" | "trait_item"
        );
    match node.kind() {
        "function_item" | "function_signature_item" => Some(if member { METHOD } else { FUNCTION }),
        "struct_item" | "enum_item" | "union_item" => Some(CLASS),
        "trait_item" => Some(INTERFACE),
        "type_item" => Some(TYPE),
        "mod_item" if node.child_by_field_name("body").is_some() => Some(TYPE),
        "const_item" | "static_item" => Some(CONST),
        "macro_definition" => Some(FUNCTION),
        _ => None,
    }
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn go_parameter_count(node: Node<'_>) -> usize {
    children(node)
        .into_iter()
        .filter(|n| {
            matches!(
                n.kind(),
                "parameter_declaration" | "variadic_parameter_declaration"
            )
        })
        .map(|declaration| {
            let names = children(declaration)
                .into_iter()
                .filter(|n| n.kind() == "identifier")
                .count();
            names.max(1)
        })
        .sum()
}

fn go_signature(node: Node<'_>) -> Option<[usize; 2]> {
    if !matches!(node.kind(), "method_declaration" | "method_elem") {
        return None;
    }
    let parameters = node.child_by_field_name("parameters")?;
    let results = node.child_by_field_name("result").map_or(0, |result| {
        if result.kind() == "parameter_list" {
            go_parameter_count(result)
        } else {
            1
        }
    });
    Some([go_parameter_count(parameters), results])
}

fn abstract_decl(
    node: Node<'_>,
    parent: Option<Node<'_>>,
    bytes: &[u8],
    lang: LanguageKind,
    kind: usize,
) -> bool {
    match lang {
        LanguageKind::Python => {
            if kind == CLASS {
                let bases = node
                    .child_by_field_name("superclasses")
                    .map(|n| text(n, bytes))
                    .unwrap_or("");
                bases
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|part| matches!(part, "ABC" | "ABCMeta"))
            } else if kind == METHOD {
                parent
                    .filter(|p| p.kind() == "decorated_definition")
                    .is_some_and(|p| {
                        children(p).into_iter().any(|child| {
                            child.kind() == "decorator"
                                && matches!(
                                    text(child, bytes).trim_start_matches('@').trim(),
                                    "abstractmethod" | "abc.abstractmethod"
                                )
                        })
                    })
            } else {
                false
            }
        }
        LanguageKind::TypeScript => {
            if kind == INTERFACE || node.kind().starts_with("abstract_") {
                return true;
            }
            let before_name = node
                .child_by_field_name("name")
                .map_or(node.end_byte(), |name| name.start_byte());
            std::str::from_utf8(&bytes[node.start_byte()..before_name])
                .unwrap_or("")
                .split_whitespace()
                .any(|part| part == "abstract")
        }
        // Rust's is set in `collect_spans`, which sees the enclosing trait.
        LanguageKind::Go | LanguageKind::Rust => false,
    }
}

/// Pre-order walk over `root` and its named descendants, reusing one cursor.
/// `enter` sees each node with its named ancestors (root first) and the
/// state each ancestor returned, and returns the node's own state.
///
/// The symbol walks were recursive over `named_children`, allocating a cursor
/// and a `Vec` per node, and asked `Node::parent()` for ancestors. In
/// tree-sitter 0.25 `ts_node_parent` has no parent pointer: it descends from
/// the tree root through `ts_node_child_with_descendant`, scanning siblings
/// at every level. An ancestor chain therefore cost O(depth² × fan-out) per
/// node, and the reference-wrapper check ran it for every node (finding 40).
/// The ancestors are the same nodes `parent()` returns: only named nodes are
/// entered, and an unnamed node's subtree was never visited before either.
fn preorder<'t, S, F>(root: Node<'t>, mut enter: F)
where
    F: FnMut(Node<'t>, &[(Node<'t>, S)]) -> S,
{
    let mut path: Vec<(Node<'t>, S)> = Vec::new();
    let state = enter(root, &path);
    path.push((root, state));
    let mut cursor = root.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let node = cursor.node();
        if node.is_named() {
            let state = enter(node, &path);
            if cursor.goto_first_child() {
                path.push((node, state));
                continue;
            }
        }
        while !cursor.goto_next_sibling() {
            // `path` mirrors the cursor's named ancestors; climbing out of
            // the root's last child empties it and ends the walk.
            if !cursor.goto_parent() {
                return;
            }
            path.pop();
            if path.is_empty() {
                return;
            }
        }
    }
}

fn collect_spans(
    root: Node<'_>,
    bytes: &[u8],
    lang: LanguageKind,
    file: usize,
    first: usize,
    spans: &mut Vec<Span>,
) {
    preorder(root, |node, ancestors| {
        let parent_node = ancestors.last().map(|entry| entry.0);
        let rust = lang == LanguageKind::Rust;
        let found = if rust {
            rust_kind(node, ancestors)
        } else {
            kind(node, parent_node, lang)
        };
        let Some(mut k) = found else {
            return;
        };
        let Some(n) = name(node, bytes) else {
            return;
        };
        let parent = spans[first..]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, s)| s.begin_byte <= node.start_byte() && node.end_byte() <= s.end_byte)
            .map(|(i, _)| (first + i) as isize)
            .unwrap_or(-1);
        if rust {
            // A Rust method is known from syntax (`rust_kind`), and a
            // function in an inline module is still a function: only one
            // inside another function's body is nested.
            if k == FUNCTION
                && parent >= 0
                && matches!(
                    spans[parent as usize].kind,
                    FUNCTION | METHOD | NESTED_FUNCTION
                )
            {
                k = NESTED_FUNCTION;
            }
        } else if k == FUNCTION && parent >= 0 {
            k = if spans[parent as usize].kind == CLASS {
                METHOD
            } else {
                NESTED_FUNCTION
            };
        }
        let decorated = parent_node.filter(|p| p.kind() == "decorated_definition");
        spans.push(Span {
            file,
            name: n,
            kind: k,
            start: node.start_position().row + 1,
            credit_start: decorated.map_or(node.start_position().row + 1, |p| {
                p.start_position().row + 1
            }),
            end: node.end_position().row + 1,
            begin_byte: node.start_byte(),
            credit_begin_byte: decorated.map_or(node.start_byte(), |p| p.start_byte()),
            end_byte: node.end_byte(),
            code_lines: 0,
            parent,
            abstract_symbol: abstract_decl(node, parent_node, bytes, lang, k)
                || (parent >= 0
                    && spans[parent as usize].kind == INTERFACE
                    && lang == LanguageKind::TypeScript)
                // A trait, and a trait method with no default body.
                || (rust
                    && (k == INTERFACE
                        || (node.kind() == "function_signature_item"
                            && parent >= 0
                            && spans[parent as usize].kind == INTERFACE))),
            go_signature: (lang == LanguageKind::Go)
                .then(|| go_signature(node))
                .flatten(),
            go_embeds: lang == LanguageKind::Go
                && k == INTERFACE
                && node.child_by_field_name("type").is_some_and(|ty| {
                    children(ty)
                        .into_iter()
                        .any(|child| child.kind() == "type_elem")
                }),
            // This signature was not a symbol before, so annotation
            // references inside it retain their enclosing owner.
            reference_owner: node.kind() != "abstract_method_signature",
        });
    });
}

fn receiver_type(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() == "type_identifier" {
        return Some(text(node, bytes).to_owned());
    }
    children(node)
        .into_iter()
        .find_map(|child| receiver_type(child, bytes))
}

fn collect_go_receivers(
    root: Node<'_>,
    bytes: &[u8],
    spans: &[Span],
    offset: usize,
    out: &mut Vec<(usize, String)>,
) {
    preorder(root, |node, _| {
        if node.kind() == "method_declaration" {
            if let Some(type_name) = node
                .child_by_field_name("receiver")
                .and_then(|n| receiver_type(n, bytes))
            {
                if let Some((i, _)) = spans
                    .iter()
                    .enumerate()
                    .find(|(_, s)| s.begin_byte == node.start_byte() && s.kind == METHOD)
                {
                    out.push((offset + i, type_name));
                }
            }
        }
    });
}

/// The type an `impl` block's methods belong to (issue #126): its self
/// type's last name, `Store` for `impl<T> crate::store::Store<T>`. The
/// methods are parented to that type's span once every file is read, as a
/// Go method is to its receiver's type.
fn rust_impl_type(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(text(node, bytes).to_owned()),
        "scoped_type_identifier" => node
            .child_by_field_name("name")
            .map(|name| text(name, bytes).to_owned()),
        "generic_type" => rust_impl_type(node.child_by_field_name("type")?, bytes),
        _ => None,
    }
}

fn collect_rust_receivers(
    root: Node<'_>,
    bytes: &[u8],
    spans: &[Span],
    out: &mut Vec<(usize, String)>,
) {
    preorder(root, |node, ancestors| {
        if node.kind() != "function_item" || ancestors.len() < 2 {
            return;
        }
        let owner = ancestors[ancestors.len() - 2].0;
        if ancestors[ancestors.len() - 1].0.kind() != "declaration_list"
            || owner.kind() != "impl_item"
        {
            return;
        }
        let Some(type_name) = owner
            .child_by_field_name("type")
            .and_then(|ty| rust_impl_type(ty, bytes))
        else {
            return;
        };
        if let Some((i, _)) = spans
            .iter()
            .enumerate()
            .find(|(_, s)| s.begin_byte == node.start_byte() && s.kind == METHOD)
        {
            out.push((i, type_name));
        }
    });
}

fn chain(node: Node<'_>, bytes: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        // `crate` and `self` are node kinds only in Rust's grammar.
        "identifier" | "type_identifier" | "package_identifier" | "this" | "super" | "crate"
        | "self" => Some(vec![text(node, bytes).to_owned()]),
        // Rust (issue #126): `a::b::f` and `value.field`. Neither kind
        // exists in the Python, Go or TypeScript grammars.
        "scoped_identifier" | "scoped_type_identifier" => {
            let base = node.child_by_field_name("path")?;
            let name = node.child_by_field_name("name")?;
            let mut parts = chain(base, bytes)?;
            parts.push(text(name, bytes).to_owned());
            Some(parts)
        }
        "field_expression" => {
            let base = node.child_by_field_name("value")?;
            let field = node.child_by_field_name("field")?;
            let mut parts = chain(base, bytes)?;
            parts.push(text(field, bytes).to_owned());
            Some(parts)
        }
        "attribute" | "member_expression" | "selector_expression" => {
            let base = node
                .child_by_field_name("object")
                .or_else(|| node.child_by_field_name("operand"))?;
            let attr = node
                .child_by_field_name("attribute")
                .or_else(|| node.child_by_field_name("property"))
                .or_else(|| node.child_by_field_name("field"))?;
            let mut parts = chain(base, bytes)?;
            parts.push(text(attr, bytes).to_owned());
            Some(parts)
        }
        _ => None,
    }
}

struct OwnerLookup<'a> {
    spans: &'a [Span],
    starts: Vec<usize>,
    next: usize,
    active: BTreeSet<(usize, usize)>,
    ends: BinaryHeap<Reverse<(usize, usize)>>,
}

impl<'a> OwnerLookup<'a> {
    fn new(spans: &'a [Span]) -> Self {
        let mut starts = (0..spans.len())
            .filter(|&i| spans[i].reference_owner)
            .collect::<Vec<_>>();
        starts.sort_by_key(|&i| (spans[i].credit_begin_byte, i));
        Self {
            spans,
            starts,
            next: 0,
            active: BTreeSet::new(),
            ends: BinaryHeap::new(),
        }
    }

    fn at(&mut self, byte: usize) -> Option<usize> {
        // collect_candidates walks syntax in source order. Maintain only
        // spans covering the current byte, ordered by the same narrowest
        // interval / first-index rule as the former full scan. This avoids
        // visiting every symbol for every syntax node in large files.
        while self.next < self.starts.len()
            && self.spans[self.starts[self.next]].credit_begin_byte <= byte
        {
            let i = self.starts[self.next];
            let span = &self.spans[i];
            self.active.insert((span.end_byte - span.begin_byte, i));
            self.ends.push(Reverse((span.end_byte, i)));
            self.next += 1;
        }
        while let Some(&Reverse((end, i))) = self.ends.peek() {
            if end > byte {
                break;
            }
            self.ends.pop();
            let span = &self.spans[i];
            self.active.remove(&(span.end_byte - span.begin_byte, i));
        }
        self.active.iter().next().map(|&(_, i)| i)
    }
}

fn expression_chains(node: Node<'_>, bytes: &[u8], out: &mut Vec<Vec<String>>) {
    if matches!(node.kind(), "call" | "call_expression") {
        // The call walker credits the callee once. Its arguments can still
        // contain references, but repeating the callee here would inflate
        // an edge count beyond the number of lexical sites.
        for child in children(node) {
            if node.child_by_field_name("function").map(|n| n.id()) != Some(child.id()) {
                expression_chains(child, bytes, out);
            }
        }
        return;
    }
    if let Some(parts) = chain(node, bytes) {
        out.push(parts);
    } else {
        for child in children(node) {
            expression_chains(child, bytes, out);
        }
    }
}

fn parameter_bindings(node: Node<'_>, bytes: &[u8], out: &mut BTreeSet<String>) {
    if matches!(node.kind(), "type" | "type_annotation" | "default_value") {
        return;
    }
    if node.kind() == "identifier" {
        out.insert(text(node, bytes).to_owned());
        return;
    }
    for child in children(node) {
        parameter_bindings(child, bytes, out);
    }
}

/// Kinds whose subtree is credited by one typed reference (decorator,
/// base class, annotation). A node below one is not collected again.
fn reference_wrapper(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "decorator"
            | "superclasses"
            | "type"
            | "type_annotation"
            | "extends_clause"
            | "implements_clause"
    )
}

/// Definitions end the upward search for a reference wrapper: a decorator
/// on a class does not cover the class body.
fn wrapper_boundary(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "class_definition" | "function_definition" | "method_definition" | "method_declaration"
    )
}

/// `covered` is whether a reference wrapper lies between `node` and its
/// nearest enclosing definition; `ancestors` are its named ancestors, root
/// first.
fn value_attribute(node: Node<'_>, covered: bool, ancestors: &[(Node<'_>, bool)]) -> bool {
    if !matches!(
        node.kind(),
        "attribute" | "member_expression" | "selector_expression"
    ) || covered
    {
        return false;
    }
    let mut child = node;
    for &(parent, _) in ancestors.iter().rev() {
        // A selector inside a longer selector is credited at the outermost
        // lexical site, if that full chain resolves. A store target does
        // not read the method it happens to name.
        if matches!(
            parent.kind(),
            "attribute" | "member_expression" | "selector_expression"
        ) && parent
            .child_by_field_name("object")
            .or_else(|| parent.child_by_field_name("operand"))
            .is_some_and(|base| base.id() == child.id())
        {
            return false;
        }
        if matches!(parent.kind(), "call" | "call_expression")
            && parent
                .child_by_field_name("function")
                .or_else(|| parent.child_by_field_name("method"))
                .is_some_and(|callee| callee.id() == child.id())
        {
            return false;
        }
        if matches!(
            parent.kind(),
            "assignment"
                | "augmented_assignment"
                | "assignment_expression"
                | "augmented_assignment_expression"
                | "short_var_declaration"
        ) && parent
            .child_by_field_name("left")
            .or_else(|| parent.child_by_field_name("name"))
            .is_some_and(|left| left.id() == child.id())
        {
            return false;
        }
        child = parent;
    }
    true
}

fn collect_superclasses(owner: usize, node: Node<'_>, bytes: &[u8], out: &mut Vec<Candidate>) {
    for child in children(node) {
        let mut chains = Vec::new();
        expression_chains(child, bytes, &mut chains);
        for chain in chains {
            out.push(Candidate {
                owner,
                chain,
                call: false,
                value: false,
                kind: if child.kind() == "keyword_argument" {
                    ANNOTATION
                } else {
                    EXTENDS
                },
                unresolved_reason: None,
            });
        }
    }
}

fn collect_candidates(
    root: Node<'_>,
    bytes: &[u8],
    owners: &mut OwnerLookup<'_>,
    out: &mut Vec<Candidate>,
    shadowed: &mut BTreeMap<usize, BTreeSet<String>>,
) {
    // Each node's state is whether its children sit below a reference
    // wrapper, so the old upward `parent()` search is one lookup here.
    preorder(root, |node, ancestors| {
        let covered: bool = ancestors.last().is_some_and(|entry| entry.1);
        let owner = owners.at(node.start_byte());
        if let Some(owner) = owner {
            if matches!(node.kind(), "parameters" | "formal_parameters") {
                parameter_bindings(node, bytes, shadowed.entry(owner).or_default());
            }
            if matches!(
                node.kind(),
                "assignment"
                    | "augmented_assignment"
                    | "variable_declarator"
                    | "short_var_declaration"
            ) {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.child_by_field_name("name"))
                {
                    parameter_bindings(left, bytes, shadowed.entry(owner).or_default());
                }
            }
            let call = matches!(node.kind(), "call" | "call_expression");
            if call {
                let callee = node
                    .child_by_field_name("function")
                    .or_else(|| node.child_by_field_name("method"));
                let resolved = callee.and_then(|n| {
                    if n.kind() == "attribute" {
                        let base = n.child_by_field_name("object")?;
                        if base.kind() == "call"
                            && base
                                .child_by_field_name("function")
                                .is_some_and(|function| text(function, bytes) == "super")
                            && base
                                .child_by_field_name("arguments")
                                .is_some_and(|args| children(args).is_empty())
                        {
                            return n.child_by_field_name("attribute").map(|attr| {
                                vec!["super".to_owned(), text(attr, bytes).to_owned()]
                            });
                        }
                    }
                    chain(n, bytes)
                });
                let parent_class_method = callee.is_some_and(|n| {
                    n.kind() == "attribute"
                        && n.child_by_field_name("object").is_some_and(|object| {
                            object.kind() == "call"
                                && object
                                    .child_by_field_name("function")
                                    .is_some_and(|function| text(function, bytes) == "super")
                        })
                });
                out.push(Candidate {
                    owner,
                    chain: resolved.clone().unwrap_or_default(),
                    call: true,
                    value: false,
                    kind: CALL,
                    unresolved_reason: resolved.is_none().then_some(if parent_class_method {
                        UnresolvedReason::ParentClassMethod
                    } else {
                        UnresolvedReason::Dynamic
                    }),
                });
            } else if !covered && reference_wrapper(node) {
                if node.kind() == "superclasses" {
                    collect_superclasses(owner, node, bytes, out);
                } else {
                    let mut chains = Vec::new();
                    expression_chains(node, bytes, &mut chains);
                    let kind = match node.kind() {
                        "superclasses" | "extends_clause" => EXTENDS,
                        "implements_clause" => IMPLEMENTS,
                        "decorator" => DECORATOR,
                        _ => ANNOTATION,
                    };
                    for parts in chains {
                        out.push(Candidate {
                            owner,
                            chain: parts,
                            call: false,
                            value: false,
                            kind,
                            unresolved_reason: None,
                        });
                    }
                }
            } else if node.kind() == "class_definition" {
                if let Some(bases) = node.child_by_field_name("superclasses") {
                    collect_superclasses(owner, bases, bytes, out);
                }
            } else if value_attribute(node, covered, ancestors) {
                if let Some(parts) = chain(node, bytes) {
                    out.push(Candidate {
                        owner,
                        chain: parts,
                        call: false,
                        value: true,
                        kind: VALUE,
                        unresolved_reason: None,
                    });
                }
            }
        }
        reference_wrapper(node) || (!wrapper_boundary(node) && covered)
    });
}

fn imports_python(
    imports: &[extract::PythonImport],
    module: &str,
    is_pkg: bool,
    modules: &BTreeMap<String, usize>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    // A function-local import must not create a file-wide binding. Direct
    // module imports are certain; conditional imports need control-flow
    // analysis and are left unresolved here.
    for imp in imports {
        let head = extract::python_head(imp, module, is_pkg);
        for (n, alias) in &imp.names {
            if n == "*" {
                continue;
            }
            let local = alias.clone().unwrap_or_else(|| {
                if imp.from {
                    n.clone()
                } else {
                    n.split('.').next().unwrap_or(n).to_owned()
                }
            });
            let binding = if imp.from {
                let full = if head.is_empty() {
                    n.clone()
                } else {
                    format!("{head}.{n}")
                };
                if let Some(&fi) = modules.get(&full) {
                    Binding::Module(fi)
                } else if let Some(&fi) = modules.get(&head) {
                    Binding::From(fi, n.clone())
                } else {
                    Binding::External
                }
            } else if alias.is_some() {
                modules
                    .get(n)
                    .copied()
                    .map(Binding::Module)
                    .unwrap_or(Binding::External)
            } else if !n.contains('.') && modules.contains_key(n) {
                Binding::Module(modules[n])
            } else {
                Binding::Prefix(n.split('.').next().unwrap_or(n).to_owned(), n.clone())
            };
            if !ambiguous.contains(&local) {
                match result.get(&local) {
                    Some(existing) if existing != &binding => {
                        result.remove(&local);
                        ambiguous.insert(local);
                    }
                    Some(_) => {}
                    None => {
                        result.insert(local, binding);
                    }
                }
            }
        }
    }
    result
}

fn imports_multi(
    imports: &[(String, Vec<(String, Option<String>)>)],
    file: &str,
    modules: &BTreeMap<String, usize>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    for (path, clauses) in imports {
        let dir = file.rsplit_once('/').map_or("", |v| v.0);
        let joined = format!("{dir}/{path}");
        let mut pieces = Vec::new();
        for part in joined.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    pieces.pop();
                }
                _ => pieces.push(part),
            }
        }
        let stem = pieces.join("/");
        // NodeNext emits `.js` specifiers for TypeScript source. The
        // existing file resolver accepts this same substitution (finding
        // 20), and only an actually mapped target is credited here.
        let source_stem = stem
            .strip_suffix(".js")
            .or_else(|| stem.strip_suffix(".jsx"))
            .unwrap_or(&stem);
        let target = [
            stem.clone(),
            format!("{source_stem}.ts"),
            format!("{source_stem}.tsx"),
            format!("{source_stem}/index.ts"),
        ]
        .into_iter()
        .find_map(|p| modules.get(&p).copied());
        if let Some(target) = target {
            for (local, original) in clauses {
                result.insert(
                    local.clone(),
                    match original {
                        Some(original) => Binding::From(target, original.clone()),
                        None => Binding::Module(target),
                    },
                );
            }
        }
    }
    result
}

fn imports_go(
    imports: &[(String, String)],
    module: Option<&str>,
    packages: &BTreeMap<String, Vec<usize>>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    let Some(module) = module else {
        return result;
    };
    for (path, alias) in imports {
        let directory = if path.as_str() == module {
            Some("")
        } else {
            path.strip_prefix(module)
                .and_then(|rest| rest.strip_prefix('/'))
        };
        if let Some(directory) = directory.filter(|dir| packages.contains_key(*dir)) {
            if alias != "_" && alias != "." {
                result.insert(alias.clone(), Binding::Package(directory.to_owned()));
            }
        }
    }
    result
}

/// A Rust file's bindings (issue #126): its `use` leaves and `mod x;`
/// declarations, made absolute from the file's module path
/// (`crate_key::a::b`, `rust::resolve`'s `module_for`) and cut at the
/// longest prefix that names a module file. `crate`, `self`, `super` and
/// every workspace crate name bind the modules they name, so a call chain
/// like `crate::store::keep` resolves through `lookup` module by module.
/// Re-exports are followed by `lookup` itself, through the target file's
/// own bindings. A file no crate reaches (its module is its path) binds
/// nothing.
fn imports_rust(
    imports: &[(String, Vec<String>)],
    module: &str,
    file: &str,
    modules: &BTreeMap<String, usize>,
    crates: &BTreeMap<String, usize>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    if module == file {
        return result;
    }
    let own = module.split("::").map(str::to_owned).collect::<Vec<_>>();
    let absolute = |segments: &[String]| -> Option<Vec<String>> {
        let (head, rest) = segments.split_first()?;
        let mut path = match head.as_str() {
            "crate" => vec![own[0].clone()],
            "self" => own.clone(),
            "super" => {
                let mut parent = own.clone();
                if parent.len() < 2 {
                    return None;
                }
                parent.pop();
                parent
            }
            name if crates.contains_key(name) => vec![name.to_owned()],
            name => {
                let mut local = own.clone();
                local.push(name.to_owned());
                local
            }
        };
        for segment in rest {
            if segment == "super" {
                if path.len() < 2 {
                    return None;
                }
                path.pop();
            } else {
                path.push(segment.clone());
            }
        }
        Some(path)
    };
    for (local, segments) in imports {
        let Some(path) = absolute(segments) else {
            continue;
        };
        let binding = (1..=path.len())
            .rev()
            .find_map(|cut| {
                let &fi = modules.get(&path[..cut].join("::"))?;
                Some(match &path[cut..] {
                    [] => Binding::Module(fi),
                    [name] => Binding::From(fi, name.clone()),
                    // `Type::Variant` and deeper: not a symbol by name.
                    _ => Binding::External,
                })
            })
            .unwrap_or(Binding::External);
        result.insert(local.clone(), binding);
    }
    let mut heads = vec![("crate".to_owned(), own[..1].join("::"))];
    heads.push(("self".to_owned(), module.to_owned()));
    if own.len() >= 2 {
        heads.push(("super".to_owned(), own[..own.len() - 1].join("::")));
    }
    for (name, target) in heads {
        if let Some(&fi) = modules.get(&target) {
            result.insert(name, Binding::Module(fi));
        }
    }
    for (name, &fi) in crates {
        result.entry(name.clone()).or_insert(Binding::Module(fi));
    }
    result
}

#[cfg(test)]
pub(crate) fn collect(
    root: Node<'_>,
    bytes: &[u8],
    lang: LanguageKind,
    flags: &[bool],
) -> ParsedSymbols {
    collect_timed(root, bytes, lang, flags, &mut CollectTimings::default())
}

pub(crate) fn collect_timed(
    root: Node<'_>,
    bytes: &[u8],
    lang: LanguageKind,
    flags: &[bool],
    timings: &mut CollectTimings,
) -> ParsedSymbols {
    let started = Instant::now();
    let mut spans = Vec::new();
    collect_spans(root, bytes, lang, 0, 0, &mut spans);
    timings.spans += started.elapsed();
    let started = Instant::now();
    let mut receivers = Vec::new();
    if lang == LanguageKind::Go {
        collect_go_receivers(root, bytes, &spans, 0, &mut receivers);
    } else if lang == LanguageKind::Rust {
        collect_rust_receivers(root, bytes, &spans, &mut receivers);
    }
    timings.receivers += started.elapsed();
    let started = Instant::now();
    // The old pass rescanned all line flags for every span, making large
    // files quadratic in their symbol count. Inclusive row ranges become
    // two prefix lookups while the tree is still scoped to this file.
    let mut code_prefix = Vec::with_capacity(flags.len() + 1);
    code_prefix.push(0usize);
    for &flag in flags {
        code_prefix.push(code_prefix.last().copied().unwrap_or(0) + usize::from(flag));
    }
    for span in &mut spans {
        let start = (span.credit_start - 1).min(flags.len());
        let end = span.end.min(flags.len());
        span.code_lines = code_prefix[end].saturating_sub(code_prefix[start]);
    }
    let mut covered = vec![false; flags.len()];
    for span in spans.iter().filter(|span| span.parent == -1) {
        covered[(span.credit_start - 1).min(flags.len())..span.end.min(flags.len())].fill(true);
    }
    let outside = flags
        .iter()
        .zip(&covered)
        .filter(|(flag, covered)| **flag && !**covered)
        .count();
    timings.lines += started.elapsed();
    let started = Instant::now();
    let imports = match lang {
        LanguageKind::Python => RawImports::Python(
            children(root)
                .into_iter()
                .filter(|n| {
                    matches!(
                        n.kind(),
                        "import_statement" | "import_from_statement" | "future_import_statement"
                    )
                })
                .flat_map(|n| extract::python_imports(n, bytes))
                .collect(),
        ),
        LanguageKind::Go => {
            let mut imports = Vec::new();
            preorder(root, |node, _| {
                if node.kind() == "import_spec" {
                    if let Some(path_node) = node.child_by_field_name("path") {
                        let path = text(path_node, bytes).trim_matches(['\'', '"', '`']);
                        let alias = node
                            .child_by_field_name("name")
                            .map(|n| text(n, bytes).to_owned())
                            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned());
                        imports.push((path.to_owned(), alias));
                    }
                }
            });
            RawImports::Go(imports)
        }
        LanguageKind::TypeScript => {
            let mut imports = Vec::new();
            for node in children(root) {
                if node.kind() != "import_statement" {
                    continue;
                }
                if let Some(source) = node.child_by_field_name("source") {
                    let path = text(source, bytes).trim_matches(['\'', '"']).to_owned();
                    let mut clauses = Vec::new();
                    for clause in children(node)
                        .into_iter()
                        .filter(|n| n.kind() == "import_clause")
                    {
                        for item in children(clause) {
                            match item.kind() {
                                "named_imports" => {
                                    for spec in children(item) {
                                        if spec.kind() == "import_specifier" {
                                            if let Some(original) = spec.child_by_field_name("name")
                                            {
                                                let imported = text(original, bytes).to_owned();
                                                let local = spec
                                                    .child_by_field_name("alias")
                                                    .map_or(imported.clone(), |n| {
                                                        text(n, bytes).to_owned()
                                                    });
                                                clauses.push((local, Some(imported)));
                                            }
                                        }
                                    }
                                }
                                "namespace_import" => {
                                    if let Some(alias) = children(item).first() {
                                        clauses.push((text(*alias, bytes).to_owned(), None));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    imports.push((path, clauses));
                }
            }
            RawImports::TypeScript(imports)
        }
        LanguageKind::Rust => {
            let syntax = extract::rust::syntax(root, bytes);
            let mut imports = syntax
                .mods
                .iter()
                .filter(|decl| decl.scope.is_empty() && !decl.inline)
                .map(|decl| {
                    (
                        decl.name.clone(),
                        vec!["self".to_owned(), decl.name.clone()],
                    )
                })
                .collect::<Vec<_>>();
            for decl in syntax.uses.iter().filter(|decl| decl.scope.is_empty()) {
                for leaf in &decl.leaves {
                    if let (Some(local), false) = (&leaf.local, leaf.glob) {
                        imports.push((local.clone(), leaf.segments.clone()));
                    }
                }
            }
            RawImports::Rust(imports)
        }
    };
    timings.imports += started.elapsed();
    let started = Instant::now();
    let mut candidates = Vec::new();
    let mut shadowed = BTreeMap::new();
    collect_candidates(
        root,
        bytes,
        &mut OwnerLookup::new(&spans),
        &mut candidates,
        &mut shadowed,
    );
    timings.candidates += started.elapsed();
    ParsedSymbols {
        spans,
        receivers,
        candidates,
        shadowed,
        imports,
        outside,
    }
}

fn lookup(
    file: usize,
    name: &str,
    infos: &[FileInfo],
    tops: &[BTreeMap<String, usize>],
    packages: &BTreeMap<String, Vec<usize>>,
    depth: usize,
) -> Option<BindingOrSymbol> {
    if depth > 4 {
        return None;
    }
    if let Some(&symbol) = tops[file].get(name) {
        return Some(BindingOrSymbol::Symbol(symbol));
    }
    if infos[file].lang == LanguageKind::Go {
        let hits = packages
            .get(&infos[file].directory)
            .into_iter()
            .flatten()
            .filter_map(|&fi| tops[fi].get(name).copied())
            .collect::<Vec<_>>();
        if hits.len() == 1 {
            return Some(BindingOrSymbol::Symbol(hits[0]));
        }
    }
    match infos[file].imports.get(name)? {
        Binding::Module(fi) => Some(BindingOrSymbol::Module(*fi)),
        Binding::From(fi, target) => lookup(*fi, target, infos, tops, packages, depth + 1),
        Binding::Prefix(prefix, target) => {
            Some(BindingOrSymbol::Prefix(prefix.clone(), target.clone()))
        }
        Binding::Package(directory) => Some(BindingOrSymbol::Package(directory.clone())),
        Binding::External => Some(BindingOrSymbol::External),
    }
}

enum BindingOrSymbol {
    Symbol(usize),
    Module(usize),
    Prefix(String, String),
    Package(String),
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MroItem {
    Known(usize),
    Unknown(usize),
}

fn class_mro(
    class: usize,
    bases: &BTreeMap<usize, Vec<MroItem>>,
    cache: &mut BTreeMap<usize, Option<Vec<MroItem>>>,
    visiting: &mut BTreeSet<usize>,
) -> Option<Vec<MroItem>> {
    if let Some(mro) = cache.get(&class) {
        return mro.clone();
    }
    if !visiting.insert(class) {
        return None;
    }
    let direct = bases.get(&class).cloned().unwrap_or_default();
    let mut sequences = Vec::new();
    for base in &direct {
        sequences.push(match base {
            MroItem::Known(base) => class_mro(*base, bases, cache, visiting)?,
            MroItem::Unknown(_) => vec![*base],
        });
    }
    sequences.push(direct);
    let mut result = vec![MroItem::Known(class)];
    while sequences.iter().any(|sequence| !sequence.is_empty()) {
        // C3 chooses the first head absent from every other sequence's
        // tail. A conflicting or cyclic hierarchy has no safe call target.
        let head = sequences
            .iter()
            .filter_map(|s| s.first().copied())
            .find(|head| {
                sequences
                    .iter()
                    .all(|s| !s.iter().skip(1).any(|item| item == head))
            })?;
        result.push(head);
        for sequence in &mut sequences {
            if sequence.first() == Some(&head) {
                sequence.remove(0);
            }
        }
    }
    visiting.remove(&class);
    cache.insert(class, Some(result.clone()));
    Some(result)
}

fn inherited_member(
    class: usize,
    name: &str,
    mros: &BTreeMap<usize, Option<Vec<MroItem>>>,
    members: &BTreeMap<usize, BTreeMap<String, usize>>,
) -> Option<usize> {
    let mro = mros.get(&class)?.as_ref()?;
    for item in mro.iter().skip(1) {
        match item {
            MroItem::Unknown(_) => return None,
            MroItem::Known(base) => {
                if let Some(&method) = members.get(base).and_then(|items| items.get(name)) {
                    return Some(method);
                }
            }
        }
    }
    None
}

fn resolve(
    candidate: &Candidate,
    spans: &[Span],
    infos: &[FileInfo],
    tops: &[BTreeMap<String, usize>],
    modules: &BTreeMap<String, usize>,
    packages: &BTreeMap<String, Vec<usize>>,
    members: &BTreeMap<usize, BTreeMap<String, usize>>,
    mros: &BTreeMap<usize, Option<Vec<MroItem>>>,
) -> Result<(usize, bool), &'static str> {
    let source = &spans[candidate.owner];
    let parts = &candidate.chain;
    if let Some(reason) = candidate.unresolved_reason {
        return Err(reason.as_str());
    }
    if parts.is_empty() {
        return Err("dynamic");
    }
    let head = parts[0].as_str();
    // Rust's `super` and `self::` name modules, bound in `imports_rust`;
    // `Self::` and `self.` name the impl's type, as `this` does.
    let rust = infos[source.file].lang == LanguageKind::Rust;
    let member_head = if rust {
        matches!(head, "Self" | "self")
    } else {
        matches!(head, "self" | "cls" | "this" | "super")
    };
    if member_head && parts.len() >= 2 {
        let mut p = Some(candidate.owner);
        while let Some(i) = p {
            if spans[i].kind == CLASS {
                if head != "super" {
                    if let Some(&direct) = members.get(&i).and_then(|m| m.get(&parts[1])) {
                        return Ok((direct, false));
                    }
                }
                return inherited_member(i, &parts[1], mros, members)
                    .map(|target| (target, true))
                    .ok_or(if head == "super" {
                        "parent_class_method"
                    } else {
                        "instance_or_untyped"
                    });
            }
            p = (spans[i].parent >= 0).then_some(spans[i].parent as usize);
        }
        return Err(if head == "super" {
            "parent_class_method"
        } else {
            "instance_or_untyped"
        });
    }
    if infos[source.file]
        .shadowed
        .get(&candidate.owner)
        .is_some_and(|names| names.contains(head))
    {
        return Err("local_or_builtin");
    }
    let mut p = Some(candidate.owner);
    let mut value = None;
    while let Some(i) = p {
        if let Some(&nested) = members.get(&i).and_then(|m| m.get(head)) {
            value = Some(BindingOrSymbol::Symbol(nested));
            break;
        }
        p = (spans[i].parent >= 0).then_some(spans[i].parent as usize);
    }
    let mut value = value
        .or_else(|| lookup(source.file, head, infos, tops, packages, 0))
        .ok_or("local_or_builtin")?;
    for part in parts.iter().skip(1) {
        value = match value {
            BindingOrSymbol::Symbol(i) => members
                .get(&i)
                .and_then(|m| m.get(part).copied())
                .map(BindingOrSymbol::Symbol)
                .ok_or("unresolved_attribute")?,
            BindingOrSymbol::Module(fi) => {
                lookup(fi, part, infos, tops, packages, 0).ok_or("unresolved_attribute")?
            }
            BindingOrSymbol::Package(directory) => {
                let hits = packages
                    .get(&directory)
                    .into_iter()
                    .flatten()
                    .filter_map(|&fi| tops[fi].get(part).copied())
                    .collect::<Vec<_>>();
                if hits.len() == 1 {
                    BindingOrSymbol::Symbol(hits[0])
                } else {
                    return Err("unresolved_attribute");
                }
            }
            BindingOrSymbol::Prefix(prefix, target) => {
                let path = format!("{prefix}.{part}");
                if path == target {
                    let &fi = modules.get(&path).ok_or("external")?;
                    BindingOrSymbol::Module(fi)
                } else if target.starts_with(&format!("{path}.")) {
                    BindingOrSymbol::Prefix(path, target)
                } else {
                    return Err("unresolved_attribute");
                }
            }
            BindingOrSymbol::External => return Err("external"),
        };
    }
    match value {
        BindingOrSymbol::Symbol(i) => Ok((i, false)),
        BindingOrSymbol::Module(_) | BindingOrSymbol::Package(_) => Err("module_only"),
        BindingOrSymbol::Prefix(_, _) | BindingOrSymbol::External => Err("external"),
    }
}

pub(crate) fn build(
    repo: &Path,
    nodes: &[SourceNode],
    spool: SymbolSpool,
) -> Result<SymbolsDocument> {
    build_with_progress(repo, nodes, spool, None)
}

pub(crate) fn build_with_progress(
    repo: &Path,
    nodes: &[SourceNode],
    mut spool: SymbolSpool,
    progress: Option<&crate::progress::StageCounter>,
) -> Result<SymbolsDocument> {
    let mut modules = BTreeMap::new();
    let mut ambiguous_modules = BTreeSet::new();
    let mut packages: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (fi, node) in nodes.iter().enumerate() {
        for key in [&node.module, &node.file] {
            if ambiguous_modules.contains(key.as_str()) {
                continue;
            }
            if let Some(previous) = modules.insert((*key).clone(), fi) {
                if previous != fi {
                    modules.remove(key.as_str());
                    ambiguous_modules.insert((*key).clone());
                }
            }
        }
        if node.lang == "go" {
            let directory = node.file.rsplit_once('/').map_or("", |v| v.0).to_owned();
            packages.entry(directory).or_default().push(fi);
        }
    }
    // Rust crate roots by crate name: a module path with no `::` that is
    // not the file's own path (issue #126).
    let rust_crates = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.lang == "rs" && node.module != node.file && !node.module.contains("::")
        })
        .map(|(fi, node)| (node.module.clone(), fi))
        .collect::<BTreeMap<_, _>>();
    let go_module = fs::read_to_string(repo.join("go.mod"))
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("module ")
                    .and_then(|rest| rest.split_whitespace().next())
                    .map(str::to_owned)
            })
        });
    let mut spans = Vec::new();
    let mut go_receivers = Vec::new();
    let mut infos = Vec::new();
    let mut module_code_lines = BTreeMap::new();
    for (fi, entry) in nodes.iter().enumerate() {
        let lang = LanguageKind::parse(&entry.lang)?;
        // A rejected Python parse or a tree-sitter cancellation leaves the
        // same empty slot that the former second parse left in this array.
        let Some(mut record) = spool.take(&entry.file)? else {
            module_code_lines.insert(fi, entry.code_lines.unwrap_or(0));
            infos.push(FileInfo {
                lang,
                directory: entry.file.rsplit_once('/').map_or("", |v| v.0).to_owned(),
                imports: BTreeMap::new(),
                candidates: Vec::new(),
                shadowed: BTreeMap::new(),
            });
            if let Some(progress) = progress {
                progress.advance(1);
            }
            continue;
        };
        let first = spans.len();
        for span in &mut record.spans {
            span.file = fi;
            if span.parent >= 0 {
                span.parent += first as isize;
            }
        }
        go_receivers.extend(
            record
                .receivers
                .into_iter()
                .map(|(i, name)| (first + i, name)),
        );
        for candidate in &mut record.candidates {
            candidate.owner += first;
        }
        let shadowed = record
            .shadowed
            .into_iter()
            .map(|(owner, names)| (first + owner, names))
            .collect();
        spans.extend(record.spans);
        module_code_lines.insert(fi, record.outside);
        let imports = match record.imports {
            RawImports::Python(imports) => imports_python(
                &imports,
                &entry.module,
                entry.file.ends_with("__init__.py"),
                &modules,
            ),
            RawImports::Go(imports) => imports_go(&imports, go_module.as_deref(), &packages),
            RawImports::TypeScript(imports) => imports_multi(&imports, &entry.file, &modules),
            RawImports::Rust(imports) => {
                imports_rust(&imports, &entry.module, &entry.file, &modules, &rust_crates)
            }
        };
        infos.push(FileInfo {
            lang,
            directory: entry.file.rsplit_once('/').map_or("", |v| v.0).to_owned(),
            imports,
            candidates: record.candidates,
            shadowed,
        });
        if let Some(progress) = progress {
            progress.advance(1);
        }
    }
    let mut go_types: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, span) in spans.iter().enumerate() {
        if nodes[span.file].lang == "go" && matches!(span.kind, CLASS | INTERFACE | TYPE) {
            let directory = nodes[span.file]
                .file
                .rsplit_once('/')
                .map_or("", |v| v.0)
                .to_owned();
            go_types
                .entry((directory, span.name.clone()))
                .or_default()
                .push(i);
        }
    }
    // A Rust `impl` names its type, which the same file usually declares
    // and a sibling file sometimes does (issue #126): the same file first,
    // then the directory, as Go's package, and only a unique match.
    let mut rust_types: BTreeMap<(usize, String), Vec<usize>> = BTreeMap::new();
    let mut rust_dir_types: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, span) in spans.iter().enumerate() {
        if nodes[span.file].lang == "rs" && matches!(span.kind, CLASS | INTERFACE | TYPE) {
            rust_types
                .entry((span.file, span.name.clone()))
                .or_default()
                .push(i);
            let directory = nodes[span.file]
                .file
                .rsplit_once('/')
                .map_or("", |v| v.0)
                .to_owned();
            rust_dir_types
                .entry((directory, span.name.clone()))
                .or_default()
                .push(i);
        }
    }
    for (method, type_name) in go_receivers {
        let file = spans[method].file;
        let directory = nodes[file].file.rsplit_once('/').map_or("", |v| v.0);
        if nodes[file].lang == "rs" {
            let owner = match rust_types.get(&(file, type_name.clone())) {
                Some(ids) => (ids.len() == 1).then(|| ids[0]),
                None => rust_dir_types
                    .get(&(directory.to_owned(), type_name))
                    .filter(|ids| ids.len() == 1)
                    .map(|ids| ids[0]),
            };
            if let Some(owner) = owner {
                spans[method].parent = owner as isize;
            }
            continue;
        }
        if let Some(ids) = go_types.get(&(directory.to_owned(), type_name)) {
            if ids.len() == 1 {
                spans[method].parent = ids[0] as isize;
            }
        }
    }
    let mut tops = vec![BTreeMap::new(); nodes.len()];
    let mut members: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut declarations: BTreeMap<(usize, isize, String), Vec<usize>> = BTreeMap::new();
    for (i, span) in spans.iter().enumerate() {
        declarations
            .entry((span.file, span.parent, span.name.clone()))
            .or_default()
            .push(i);
    }
    // Multiple declarations with the same lexical name may be guarded by
    // branches or overwritten later. Without execution order, neither is
    // a certain target, so they remain in the hierarchy but resolve no edge.
    for ((file, parent, name), ids) in declarations {
        if ids.len() != 1 {
            continue;
        }
        if parent < 0 {
            tops[file].insert(name, ids[0]);
        } else {
            members
                .entry(parent as usize)
                .or_default()
                .insert(name, ids[0]);
        }
    }
    let mut bases = BTreeMap::<usize, Vec<MroItem>>::new();
    let no_mros = BTreeMap::new();
    let mut unknown_base = 0;
    let mut seen_bases = BTreeSet::new();
    for info in &infos {
        for candidate in info
            .candidates
            .iter()
            .filter(|candidate| candidate.kind == EXTENDS)
        {
            if !matches!(spans[candidate.owner].kind, CLASS | INTERFACE) {
                continue;
            }
            if !seen_bases.insert((candidate.owner, candidate.chain.clone())) {
                continue;
            }
            let resolved = resolve(
                candidate, &spans, &infos, &tops, &modules, &packages, &members, &no_mros,
            )
            .ok()
            .map(|(target, _)| target)
            .filter(|&target| {
                matches!(spans[target].kind, CLASS | INTERFACE)
                    && nodes[spans[target].file].lang == nodes[spans[candidate.owner].file].lang
            });
            let item = resolved.map_or_else(
                || {
                    unknown_base += 1;
                    MroItem::Unknown(unknown_base)
                },
                MroItem::Known,
            );
            bases.entry(candidate.owner).or_default().push(item);
        }
    }
    let mut mros = BTreeMap::new();
    for (i, span) in spans.iter().enumerate() {
        if matches!(span.kind, CLASS | INTERFACE) {
            let mro = class_mro(i, &bases, &mut mros, &mut BTreeSet::new());
            mros.insert(i, mro);
        }
    }
    let mut edges = BTreeMap::<(usize, usize, usize), usize>::new();
    let mut coverage = SymbolCoverage::default();
    for info in &infos {
        for candidate in &info.candidates {
            if candidate.call {
                coverage.calls_total += 1;
            }
            match resolve(
                candidate, &spans, &infos, &tops, &modules, &packages, &members, &mros,
            ) {
                Ok((target, inherited)) => {
                    if candidate.call {
                        coverage.calls_resolved += 1;
                        if inherited {
                            coverage.inherited_calls_resolved += 1;
                        }
                    }
                    if candidate.value
                        && !matches!(spans[target].kind, FUNCTION | METHOD | NESTED_FUNCTION)
                    {
                        continue;
                    }
                    let mut p = Some(candidate.owner);
                    let ancestor = loop {
                        match p {
                            Some(i) if i == target => break true,
                            Some(i) if spans[i].parent >= 0 => p = Some(spans[i].parent as usize),
                            _ => break false,
                        }
                    };
                    if !ancestor {
                        *edges
                            .entry((candidate.owner, target, candidate.kind))
                            .or_default() += 1;
                    }
                }
                Err(reason) => {
                    if candidate.call {
                        *coverage.unresolved.entry(reason.to_owned()).or_default() += 1;
                    }
                }
            }
        }
    }
    for (i, span) in spans.iter().enumerate() {
        if span.kind != METHOD || span.parent < 0 || nodes[span.file].lang == "go" {
            continue;
        }
        let class = span.parent as usize;
        if !matches!(spans[class].kind, CLASS | INTERFACE) {
            continue;
        }
        if let Some(base_method) = inherited_member(class, &span.name, &mros, &members) {
            *edges.entry((i, base_method, OVERRIDES)).or_default() += 1;
        }
    }
    // Go's implicit implementation is reported separately because matching
    // names and arity cannot establish parameter/result type identity.
    let go_structs = spans
        .iter()
        .enumerate()
        .filter(|(_, span)| span.kind == CLASS && nodes[span.file].lang == "go")
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let go_interfaces = spans
        .iter()
        .enumerate()
        .filter(|(_, span)| {
            span.kind == INTERFACE && nodes[span.file].lang == "go" && !span.go_embeds
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    for &structure in &go_structs {
        for &interface in &go_interfaces {
            let required = members
                .get(&interface)
                .into_iter()
                .flat_map(|m| m.values())
                .filter(|&&member| spans[member].kind == METHOD)
                .collect::<Vec<_>>();
            if required.is_empty()
                || required.iter().any(|&&member| {
                    members
                        .get(&structure)
                        .and_then(|m| m.get(&spans[member].name))
                        .is_none_or(|&found| {
                            spans[found].go_signature != spans[member].go_signature
                                || spans[found].go_signature.is_none()
                        })
                })
            {
                continue;
            }
            *edges
                .entry((structure, interface, POSSIBLE_IMPLEMENTATION))
                .or_default() += 1;
            coverage.possible_implementations += 1;
        }
    }
    // The traversal order is source order for each file. The explicit sort
    // also fixes equal-start symbols and keeps indices deterministic.
    let mut order = (0..spans.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        spans[a]
            .file
            .cmp(&spans[b].file)
            .then_with(|| spans[a].start.cmp(&spans[b].start))
            .then_with(|| spans[b].end.cmp(&spans[a].end))
            .then_with(|| spans[a].name.cmp(&spans[b].name))
    });
    let mut index = vec![0; spans.len()];
    for (new, old) in order.iter().enumerate() {
        index[*old] = new;
    }
    let symbols: Vec<HierSymbolRow> = order
        .into_iter()
        .map(|old| {
            let s = &spans[old];
            HierSymbolRow((
                s.file,
                s.name.clone(),
                s.kind,
                s.start,
                s.end,
                if s.parent < 0 {
                    -1
                } else {
                    index[s.parent as usize] as isize
                },
                s.code_lines,
                s.abstract_symbol,
            ))
        })
        .collect();
    let mut edges = edges
        .into_iter()
        .map(|((a, b, kind), count)| ((index[a], index[b], kind), count))
        .collect::<BTreeMap<_, _>>();
    let mut kinds = symbol_edge_kinds();
    if let Some(scip) = spool.scip.take().filter(|scip| !scip.languages.is_empty()) {
        apply_scip(&symbols, nodes, &scip, &mut edges);
        coverage.possible_implementations = edges
            .keys()
            .filter(|(_, _, kind)| *kind == POSSIBLE_IMPLEMENTATION)
            .count();
        kinds.push("reference".to_owned());
    }
    let mut edges = edges
        .into_iter()
        .map(|((a, b, kind), count)| [a, b, count, kind])
        .collect::<Vec<_>>();
    edges.sort_unstable();
    Ok(SymbolsDocument {
        files: (0..nodes.len()).collect(),
        symbols,
        edges,
        kinds,
        module_code_lines,
        coverage,
        symbol_rings: None,
        module_rings: None,
        header_rings: None,
    })
}

/// The innermost symbol span containing a 1-based line of a file, over the
/// finished symbols document -- P0's `Spans` (eval/scip_ingest.py) exactly:
/// per file, a line -> symbol table filled largest span first (then by
/// depth, then by index), so a nested symbol overwrites its container.
struct Innermost<'a> {
    symbols: &'a [HierSymbolRow],
    by_file: BTreeMap<usize, Vec<usize>>,
    depth: Vec<usize>,
    tables: BTreeMap<usize, Vec<isize>>,
}

impl<'a> Innermost<'a> {
    fn new(symbols: &'a [HierSymbolRow]) -> Self {
        let mut by_file = BTreeMap::<usize, Vec<usize>>::new();
        let mut depth = vec![0; symbols.len()];
        for (i, row) in symbols.iter().enumerate() {
            by_file.entry(row.0 .0).or_default().push(i);
            let mut parent = row.0 .5;
            while parent >= 0 {
                depth[i] += 1;
                parent = symbols[parent as usize].0 .5;
            }
        }
        Self {
            symbols,
            by_file,
            depth,
            tables: BTreeMap::new(),
        }
    }

    fn at(&mut self, file: usize, line: usize) -> Option<usize> {
        if !self.tables.contains_key(&file) {
            let ids = self.by_file.get(&file)?;
            let last = ids.iter().map(|&i| self.symbols[i].0 .4).max().unwrap_or(0);
            let mut table = vec![-1isize; last + 2];
            let mut order = ids.clone();
            order.sort_by_key(|&i| {
                let row = &self.symbols[i].0;
                (Reverse(row.4.saturating_sub(row.3)), self.depth[i], i)
            });
            for i in order {
                let row = &self.symbols[i].0;
                if row.3 > row.4 {
                    continue;
                }
                for slot in &mut table[row.3..=row.4] {
                    *slot = i as isize;
                }
            }
            self.tables.insert(file, table);
        }
        let table = &self.tables[&file];
        table
            .get(line)
            .copied()
            .filter(|&owner| owner >= 0)
            .map(|owner| owner as usize)
    }

    fn is_self_or_ancestor(&self, target: usize, owner: usize) -> bool {
        let mut current = owner as isize;
        while current >= 0 {
            if current as usize == target {
                return true;
            }
            current = self.symbols[current as usize].0 .5;
        }
        false
    }
}

fn is_type(kind: usize) -> bool {
    matches!(kind, CLASS | INTERFACE | TYPE)
}

fn is_callable(kind: usize) -> bool {
    matches!(kind, FUNCTION | METHOD | NESTED_FUNCTION)
}

/// `--refs scip`: replace the hand-written reference edges of every symbol
/// in a SCIP-path language with SCIP's, in the finished symbols document's
/// index space (`edges` is keyed `(source, target, kind)`).
///
/// - **References** (call, annotation, decorator, value): the hand-written
///   rows whose source is in a SCIP language are dropped, and every SCIP
///   occurrence is credited as P0 credits it -- source and target are the
///   innermost spans containing the reference line and the definition line;
///   module-level references (no enclosing span) and references to the
///   enclosing symbol or one of its ancestors are not credited. A pair keeps
///   the hand-written kind when the tree-sitter pass resolved the same pair
///   (the smallest kind index if several: call before annotation), since
///   only syntax knows which it was; otherwise it is `reference`. Counts are
///   SCIP's occurrences. A hand pair SCIP does not confirm is dropped: SCIP
///   is the type checker's resolution, and finding 41 confirmed 86.8-99.4%
///   of hand call pairs.
/// - **Inheritance**: SCIP `is_implementation` relationships are added to
///   the hand-written extends/implements/overrides rows, which stay: they
///   are resolved declarations, not guesses, and finding 41 found SCIP's
///   relationships covering 3,016 of django's 3,544 of them, not all. A
///   type -> interface relationship from a non-interface is `implements`;
///   any other type -> type relationship is `extends` (a TypeScript or Go
///   interface -> interface one included); callable -> callable is
///   `overrides`, which for Go is a method satisfying an interface method.
/// - **Go's `possible_implementation`** rows are dropped: scip-go proves
///   satisfaction exactly, so its `implements` rows replace the name-and-
///   arity candidates finding 37 warned about.
fn apply_scip(
    symbols: &[HierSymbolRow],
    nodes: &[SourceNode],
    scip: &ScipSymbolRefs,
    edges: &mut BTreeMap<(usize, usize, usize), usize>,
) {
    let on_scip = |symbol: usize| scip.languages.contains(&nodes[symbols[symbol].0 .0].lang);
    let reference_kind = |kind: usize| matches!(kind, CALL | ANNOTATION | DECORATOR | VALUE);
    let mut hand_kind = BTreeMap::<(usize, usize), usize>::new();
    for &(a, b, kind) in edges.keys() {
        if reference_kind(kind) && on_scip(a) {
            hand_kind
                .entry((a, b))
                .and_modify(|known| *known = (*known).min(kind))
                .or_insert(kind);
        }
    }
    edges.retain(|&(a, _, kind), _| {
        !(on_scip(a) && (reference_kind(kind) || kind == POSSIBLE_IMPLEMENTATION))
    });

    let file_of = nodes
        .iter()
        .enumerate()
        .map(|(fi, node)| (node.file.as_str(), fi))
        .collect::<BTreeMap<_, _>>();
    let remap = scip
        .files
        .iter()
        .map(|file| file_of.get(file.as_str()).copied())
        .collect::<Vec<_>>();
    let mut spans = Innermost::new(symbols);
    let mut credited = BTreeMap::<(usize, usize), usize>::new();
    for &([from_file, from_line, to_file, to_line], count) in &scip.refs {
        let (Some(from), Some(to)) = (remap[from_file as usize], remap[to_file as usize]) else {
            continue;
        };
        let Some(owner) = spans.at(from, from_line as usize) else {
            continue;
        };
        let Some(target) = spans.at(to, to_line as usize) else {
            continue;
        };
        if !on_scip(owner) || spans.is_self_or_ancestor(target, owner) {
            continue;
        }
        *credited.entry((owner, target)).or_default() += count as usize;
    }
    for ((owner, target), count) in credited {
        let kind = hand_kind
            .get(&(owner, target))
            .copied()
            .unwrap_or(REFERENCE);
        *edges.entry((owner, target, kind)).or_default() += count;
    }

    for &[source_file, source_line, target_file, target_line] in &scip.implementations {
        let (Some(source_fi), Some(target_fi)) =
            (remap[source_file as usize], remap[target_file as usize])
        else {
            continue;
        };
        let (Some(source), Some(target)) = (
            spans.at(source_fi, source_line as usize),
            spans.at(target_fi, target_line as usize),
        ) else {
            continue;
        };
        if source == target || !on_scip(source) {
            continue;
        }
        let (source_kind, target_kind) = (symbols[source].0 .2, symbols[target].0 .2);
        let kind = if is_type(source_kind) && is_type(target_kind) {
            if target_kind == INTERFACE && source_kind != INTERFACE {
                IMPLEMENTS
            } else {
                EXTENDS
            }
        } else if is_callable(source_kind) && is_callable(target_kind) {
            OVERRIDES
        } else {
            continue;
        };
        edges.entry((source, target, kind)).or_insert(1);
    }
}

pub(crate) fn write_sibling(
    repo: &Path,
    nodes: &[SourceNode],
    map_path: &Path,
    spool: SymbolSpool,
) -> Result<()> {
    write_sibling_with_progress(
        repo,
        nodes,
        map_path,
        spool,
        &crate::progress::Progress::silent(),
    )
}

pub(crate) fn write_sibling_with_progress(
    repo: &Path,
    nodes: &[SourceNode],
    map_path: &Path,
    spool: SymbolSpool,
    progress: &crate::progress::Progress,
) -> Result<()> {
    let map: MapDocument = serde_json::from_slice(&fs::read(map_path)?)?;
    ensure!(
        map.files.len() == nodes.len()
            && map
                .files
                .iter()
                .zip(nodes)
                .all(|(file, node)| file == &node.file),
        "symbol source file order differs from map F order"
    );
    let symbols_stage = progress.stage(crate::progress::StageId::Symbols, Some(nodes.len() as u64));
    let mut document = build_with_progress(repo, nodes, spool, Some(&symbols_stage))?;
    symbols_stage.set(nodes.len() as u64);
    symbols_stage.finish();
    let cards_stage = progress.stage(
        crate::progress::StageId::SymbolCards,
        Some(nodes.len() as u64),
    );
    crate::symbol_cards::attach_with_progress(&map, &mut document, Some(&cards_stage))?;
    cards_stage.set(nodes.len() as u64);
    cards_stage.finish();
    let output = map_path.with_extension("symbols.json");
    let write_stage = progress.stage(crate::progress::StageId::Write, None);
    let temporary = map_path.with_extension("symbols.json.tmp");
    let district_dir = map_path.with_extension("symbols");
    let temporary_dir = map_path.with_extension("symbols.tmp");
    if temporary_dir.exists() {
        fs::remove_dir_all(&temporary_dir)?;
    }
    fs::create_dir(&temporary_dir)?;
    for key in map.districts.keys() {
        let id = key
            .parse::<usize>()
            .with_context(|| format!("invalid district {key}"))?;
        let response = document
            .district(&map, id)
            .with_context(|| format!("missing district {id}"))?;
        let bytes = serde_json::to_vec(&response)?;
        let byte_count = bytes.len() as u64;
        fs::write(temporary_dir.join(format!("{id}.json")), bytes)?;
        write_stage.advance(byte_count);
    }
    let bytes = serde_json::to_vec(&document)?;
    let byte_count = bytes.len() as u64;
    fs::write(&temporary, bytes)?;
    write_stage.advance(byte_count);
    fs::rename(&temporary, &output)?;
    if district_dir.exists() {
        fs::remove_dir_all(&district_dir)?;
    }
    fs::rename(&temporary_dir, &district_dir)?;
    write_stage.finish();
    Ok(())
}

impl SymbolsDocument {
    pub fn district(&self, map: &MapDocument, district: usize) -> Option<DistrictSymbols> {
        if !map.districts.contains_key(&district.to_string()) {
            return None;
        }
        let files = map
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| (n.district() == district).then_some(i))
            .collect::<Vec<_>>();
        let file_set = files.iter().copied().collect::<BTreeSet<_>>();
        let mut symbol_indices = self
            .symbols
            .iter()
            .enumerate()
            .filter_map(|(i, s)| file_set.contains(&s.0 .0).then_some(i))
            .collect::<BTreeSet<_>>();
        let edges = self
            .edges
            .iter()
            .filter(|e| symbol_indices.contains(&e[0]) || symbol_indices.contains(&e[1]))
            .copied()
            .collect::<Vec<_>>();
        for e in &edges {
            symbol_indices.insert(e[0]);
            symbol_indices.insert(e[1]);
        }
        let symbol_indices = symbol_indices.into_iter().collect::<Vec<_>>();
        let symbols = symbol_indices
            .iter()
            .map(|&i| self.symbols[i].clone())
            .collect();
        let module_code_lines = self
            .module_code_lines
            .iter()
            .filter(|(fi, _)| file_set.contains(fi))
            .map(|(fi, count)| (*fi, *count))
            .collect();
        let symbol_rings = self
            .symbol_rings
            .as_ref()
            .map(|rings| symbol_indices.iter().map(|&i| rings[i].clone()).collect());
        let module_rings = self.module_rings.as_ref().map(|rings| {
            rings
                .iter()
                .filter(|(fi, _)| file_set.contains(fi))
                .map(|(fi, ring)| (*fi, ring.clone()))
                .collect()
        });
        let header_rings = self.header_rings.as_ref().map(|rings| {
            rings
                .iter()
                .filter(|(i, _)| symbol_indices.binary_search(i).is_ok())
                .map(|(i, ring)| (*i, ring.clone()))
                .collect()
        });
        Some(DistrictSymbols {
            district,
            files,
            symbol_indices,
            symbols,
            edges,
            kinds: self.kinds.clone(),
            module_code_lines,
            symbol_rings,
            module_rings,
            header_rings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn source(file: &str, module: &str, lang: &str) -> SourceNode {
        SourceNode {
            file: file.to_owned(),
            module: module.to_owned(),
            lang: lang.to_owned(),
            loc: 0,
            code_lines: None,
            complexity: 0,
            churn: 0,
            fanin: 0.0,
        }
    }

    fn fixture(files: &[(&str, &str, &str)]) -> (tempfile::TempDir, Vec<SourceNode>) {
        let dir = tempfile::tempdir().unwrap();
        let mut nodes = Vec::new();
        for (file, module, contents) in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
            nodes.push(source(file, module, "py"));
        }
        (dir, nodes)
    }

    fn id(doc: &SymbolsDocument, file: usize, name: &str) -> usize {
        doc.symbols
            .iter()
            .position(|s| s.0 .0 == file && s.0 .1 == name)
            .unwrap()
    }

    fn edge(doc: &SymbolsDocument, from: usize, to: usize) -> bool {
        doc.edges.iter().any(|e| e[0] == from && e[1] == to)
    }

    fn typed_edge(doc: &SymbolsDocument, from: usize, to: usize, kind: usize) -> bool {
        doc.edges
            .iter()
            .any(|e| e[0] == from && e[1] == to && e[3] == kind)
    }

    fn build_fixture(repo: &Path, nodes: &[SourceNode]) -> SymbolsDocument {
        let mut spool = SymbolSpool::new().unwrap();
        let mut parser = Parser::new();
        for node in nodes {
            let lang = LanguageKind::parse(&node.lang).unwrap();
            let grammar = match lang {
                LanguageKind::Python => tree_sitter_python::LANGUAGE.into(),
                LanguageKind::Go => tree_sitter_go::LANGUAGE.into(),
                LanguageKind::TypeScript if node.file.ends_with(".tsx") => {
                    tree_sitter_typescript::LANGUAGE_TSX.into()
                }
                LanguageKind::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                LanguageKind::Rust => tree_sitter_rust::LANGUAGE.into(),
            };
            parser.set_language(&grammar).unwrap();
            let bytes = fs::read(repo.join(&node.file)).unwrap();
            let tree = parser.parse(&bytes, None).unwrap();
            let root = tree.root_node();
            let flags = extract::code_line_flags(root, &bytes, lang);
            spool
                .insert(&node.file, &collect(root, &bytes, lang, &flags))
                .unwrap();
        }
        build(repo, nodes, spool).unwrap()
    }

    #[test]
    fn owner_lookup_matches_narrowest_containing_span() {
        let bytes = b"@wrap\nclass Outer:\n    def a(self):\n        def nested():\n            pass\n        nested()\n    def b(self):\n        pass\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(bytes, None).unwrap();
        let root = tree.root_node();
        let mut spans = Vec::new();
        collect_spans(root, bytes, LanguageKind::Python, 0, 0, &mut spans);
        let mut lookup = OwnerLookup::new(&spans);
        fn check(node: Node<'_>, spans: &[Span], lookup: &mut OwnerLookup<'_>) {
            let byte = node.start_byte();
            let expected = spans
                .iter()
                .enumerate()
                .filter(|(_, span)| span.credit_begin_byte <= byte && byte < span.end_byte)
                .min_by_key(|(_, span)| span.end_byte - span.begin_byte)
                .map(|(i, _)| i);
            assert_eq!(lookup.at(byte), expected, "{} at byte {byte}", node.kind());
            for child in children(node) {
                check(child, spans, lookup);
            }
        }
        check(root, &spans, &mut lookup);
    }

    #[test]
    fn complete_hierarchy_keeps_dunders_and_more_than_sixty() {
        let mut body = "class Outer:\n    def __init__(self):\n        def inner():\n            pass\n    class Nested:\n        def work(self):\n            pass\n".to_owned();
        for n in 0..65 {
            body.push_str(&format!("\ndef f{n}():\n    pass\n"));
        }
        let (dir, nodes) = fixture(&[("mod.py", "mod", &body)]);
        let doc = build_fixture(dir.path(), &nodes);
        assert!(doc.symbols.len() > 60);
        let outer = id(&doc, 0, "Outer");
        let init = id(&doc, 0, "__init__");
        let inner = id(&doc, 0, "inner");
        let nested = id(&doc, 0, "Nested");
        let work = id(&doc, 0, "work");
        assert_eq!(doc.symbols[init].0 .5, outer as isize);
        assert_eq!(doc.symbols[init].0 .2, METHOD);
        assert_eq!(doc.symbols[inner].0 .5, init as isize);
        assert_eq!(doc.symbols[inner].0 .2, NESTED_FUNCTION);
        assert_eq!(doc.symbols[nested].0 .5, outer as isize);
        assert_eq!(doc.symbols[work].0 .5, nested as isize);
        assert_eq!(doc.symbols[work].0 .2, METHOD);
        assert_eq!(doc.symbols[outer].0 .3, 1);
    }

    #[test]
    fn resolves_import_reexport_module_class_self_and_excludes_ancestors() {
        let (dir, nodes) = fixture(&[
            ("pkg/__init__.py", "pkg", "from .core import Target\n"),
            ("pkg/core.py", "pkg.core", "class Target:\n    def method(self):\n        pass\n"),
            ("caller.py", "caller", "from pkg import Target\nimport pkg.core as core\n\nclass Caller(Target):\n    def method(self):\n        self.helper()\n        Target.method()\n        core.Target.method()\n        super().method()\n        Target()\n    def helper(self):\n        pass\n\ndef outer():\n    def inner():\n        outer()\n    inner()\n\ndef typed(x: Target) -> Target:\n    return x\n\n@Target\ndef decorated():\n    pass\n"),
        ]);
        let doc = build_fixture(dir.path(), &nodes);
        let target = id(&doc, 1, "Target");
        let target_method = id(&doc, 1, "method");
        let caller = id(&doc, 2, "Caller");
        let method = id(&doc, 2, "method");
        let helper = id(&doc, 2, "helper");
        let outer = id(&doc, 2, "outer");
        let inner = id(&doc, 2, "inner");
        assert!(edge(&doc, caller, target));
        assert!(edge(&doc, method, target_method));
        assert!(edge(&doc, method, helper));
        assert!(edge(&doc, method, target));
        assert!(edge(&doc, outer, inner));
        assert!(!edge(&doc, inner, outer));
        assert!(edge(&doc, id(&doc, 2, "typed"), target));
        assert!(edge(&doc, id(&doc, 2, "decorated"), target));
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == id(&doc, 2, "typed") && e[1] == target)
                .unwrap()[2],
            2
        );
        assert!(doc.coverage.calls_resolved > 0);
        assert!(typed_edge(&doc, caller, target, EXTENDS));
        assert!(typed_edge(&doc, method, target_method, CALL));
        assert!(typed_edge(&doc, method, target_method, OVERRIDES));
        assert!(doc.coverage.inherited_calls_resolved >= 1);
    }

    #[test]
    fn python_method_values_count_conditional_callbacks_and_exact_qualified_names() {
        let (dir, nodes) = fixture(&[
            ("helper.py", "helper", "def callback():\n    pass\n"),
            (
                "handler.py",
                "handler",
                "import helper\nfrom functools import partial\n\nclass Handler:\n    def load_middleware(self, is_async):\n        get_response = self._get_response_async if is_async else self._get_response\n        register(self._get_response)\n        partial(self._get_response_async)\n        other = Handler._get_response\n        imported = helper.callback\n    def store(self):\n        self._get_response = other\n    def _get_response(self):\n        pass\n    def _get_response_async(self):\n        pass\n",
            ),
        ]);
        let doc = build_fixture(dir.path(), &nodes);
        let load = id(&doc, 1, "load_middleware");
        let sync = id(&doc, 1, "_get_response");
        let asynchronous = id(&doc, 1, "_get_response_async");
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == load && e[1] == sync)
                .unwrap()[2],
            3
        );
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == load && e[1] == asynchronous)
                .unwrap()[2],
            2
        );
        assert!(edge(&doc, load, id(&doc, 0, "callback")));
        assert!(!edge(&doc, id(&doc, 1, "store"), sync));
    }

    #[test]
    fn python_diamond_uses_c3_and_external_base_blocks_later_methods() {
        let (dir, nodes) = fixture(&[(
            "m.py", "m",
            "class A:\n    def hit(self): pass\nclass B(A):\n    def hit(self): pass\nclass C(A):\n    def hit(self): pass\nclass D(B, C):\n    def use(self):\n        self.hit()\n        super().hit()\nclass Blocked(External, B):\n    def use(self): self.hit()\nclass Safe(B, External):\n    def use(self): self.hit()\n",
        )]);
        let doc = build_fixture(dir.path(), &nodes);
        let b = id(&doc, 0, "B");
        let b_hit = doc
            .symbols
            .iter()
            .position(|s| s.0 .1 == "hit" && s.0 .5 == b as isize)
            .unwrap();
        let d = id(&doc, 0, "D");
        let d_use = doc
            .symbols
            .iter()
            .position(|s| s.0 .1 == "use" && s.0 .5 == d as isize)
            .unwrap();
        let blocked = id(&doc, 0, "Blocked");
        let blocked_use = doc
            .symbols
            .iter()
            .position(|s| s.0 .1 == "use" && s.0 .5 == blocked as isize)
            .unwrap();
        let safe = id(&doc, 0, "Safe");
        let safe_use = doc
            .symbols
            .iter()
            .position(|s| s.0 .1 == "use" && s.0 .5 == safe as isize)
            .unwrap();
        assert!(typed_edge(&doc, d_use, b_hit, CALL));
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == d_use && e[1] == b_hit && e[3] == CALL)
                .unwrap()[2],
            2
        );
        assert!(!typed_edge(&doc, blocked_use, b_hit, CALL));
        assert!(typed_edge(&doc, safe_use, b_hit, CALL));
        assert!(doc.coverage.unresolved["instance_or_untyped"] >= 1);
        assert_eq!(doc.coverage.inherited_calls_resolved, 3);
    }

    #[test]
    fn python_and_typescript_abstract_inheritance_and_implements() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.py"), "from abc import ABC, abstractmethod\nclass Base(ABC):\n    @abstractmethod\n    def work(self): pass\nclass Child(Base):\n    def work(self): pass\n    def use(self): super().work()\nclass Meta(metaclass=ABCMeta): pass\n").unwrap();
        fs::write(dir.path().join("a.ts"), "interface I { run(): void; value: number }\nabstract class Base { abstract run(): void; base() {} }\nclass Child extends Base implements I { value = 1; run() {} use() { this.base(); super.base(); } }\n").unwrap();
        let nodes = vec![source("a.py", "a", "py"), source("a.ts", "a.ts", "ts")];
        let doc = build_fixture(dir.path(), &nodes);
        let py_base = id(&doc, 0, "Base");
        let py_meta = id(&doc, 0, "Meta");
        let ts_i = id(&doc, 1, "I");
        let ts_base = id(&doc, 1, "Base");
        let ts_child = id(&doc, 1, "Child");
        assert!(doc.symbols[py_base].0 .7 && doc.symbols[py_meta].0 .7);
        assert!(doc.symbols[ts_i].0 .7 && doc.symbols[ts_base].0 .7);
        assert!(doc
            .symbols
            .iter()
            .any(|s| s.0 .1 == "work" && s.0 .5 == py_base as isize && s.0 .7));
        assert!(doc
            .symbols
            .iter()
            .any(|s| s.0 .1 == "run" && s.0 .5 == ts_base as isize && s.0 .7));
        assert!(typed_edge(&doc, ts_child, ts_base, EXTENDS));
        assert!(typed_edge(&doc, ts_child, ts_i, IMPLEMENTS));
        let ts_base_method = doc
            .symbols
            .iter()
            .position(|s| s.0 .1 == "base" && s.0 .5 == ts_base as isize)
            .unwrap();
        let ts_use = doc
            .symbols
            .iter()
            .position(|s| s.0 .1 == "use" && s.0 .5 == ts_child as isize)
            .unwrap();
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == ts_use && e[1] == ts_base_method && e[3] == CALL)
                .unwrap()[2],
            2
        );
        assert!(doc
            .symbols
            .iter()
            .any(|s| s.0 .1 == "run" && s.0 .5 == ts_i as isize && s.0 .7));
    }

    #[test]
    fn go_possible_implementation_requires_every_method_and_arity() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.go"), "package p\ntype Need interface { Run(x int) int; Stop() }\ntype Good struct{}\nfunc (Good) Run(x int) int { return x }\nfunc (Good) Stop() {}\ntype Missing struct{}\nfunc (Missing) Run(x int) int { return x }\ntype Wrong struct{}\nfunc (Wrong) Run() int { return 0 }\nfunc (Wrong) Stop() {}\n").unwrap();
        let doc = build_fixture(dir.path(), &[source("a.go", "a.go", "go")]);
        let need = id(&doc, 0, "Need");
        assert!(typed_edge(
            &doc,
            id(&doc, 0, "Good"),
            need,
            POSSIBLE_IMPLEMENTATION
        ));
        assert!(!typed_edge(
            &doc,
            id(&doc, 0, "Missing"),
            need,
            POSSIBLE_IMPLEMENTATION
        ));
        assert!(!typed_edge(
            &doc,
            id(&doc, 0, "Wrong"),
            need,
            POSSIBLE_IMPLEMENTATION
        ));
        assert_eq!(doc.coverage.possible_implementations, 1);
    }

    #[test]
    fn old_symbol_rows_and_edges_load_as_nonabstract_and_unknown() {
        let doc: SymbolsDocument = serde_json::from_value(serde_json::json!({
            "files": [0],
            "symbols": [[0, "f", 1, 1, 2, -1, 2]],
            "edges": [[0, 0, 1]],
            "module_code_lines": {},
            "coverage": {"calls_total": 0, "calls_resolved": 0, "unresolved": {}}
        }))
        .unwrap();
        assert!(!doc.symbols[0].0 .7);
        assert_eq!(doc.edges, vec![[0, 0, 1, 0]]);
        assert_eq!(doc.kinds[0], "unknown");
    }

    #[test]
    fn typescript_and_go_values_use_only_existing_type_facts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.ts"),
            "class Box { method() {} use() { const cb = this.method; register(this.method); this.method = cb; } }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a.go"),
            "package p\ntype Thing struct{}\nfunc (t *Thing) Work() {}\nfunc Use(x Thing) { _ = Thing.Work; _ = x.Work }\n",
        )
        .unwrap();
        let nodes = vec![source("a.ts", "a.ts", "ts"), source("a.go", "a.go", "go")];
        let doc = build_fixture(dir.path(), &nodes);
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == id(&doc, 0, "use") && e[1] == id(&doc, 0, "method"))
                .unwrap()[2],
            2
        );
        assert_eq!(
            doc.edges
                .iter()
                .find(|e| e[0] == id(&doc, 1, "Use") && e[1] == id(&doc, 1, "Work"))
                .unwrap()[2],
            1
        );
    }

    #[test]
    fn module_lines_and_unknown_calls_are_lower_bounds() {
        let (dir, nodes) = fixture(&[("m.py", "m", "# comment\nVALUE = 1\n\ndef known():\n    \"\"\"doc\"\"\"\n    missing()\n    (factory())()\n")]);
        let doc = build_fixture(dir.path(), &nodes);
        assert_eq!(doc.module_code_lines[&0], 1);
        assert!(doc.coverage.calls_total >= 3);
        assert_eq!(doc.coverage.calls_resolved, 0);
        assert!(!doc.coverage.unresolved.is_empty());
        assert_eq!(doc.symbols[id(&doc, 0, "known")].0 .6, 3);
    }

    #[test]
    fn follows_four_reexport_hops_but_no_further() {
        let (dir, nodes) = fixture(&[
            ("p/__init__.py", "p", "from .a import X\n"),
            ("p/a.py", "p.a", "from .b import X\n"),
            ("p/b.py", "p.b", "from .c import X\n"),
            ("p/c.py", "p.c", "class X:\n    pass\n"),
            ("use.py", "use", "from p import X\ndef f():\n    X()\n"),
        ]);
        let doc = build_fixture(dir.path(), &nodes);
        assert!(edge(&doc, id(&doc, 4, "f"), id(&doc, 3, "X")));
    }

    #[test]
    fn go_and_typescript_have_untruncated_hierarchies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.go"),
            "package p\ntype Thing struct{}\nfunc (t *Thing) Work() {}\nfunc Free() {}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("a.ts"), "class Box { method() { function nested() {} } }\ninterface Shape {}\ntype Name = string;\nconst f = () => 1;\n").unwrap();
        let nodes = vec![source("a.go", "a.go", "go"), source("a.ts", "a.ts", "ts")];
        let doc = build_fixture(dir.path(), &nodes);
        assert_eq!(doc.symbols[id(&doc, 0, "Thing")].0 .2, CLASS);
        assert_eq!(doc.symbols[id(&doc, 0, "Work")].0 .2, METHOD);
        assert_eq!(
            doc.symbols[id(&doc, 0, "Work")].0 .5,
            id(&doc, 0, "Thing") as isize
        );
        assert_eq!(
            doc.symbols[id(&doc, 1, "method")].0 .5,
            id(&doc, 1, "Box") as isize
        );
        assert_eq!(
            doc.symbols[id(&doc, 1, "nested")].0 .5,
            id(&doc, 1, "method") as isize
        );
        assert_eq!(doc.symbols[id(&doc, 1, "Shape")].0 .2, INTERFACE);
    }

    #[test]
    fn rust_impl_methods_join_their_type_and_paths_resolve_through_modules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "mod store;\npub use store::Store;\n\npub trait Shape {\n    fn area(&self) -> u32;\n    fn twice(&self) -> u32 {\n        self.area() * 2\n    }\n}\n\npub fn build() -> Store {\n    store::make();\n    crate::store::make();\n    Store::open()\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/store.rs"),
            "pub struct Store;\n\nimpl Store {\n    pub fn open() -> Store {\n        Self::close();\n        Store\n    }\n\n    fn close() {}\n}\n\npub fn make() {}\n",
        )
        .unwrap();
        let nodes = vec![
            source("src/lib.rs", "demo", "rs"),
            source("src/store.rs", "demo::store", "rs"),
        ];
        let doc = build_fixture(dir.path(), &nodes);
        let store = id(&doc, 1, "Store");
        let open = id(&doc, 1, "open");
        let close = id(&doc, 1, "close");
        assert_eq!(doc.symbols[store].0 .2, CLASS);
        assert_eq!(doc.symbols[open].0 .2, METHOD);
        assert_eq!(doc.symbols[open].0 .5, store as isize);
        assert_eq!(doc.symbols[close].0 .5, store as isize);
        let shape = id(&doc, 0, "Shape");
        let area = id(&doc, 0, "area");
        let twice = id(&doc, 0, "twice");
        assert_eq!(doc.symbols[shape].0 .2, INTERFACE);
        assert!(doc.symbols[shape].0 .7);
        assert_eq!(doc.symbols[area].0 .2, METHOD);
        assert_eq!(doc.symbols[area].0 .5, shape as isize);
        assert!(
            doc.symbols[area].0 .7,
            "a trait method with no body is abstract"
        );
        assert!(!doc.symbols[twice].0 .7);
        let build = id(&doc, 0, "build");
        let make = id(&doc, 1, "make");
        assert!(typed_edge(&doc, build, make, CALL));
        assert!(typed_edge(&doc, build, open, CALL));
        assert!(typed_edge(&doc, open, close, CALL));
    }

    #[test]
    fn resolves_go_package_and_typescript_import_alias() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("p")).unwrap();
        std::fs::write(dir.path().join("go.mod"), "module example.com/repo\n").unwrap();
        std::fs::write(
            dir.path().join("p/one.go"),
            "package p\ntype Thing struct{}\nfunc Target() {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("p/two.go"),
            "package p\nfunc (t *Thing) Work() {}\nfunc Local() { Target() }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("main.go"),
            "package main\nimport \"example.com/repo/p\"\nfunc Caller() { p.Target() }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("thing.ts"),
            "export class Thing { run() {} }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("use.ts"),
            "import { Thing as Alias } from './thing.js';\nfunction use() { Alias.run(); }\n",
        )
        .unwrap();
        let nodes = vec![
            source("p/one.go", "p/one.go", "go"),
            source("p/two.go", "p/two.go", "go"),
            source("main.go", "main.go", "go"),
            source("thing.ts", "thing.ts", "ts"),
            source("use.ts", "use.ts", "ts"),
        ];
        let doc = build_fixture(dir.path(), &nodes);
        assert_eq!(
            doc.symbols[id(&doc, 1, "Work")].0 .5,
            id(&doc, 0, "Thing") as isize
        );
        assert!(edge(&doc, id(&doc, 1, "Local"), id(&doc, 0, "Target")));
        assert!(edge(&doc, id(&doc, 2, "Caller"), id(&doc, 0, "Target")));
        assert!(edge(&doc, id(&doc, 4, "use"), id(&doc, 3, "run")));
    }

    /// P0's self-test crediting (eval/scip_ingest.py `self_test`): the two
    /// calls of `f` inside `g` credit g -> f twice, the import and the
    /// module-level use of `C` credit nothing, and C's implementation of
    /// Base is an `extends` row. A hand-written pair SCIP confirms keeps its
    /// syntactic kind with SCIP's count; one SCIP does not confirm is
    /// dropped; a hand-written extends row stays.
    #[test]
    fn scip_references_credit_innermost_spans_like_the_p0_oracle() {
        let row = |file: usize, name: &str, kind: usize, start: usize, end: usize| {
            HierSymbolRow((file, name.to_owned(), kind, start, end, -1, 1, false))
        };
        let symbols = vec![
            row(0, "f", FUNCTION, 2, 3),
            row(0, "C", CLASS, 5, 8),
            row(1, "Base", CLASS, 3, 4),
            row(1, "g", FUNCTION, 6, 9),
        ];
        let nodes = ["pkg/a.py", "pkg/b.py", "pkg/c.py"]
            .map(|file| source(file, file, "py"))
            .to_vec();
        let scip = ScipSymbolRefs {
            languages: BTreeSet::from(["py".to_owned()]),
            files: nodes.iter().map(|node| node.file.clone()).collect(),
            refs: vec![([1, 1, 0, 1], 1), ([1, 7, 0, 2], 2), ([1, 10, 0, 5], 1)],
            implementations: vec![[0, 5, 1, 3]],
        };

        let mut edges = BTreeMap::new();
        apply_scip(&symbols, &nodes, &scip, &mut edges);
        assert_eq!(
            edges,
            BTreeMap::from([((1, 2, EXTENDS), 1), ((3, 0, REFERENCE), 2)])
        );

        let mut edges = BTreeMap::from([
            ((3, 0, CALL), 1),
            ((3, 0, VALUE), 1),
            ((3, 2, CALL), 5),
            ((1, 2, EXTENDS), 1),
        ]);
        apply_scip(&symbols, &nodes, &scip, &mut edges);
        assert_eq!(
            edges,
            BTreeMap::from([((1, 2, EXTENDS), 1), ((3, 0, CALL), 2)])
        );
    }

    #[test]
    fn scip_go_implements_replaces_possible_implementation() {
        let symbols = vec![
            HierSymbolRow((0, "Store".to_owned(), CLASS, 1, 3, -1, 1, false)),
            HierSymbolRow((0, "Get".to_owned(), METHOD, 5, 7, 0, 1, false)),
            HierSymbolRow((1, "Getter".to_owned(), INTERFACE, 1, 3, -1, 1, false)),
            HierSymbolRow((1, "Get".to_owned(), METHOD, 2, 2, 2, 1, false)),
        ];
        let nodes = ["store.go", "api/getter.go"]
            .map(|file| source(file, file, "go"))
            .to_vec();
        let scip = ScipSymbolRefs {
            languages: BTreeSet::from(["go".to_owned()]),
            files: nodes.iter().map(|node| node.file.clone()).collect(),
            refs: Vec::new(),
            implementations: vec![[0, 1, 1, 1], [0, 5, 1, 2]],
        };
        let mut edges = BTreeMap::from([((0, 2, POSSIBLE_IMPLEMENTATION), 1)]);
        apply_scip(&symbols, &nodes, &scip, &mut edges);
        assert_eq!(
            edges,
            BTreeMap::from([((0, 2, IMPLEMENTS), 1), ((1, 3, OVERRIDES), 1)])
        );
    }
}

/// The recursive `parent()`-based walks replaced in finding 40, kept verbatim
/// (renamed `old_*`) so a test can assert the cursor walks collect exactly
/// the same spans, receivers, imports and candidates.
#[cfg(test)]
mod parent_walk_reference {
    use super::*;
    use tree_sitter::Parser;

    fn old_kind(node: Node<'_>, lang: LanguageKind) -> Option<usize> {
        match lang {
            LanguageKind::Python => match node.kind() {
                "class_definition" => Some(CLASS),
                "function_definition" => Some(FUNCTION),
                _ => None,
            },
            LanguageKind::Go => match node.kind() {
                "function_declaration" => Some(FUNCTION),
                "method_declaration" | "method_elem" => Some(METHOD),
                "type_spec" => Some(match node.child_by_field_name("type").map(|n| n.kind()) {
                    Some("struct_type") => CLASS,
                    Some("interface_type") => INTERFACE,
                    _ => TYPE,
                }),
                _ => None,
            },
            LanguageKind::TypeScript => match node.kind() {
                "class_declaration" | "abstract_class_declaration" | "class" => Some(CLASS),
                "function_declaration" => Some(FUNCTION),
                "method_definition" | "method_signature" | "abstract_method_signature" => {
                    Some(METHOD)
                }
                "interface_declaration" => Some(INTERFACE),
                "type_alias_declaration" => Some(TYPE),
                "variable_declarator" => {
                    let value = node.child_by_field_name("value");
                    if value.is_some_and(|n| {
                        matches!(n.kind(), "arrow_function" | "function_expression")
                    }) {
                        Some(FUNCTION)
                    } else if node
                        .parent()
                        .is_some_and(|n| n.kind() == "lexical_declaration")
                        && node
                            .end_position()
                            .row
                            .saturating_sub(node.start_position().row)
                            >= 2
                    {
                        Some(CONST)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            LanguageKind::Rust => None,
        }
    }

    fn old_abstract_decl(node: Node<'_>, bytes: &[u8], lang: LanguageKind, kind: usize) -> bool {
        match lang {
            LanguageKind::Python => {
                if kind == CLASS {
                    let bases = node
                        .child_by_field_name("superclasses")
                        .map(|n| text(n, bytes))
                        .unwrap_or("");
                    bases
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .any(|part| matches!(part, "ABC" | "ABCMeta"))
                } else if kind == METHOD {
                    node.parent()
                        .filter(|p| p.kind() == "decorated_definition")
                        .is_some_and(|p| {
                            children(p).into_iter().any(|child| {
                                child.kind() == "decorator"
                                    && matches!(
                                        text(child, bytes).trim_start_matches('@').trim(),
                                        "abstractmethod" | "abc.abstractmethod"
                                    )
                            })
                        })
                } else {
                    false
                }
            }
            LanguageKind::TypeScript => {
                if kind == INTERFACE || node.kind().starts_with("abstract_") {
                    return true;
                }
                let before_name = node
                    .child_by_field_name("name")
                    .map_or(node.end_byte(), |name| name.start_byte());
                std::str::from_utf8(&bytes[node.start_byte()..before_name])
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|part| part == "abstract")
            }
            LanguageKind::Go | LanguageKind::Rust => false,
        }
    }

    fn old_collect_spans(
        node: Node<'_>,
        bytes: &[u8],
        lang: LanguageKind,
        file: usize,
        first: usize,
        spans: &mut Vec<Span>,
    ) {
        if let Some(mut k) = old_kind(node, lang) {
            if let Some(n) = name(node, bytes) {
                let parent = spans[first..]
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, s)| {
                        s.begin_byte <= node.start_byte() && node.end_byte() <= s.end_byte
                    })
                    .map(|(i, _)| (first + i) as isize)
                    .unwrap_or(-1);
                if k == FUNCTION && parent >= 0 {
                    k = if spans[parent as usize].kind == CLASS {
                        METHOD
                    } else {
                        NESTED_FUNCTION
                    };
                }
                spans.push(Span {
                    file,
                    name: n,
                    kind: k,
                    start: node.start_position().row + 1,
                    credit_start: node
                        .parent()
                        .filter(|p| p.kind() == "decorated_definition")
                        .map_or(node.start_position().row + 1, |p| {
                            p.start_position().row + 1
                        }),
                    end: node.end_position().row + 1,
                    begin_byte: node.start_byte(),
                    credit_begin_byte: node
                        .parent()
                        .filter(|p| p.kind() == "decorated_definition")
                        .map_or(node.start_byte(), |p| p.start_byte()),
                    end_byte: node.end_byte(),
                    code_lines: 0,
                    parent,
                    abstract_symbol: old_abstract_decl(node, bytes, lang, k)
                        || (parent >= 0
                            && spans[parent as usize].kind == INTERFACE
                            && lang == LanguageKind::TypeScript),
                    go_signature: (lang == LanguageKind::Go)
                        .then(|| go_signature(node))
                        .flatten(),
                    go_embeds: lang == LanguageKind::Go
                        && k == INTERFACE
                        && node.child_by_field_name("type").is_some_and(|ty| {
                            children(ty)
                                .into_iter()
                                .any(|child| child.kind() == "type_elem")
                        }),
                    // This signature was not a symbol before, so annotation
                    // references inside it retain their enclosing owner.
                    reference_owner: node.kind() != "abstract_method_signature",
                });
            }
        }
        for child in children(node) {
            old_collect_spans(child, bytes, lang, file, first, spans);
        }
    }

    fn old_collect_go_receivers(
        root: Node<'_>,
        bytes: &[u8],
        spans: &[Span],
        offset: usize,
        out: &mut Vec<(usize, String)>,
    ) {
        fn visit(
            node: Node<'_>,
            bytes: &[u8],
            spans: &[Span],
            offset: usize,
            out: &mut Vec<(usize, String)>,
        ) {
            if node.kind() == "method_declaration" {
                if let Some(type_name) = node
                    .child_by_field_name("receiver")
                    .and_then(|n| receiver_type(n, bytes))
                {
                    if let Some((i, _)) = spans
                        .iter()
                        .enumerate()
                        .find(|(_, s)| s.begin_byte == node.start_byte() && s.kind == METHOD)
                    {
                        out.push((offset + i, type_name));
                    }
                }
            }
            for child in children(node) {
                visit(child, bytes, spans, offset, out);
            }
        }
        visit(root, bytes, spans, offset, out);
    }

    fn old_covered_by_reference_wrapper(node: Node<'_>) -> bool {
        let mut parent = node.parent();
        while let Some(p) = parent {
            if matches!(
                p.kind(),
                "decorator"
                    | "superclasses"
                    | "type"
                    | "type_annotation"
                    | "extends_clause"
                    | "implements_clause"
            ) {
                return true;
            }
            if matches!(
                p.kind(),
                "class_definition"
                    | "function_definition"
                    | "method_definition"
                    | "method_declaration"
            ) {
                break;
            }
            parent = p.parent();
        }
        false
    }

    fn old_value_attribute(node: Node<'_>) -> bool {
        if !matches!(
            node.kind(),
            "attribute" | "member_expression" | "selector_expression"
        ) || old_covered_by_reference_wrapper(node)
        {
            return false;
        }
        let mut child = node;
        while let Some(parent) = child.parent() {
            // A selector inside a longer selector is credited at the outermost
            // lexical site, if that full chain resolves. A store target does
            // not read the method it happens to name.
            if matches!(
                parent.kind(),
                "attribute" | "member_expression" | "selector_expression"
            ) && parent
                .child_by_field_name("object")
                .or_else(|| parent.child_by_field_name("operand"))
                .is_some_and(|base| base.id() == child.id())
            {
                return false;
            }
            if matches!(parent.kind(), "call" | "call_expression")
                && parent
                    .child_by_field_name("function")
                    .or_else(|| parent.child_by_field_name("method"))
                    .is_some_and(|callee| callee.id() == child.id())
            {
                return false;
            }
            if matches!(
                parent.kind(),
                "assignment"
                    | "augmented_assignment"
                    | "assignment_expression"
                    | "augmented_assignment_expression"
                    | "short_var_declaration"
            ) && parent
                .child_by_field_name("left")
                .or_else(|| parent.child_by_field_name("name"))
                .is_some_and(|left| left.id() == child.id())
            {
                return false;
            }
            child = parent;
        }
        true
    }

    fn old_collect_candidates(
        node: Node<'_>,
        bytes: &[u8],
        owners: &mut OwnerLookup<'_>,
        out: &mut Vec<Candidate>,
        shadowed: &mut BTreeMap<usize, BTreeSet<String>>,
    ) {
        let owner = owners.at(node.start_byte());
        if let Some(owner) = owner {
            if matches!(node.kind(), "parameters" | "formal_parameters") {
                parameter_bindings(node, bytes, shadowed.entry(owner).or_default());
            }
            if matches!(
                node.kind(),
                "assignment"
                    | "augmented_assignment"
                    | "variable_declarator"
                    | "short_var_declaration"
            ) {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.child_by_field_name("name"))
                {
                    parameter_bindings(left, bytes, shadowed.entry(owner).or_default());
                }
            }
            let call = matches!(node.kind(), "call" | "call_expression");
            if call {
                let callee = node
                    .child_by_field_name("function")
                    .or_else(|| node.child_by_field_name("method"));
                let resolved = callee.and_then(|n| {
                    if n.kind() == "attribute" {
                        let base = n.child_by_field_name("object")?;
                        if base.kind() == "call"
                            && base
                                .child_by_field_name("function")
                                .is_some_and(|function| text(function, bytes) == "super")
                            && base
                                .child_by_field_name("arguments")
                                .is_some_and(|args| children(args).is_empty())
                        {
                            return n.child_by_field_name("attribute").map(|attr| {
                                vec!["super".to_owned(), text(attr, bytes).to_owned()]
                            });
                        }
                    }
                    chain(n, bytes)
                });
                let parent_class_method = callee.is_some_and(|n| {
                    n.kind() == "attribute"
                        && n.child_by_field_name("object").is_some_and(|object| {
                            object.kind() == "call"
                                && object
                                    .child_by_field_name("function")
                                    .is_some_and(|function| text(function, bytes) == "super")
                        })
                });
                out.push(Candidate {
                    owner,
                    chain: resolved.clone().unwrap_or_default(),
                    call: true,
                    value: false,
                    kind: CALL,
                    unresolved_reason: resolved.is_none().then_some(if parent_class_method {
                        UnresolvedReason::ParentClassMethod
                    } else {
                        UnresolvedReason::Dynamic
                    }),
                });
            } else if !old_covered_by_reference_wrapper(node)
                && matches!(
                    node.kind(),
                    "decorator"
                        | "superclasses"
                        | "type"
                        | "type_annotation"
                        | "extends_clause"
                        | "implements_clause"
                )
            {
                if node.kind() == "superclasses" {
                    collect_superclasses(owner, node, bytes, out);
                } else {
                    let mut chains = Vec::new();
                    expression_chains(node, bytes, &mut chains);
                    let kind = match node.kind() {
                        "superclasses" | "extends_clause" => EXTENDS,
                        "implements_clause" => IMPLEMENTS,
                        "decorator" => DECORATOR,
                        _ => ANNOTATION,
                    };
                    for parts in chains {
                        out.push(Candidate {
                            owner,
                            chain: parts,
                            call: false,
                            value: false,
                            kind,
                            unresolved_reason: None,
                        });
                    }
                }
            } else if node.kind() == "class_definition" {
                if let Some(bases) = node.child_by_field_name("superclasses") {
                    collect_superclasses(owner, bases, bytes, out);
                }
            } else if old_value_attribute(node) {
                if let Some(parts) = chain(node, bytes) {
                    out.push(Candidate {
                        owner,
                        chain: parts,
                        call: false,
                        value: true,
                        kind: VALUE,
                        unresolved_reason: None,
                    });
                }
            }
        }
        for child in children(node) {
            old_collect_candidates(child, bytes, owners, out, shadowed);
        }
    }

    fn old_go_imports(node: Node<'_>, bytes: &[u8], out: &mut Vec<(String, String)>) {
        if node.kind() == "import_spec" {
            if let Some(path_node) = node.child_by_field_name("path") {
                let path = text(path_node, bytes).trim_matches(['\'', '"', '`']);
                let alias = node
                    .child_by_field_name("name")
                    .map(|n| text(n, bytes).to_owned())
                    .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned());
                out.push((path.to_owned(), alias));
            }
        }
        for child in children(node) {
            old_go_imports(child, bytes, out);
        }
    }

    fn grammar(lang: LanguageKind, path: &str) -> tree_sitter::Language {
        match lang {
            LanguageKind::Python => tree_sitter_python::LANGUAGE.into(),
            LanguageKind::Go => tree_sitter_go::LANGUAGE.into(),
            LanguageKind::TypeScript if path.ends_with(".tsx") => {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            LanguageKind::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            LanguageKind::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }

    fn assert_same(lang: LanguageKind, path: &str, bytes: &[u8]) {
        let mut parser = Parser::new();
        parser.set_language(&grammar(lang, path)).unwrap();
        let tree = parser.parse(bytes, None).unwrap();
        let root = tree.root_node();

        let mut old_spans = Vec::new();
        old_collect_spans(root, bytes, lang, 0, 0, &mut old_spans);
        let mut new_spans = Vec::new();
        collect_spans(root, bytes, lang, 0, 0, &mut new_spans);
        assert_eq!(
            serde_json::to_string(&old_spans).unwrap(),
            serde_json::to_string(&new_spans).unwrap(),
            "spans differ in {path}"
        );

        let mut old_receivers = Vec::new();
        old_collect_go_receivers(root, bytes, &old_spans, 0, &mut old_receivers);
        let mut new_receivers = Vec::new();
        collect_go_receivers(root, bytes, &new_spans, 0, &mut new_receivers);
        assert_eq!(old_receivers, new_receivers, "receivers differ in {path}");

        if lang == LanguageKind::Go {
            let mut old_imports = Vec::new();
            old_go_imports(root, bytes, &mut old_imports);
            let flags = extract::code_line_flags(root, bytes, lang);
            let RawImports::Go(new_imports) = collect(root, bytes, lang, &flags).imports else {
                panic!("Go record without Go imports");
            };
            assert_eq!(old_imports, new_imports, "Go imports differ in {path}");
        }

        let mut old_candidates = Vec::new();
        let mut old_shadowed = BTreeMap::new();
        old_collect_candidates(
            root,
            bytes,
            &mut OwnerLookup::new(&old_spans),
            &mut old_candidates,
            &mut old_shadowed,
        );
        let mut new_candidates = Vec::new();
        let mut new_shadowed = BTreeMap::new();
        collect_candidates(
            root,
            bytes,
            &mut OwnerLookup::new(&new_spans),
            &mut new_candidates,
            &mut new_shadowed,
        );
        assert_eq!(
            serde_json::to_string(&old_candidates).unwrap(),
            serde_json::to_string(&new_candidates).unwrap(),
            "candidates differ in {path}"
        );
        assert_eq!(
            old_shadowed, new_shadowed,
            "shadowed names differ in {path}"
        );
    }

    const PYTHON: &str = r#"
import abc
from typing import Optional as Opt

@decorate(helper.make)
class Base(abc.ABC, metaclass=Meta):
    field: Opt[int] = None

    @abc.abstractmethod
    def run(self, x: pkg.Type = default.value) -> other.Result:
        callback = self.handler
        self.slot = self.other
        obj.attr.chain = value.read
        return super().run(fn=self.fallback, *args)

    def handler(self):
        def inner(y: typing.Any):
            return y.attr if cond else self.handler
        items = [self.run(i) for i in range(3)]
        return functools.partial(self.run, inner)

class Child(Base):
    @property
    def value(self) -> "Base":
        return Base.run(self) or module.func

def top(a, b: int, *rest, **kw):
    x = lambda q: q.attr
    return a.b.c(x)
"#;

    const TYPESCRIPT: &str = r#"
import { Thing as Alias, other } from "./thing";
import * as ns from "../ns";

export abstract class Shape<T extends Base> extends Base implements ns.Face, Other {
  private readonly field: ns.Type<T> = this.make;
  abstract area(): number;
  constructor(private readonly dep: Alias) { super(dep.value); }
  method(arg: Foo.Bar): Result {
    const handler = this.other;
    this.slot = this.other;
    return this.helper(arg.field, () => this.method);
  }
}

export interface Face extends Base { run(x: number): void; name: string }
type Pair = { a: ns.A; b: B };
const table = {
  a: 1,
  b: other.value,
};
export const arrow = async (x: Alias): Promise<void> => { await ns.go(x.y); };
function plain(this: Window, a = defaults.a) { return plain.call(this, a); }
const Comp = () => <div onClick={this.handle} data={props.item.value}>{items.map((i) => <Item key={i.id} />)}</div>;
"#;

    const GO: &str = r#"
package main

import (
	"fmt"
	alias "example.com/mod/pkg"
	_ "example.com/mod/side"
)

type Shape interface {
	Area() float64
	Perimeter(scale float64, extra ...int) (float64, error)
}

type Embeds interface {
	Shape
	~int | ~float64
}

type Square struct {
	side float64
	inner alias.Thing
}

func (s *Square) Area() float64 { return s.side * s.side }

func (s Square) Perimeter(scale float64, extra ...int) (float64, error) {
	f := s.Area
	s.side = alias.Value
	return fmt.Sprint(s.inner.Method(f)), nil
}

func main() {
	sq := &Square{side: 2}
	handler := sq.Area
	fmt.Println(handler(), Square.Area, alias.Func)
}
"#;

    #[test]
    fn cursor_walks_match_parent_walks_on_samples() {
        assert_same(LanguageKind::Python, "sample.py", PYTHON.as_bytes());
        assert_same(LanguageKind::TypeScript, "sample.ts", TYPESCRIPT.as_bytes());
        assert_same(
            LanguageKind::TypeScript,
            "sample.tsx",
            TYPESCRIPT.as_bytes(),
        );
        assert_same(LanguageKind::Go, "sample.go", GO.as_bytes());
    }

    fn visit_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries = entries
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if !path.ends_with("node_modules") {
                    visit_sources(&path, out);
                }
            } else if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("py" | "ts" | "tsx")
            ) {
                out.push(path);
            }
        }
    }

    /// The frozen Python reference and the web client are real code with
    /// deep expression nesting, decorators and JSX.
    #[test]
    fn cursor_walks_match_parent_walks_on_repository_sources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        visit_sources(&root.join("src/tolmap"), &mut files);
        visit_sources(&root.join("web/src"), &mut files);
        assert!(
            files.len() > 20,
            "expected repository sources, found {}",
            files.len()
        );
        for path in files {
            let bytes = fs::read(&path).unwrap();
            let name = path.display().to_string();
            let lang = if name.ends_with(".py") {
                LanguageKind::Python
            } else {
                LanguageKind::TypeScript
            };
            assert_same(lang, &name, &bytes);
        }
    }
}
