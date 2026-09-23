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
    credit_begin_byte: usize,
    end_byte: usize,
    code_lines: usize,
    parent: isize,
}

#[derive(Clone)]
enum Binding {
    Module(usize),
    From(usize, String),
    Prefix(String),
    Package(String),
    External,
}

struct Candidate {
    owner: usize,
    chain: Vec<String>,
    call: bool,
    dynamic: bool,
}

struct FileInfo {
    lang: LanguageKind,
    directory: String,
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

fn attach_go_methods(root: Node<'_>, bytes: &[u8], spans: &mut [Span], offset: usize) {
    let types = spans
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == CLASS || s.kind == INTERFACE || s.kind == TYPE)
        .map(|(i, s)| (s.name.clone(), i))
        .collect::<BTreeMap<_, _>>();
    fn visit(
        node: Node<'_>,
        bytes: &[u8],
        spans: &mut [Span],
        types: &BTreeMap<String, usize>,
        offset: usize,
    ) {
        if node.kind() == "method_declaration" {
            if let Some(type_name) = node
                .child_by_field_name("receiver")
                .and_then(|n| receiver_type(n, bytes))
            {
                if let Some(&parent) = types.get(&type_name) {
                    if let Some(method) = spans
                        .iter_mut()
                        .find(|s| s.begin_byte == node.start_byte() && s.kind == METHOD)
                    {
                        method.parent = (offset + parent) as isize;
                    }
                }
            }
        }
        for child in children(node) {
            visit(child, bytes, spans, types, offset);
        }
    }
    visit(root, bytes, spans, &types, offset);
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
    fn exclude(node: Node<'_>, lines: &[&[u8]], flags: &mut [bool], lang: LanguageKind) {
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
                    let line = String::from_utf8_lossy(lines.get(row).copied().unwrap_or(&[]));
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
            exclude(child, lines, flags, lang);
        }
    }
    exclude(root, &lines, &mut flags, lang);
    flags
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

fn owner_at(spans: &[Span], offset: usize, byte: usize) -> Option<usize> {
    spans
        .iter()
        .enumerate()
        .filter(|(_, s)| s.credit_begin_byte <= byte && byte < s.end_byte)
        .min_by_key(|(_, s)| s.end_byte - s.begin_byte)
        .map(|(i, _)| offset + i)
}

