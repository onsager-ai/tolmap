//! Complete symbol data lives beside the parity-constrained map document.
//! Extraction collects compact spans and candidate references while each
//! file's parse tree is alive. This pass resolves them after map file order
//! is known. It never changes `GraphData.symbols` or `uses`.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use tree_sitter::Node;

use crate::extract::{self, LanguageKind};
use crate::schema::{
    DistrictSymbols, HierSymbolRow, MapDocument, SourceNode, SymbolCoverage, SymbolsDocument,
};

const CLASS: usize = 0;
const FUNCTION: usize = 1;
const METHOD: usize = 2;
const NESTED_FUNCTION: usize = 3;
const INTERFACE: usize = 4;
const TYPE: usize = 5;
const CONST: usize = 6;

#[derive(Clone)]
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
}

#[derive(Clone, PartialEq, Eq)]
enum Binding {
    Module(usize),
    From(usize, String),
    Prefix(String, String),
    Package(String),
    External,
}

struct Candidate {
    owner: usize,
    chain: Vec<String>,
    call: bool,
    unresolved_reason: Option<&'static str>,
}

struct FileInfo {
    lang: LanguageKind,
    directory: String,
    imports: BTreeMap<String, Binding>,
    candidates: Vec<Candidate>,
    shadowed: BTreeMap<usize, BTreeSet<String>>,
}

pub(crate) struct ParsedSymbols {
    spans: Vec<Span>,
    receivers: Vec<(usize, String)>,
    candidates: Vec<Candidate>,
    shadowed: BTreeMap<usize, BTreeSet<String>>,
    imports: RawImports,
    outside: usize,
}

enum RawImports {
    Python(Vec<extract::PythonImport>),
    Go(Vec<(String, String)>),
    TypeScript(Vec<(String, Vec<(String, Option<String>)>)>),
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

fn kind(node: Node<'_>, lang: LanguageKind) -> Option<usize> {
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
            "class_declaration" | "class" => Some(CLASS),
            "function_declaration" => Some(FUNCTION),
            "method_definition" | "method_signature" => Some(METHOD),
            "interface_declaration" => Some(INTERFACE),
            "type_alias_declaration" => Some(TYPE),
            "variable_declarator" => {
                let value = node.child_by_field_name("value");
                if value
                    .is_some_and(|n| matches!(n.kind(), "arrow_function" | "function_expression"))
                {
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
    }
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn collect_spans(
    node: Node<'_>,
    bytes: &[u8],
    lang: LanguageKind,
    file: usize,
    first: usize,
    spans: &mut Vec<Span>,
) {
    if let Some(mut k) = kind(node, lang) {
        if let Some(n) = name(node, bytes) {
            let parent = spans[first..]
                .iter()
                .enumerate()
                .rev()
                .find(|(_, s)| s.begin_byte <= node.start_byte() && node.end_byte() <= s.end_byte)
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
            });
        }
    }
    for child in children(node) {
        collect_spans(child, bytes, lang, file, first, spans);
    }
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

fn chain(node: Node<'_>, bytes: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" | "package_identifier" | "this" => {
            Some(vec![text(node, bytes).to_owned()])
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
        let mut starts = (0..spans.len()).collect::<Vec<_>>();
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

fn covered_by_reference_wrapper(node: Node<'_>) -> bool {
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
            "class_definition" | "function_definition" | "method_definition" | "method_declaration"
        ) {
            break;
        }
        parent = p.parent();
    }
    false
}

fn collect_candidates(
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
            "assignment" | "augmented_assignment" | "variable_declarator" | "short_var_declaration"
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
            let resolved = callee.and_then(|n| chain(n, bytes));
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
                unresolved_reason: resolved.is_none().then_some(if parent_class_method {
                    "parent_class_method"
                } else {
                    "dynamic"
                }),
            });
        } else if !covered_by_reference_wrapper(node)
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
            let mut chains = Vec::new();
            expression_chains(node, bytes, &mut chains);
            for parts in chains {
                out.push(Candidate {
                    owner,
                    chain: parts,
                    call: false,
                    unresolved_reason: None,
                });
            }
        } else if node.kind() == "class_definition" {
            if let Some(bases) = node.child_by_field_name("superclasses") {
                let mut chains = Vec::new();
                expression_chains(bases, bytes, &mut chains);
                for parts in chains {
                    out.push(Candidate {
                        owner,
                        chain: parts,
                        call: false,
                        unresolved_reason: None,
                    });
                }
            }
        }
    }
    for child in children(node) {
        collect_candidates(child, bytes, owners, out, shadowed);
    }
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

pub(crate) fn collect(
    root: Node<'_>,
    bytes: &[u8],
    lang: LanguageKind,
    flags: &[bool],
) -> ParsedSymbols {
    let mut spans = Vec::new();
    collect_spans(root, bytes, lang, 0, 0, &mut spans);
    let mut receivers = Vec::new();
    if lang == LanguageKind::Go {
        collect_go_receivers(root, bytes, &spans, 0, &mut receivers);
    }
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
            fn visit(node: Node<'_>, bytes: &[u8], out: &mut Vec<(String, String)>) {
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
                    visit(child, bytes, out);
                }
            }
            let mut imports = Vec::new();
            visit(root, bytes, &mut imports);
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
    };
    let mut candidates = Vec::new();
    let mut shadowed = BTreeMap::new();
    collect_candidates(
        root,
        bytes,
        &mut OwnerLookup::new(&spans),
        &mut candidates,
        &mut shadowed,
    );
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

