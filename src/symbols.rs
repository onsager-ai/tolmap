//! Complete symbol data lives beside the parity-constrained map document.
//! This pass reparses one mapped file at a time, retaining only compact spans
//! and candidate references. It never changes `GraphData.symbols` or `uses`.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

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
    end: usize,
    begin_byte: usize,
    end_byte: usize,
    code_lines: usize,
    parent: isize,
}

#[derive(Clone)]
enum Binding {
    Module(usize),
    From(usize, String),
    Prefix(String),
    External,
}

struct Candidate {
    owner: usize,
    chain: Vec<String>,
    call: bool,
    dynamic: bool,
}

struct FileInfo {
    module: String,
    imports: BTreeMap<String, Binding>,
    candidates: Vec<Candidate>,
    shadowed: BTreeMap<usize, BTreeSet<String>>,
}

fn text<'a>(node: Node<'_>, bytes: &'a [u8]) -> &'a str {
    std::str::from_utf8(&bytes[node.byte_range()]).unwrap_or("")
}

fn name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| text(n, bytes).to_owned())
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|n| {
                    matches!(
                        n.kind(),
                        "identifier"
                            | "type_identifier"
                            | "field_identifier"
                            | "property_identifier"
                    )
                })
                .map(|n| text(n, bytes).to_owned())
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
            "method_declaration" => Some(METHOD),
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
            "method_definition" => Some(METHOD),
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
    spans: &mut Vec<Span>,
) {
    if let Some(mut k) = kind(node, lang) {
        if let Some(n) = name(node, bytes) {
            let parent = spans
                .iter()
                .enumerate()
                .rev()
                .find(|(_, s)| {
                    s.file == file
                        && s.begin_byte <= node.start_byte()
                        && node.end_byte() <= s.end_byte
                })
                .map(|(i, _)| i as isize)
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
                end: node.end_position().row + 1,
                begin_byte: node.start_byte(),
                end_byte: node.end_byte(),
                code_lines: 0,
                parent,
            });
        }
    }
    for child in children(node) {
        collect_spans(child, bytes, lang, file, spans);
    }
}

// TODO(feat/code-lines): reuse its shared code-line counter when that PR lands.
// The local counter excludes blank, comment-only, and Python docstring lines.
fn code_line_flags(root: Node<'_>, bytes: &[u8], lang: LanguageKind) -> Vec<bool> {
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    let mut flags = lines
        .iter()
        .map(|line| {
            let s = String::from_utf8_lossy(line);
            let s = s.trim();
            !s.is_empty()
                && !s.starts_with('#')
                && !s.starts_with("//")
                && !s.starts_with("/*")
                && !s.starts_with('*')
        })
        .collect::<Vec<_>>();
    fn exclude(node: Node<'_>, flags: &mut [bool], lang: LanguageKind) {
        let docstring = lang == LanguageKind::Python
            && node.kind() == "expression_statement"
            && node
                .parent()
                .is_some_and(|p| matches!(p.kind(), "module" | "block"))
            && node
                .parent()
                .and_then(|p| children(p).first().copied())
                .is_some_and(|first| first.id() == node.id())
            && children(node)
                .first()
                .is_some_and(|first| first.kind() == "string");
        if node.kind() == "comment" || docstring {
            for row in node.start_position().row..=node.end_position().row {
                if let Some(flag) = flags.get_mut(row) {
                    let line = String::from_utf8_lossy(
                        bytes.split(|b| *b == b'\n').nth(row).unwrap_or(&[]),
                    );
                    if docstring
                        || line.trim_start().starts_with('#')
                        || line.trim_start().starts_with("//")
                        || line.trim_start().starts_with("/*")
                        || line.trim_start().starts_with('*')
                    {
                        *flag = false;
                    }
                }
            }
        }
        for child in children(node) {
            exclude(child, flags, lang);
        }
    }
    exclude(root, &mut flags, lang);
    flags
}

fn chain(node: Node<'_>, bytes: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" => Some(vec![text(node, bytes).to_owned()]),
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

fn owner_at(spans: &[Span], file: usize, byte: usize) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .filter(|(_, s)| s.file == file && s.begin_byte <= byte && byte < s.end_byte)
        .min_by_key(|(_, s)| s.end_byte - s.begin_byte)
        .map(|(i, _)| i)
}

