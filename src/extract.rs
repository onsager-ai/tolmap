use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use tree_sitter::{Language, Node, Parser, Tree};

use crate::schema::{GraphData, SignalEdge, SourceNode, SymbolRow};

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

const PY_SKIP_DIR: &[&str] = &[
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

const MULTI_SKIP_DIR: &[&str] = &[
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

    fn tree_sitter(self) -> Language {
        match self {
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

#[derive(Clone)]
struct ParsedFile {
    source: Vec<u8>,
    tree: Tree,
    loc: usize,
    complexity: usize,
    identifiers: BTreeMap<String, usize>,
    symbols: Vec<SymbolRow>,
}

#[derive(Clone, Debug)]
struct PythonImport {
    from: bool,
    level: usize,
    module: String,
    names: Vec<(String, Option<String>)>,
}

pub fn build(repo: &Path, pkg: &str, language: LanguageKind) -> Result<GraphData> {
    ensure!(
        repo.is_dir(),
        "repository {} is not a directory",
        repo.display()
    );
    let files = source_files(repo, pkg, language)?;
    let mut parser = Parser::new();
    parser
        .set_language(&language.tree_sitter())
        .context("initialize tree-sitter parser")?;

    let mut parsed = BTreeMap::new();
    for file in &files {
        let source = fs::read(repo.join(file)).with_context(|| format!("read {file}"))?;
        let Some(tree) = parser.parse(&source, None) else {
            continue;
        };
        // ast.parse rejects a Python file as a unit. Matching that behavior is
        // important: accepting the valid half would guess edges upward.
        if language == LanguageKind::Python && tree.root_node().has_error() {
            continue;
        }
        let (complexity, identifiers, symbols) = match language {
            LanguageKind::Python => python_metrics(tree.root_node(), &source),
            LanguageKind::Go => multi_metrics(tree.root_node(), &source, language),
            LanguageKind::TypeScript => multi_metrics(tree.root_node(), &source, language),
        };
        parsed.insert(
            file.clone(),
            ParsedFile {
                loc: source.iter().filter(|&&byte| byte == b'\n').count() + 1,
                source,
                tree,
                complexity,
                identifiers,
                symbols,
            },
        );
    }

    match language {
        LanguageKind::Python => build_python(repo, pkg, parsed),
        LanguageKind::Go | LanguageKind::TypeScript => build_multi(repo, pkg, language, parsed),
    }
}

fn source_files(repo: &Path, pkg: &str, language: LanguageKind) -> Result<Vec<String>> {
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
                name.ends_with(".ts")
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
                if !name.starts_with('_') {
                    symbols.push(symbol_row(name, kind, node));
                }
            }
        }
    }
    symbols.sort_by_key(|symbol| symbol.0 .2);
    symbols.truncate(60);
    (complexity, identifiers, symbols)
}

fn build_python(repo: &Path, pkg: &str, parsed: BTreeMap<String, ParsedFile>) -> Result<GraphData> {
    let mut modules = BTreeMap::<String, String>::new();
    for file in parsed.keys() {
        modules.insert(module_name(file, pkg), file.clone());
    }
    let known = modules.keys().cloned().collect::<BTreeSet<_>>();
    let file_of = modules.clone();

    let mut static_edges = BTreeMap::<(String, String), f64>::new();
    let mut directed = BTreeMap::<(String, String), f64>::new();
    let mut fanin = BTreeMap::<String, f64>::new();
    let mut uses = BTreeSet::<(String, String, String)>::new();
    for (module, file) in &modules {
        let parsed_file = &parsed[file];
        let imports = python_imports(parsed_file.tree.root_node(), &parsed_file.source);
        for import in &imports {
            for target in resolve_python(import, module, &known) {
                let target_file = &file_of[&target];
                if target_file == file {
                    continue;
                }
                *static_edges
                    .entry(ordered_pair(file, target_file))
                    .or_default() += 1.0;
                *directed
                    .entry((file.clone(), target_file.clone()))
                    .or_default() += 1.0;
                *fanin.entry(target_file.clone()).or_default() += 1.0;
            }
        }
        for (target_file, name) in python_uses(
            parsed_file.tree.root_node(),
            &parsed_file.source,
            module,
            file,
            &imports,
            &known,
            &file_of,
        ) {
            uses.insert((file.clone(), target_file, name));
        }
    }

    finish_graph(
        repo,
        pkg,
        LanguageKind::Python,
        parsed,
        static_edges,
        directed,
        fanin,
        uses,
        |file| module_name(file, pkg),
    )
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

fn python_imports(root: Node<'_>, source: &[u8]) -> Vec<PythonImport> {
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

fn python_head(import: &PythonImport, current_module: &str) -> String {
    if import.level == 0 {
        return import.module.clone();
    }
    let parts = current_module.split('.').collect::<Vec<_>>();
    let keep = if import.level <= parts.len() {
        parts.len() - import.level + 1
    } else {
        0
    };
    let prefix = parts[..keep].join(".");
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
) -> BTreeSet<String> {
    let mut hits = Vec::new();
    if import.from {
        let head = python_head(import, current_module);
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

fn python_uses(
    root: Node<'_>,
    source: &[u8],
    current_module: &str,
    current_file: &str,
    imports: &[PythonImport],
    known: &BTreeSet<String>,
    file_of: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut aliases = BTreeMap::<String, String>::new();
    for import in imports {
        if import.from {
            let targets = resolve_python(import, current_module, known);
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
            let head = python_head(import, current_module);
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
            let Some(target_file) = aliases.get(text(object, source)) else {
                continue;
            };
            if target_file == current_file {
                continue;
            }
            if let Some(attribute) = child_text(node, "attribute", source) {
                result.push((target_file.clone(), attribute));
            }
        }
    }
    result
}

fn build_multi(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    parsed: BTreeMap<String, ParsedFile>,
) -> Result<GraphData> {
    let files = parsed.keys().cloned().collect::<BTreeSet<_>>();
    let mut by_directory = BTreeMap::<String, Vec<String>>::new();
    for file in &files {
        by_directory
            .entry(directory_name(file).to_owned())
            .or_default()
            .push(file.clone());
    }
    let go_module = if language == LanguageKind::Go {
        go_module_path(repo)?
    } else {
        String::new()
    };

    let mut static_edges = BTreeMap::<(String, String), f64>::new();
    let mut directed = BTreeMap::<(String, String), f64>::new();
    let mut fanin = BTreeMap::<String, f64>::new();
    let mut uses = BTreeSet::<(String, String, String)>::new();
    for (file, parsed_file) in &parsed {
        let imports = match language {
            LanguageKind::Go => go_imports(parsed_file.tree.root_node(), &parsed_file.source),
            LanguageKind::TypeScript => {
                typescript_imports(parsed_file.tree.root_node(), &parsed_file.source)
            }
            LanguageKind::Python => unreachable!(),
        };
        for path in imports {
            let targets = resolve_multi(language, &path, file, &go_module, &by_directory, &files);
            if targets.is_empty() {
                continue;
            }
            let share = 1.0 / targets.len() as f64;
            for target in targets {
                if &target == file {
                    continue;
                }
                *static_edges.entry(ordered_pair(file, &target)).or_default() += share;
                *directed.entry((file.clone(), target.clone())).or_default() += share;
                *fanin.entry(target).or_default() += share;
            }
        }
        let references = match language {
            LanguageKind::Go => go_selectors(parsed_file.tree.root_node(), &parsed_file.source),
            LanguageKind::TypeScript => {
                typescript_named(parsed_file.tree.root_node(), &parsed_file.source)
            }
            LanguageKind::Python => unreachable!(),
        };
        for (path, name) in references {
            for target in resolve_multi(language, &path, file, &go_module, &by_directory, &files) {
                if &target != file {
                    uses.insert((file.clone(), target, name.clone()));
                }
            }
        }
    }

    finish_graph(
        repo,
        pkg,
        language,
        parsed,
        static_edges,
        directed,
        fanin,
        uses,
        str::to_owned,
    )
}

fn go_module_path(repo: &Path) -> Result<String> {
    let path = repo.join("go.mod");
    if !path.is_file() {
        return Ok(String::new());
    }
    let contents = fs::read_to_string(path)?;
    Ok(contents
        .lines()
        .find_map(|line| line.strip_prefix("module "))
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
        .to_owned())
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

fn resolve_multi(
    language: LanguageKind,
    import: &str,
    source_file: &str,
    go_module: &str,
    by_directory: &BTreeMap<String, Vec<String>>,
    by_file: &BTreeSet<String>,
) -> Vec<String> {
    match language {
        LanguageKind::Go => {
            if go_module.is_empty() || !import.starts_with(go_module) {
                return Vec::new();
            }
            let relative = import[go_module.len()..].trim_start_matches('/');
            by_directory.get(relative).cloned().unwrap_or_default()
        }
        LanguageKind::TypeScript => {
            if !import.starts_with('.') {
                return Vec::new();
            }
            let base = normalize_relative(directory_name(source_file), import);
            [
                format!("{base}.ts"),
                format!("{base}/index.ts"),
                format!("{base}.tsx"),
                base,
            ]
            .into_iter()
            .find(|candidate| by_file.contains(candidate))
            .into_iter()
            .collect()
        }
        LanguageKind::Python => Vec::new(),
    }
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

fn finish_graph<F>(
    repo: &Path,
    pkg: &str,
    language: LanguageKind,
    parsed: BTreeMap<String, ParsedFile>,
    static_edges: BTreeMap<(String, String), f64>,
    directed: BTreeMap<(String, String), f64>,
    fanin: BTreeMap<String, f64>,
    uses: BTreeSet<(String, String, String)>,
    module_for: F,
) -> Result<GraphData>
where
    F: Fn(&str) -> String,
{
    let files = parsed.keys().cloned().collect::<Vec<_>>();
    let history = git_history(repo, &files, 4000)?;
    let semantic = semantic_vectors(&parsed);
    let mut candidates = static_edges.keys().cloned().collect::<BTreeSet<_>>();
    candidates.extend(history.cochange.keys().cloned());

    if files.len() <= 600 {
        for i in 0..files.len() {
            for j in i + 1..files.len() {
                add_semantic_candidate(&files[i], &files[j], &semantic, &mut candidates);
            }
        }
    } else {
        let mut by_directory = BTreeMap::<String, Vec<&String>>::new();
        for file in &files {
            by_directory
                .entry(directory_name(file).to_owned())
                .or_default()
                .push(file);
        }
        for directory_files in by_directory.values() {
            for i in 0..directory_files.len() {
                for j in i + 1..directory_files.len() {
                    add_semantic_candidate(
                        directory_files[i],
                        directory_files[j],
                        &semantic,
                        &mut candidates,
                    );
                }
            }
        }
    }

    let static_max = static_edges
        .values()
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let mut edges = Vec::new();
    for (a, b) in candidates {
        let static_signal = static_edges
            .get(&(a.clone(), b.clone()))
            .copied()
            .unwrap_or(0.0)
            / static_max;
        let cochange = history
            .cochange
            .get(&(a.clone(), b.clone()))
            .copied()
            .unwrap_or(0.0)
            .min(1.0);
        let proximity = proximity(&a, &b);
        let semantic_signal = cosine(&a, &b, &semantic);
        let weight =
            ALPHA * static_signal + BETA * cochange + GAMMA * proximity + DELTA * semantic_signal;
        if weight < 0.02 {
            continue;
        }
        edges.push(SignalEdge {
            a,
            b,
            weight: round_to(weight, 5),
            static_signal: round_to(static_signal, 4),
            cochange: round_to(cochange, 4),
            proximity: round_to(proximity, 4),
            semantic: round_to(semantic_signal, 4),
        });
    }

    let nodes = files
        .iter()
        .map(|file| {
            let value = &parsed[file];
            SourceNode {
                file: file.clone(),
                loc: value.loc,
                complexity: value.complexity,
                churn: history.churn.get(file).copied().unwrap_or(0),
                fanin: if language == LanguageKind::Python {
                    fanin.get(file).copied().unwrap_or(0.0)
                } else {
                    round_to(fanin.get(file).copied().unwrap_or(0.0), 2)
                },
                module: module_for(file),
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
        pkg: pkg.to_owned(),
        lang: language.as_str().to_owned(),
        imports: directed
            .into_iter()
            .map(|((a, b), value)| {
                let value = if language == LanguageKind::Python {
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
    })
}

fn add_semantic_candidate(
    a: &str,
    b: &str,
    semantic: &BTreeMap<String, BTreeMap<String, f64>>,
    candidates: &mut BTreeSet<(String, String)>,
) {
    let pair = ordered_pair(a, b);
    if !candidates.contains(&pair) && cosine(a, b, semantic) > 0.28 {
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

fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
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

fn git_history(repo: &Path, files: &[String], max_commits: usize) -> Result<GitHistory> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "log",
            &format!("-n{max_commits}"),
            "--no-merges",
            "--pretty=format:@%H",
            "--name-only",
        ])
        .output()
        .context("run git log for co-change")?;
    ensure!(
        output.status.success(),
        "git log failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file_set = files.iter().cloned().collect::<BTreeSet<_>>();
    let mut commits = Vec::<BTreeSet<String>>::new();
    let mut current = None::<BTreeSet<String>>;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
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
    fn relative_import_resolution_matches_python_levels() {
        let known = ["pkg.a".to_owned(), "pkg.a.b".to_owned()]
            .into_iter()
            .collect();
        let import = PythonImport {
            from: true,
            level: 2,
            module: String::new(),
            names: vec![("b".to_owned(), None)],
        };
        assert!(resolve_python(&import, "pkg.a.current", &known).contains("pkg.a.b"));
    }

    #[test]
    fn go_package_import_spreads_across_directory() {
        let files = ["x/a.go".to_owned(), "x/b.go".to_owned()]
            .into_iter()
            .collect();
        let directories = [(
            "x".to_owned(),
            vec!["x/a.go".to_owned(), "x/b.go".to_owned()],
        )]
        .into_iter()
        .collect();
        assert_eq!(
            resolve_multi(
                LanguageKind::Go,
                "example/x",
                "main.go",
                "example",
                &directories,
                &files
            )
            .len(),
            2
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