fn expression_chains(node: Node<'_>, bytes: &[u8], out: &mut Vec<Vec<String>>) {
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

fn collect_candidates(
    node: Node<'_>,
    bytes: &[u8],
    offset: usize,
    spans: &[Span],
    out: &mut Vec<Candidate>,
    shadowed: &mut BTreeMap<usize, BTreeSet<String>>,
) {
    let owner = owner_at(spans, offset, node.start_byte());
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
            let mut chains = Vec::new();
            expression_chains(node, bytes, &mut chains);
            for parts in chains {
                out.push(Candidate {
                    owner,
                    chain: parts,
                    call: false,
                    dynamic: false,
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
                        dynamic: false,
                    });
                }
            }
        }
    }
    for child in children(node) {
        collect_candidates(child, bytes, offset, spans, out, shadowed);
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
            } else if alias.is_some() {
                modules
                    .get(n)
                    .copied()
                    .map(Binding::Module)
                    .unwrap_or(Binding::External)
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
        if let Some(source) = node.child_by_field_name("source") {
            let path = text(source, bytes).trim_matches(['\'', '"']);
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
                for clause in children(node)
                    .into_iter()
                    .filter(|n| n.kind() == "import_clause")
                {
                    for item in children(clause) {
                        match item.kind() {
                            "named_imports" => {
                                for spec in children(item) {
                                    if spec.kind() == "import_specifier" {
                                        if let Some(original) = spec.child_by_field_name("name") {
                                            let imported = text(original, bytes).to_owned();
                                            let local = spec
                                                .child_by_field_name("alias")
                                                .map_or(imported.clone(), |n| {
                                                    text(n, bytes).to_owned()
                                                });
                                            result.insert(local, Binding::From(target, imported));
                                        }
                                    }
                                }
                            }
                            "namespace_import" => {
                                if let Some(alias) = children(item).first() {
                                    result.insert(
                                        text(*alias, bytes).to_owned(),
                                        Binding::Module(target),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    result
}

fn imports_go(
    root: Node<'_>,
    bytes: &[u8],
    module: Option<&str>,
    packages: &BTreeMap<String, Vec<usize>>,
) -> BTreeMap<String, Binding> {
    let mut result = BTreeMap::new();
    let Some(module) = module else {
        return result;
    };
    fn visit(
        node: Node<'_>,
        bytes: &[u8],
        module: &str,
        packages: &BTreeMap<String, Vec<usize>>,
        result: &mut BTreeMap<String, Binding>,
    ) {
        if node.kind() == "import_spec" {
            if let Some(path_node) = node.child_by_field_name("path") {
                let path = text(path_node, bytes).trim_matches(['\'', '"', '`']);
                let directory = if path == module {
                    Some("")
                } else {
                    path.strip_prefix(module)
                        .and_then(|rest| rest.strip_prefix('/'))
                };
                if let Some(directory) = directory.filter(|dir| packages.contains_key(*dir)) {
                    let alias = node
                        .child_by_field_name("name")
                        .map(|n| text(n, bytes).to_owned())
                        .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).to_owned());
                    if alias != "_" && alias != "." {
                        result.insert(alias, Binding::Package(directory.to_owned()));
                    }
                }
            }
        }
        for child in children(node) {
            visit(child, bytes, module, packages, result);
        }
    }
    visit(root, bytes, module, packages, &mut result);
    result
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
        Binding::Prefix(prefix) => Some(BindingOrSymbol::Prefix(prefix.clone())),
        Binding::Package(directory) => Some(BindingOrSymbol::Package(directory.clone())),
        Binding::External => Some(BindingOrSymbol::External),
    }
}

enum BindingOrSymbol {
    Symbol(usize),
    Module(usize),
    Prefix(String),
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
    if candidate.dynamic || parts.is_empty() {
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
        BindingOrSymbol::Module(_) | BindingOrSymbol::Package(_) => Err("module_only"),
        BindingOrSymbol::Prefix(_) | BindingOrSymbol::External => Err("external"),
    }
}

pub fn build(repo: &Path, nodes: &[SourceNode]) -> Result<SymbolsDocument> {
    let mut modules = BTreeMap::new();
    let mut packages: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (fi, node) in nodes.iter().enumerate() {
        modules.insert(node.module.clone(), fi);
        modules.insert(node.file.clone(), fi);
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
        collect_spans(root, &bytes, lang, fi, first, &mut spans);
        if lang == LanguageKind::Go {
            attach_go_methods(root, &bytes, &mut spans[first..], first);
        }
        let flags = code_line_flags(root, &bytes, lang);
        for span in &mut spans[first..] {
            span.code_lines = flags
                .iter()
                .enumerate()
                .filter(|(row, flag)| **flag && *row >= span.start - 1 && *row < span.end)
                .count();
        }
        let outside = flags
            .iter()
            .enumerate()
            .filter(|(row, flag)| {
                **flag
                    && !spans[first..]
                        .iter()
                        .any(|s| s.parent == -1 && *row >= s.start - 1 && *row < s.end)
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
        } else if lang == LanguageKind::Go {
            imports_go(root, &bytes, go_module.as_deref(), &packages)
        } else {
            imports_multi(root, &bytes, &entry.file, &modules)
        };
        let mut candidates = Vec::new();
        let mut shadowed = BTreeMap::new();
        collect_candidates(
            root,
            &bytes,
            first,
            &spans[first..],
            &mut candidates,
            &mut shadowed,
        );
        infos.push(FileInfo {
            lang,
            directory: entry.file.rsplit_once('/').map_or("", |v| v.0).to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str, module: &str, lang: &str) -> SourceNode {
        SourceNode {
            file: file.to_owned(),
            module: module.to_owned(),
            lang: lang.to_owned(),
            loc: 0,
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

    #[test]
    fn complete_hierarchy_keeps_dunders_and_more_than_sixty() {
        let mut body = "class Outer:\n    def __init__(self):\n        def inner():\n            pass\n    class Nested:\n        def work(self):\n            pass\n".to_owned();
        for n in 0..65 {
            body.push_str(&format!("\ndef f{n}():\n    pass\n"));
        }
        let (dir, nodes) = fixture(&[("mod.py", "mod", &body)]);
        let doc = build(dir.path(), &nodes).unwrap();
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
            ("caller.py", "caller", "from pkg import Target\nimport pkg.core as core\n\nclass Caller(Target):\n    def method(self):\n        self.helper()\n        Target.method()\n        core.Target.method()\n        Target()\n    def helper(self):\n        pass\n\ndef outer():\n    def inner():\n        outer()\n    inner()\n\ndef typed(x: Target) -> Target:\n    return x\n\n@Target\ndef decorated():\n    pass\n"),
        ]);
        let doc = build(dir.path(), &nodes).unwrap();
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
        assert!(doc.coverage.calls_resolved > 0);
    }

    #[test]
    fn module_lines_and_unknown_calls_are_lower_bounds() {
        let (dir, nodes) = fixture(&[("m.py", "m", "# comment\nVALUE = 1\n\ndef known():\n    \"\"\"doc\"\"\"\n    missing()\n    (factory())()\n")]);
        let doc = build(dir.path(), &nodes).unwrap();
        assert_eq!(doc.module_code_lines[&0], 1);
        assert!(doc.coverage.calls_total >= 3);
        assert_eq!(doc.coverage.calls_resolved, 0);
        assert!(!doc.coverage.unresolved.is_empty());
        assert_eq!(doc.symbols[id(&doc, 0, "known")].0 .6, 3);
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
        let doc = build(dir.path(), &nodes).unwrap();
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
        std::fs::write(dir.path().join("p/one.go"), "package p\nfunc Target() {}\n").unwrap();
        std::fs::write(
            dir.path().join("p/two.go"),
            "package p\nfunc Local() { Target() }\n",
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
            "import { Thing as Alias } from './thing';\nfunction use() { Alias.run(); }\n",
        )
        .unwrap();
        let nodes = vec![
            source("p/one.go", "p/one.go", "go"),
            source("p/two.go", "p/two.go", "go"),
            source("main.go", "main.go", "go"),
            source("thing.ts", "thing.ts", "ts"),
            source("use.ts", "use.ts", "ts"),
        ];
        let doc = build(dir.path(), &nodes).unwrap();
        assert!(edge(&doc, id(&doc, 1, "Local"), id(&doc, 0, "Target")));
        assert!(edge(&doc, id(&doc, 2, "Caller"), id(&doc, 0, "Target")));
        assert!(edge(&doc, id(&doc, 4, "use"), id(&doc, 3, "run")));
    }
}