fn collect_candidates(
    node: Node<'_>,
    bytes: &[u8],
    file: usize,
    spans: &[Span],
    out: &mut Vec<Candidate>,
    shadowed: &mut BTreeMap<usize, BTreeSet<String>>,
) {
    let owner = owner_at(spans, file, node.start_byte());
    if let Some(owner) = owner {
        if matches!(node.kind(), "parameters" | "formal_parameters") {
            for child in children(node) {
                if child.kind() == "identifier" {
                    shadowed
                        .entry(owner)
                        .or_default()
                        .insert(text(child, bytes).to_owned());
                }
            }
        }
        let call = matches!(node.kind(), "call" | "call_expression");
        if call {
            let callee = node
                .child_by_field_name("function")
                .or_else(|| node.child_by_field_name("method"));
            let resolved = callee.and_then(|n| chain(n, bytes));
            out.push(Candidate {
                owner,
                chain: resolved.clone().unwrap_or_default(),
                call: true,
                dynamic: resolved.is_none(),
            });
        } else if matches!(
            node.kind(),
            "decorator"
                | "superclasses"
                | "type"
                | "type_annotation"
                | "extends_clause"
                | "implements_clause"
        ) {
            for child in children(node) {
                if let Some(parts) = chain(child, bytes) {
                    out.push(Candidate {
                        owner,
                        chain: parts,
                        call: false,
                        dynamic: false,
                    });
                }
            }
        }
    }
    for child in children(node) {
        collect_candidates(child, bytes, file, spans, out, shadowed);
    }
}

fn imports_python(
    root: Node<'_>,
    bytes: &[u8],
    module: &str,
    is_pkg: bool,
    modules: &BTreeMap<String, usize>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    for imp in extract::python_imports(root, bytes) {
        let head = extract::python_head(&imp, module, is_pkg);
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
            } else if let Some(&fi) = modules.get(n) {
                Binding::Module(fi)
            } else {
                Binding::Prefix(n.split('.').next().unwrap_or(n).to_owned())
            };
            result.insert(local, binding);
        }
    }
    result
}

fn imports_multi(
    root: Node<'_>,
    bytes: &[u8],
    file: &str,
    modules: &BTreeMap<String, usize>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    for node in children(root) {
        if node.kind() != "import_statement" {
            continue;
        }
        let raw = text(node, bytes);
        if let Some(path) = raw.split(['\'', '"']).nth(1) {
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
            let target = [
                stem.clone(),
                format!("{stem}.ts"),
                format!("{stem}.tsx"),
                format!("{stem}/index.ts"),
            ]
            .into_iter()
            .find_map(|p| modules.get(&p).copied());
            if let Some(target) = target {
                let before = raw.split("from").next().unwrap_or("");
                for token in before.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
                    if !token.is_empty() && !matches!(token, "import" | "type" | "as") {
                        result.insert(token.to_owned(), Binding::From(target, token.to_owned()));
                    }
                }
            }
        }
    }
    result
}

fn lookup(
    file: usize,
    name: &str,
    infos: &[FileInfo],
    tops: &[BTreeMap<String, usize>],
    depth: usize,
) -> Option<BindingOrSymbol> {
    if depth > 4 {
        return None;
    }
    if let Some(&symbol) = tops[file].get(name) {
        return Some(BindingOrSymbol::Symbol(symbol));
    }
    match infos[file].imports.get(name)? {
        Binding::Module(fi) => Some(BindingOrSymbol::Module(*fi)),
        Binding::From(fi, target) => lookup(*fi, target, infos, tops, depth + 1),
        Binding::Prefix(prefix) => Some(BindingOrSymbol::Prefix(prefix.clone())),
        Binding::External => Some(BindingOrSymbol::External),
    }
}

enum BindingOrSymbol {
    Symbol(usize),
    Module(usize),
    Prefix(String),
    External,
}

fn resolve(
    candidate: &Candidate,
    spans: &[Span],
    infos: &[FileInfo],
    tops: &[BTreeMap<String, usize>],
    modules: &BTreeMap<String, usize>,
    members: &BTreeMap<usize, BTreeMap<String, usize>>,
) -> Result<usize, &'static str> {
    let source = &spans[candidate.owner];
    let parts = &candidate.chain;
    if candidate.dynamic || parts.is_empty() {
        return Err("dynamic");
    }
    let head = parts[0].as_str();
    if (head == "self" || head == "cls") && parts.len() >= 2 {
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
        .or_else(|| lookup(source.file, head, infos, tops, 0))
        .ok_or("local_or_builtin")?;
    for part in parts.iter().skip(1) {
        value = match value {
            BindingOrSymbol::Symbol(i) => members
                .get(&i)
                .and_then(|m| m.get(part).copied())
                .map(BindingOrSymbol::Symbol)
                .ok_or("unresolved_attribute")?,
            BindingOrSymbol::Module(fi) => {
                lookup(fi, part, infos, tops, 0).ok_or("unresolved_attribute")?
            }
            BindingOrSymbol::Prefix(prefix) => {
                let path = format!("{prefix}.{part}");
                if let Some(&fi) = modules.get(&path) {
                    BindingOrSymbol::Module(fi)
                } else {
                    return Err("external");
                }
            }
            BindingOrSymbol::External => return Err("external"),
        };
    }
    match value {
        BindingOrSymbol::Symbol(i) => Ok(i),
        BindingOrSymbol::Module(_) => Err("module_only"),
        BindingOrSymbol::Prefix(_) | BindingOrSymbol::External => Err("external"),
    }
}