fn resolve(
    candidate: &Candidate,
    spans: &[Span],
    infos: &[FileInfo],
    tops: &[BTreeMap<String, usize>],
    modules: &BTreeMap<String, usize>,
    packages: &BTreeMap<String, Vec<usize>>,
    members: &BTreeMap<usize, BTreeMap<String, usize>>,
) -> Result<usize, &'static str> {
    let source = &spans[candidate.owner];
    let parts = &candidate.chain;
    if let Some(reason) = candidate.unresolved_reason {
        return Err(reason);
    }
    if parts.is_empty() {
        return Err("dynamic");
    }
    let head = parts[0].as_str();
    if (head == "self" || head == "cls" || head == "this") && parts.len() >= 2 {
        let mut p = Some(candidate.owner);
        while let Some(i) = p {
            if spans[i].kind == CLASS {
                return members
                    .get(&i)
                    .and_then(|m| m.get(&parts[1]).copied())
                    .ok_or("instance_or_untyped");
            }
            p = (spans[i].parent >= 0).then_some(spans[i].parent as usize);
        }
        return Err("instance_or_untyped");
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
        BindingOrSymbol::Symbol(i) => Ok(i),
        BindingOrSymbol::Module(_) | BindingOrSymbol::Package(_) => Err("module_only"),
        BindingOrSymbol::Prefix(_, _) | BindingOrSymbol::External => Err("external"),
    }
}

pub(crate) fn build(
    repo: &Path,
    nodes: &[SourceNode],
    mut records: BTreeMap<String, ParsedSymbols>,
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
        let Some(mut record) = records.remove(&entry.file) else {
            module_code_lines.insert(fi, entry.code_lines.unwrap_or(0));
            infos.push(FileInfo {
                lang,
                directory: entry.file.rsplit_once('/').map_or("", |v| v.0).to_owned(),
                imports: BTreeMap::new(),
                candidates: Vec::new(),
                shadowed: BTreeMap::new(),
            });
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
        };
        infos.push(FileInfo {
            lang,
            directory: entry.file.rsplit_once('/').map_or("", |v| v.0).to_owned(),
            imports,
            candidates: record.candidates,
            shadowed,
        });
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
    for (method, type_name) in go_receivers {
        let file = spans[method].file;
        let directory = nodes[file].file.rsplit_once('/').map_or("", |v| v.0);
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
    let mut edges = BTreeMap::<(usize, usize), usize>::new();
    let mut coverage = SymbolCoverage::default();
    for info in &infos {
        for candidate in &info.candidates {
            if candidate.call {
                coverage.calls_total += 1;
            }
            match resolve(
                candidate, &spans, &infos, &tops, &modules, &packages, &members,
            ) {
                Ok(target) => {
                    if candidate.call {
                        coverage.calls_resolved += 1;
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
                        *edges.entry((candidate.owner, target)).or_default() += 1;
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
    let symbols = order
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
            ))
        })
        .collect();
    let mut edges = edges
        .into_iter()
        .map(|((a, b), count)| [index[a], index[b], count])
        .collect::<Vec<_>>();
    edges.sort_unstable();
    Ok(SymbolsDocument {
        files: (0..nodes.len()).collect(),
        symbols,
        edges,
        module_code_lines,
        coverage,
        symbol_rings: None,
        module_rings: None,
        header_rings: None,
    })
}

pub(crate) fn write_sibling(
    repo: &Path,
    nodes: &[SourceNode],
    map_path: &Path,
    records: BTreeMap<String, ParsedSymbols>,
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
    let started = Instant::now();
    let mut document = build(repo, nodes, records)?;
    eprintln!("symbol resolution: {:.2}s", started.elapsed().as_secs_f64());
    let cards_started = Instant::now();
    crate::symbol_cards::attach(&map, &mut document)?;
    eprintln!(
        "symbol cards: {:.2}s",
        cards_started.elapsed().as_secs_f64()
    );
    let output = map_path.with_extension("symbols.json");
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
        fs::write(
            temporary_dir.join(format!("{id}.json")),
            serde_json::to_vec(&response)?,
        )?;
    }
    fs::write(&temporary, serde_json::to_vec(&document)?)?;
    fs::rename(&temporary, &output)?;
    if district_dir.exists() {
        fs::remove_dir_all(&district_dir)?;
    }
    fs::rename(&temporary_dir, &district_dir)?;
    eprintln!("wrote {}", output.display());
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

    fn build_fixture(repo: &Path, nodes: &[SourceNode]) -> SymbolsDocument {
        let mut records = BTreeMap::new();
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
            };
            parser.set_language(&grammar).unwrap();
            let bytes = fs::read(repo.join(&node.file)).unwrap();
            let tree = parser.parse(&bytes, None).unwrap();
            let root = tree.root_node();
            let flags = extract::code_line_flags(root, &bytes, lang);
            records.insert(node.file.clone(), collect(root, &bytes, lang, &flags));
        }
        build(repo, nodes, records).unwrap()
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
        assert_eq!(doc.coverage.unresolved["parent_class_method"], 1);
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
}