pub fn build(repo: &Path, nodes: &[SourceNode]) -> Result<SymbolsDocument> {
    let mut modules = BTreeMap::new();
    for (fi, node) in nodes.iter().enumerate() {
        modules.insert(node.module.clone(), fi);
        modules.insert(node.file.clone(), fi);
    }
    let mut spans = Vec::new();
    let mut infos = Vec::new();
    let mut module_code_lines = BTreeMap::new();
    for (fi, entry) in nodes.iter().enumerate() {
        let lang = LanguageKind::parse(&entry.lang)?;
        let bytes =
            fs::read(repo.join(&entry.file)).with_context(|| format!("read {}", entry.file))?;
        let grammar = match lang {
            LanguageKind::Python => tree_sitter_python::LANGUAGE.into(),
            LanguageKind::Go => tree_sitter_go::LANGUAGE.into(),
            LanguageKind::TypeScript if entry.file.ends_with(".tsx") => {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            }
            LanguageKind::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        };
        let mut parser = Parser::new();
        parser.set_language(&grammar)?;
        let Some(tree) = parser.parse(&bytes, None) else {
            continue;
        };
        if lang == LanguageKind::Python && tree.root_node().has_error() {
            continue;
        }
        let root = tree.root_node();
        let first = spans.len();
        collect_spans(root, &bytes, lang, fi, &mut spans);
        let flags = code_line_flags(root, &bytes, lang);
        for span in &mut spans[first..] {
            span.code_lines = flags
                .iter()
                .enumerate()
                .filter(|(row, flag)| **flag && *row + 1 >= span.start && *row + 1 <= span.end)
                .count();
        }
        let outside = flags
            .iter()
            .enumerate()
            .filter(|(row, flag)| {
                **flag
                    && !spans[first..]
                        .iter()
                        .any(|s| s.parent == -1 && *row + 1 >= s.start && *row + 1 <= s.end)
            })
            .count();
        module_code_lines.insert(fi, outside);
        let imports = if lang == LanguageKind::Python {
            imports_python(
                root,
                &bytes,
                &entry.module,
                entry.file.ends_with("__init__.py"),
                &modules,
            )
        } else {
            imports_multi(root, &bytes, &entry.file, &modules)
        };
        let mut candidates = Vec::new();
        let mut shadowed = BTreeMap::new();
        collect_candidates(root, &bytes, fi, &spans, &mut candidates, &mut shadowed);
        infos.push(FileInfo {
            module: entry.module.clone(),
            imports,
            candidates,
            shadowed,
        });
    }
    let mut tops = vec![BTreeMap::new(); nodes.len()];
    let mut members: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    for (i, span) in spans.iter().enumerate() {
        if span.parent < 0 {
            tops[span.file].insert(span.name.clone(), i);
        } else {
            members
                .entry(span.parent as usize)
                .or_default()
                .insert(span.name.clone(), i);
        }
    }
    let mut edges = BTreeMap::<(usize, usize), usize>::new();
    let mut coverage = SymbolCoverage::default();
    for info in &infos {
        for candidate in &info.candidates {
            if candidate.call {
                coverage.calls_total += 1;
            }
            match resolve(candidate, &spans, &infos, &tops, &modules, &members) {
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
    })
}

pub fn write_sibling(repo: &Path, nodes: &[SourceNode], map_path: &Path) -> Result<()> {
    let document = build(repo, nodes)?;
    let output = map_path.with_extension("symbols.json");
    let temporary = map_path.with_extension("symbols.json.tmp");
    fs::write(&temporary, serde_json::to_vec(&document)?)?;
    fs::rename(&temporary, &output)?;
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
        Some(DistrictSymbols {
            district,
            files,
            symbol_indices,
            symbols,
            edges,
            module_code_lines,
        })
    }
}
