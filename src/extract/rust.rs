//! Rust as a source language (issue #126, finding 57).
//!
//! Phase 1 ([`syntax`], [`metrics`]) reads one file's tree while it is alive:
//! its `mod` declarations, module-level items, `use` declarations, inherent
//! `impl` blocks, and the paths and identifiers it names. Phase 2
//! ([`resolve`]) needs every file at once: it builds each crate's module tree
//! from its root (`lib.rs`, `main.rs`, `[lib] path`, `[[bin]] path`, the
//! build script), following `mod foo;` to `foo.rs` or `foo/mod.rs` and
//! `#[path = "..."]` where one is given, and then resolves every `use` and
//! every qualified path against that tree.
//!
//! An edge means a name is used (owner decision, "Uses only"):
//! - A `use` leaf links the file that defines the item it brings in, once
//!   the file references the name, or always when the `use` is itself a
//!   re-export (`pub use`, any visibility). `pub use` chains are followed to
//!   the defining file for up to four hops, the depth of
//!   `PYTHON_REEXPORT_HOPS` and `TS_REEXPORT_HOPS`.
//! - A leaf that brings in a module (`use crate::store;`) links the files
//!   that define what the file selects through it (`store::keep`), as a Go
//!   import links the files that declare what it selects (finding 50).
//! - A qualified path written in full (`crate::schema::FileId`,
//!   `super::helper()`, `cli::run()`, `other_crate::f()`) links the file
//!   that defines what it names. `Type::method` links the type's file and,
//!   when exactly one inherent `impl` of that type declares `method`, the
//!   file that `impl` sits in.
//! - `mod foo;` is not an edge: it declares a namespace and uses nothing in
//!   it, which is also how SCIP counts it (a namespace symbol).
//!
//! Where a chain cannot be followed with certainty -- a name the module does
//! not declare visibly (macro-generated), a glob re-export in the module the
//! path names, a re-export of something outside the parsed set -- the file of
//! the module the path named is linked instead, and never a module further
//! along the chain. That is the rule the Python resolver keeps for a
//! package's uncertain re-exports (finding 49's celery correction).
//!
//! Deliberately unresolved, so the graph stays a lower bound (CLAUDE.md):
//! - glob imports (`use a::*`) in the importing file: nothing says which of
//!   the module's names the file uses;
//! - items a macro generates, and paths inside `macro_rules!` bodies;
//! - trait methods called on a value (`x.run()`), and every other method or
//!   field reached on a value: syntax cannot say which type `x` has. This is
//!   Go's "member via value" (finding 50);
//! - `Self::`, `<T as Trait>::` and `::global` paths.
//!
//! `cfg` alternatives follow the Go rule of finding 54. When one name
//! resolves to items or modules in several files, the tie is broken only if
//! every alternative's `cfg` evaluates for [`RUST_CFG_TARGET`], exactly one
//! file's alternative is in, and the importing module is itself in.
//! Otherwise every alternative stays linked. A name one file declares
//! resolves to it whatever its `cfg` says. `feature`, `test`,
//! `debug_assertions` and custom cfgs are unknown: a crate's features are
//! whatever its dependents enable, and rust-analyzer (the oracle) sets `test`
//! and `debug_assertions` where `cargo build` does not.
//!
//! Each `use` declaration shares a mass of 1 among the files its leaves
//! reach, the importer included, and the self-pair is then skipped (finding
//! 52). Qualified paths written in full share a mass of 1 per module they go
//! through, the unit a Go import has.
//!
//! Everything here iterates BTreeMaps, BTreeSets and vectors in source or
//! file order, so a map is byte-identical from run to run (finding 9).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tree_sitter::Node;

use super::{
    directory_name, file_ids, join_slash, ordered_file_pair, relative_slash, symbol_row, text,
    FileRaw, ParsedFile, SourceIntermediate, KIND_CLASS, KIND_CONST, KIND_FUNC, KIND_INTERFACE,
    KIND_METHOD, KIND_TYPE, MULTI_SKIP_DIR,
};
use crate::extract::LanguageKind;
use crate::schema::{FileId, SymbolRow};

/// The one target `cfg` ties are broken for: a plain `cargo build` on a
/// 64-bit Linux runner, the platform the oracle (rust-analyzer, finding 55)
/// runs on in CI.
pub(crate) const RUST_CFG_TARGET: &str = "x86_64-unknown-linux-gnu";

/// Directories the Rust walk skips on top of the Go/TypeScript list:
/// `target/` is cargo's build output, and `benches/` holds benchmark crates,
/// scaffolding like `tests/` and `examples/`.
pub(crate) const RUST_SKIP_DIR: &[&str] = &["target", "benches"];

pub(crate) fn skip_dir(name: &str) -> bool {
    MULTI_SKIP_DIR.contains(&name) || RUST_SKIP_DIR.contains(&name) || name.starts_with('.')
}

/// How far a `pub use` chain is followed, as for Python and TypeScript.
const RUST_REEXPORT_HOPS: usize = 4;

// ---------------------------------------------------------------------
// cfg
// ---------------------------------------------------------------------

/// Whether something is compiled for [`RUST_CFG_TARGET`]. Three-valued, as
/// `go_build::GoBuild` is: `Unknown` where the answer depends on something
/// the source does not settle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Cfg {
    #[default]
    In,
    Out,
    Unknown,
}

impl Cfg {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Out, _) | (_, Self::Out) => Self::Out,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::In,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::In, _) | (_, Self::In) => Self::In,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::Out,
        }
    }

    fn not(self) -> Self {
        match self {
            Self::In => Self::Out,
            Self::Out => Self::In,
            Self::Unknown => Self::Unknown,
        }
    }
}

fn truth(value: bool) -> Cfg {
    if value {
        Cfg::In
    } else {
        Cfg::Out
    }
}

/// One cfg option, for [`RUST_CFG_TARGET`].
fn cfg_option(name: &str, value: Option<&str>) -> Cfg {
    match (name, value) {
        ("unix", None) => Cfg::In,
        ("windows", None) => Cfg::Out,
        // Set only by rustdoc and by docs.rs builds.
        ("doc" | "docsrs", None) => Cfg::Out,
        ("target_os", Some(value)) => truth(value == "linux"),
        ("target_family", Some(value)) => truth(value == "unix"),
        ("target_arch", Some(value)) => truth(value == "x86_64"),
        ("target_pointer_width", Some(value)) => truth(value == "64"),
        ("target_endian", Some(value)) => truth(value == "little"),
        ("target_env", Some(value)) => truth(value == "gnu"),
        ("target_vendor", Some(value)) => truth(value == "unknown"),
        // `feature`, `test`, `debug_assertions`, `panic`, `target_feature`,
        // `target_has_atomic` and custom cfgs (`loom`, `tokio_unstable`).
        _ => Cfg::Unknown,
    }
}

/// Splits a cfg predicate's token tree into identifiers, string literals
/// (kept with their opening quote), and `(`, `)`, `,`, `=`.
fn cfg_tokens(source: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if matches!(c, '(' | ')' | ',' | '=') {
            tokens.push(c.to_string());
        } else if c == '"' {
            let mut value = String::from("\"");
            for (_, next) in chars.by_ref() {
                if next == '"' {
                    break;
                }
                value.push(next);
            }
            tokens.push(value);
        } else if c.is_alphanumeric() || c == '_' {
            let mut end = start + c.len_utf8();
            while let Some(&(index, next)) = chars.peek() {
                if next.is_alphanumeric() || next == '_' {
                    end = index + next.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(source[start..end].to_owned());
        } else {
            // Anything else makes the predicate unreadable here.
            tokens.push(c.to_string());
        }
    }
    tokens
}

fn cfg_predicate(tokens: &[String], position: &mut usize) -> Option<Cfg> {
    let name = tokens.get(*position)?.clone();
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    *position += 1;
    match tokens.get(*position).map(String::as_str) {
        Some("=") => {
            *position += 1;
            let value = tokens.get(*position)?.strip_prefix('"')?.to_owned();
            *position += 1;
            Some(cfg_option(&name, Some(&value)))
        }
        Some("(") => {
            *position += 1;
            let mut values = Vec::new();
            loop {
                match tokens.get(*position).map(String::as_str) {
                    Some(")") => {
                        *position += 1;
                        break;
                    }
                    Some(",") => *position += 1,
                    Some(_) => values.push(cfg_predicate(tokens, position)?),
                    None => return None,
                }
            }
            match name.as_str() {
                "all" => Some(values.into_iter().fold(Cfg::In, Cfg::and)),
                "any" => Some(values.into_iter().fold(Cfg::Out, Cfg::or)),
                "not" if values.len() == 1 => Some(values[0].not()),
                _ => None,
            }
        }
        _ => Some(cfg_option(&name, None)),
    }
}

/// A `#[cfg(...)]` attribute's token tree, `(predicate)`.
pub(crate) fn eval_cfg(token_tree: &str) -> Cfg {
    let tokens = cfg_tokens(token_tree);
    let mut position = 0;
    if tokens.first().map(String::as_str) != Some("(") {
        return Cfg::Unknown;
    }
    position += 1;
    let Some(value) = cfg_predicate(&tokens, &mut position) else {
        return Cfg::Unknown;
    };
    if tokens.get(position).map(String::as_str) == Some(")") && position + 1 == tokens.len() {
        value
    } else {
        Cfg::Unknown
    }
}

// ---------------------------------------------------------------------
// Phase 1: one file's syntax
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ItemKind {
    /// struct, enum, union or type alias: may be followed by `::assoc`.
    Type,
    Trait,
    /// fn, const or static.
    Value,
    Macro,
}

/// A `mod` declaration. `scope` is the inline modules it sits in, from the
/// file's root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModDecl {
    pub(crate) scope: Vec<String>,
    pub(crate) name: String,
    pub(crate) inline: bool,
    pub(crate) path: Option<String>,
    pub(crate) cfg: Cfg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ItemDecl {
    pub(crate) scope: Vec<String>,
    pub(crate) name: String,
    pub(crate) kind: ItemKind,
    pub(crate) cfg: Cfg,
}

/// One name a `use` declaration brings in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UseLeaf {
    /// The name it binds; `None` for `as _` and for a glob.
    pub(crate) local: Option<String>,
    pub(crate) segments: Vec<String>,
    pub(crate) glob: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UseDecl {
    pub(crate) scope: Vec<String>,
    /// Any visibility: `pub`, `pub(crate)`, `pub(super)`, `pub(in ..)`.
    pub(crate) public: bool,
    /// `extern crate x [as y];`: the path names a crate, never a local item.
    pub(crate) extern_crate: bool,
    pub(crate) cfg: Cfg,
    pub(crate) leaves: Vec<UseLeaf>,
}

/// An inherent `impl Type { .. }` (no trait): which methods it declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImplDecl {
    pub(crate) scope: Vec<String>,
    pub(crate) self_path: Vec<String>,
    pub(crate) methods: BTreeSet<String>,
}

/// What resolution needs from one Rust file, captured while its tree is
/// alive (see `FileRaw`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RustSyntax {
    pub(crate) mods: Vec<ModDecl>,
    pub(crate) items: Vec<ItemDecl>,
    pub(crate) uses: Vec<UseDecl>,
    pub(crate) impls: Vec<ImplDecl>,
    /// Maximal qualified paths outside `use` declarations, attributes and
    /// `macro_rules!` bodies, by scope. Macro arguments are read token by
    /// token: `a::b::c` there is a path too.
    pub(crate) paths: BTreeMap<Vec<String>, BTreeSet<Vec<String>>>,
    /// Identifiers that could name something a `use` brought in, by scope:
    /// every identifier outside `use` declarations and attributes that is
    /// not the last segment of a qualified path.
    pub(crate) idents: BTreeMap<Vec<String>, BTreeSet<String>>,
}

#[derive(Default)]
struct Attrs {
    cfg: Cfg,
    path: Option<String>,
}

fn string_value(node: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = text(node, source);
    let raw = raw.strip_prefix('r').unwrap_or(raw);
    let raw = raw.trim_matches('#');
    Some(raw.strip_prefix('"')?.strip_suffix('"')?.to_owned())
}

fn read_attribute(item: Node<'_>, source: &[u8], attrs: &mut Attrs) {
    let mut cursor = item.walk();
    let Some(attribute) = item
        .named_children(&mut cursor)
        .find(|child| child.kind() == "attribute")
    else {
        return;
    };
    let mut cursor = attribute.walk();
    let name = attribute
        .named_children(&mut cursor)
        .find(|child| matches!(child.kind(), "identifier" | "scoped_identifier"))
        .map(|child| text(child, source));
    match name {
        Some("cfg") => {
            let value = attribute
                .child_by_field_name("arguments")
                .map_or(Cfg::Unknown, |arguments| eval_cfg(text(arguments, source)));
            attrs.cfg = attrs.cfg.and(value);
        }
        Some("path") => {
            if let Some(value) = attribute
                .child_by_field_name("value")
                .and_then(|value| string_value(value, source))
            {
                attrs.path = Some(value);
            }
        }
        _ => {}
    }
}

/// The segments of a path node, or `None` for a path this resolver does not
/// follow: `::global`, `<T as Trait>::x`, and macro metavariables.
fn path_segments(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "type_identifier" | "crate" | "self" | "super" => {
            Some(vec![text(node, source).to_owned()])
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            let mut segments = path_segments(node.child_by_field_name("path")?, source)?;
            segments.push(text(node.child_by_field_name("name")?, source).to_owned());
            Some(segments)
        }
        "generic_type" | "generic_type_with_turbofish" => {
            path_segments(node.child_by_field_name("type")?, source)
        }
        _ => None,
    }
}

fn use_leaves(node: Node<'_>, source: &[u8], prefix: &[String], out: &mut Vec<UseLeaf>) {
    match node.kind() {
        "identifier" | "crate" | "self" | "super" | "scoped_identifier" => {
            let Some(segments) = path_segments(node, source) else {
                return;
            };
            let mut full = prefix.to_vec();
            full.extend(segments);
            // `use a::{self}` binds `a`.
            if full.len() > 1 && full.last().map(String::as_str) == Some("self") {
                full.pop();
            }
            let local = full
                .last()
                .filter(|name| !matches!(name.as_str(), "crate" | "self" | "super"))
                .cloned();
            out.push(UseLeaf {
                local,
                segments: full,
                glob: false,
            });
        }
        "use_as_clause" => {
            let Some(segments) = node
                .child_by_field_name("path")
                .and_then(|path| path_segments(path, source))
            else {
                return;
            };
            let mut full = prefix.to_vec();
            full.extend(segments);
            if full.len() > 1 && full.last().map(String::as_str) == Some("self") {
                full.pop();
            }
            let local = node
                .child_by_field_name("alias")
                .map(|alias| text(alias, source).to_owned())
                .filter(|alias| alias != "_");
            out.push(UseLeaf {
                local,
                segments: full,
                glob: false,
            });
        }
        "use_list" => {
            let mut cursor = node.walk();
            let children = node.named_children(&mut cursor).collect::<Vec<_>>();
            for child in children {
                use_leaves(child, source, prefix, out);
            }
        }
        "scoped_use_list" => {
            let mut next = prefix.to_vec();
            if let Some(path) = node.child_by_field_name("path") {
                let Some(segments) = path_segments(path, source) else {
                    return;
                };
                next.extend(segments);
            }
            if let Some(list) = node.child_by_field_name("list") {
                use_leaves(list, source, &next, out);
            }
        }
        "use_wildcard" => {
            let mut cursor = node.walk();
            let path = node.named_children(&mut cursor).next();
            let mut full = prefix.to_vec();
            if let Some(path) = path {
                let Some(segments) = path_segments(path, source) else {
                    return;
                };
                full.extend(segments);
            }
            out.push(UseLeaf {
                local: None,
                segments: full,
                glob: true,
            });
        }
        _ => {}
    }
}

#[derive(Clone, Copy)]
struct VisitContext {
    cfg: Cfg,
    /// Items here are module items (the file's root, an inline module, an
    /// `extern` block), not items local to a function body or an `impl`.
    item_level: bool,
}

struct Visitor<'a> {
    source: &'a [u8],
    scope: Vec<String>,
    out: RustSyntax,
}

impl Visitor<'_> {
    fn children(&mut self, node: Node<'_>, context: VisitContext) {
        let mut cursor = node.walk();
        let children = node.named_children(&mut cursor).collect::<Vec<_>>();
        // Outer attributes are siblings before the item they apply to.
        let mut attrs = Attrs::default();
        for child in children {
            match child.kind() {
                "attribute_item" => read_attribute(child, self.source, &mut attrs),
                "line_comment" | "block_comment" => {}
                _ => {
                    let taken = std::mem::take(&mut attrs);
                    self.visit(child, context, taken);
                }
            }
        }
    }

    fn item(&mut self, node: Node<'_>, kind: ItemKind, cfg: Cfg) {
        if let Some(name) = node.child_by_field_name("name") {
            self.out.items.push(ItemDecl {
                scope: self.scope.clone(),
                name: text(name, self.source).to_owned(),
                kind,
                cfg,
            });
        }
    }

    fn ident(&mut self, node: Node<'_>) {
        self.out
            .idents
            .entry(self.scope.clone())
            .or_default()
            .insert(text(node, self.source).to_owned());
    }

    fn path(&mut self, segments: Vec<String>) {
        if segments.len() >= 2 {
            self.out
                .paths
                .entry(self.scope.clone())
                .or_default()
                .insert(segments);
        }
    }

    /// A path's head identifier is a reference (it may be a `use`d name);
    /// its later segments are not. Generic arguments inside it are walked.
    fn path_parts(&mut self, node: Node<'_>, context: VisitContext) {
        match node.kind() {
            "scoped_identifier" | "scoped_type_identifier" => {
                if let Some(path) = node.child_by_field_name("path") {
                    self.path_parts(path, context);
                }
            }
            "generic_type" | "generic_type_with_turbofish" => {
                if let Some(inner) = node.child_by_field_name("type") {
                    self.path_parts(inner, context);
                }
                if let Some(arguments) = node.child_by_field_name("type_arguments") {
                    self.children(arguments, context);
                }
            }
            "identifier" | "type_identifier" => self.ident(node),
            "bracketed_type" => self.children(node, context),
            _ => {}
        }
    }

    /// A macro's arguments are a flat token tree: `a :: b :: c` is read as
    /// the path `a::b::c`, and a run's first identifier as a reference.
    fn tokens(&mut self, tree: Node<'_>) {
        let mut cursor = tree.walk();
        let tokens = tree.children(&mut cursor).collect::<Vec<_>>();
        let mut run = Vec::<String>::new();
        let mut after_separator = false;
        for token in tokens {
            match token.kind() {
                "identifier" | "crate" | "self" | "super" => {
                    if !run.is_empty() && !after_separator {
                        self.path(std::mem::take(&mut run));
                    }
                    if run.is_empty() && token.kind() == "identifier" {
                        self.ident(token);
                    }
                    run.push(text(token, self.source).to_owned());
                    after_separator = false;
                }
                "::" => after_separator = !run.is_empty(),
                "token_tree" => {
                    self.path(std::mem::take(&mut run));
                    after_separator = false;
                    self.tokens(token);
                }
                _ => {
                    self.path(std::mem::take(&mut run));
                    after_separator = false;
                }
            }
        }
        self.path(run);
    }

    fn visit(&mut self, node: Node<'_>, context: VisitContext, attrs: Attrs) {
        let cfg = context.cfg.and(attrs.cfg);
        let inner = VisitContext {
            cfg,
            item_level: false,
        };
        match node.kind() {
            "use_declaration" => {
                let mut cursor = node.walk();
                let public = node
                    .named_children(&mut cursor)
                    .any(|child| child.kind() == "visibility_modifier");
                let mut leaves = Vec::new();
                if let Some(argument) = node.child_by_field_name("argument") {
                    use_leaves(argument, self.source, &[], &mut leaves);
                }
                self.out.uses.push(UseDecl {
                    scope: self.scope.clone(),
                    public,
                    extern_crate: false,
                    cfg,
                    leaves,
                });
            }
            "extern_crate_declaration" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return;
                };
                let name = text(name, self.source).to_owned();
                let local = node
                    .child_by_field_name("alias")
                    .map_or_else(|| name.clone(), |alias| text(alias, self.source).to_owned());
                let mut cursor = node.walk();
                let public = node
                    .named_children(&mut cursor)
                    .any(|child| child.kind() == "visibility_modifier");
                self.out.uses.push(UseDecl {
                    scope: self.scope.clone(),
                    public,
                    extern_crate: true,
                    cfg,
                    leaves: vec![UseLeaf {
                        local: Some(local).filter(|local| local != "_"),
                        segments: vec![name],
                        glob: false,
                    }],
                });
            }
            "attribute_item" | "inner_attribute_item" | "visibility_modifier" => {}
            "line_comment" | "block_comment" => {}
            "macro_definition" => {
                if context.item_level {
                    self.item(node, ItemKind::Macro, cfg);
                }
            }
            "mod_item" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return;
                };
                let name = text(name, self.source).to_owned();
                let body = node.child_by_field_name("body");
                if context.item_level {
                    self.out.mods.push(ModDecl {
                        scope: self.scope.clone(),
                        name: name.clone(),
                        inline: body.is_some(),
                        path: attrs.path,
                        cfg,
                    });
                }
                if let Some(body) = body {
                    if context.item_level {
                        self.scope.push(name);
                        self.children(
                            body,
                            VisitContext {
                                cfg,
                                item_level: true,
                            },
                        );
                        self.scope.pop();
                    } else {
                        self.children(body, inner);
                    }
                }
            }
            "function_item" | "function_signature_item" | "const_item" | "static_item" => {
                if context.item_level {
                    self.item(node, ItemKind::Value, cfg);
                }
                self.children(node, inner);
            }
            "struct_item" | "enum_item" | "union_item" | "type_item" => {
                if context.item_level {
                    self.item(node, ItemKind::Type, cfg);
                }
                self.children(node, inner);
            }
            "trait_item" => {
                if context.item_level {
                    self.item(node, ItemKind::Trait, cfg);
                }
                self.children(node, inner);
            }
            "impl_item" => {
                if context.item_level && node.child_by_field_name("trait").is_none() {
                    if let Some(self_path) = node
                        .child_by_field_name("type")
                        .and_then(|ty| path_segments(ty, self.source))
                    {
                        let mut methods = BTreeSet::new();
                        if let Some(body) = node.child_by_field_name("body") {
                            let mut cursor = body.walk();
                            for child in body.named_children(&mut cursor) {
                                if child.kind() == "function_item" {
                                    if let Some(name) = child.child_by_field_name("name") {
                                        methods.insert(text(name, self.source).to_owned());
                                    }
                                }
                            }
                        }
                        self.out.impls.push(ImplDecl {
                            scope: self.scope.clone(),
                            self_path,
                            methods,
                        });
                    }
                }
                self.children(node, inner);
            }
            "foreign_mod_item" => {
                // `extern "C" { fn f(); }` declares module items.
                if let Some(body) = node.child_by_field_name("body") {
                    self.children(
                        body,
                        VisitContext {
                            cfg,
                            item_level: context.item_level,
                        },
                    );
                }
            }
            "scoped_identifier" | "scoped_type_identifier" => {
                if let Some(segments) = path_segments(node, self.source) {
                    self.path(segments);
                }
                self.path_parts(node, inner);
            }
            "identifier" | "type_identifier" => self.ident(node),
            "macro_invocation" => {
                // The macro's own name is not followed: macros are
                // deliberately unresolved.
                let mut cursor = node.walk();
                let trees = node
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "token_tree")
                    .collect::<Vec<_>>();
                for tree in trees {
                    self.tokens(tree);
                }
            }
            "token_tree" => self.tokens(node),
            _ => self.children(
                node,
                VisitContext {
                    cfg,
                    item_level: context.item_level && node.kind() == "source_file",
                },
            ),
        }
    }
}

pub(crate) fn syntax(root: Node<'_>, source: &[u8]) -> RustSyntax {
    let mut visitor = Visitor {
        source,
        scope: Vec::new(),
        out: RustSyntax::default(),
    };
    visitor.children(
        root,
        VisitContext {
            cfg: Cfg::In,
            item_level: true,
        },
    );
    visitor.out
}

/// Branch points for the complexity proxy: the Rust forms of the Go and
/// TypeScript `BRANCHY` list. A `binary_expression` counts only for `&&` and
/// `||`; Rust's arithmetic is not a branch.
const RUST_BRANCHY: &[&str] = &[
    "if_expression",
    "match_arm",
    "while_expression",
    "for_expression",
    "loop_expression",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetricContext {
    Plain,
    /// An `impl` or `trait` item: its declaration list holds members.
    Owner,
    Member,
}

/// Per-file metrics and the map's `S` rows: fn, struct, enum, union, trait,
/// methods (a function in an `impl` or `trait` body), inline `mod`,
/// const/static, type alias and `macro_rules!`. Kinds use the map's
/// existing codes, so there is no schema change: struct, enum and union are
/// classes (they carry methods), a trait is an interface, an inline module
/// and a type alias are types, and a macro is a function.
pub(crate) fn metrics(
    root: Node<'_>,
    source: &[u8],
) -> (usize, BTreeMap<String, usize>, Vec<SymbolRow>) {
    let mut complexity = 0;
    let mut identifiers = BTreeMap::<String, usize>::new();
    let mut symbols = Vec::new();
    let mut stack = vec![(root, MetricContext::Plain)];
    while let Some((node, context)) = stack.pop() {
        let kind = node.kind();
        if RUST_BRANCHY.contains(&kind) {
            complexity += 1;
        } else if kind == "binary_expression" {
            if let Some(operator) = node.child_by_field_name("operator") {
                if matches!(text(operator, source), "&&" | "||") {
                    complexity += 1;
                }
            }
        } else if matches!(kind, "identifier" | "type_identifier" | "field_identifier") {
            let value = text(node, source);
            if value.len() > 2 {
                *identifiers.entry(value.to_owned()).or_default() += 1;
            }
        }
        let symbol = match kind {
            "function_item" | "function_signature_item" => {
                Some(if context == MetricContext::Member {
                    KIND_METHOD
                } else {
                    KIND_FUNC
                })
            }
            "struct_item" | "enum_item" | "union_item" => Some(KIND_CLASS),
            "trait_item" => Some(KIND_INTERFACE),
            "type_item" => Some(KIND_TYPE),
            "mod_item" if node.child_by_field_name("body").is_some() => Some(KIND_TYPE),
            "const_item" | "static_item" => Some(KIND_CONST),
            "macro_definition" => Some(KIND_FUNC),
            _ => None,
        };
        if let Some(symbol) = symbol {
            if let Some(name) = node.child_by_field_name("name") {
                symbols.push(symbol_row(text(name, source).to_owned(), symbol, node));
            }
        }
        let child_context = match (kind, context) {
            ("impl_item" | "trait_item", _) => MetricContext::Owner,
            ("declaration_list", MetricContext::Owner) => MetricContext::Member,
            _ => MetricContext::Plain,
        };
        let mut cursor = node.walk();
        let children = node.children(&mut cursor).collect::<Vec<_>>();
        stack.extend(
            children
                .into_iter()
                .rev()
                .map(|child| (child, child_context)),
        );
    }
    symbols.sort_by_key(|symbol| symbol.0 .2);
    symbols.truncate(60);
    (complexity, identifiers, symbols)
}

// ---------------------------------------------------------------------
// Crates, from Cargo.toml
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TargetKind {
    Lib,
    Bin,
    Build,
}

/// One crate a package builds: its root file and the crates it can name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CrateTarget {
    /// The module-path prefix of its files: the lib's crate name, or
    /// `package[bin:name]` / `package[build]`, which no Rust path can spell.
    pub(crate) key: String,
    pub(crate) root: String,
    edition_2015: bool,
    /// Extern crate name -> the key of a workspace library it names.
    deps: BTreeMap<String, String>,
    /// Every dependency's extern name, workspace or not: a path headed by
    /// one is another crate's, never a glob-imported name.
    externs: BTreeSet<String>,
}

fn crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

fn manifests(repo: &Path) -> BTreeMap<String, toml::Value> {
    let mut found = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = entries
            .filter_map(std::result::Result::ok)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if !skip_dir(&name) {
                    stack.push(entry.path());
                }
            } else if file_type.is_file() && name == "Cargo.toml" {
                let Ok(here) = relative_slash(repo, &directory) else {
                    continue;
                };
                if let Some(value) = fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|text| text.parse::<toml::Value>().ok())
                {
                    found.insert(here, value);
                }
            }
        }
    }
    found
}

/// The nearest manifest at or above `directory` that declares
/// `[workspace]`.
fn workspace_of<'a>(
    directory: &str,
    manifests: &'a BTreeMap<String, toml::Value>,
) -> Option<(&'a String, &'a toml::Value)> {
    let mut current = directory.to_owned();
    loop {
        if let Some((key, value)) = manifests.get_key_value(&current) {
            if value.get("workspace").is_some() {
                return Some((key, value));
            }
        }
        if current.is_empty() {
            return None;
        }
        current = directory_name(&current).to_owned();
    }
}

fn str_at<'a>(value: &'a toml::Value, keys: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in keys {
        current = current.get(key)?;
    }
    current.as_str()
}

/// Every crate target of every package under `repo`, in a fixed order:
/// package directory, then lib, bins by name, build script.
pub(crate) fn crate_targets(repo: &Path) -> Vec<CrateTarget> {
    let manifests = manifests(repo);
    // Package directory -> (package name, lib crate name, edition 2015?).
    let mut packages = BTreeMap::<String, (String, String, bool)>::new();
    for (directory, manifest) in &manifests {
        let Some(package) = manifest.get("package") else {
            continue;
        };
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let lib_name = str_at(manifest, &["lib", "name"])
            .map(crate_ident)
            .unwrap_or_else(|| crate_ident(name));
        let edition = package
            .get("edition")
            .and_then(|edition| match edition {
                toml::Value::String(value) => Some(value.clone()),
                toml::Value::Table(table)
                    if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) =>
                {
                    workspace_of(directory, &manifests).and_then(|(_, workspace)| {
                        str_at(workspace, &["workspace", "package", "edition"]).map(str::to_owned)
                    })
                }
                _ => None,
            })
            // Cargo's default when a manifest names no edition.
            .unwrap_or_else(|| "2015".to_owned());
        packages.insert(
            directory.clone(),
            (name.to_owned(), lib_name, edition == "2015"),
        );
    }
    let mut targets = Vec::new();
    for (directory, (package, lib_name, edition_2015)) in &packages {
        let manifest = &manifests[directory];
        let exists = |relative: &str| repo.join(join_slash(directory, relative)).is_file();
        // Dependencies that are workspace libraries, by extern crate name.
        let mut deps = BTreeMap::new();
        let mut tables = Vec::new();
        for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(table) = manifest.get(key).and_then(toml::Value::as_table) {
                tables.push(table);
            }
        }
        if let Some(target) = manifest.get("target").and_then(toml::Value::as_table) {
            for platform in target.values() {
                for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(table) = platform.get(key).and_then(toml::Value::as_table) {
                        tables.push(table);
                    }
                }
            }
        }
        let mut externs = ["std", "core", "alloc", "proc_macro", "test"]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for table in tables {
            for (key, spec) in table {
                externs.insert(crate_ident(key));
                let (base, spec) =
                    if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                        let Some((root, workspace)) = workspace_of(directory, &manifests) else {
                            continue;
                        };
                        let Some(spec) = workspace
                            .get("workspace")
                            .and_then(|workspace| workspace.get("dependencies"))
                            .and_then(|deps| deps.get(key))
                        else {
                            continue;
                        };
                        (root.as_str(), spec)
                    } else {
                        (directory.as_str(), spec)
                    };
                let Some(path) = spec.get("path").and_then(toml::Value::as_str) else {
                    continue;
                };
                let Some((_, dep_lib, _)) = packages.get(&join_slash(base, path)) else {
                    continue;
                };
                let renamed = spec.get("package").is_some();
                let local = if renamed {
                    crate_ident(key)
                } else {
                    dep_lib.clone()
                };
                deps.insert(local, dep_lib.clone());
            }
        }
        let lib_path = str_at(manifest, &["lib", "path"])
            .map(str::to_owned)
            .or_else(|| exists("src/lib.rs").then(|| "src/lib.rs".to_owned()));
        let has_lib = lib_path.is_some();
        if let Some(path) = lib_path {
            targets.push((
                TargetKind::Lib,
                lib_name.clone(),
                CrateTarget {
                    key: lib_name.clone(),
                    root: join_slash(directory, &path),
                    edition_2015: *edition_2015,
                    deps: deps.clone(),
                    externs: externs.clone(),
                },
            ));
        }
        // Bins can also name their own package's library.
        let mut bin_deps = deps.clone();
        if has_lib {
            bin_deps.insert(lib_name.clone(), lib_name.clone());
        }
        let mut bins = BTreeMap::<String, String>::new();
        let autobins = manifest
            .get("package")
            .and_then(|package| package.get("autobins"))
            .and_then(toml::Value::as_bool)
            != Some(false);
        if autobins {
            if exists("src/main.rs") {
                bins.insert(package.clone(), "src/main.rs".to_owned());
            }
            let bin_dir = repo.join(join_slash(directory, "src/bin"));
            if let Ok(entries) = fs::read_dir(&bin_dir) {
                let mut entries = entries
                    .filter_map(std::result::Result::ok)
                    .collect::<Vec<_>>();
                entries.sort_by_key(|entry| entry.file_name());
                for entry in entries {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if let Some(stem) = name.strip_suffix(".rs") {
                        bins.insert(stem.to_owned(), format!("src/bin/{name}"));
                    } else if entry.path().join("main.rs").is_file() {
                        bins.insert(name.clone(), format!("src/bin/{name}/main.rs"));
                    }
                }
            }
        }
        if let Some(explicit) = manifest.get("bin").and_then(toml::Value::as_array) {
            for bin in explicit {
                let Some(name) = bin.get("name").and_then(toml::Value::as_str) else {
                    continue;
                };
                let path = bin
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        [
                            format!("src/bin/{name}.rs"),
                            format!("src/bin/{name}/main.rs"),
                            "src/main.rs".to_owned(),
                        ]
                        .into_iter()
                        .find(|candidate| exists(candidate))
                    });
                if let Some(path) = path {
                    bins.insert(name.to_owned(), path);
                }
            }
        }
        for (name, path) in bins {
            targets.push((
                TargetKind::Bin,
                name.clone(),
                CrateTarget {
                    key: format!("{package}[bin:{name}]"),
                    root: join_slash(directory, &path),
                    edition_2015: *edition_2015,
                    deps: bin_deps.clone(),
                    externs: externs.clone(),
                },
            ));
        }
        let build = match manifest
            .get("package")
            .and_then(|package| package.get("build"))
        {
            Some(toml::Value::String(path)) => Some(path.clone()),
            Some(toml::Value::Boolean(false)) => None,
            _ => exists("build.rs").then(|| "build.rs".to_owned()),
        };
        if let Some(path) = build {
            targets.push((
                TargetKind::Build,
                String::new(),
                CrateTarget {
                    key: format!("{package}[build]"),
                    root: join_slash(directory, &path),
                    edition_2015: *edition_2015,
                    deps: BTreeMap::new(),
                    externs: externs.clone(),
                },
            ));
        }
    }
    targets.into_iter().map(|(_, _, target)| target).collect()
}

// ---------------------------------------------------------------------
// Phase 2: module trees and resolution
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Module {
    krate: usize,
    file: FileId,
    scope: Vec<String>,
    path: Vec<String>,
    parent: Option<usize>,
    /// Where `mod x;` looks for `x.rs` and `x/mod.rs`.
    children_dir: String,
    cfg: Cfg,
    children: BTreeMap<String, Vec<usize>>,
    items: BTreeMap<String, Vec<(ItemKind, Cfg)>>,
    /// Local name -> (use declaration index in the file, leaf index).
    uses: BTreeMap<String, Vec<(usize, usize)>>,
    globs: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ItemRef {
    module: usize,
    name: String,
    kind: ItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Res {
    Module(usize),
    Item(ItemRef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Missing {
    /// The module neither declares nor imports the name, and has no glob.
    Undeclared,
    /// The module has a glob import the name may come from.
    Glob,
    /// The module binds the name through a chain that could not be
    /// followed to a definition.
    Uncertain,
}

/// Where a path leads.
#[derive(Clone, Debug, Default)]
struct Reach {
    /// Whether the module the path is written in is in the target build;
    /// a cfg tie is broken only for one that is (finding 54's rule).
    importer_in: bool,
    items: BTreeSet<ItemRef>,
    /// `(file, name, module the name was looked up in)` for every file the
    /// path reaches by name: the item's own, and an inherent `impl`'s for
    /// `Type::method`.
    targets: BTreeSet<(FileId, String, usize)>,
    /// Modules the path names itself.
    modules: BTreeSet<usize>,
    /// Modules where a name could not be followed with certainty.
    uncertain: BTreeSet<usize>,
    external: bool,
    unresolved: bool,
    glob: bool,
    tie_kept: bool,
    tie_broken: bool,
    /// The `use` leaf being resolved, `(module, declaration, leaf)`: its
    /// path's first segment never names itself. `use memchr::memchr;`
    /// binds `memchr`, and without this its head found that binding
    /// instead of the crate. rustc excludes an import from its own
    /// resolution the same way. Consumed by the first lookup.
    skip: Option<(usize, usize, usize)>,
}

impl Reach {
    fn for_importer(importer_in: bool) -> Self {
        Self {
            importer_in,
            ..Self::default()
        }
    }

    fn for_leaf(importer_in: bool, leaf: (usize, usize, usize)) -> Self {
        Self {
            importer_in,
            skip: Some(leaf),
            ..Self::default()
        }
    }
}

struct Resolver<'a> {
    files: &'a [String],
    syntax: &'a BTreeMap<FileId, &'a RustSyntax>,
    crates: &'a [CrateTarget],
    modules: Vec<Module>,
    by_scope: BTreeMap<(FileId, Vec<String>), usize>,
    /// Crate index -> its root module.
    roots: Vec<Option<usize>>,
    /// Lib crate key -> crate index.
    libs: BTreeMap<String, usize>,
    /// `(module, type)` -> method -> files of the inherent impls declaring it.
    impls: BTreeMap<(usize, String), BTreeMap<String, BTreeSet<FileId>>>,
}

fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".rs").unwrap_or(name)
}

impl<'a> Resolver<'a> {
    fn new(
        files: &'a [String],
        ids: &BTreeMap<String, FileId>,
        syntax: &'a BTreeMap<FileId, &'a RustSyntax>,
        crates: &'a [CrateTarget],
    ) -> Self {
        let mut resolver = Self {
            files,
            syntax,
            crates,
            modules: Vec::new(),
            by_scope: BTreeMap::new(),
            roots: vec![None; crates.len()],
            libs: BTreeMap::new(),
            impls: BTreeMap::new(),
        };
        for (index, krate) in crates.iter().enumerate() {
            if !krate.key.contains('[') {
                resolver.libs.entry(krate.key.clone()).or_insert(index);
            }
        }
        for (index, krate) in crates.iter().enumerate() {
            let Some(&root) = ids.get(&krate.root) else {
                continue;
            };
            if resolver.by_scope.contains_key(&(root, Vec::new())) {
                // A file is one module, of the first crate that reaches it.
                continue;
            }
            let id = resolver.add_module(Module {
                krate: index,
                file: root,
                scope: Vec::new(),
                path: Vec::new(),
                parent: None,
                children_dir: directory_name(&krate.root).to_owned(),
                cfg: Cfg::In,
                children: BTreeMap::new(),
                items: BTreeMap::new(),
                uses: BTreeMap::new(),
                globs: false,
            });
            resolver.roots[index] = Some(id);
            let mut queue = std::collections::VecDeque::from([id]);
            while let Some(current) = queue.pop_front() {
                for child in resolver.expand(current, ids) {
                    queue.push_back(child);
                }
            }
        }
        for module in &mut resolver.modules {
            let Some(syntax) = syntax.get(&module.file) else {
                continue;
            };
            for item in syntax
                .items
                .iter()
                .filter(|item| item.scope == module.scope)
            {
                module
                    .items
                    .entry(item.name.clone())
                    .or_default()
                    .push((item.kind, item.cfg));
            }
            for (use_index, decl) in syntax.uses.iter().enumerate() {
                if decl.scope != module.scope {
                    continue;
                }
                for (leaf_index, leaf) in decl.leaves.iter().enumerate() {
                    if leaf.glob {
                        module.globs = true;
                    } else if let Some(local) = &leaf.local {
                        module
                            .uses
                            .entry(local.clone())
                            .or_default()
                            .push((use_index, leaf_index));
                    }
                }
            }
        }
        // Inherent impls, keyed by the type item their self type resolves to.
        let mut impls = BTreeMap::<(usize, String), BTreeMap<String, BTreeSet<FileId>>>::new();
        for (&(file, ref scope), &module) in &resolver.by_scope {
            let Some(syntax) = syntax.get(&file) else {
                continue;
            };
            for decl in syntax.impls.iter().filter(|decl| &decl.scope == scope) {
                let mut reach = Reach::for_importer(false);
                resolver.follow(module, &decl.self_path, false, false, 0, &mut reach);
                let mut types = reach
                    .items
                    .iter()
                    .filter(|item| item.kind == ItemKind::Type);
                let (Some(item), None) = (types.next(), types.next()) else {
                    continue;
                };
                let methods = impls.entry((item.module, item.name.clone())).or_default();
                for method in &decl.methods {
                    methods.entry(method.clone()).or_default().insert(file);
                }
            }
        }
        resolver.impls = impls;
        resolver
    }

    fn add_module(&mut self, module: Module) -> usize {
        let id = self.modules.len();
        self.by_scope
            .insert((module.file, module.scope.clone()), id);
        self.modules.push(module);
        id
    }

    /// Adds `module`'s child modules; returns the ones that are new.
    fn expand(&mut self, module: usize, ids: &BTreeMap<String, FileId>) -> Vec<usize> {
        let (file, scope, children_dir, krate, path, cfg) = {
            let m = &self.modules[module];
            (
                m.file,
                m.scope.clone(),
                m.children_dir.clone(),
                m.krate,
                m.path.clone(),
                m.cfg,
            )
        };
        // Copied out of `self` so the borrow outlives `add_module` below.
        let syntax_map: &'a BTreeMap<FileId, &'a RustSyntax> = self.syntax;
        let Some(syntax) = syntax_map.get(&file) else {
            return Vec::new();
        };
        let mut added = Vec::new();
        for decl in syntax.mods.iter().filter(|decl| decl.scope == scope) {
            let (child_file, child_scope, child_dir) = if decl.inline {
                let mut child_scope = scope.clone();
                child_scope.push(decl.name.clone());
                (file, child_scope, join_slash(&children_dir, &decl.name))
            } else {
                let target = if let Some(relative) = &decl.path {
                    // Relative to the declaring file's directory, or, inside
                    // an inline module, to that module's directory.
                    let base = if scope.is_empty() {
                        directory_name(&self.files[file as usize]).to_owned()
                    } else {
                        children_dir.clone()
                    };
                    Some(join_slash(&base, relative))
                } else {
                    [
                        join_slash(&children_dir, &format!("{}.rs", decl.name)),
                        join_slash(&children_dir, &format!("{}/mod.rs", decl.name)),
                    ]
                    .into_iter()
                    .find(|candidate| ids.contains_key(candidate))
                };
                let Some((target, &child_file)) =
                    target.and_then(|target| ids.get(&target).map(|id| (target, id)))
                else {
                    continue;
                };
                // A file loaded through `#[path]`, or named `mod.rs`, owns
                // its directory; any other module file's children live in
                // a directory named after it.
                let mod_rs = decl.path.is_some() || file_stem(&target) == "mod";
                let dir = if mod_rs {
                    directory_name(&target).to_owned()
                } else {
                    join_slash(directory_name(&target), file_stem(&target))
                };
                (child_file, Vec::new(), dir)
            };
            let child = match self.by_scope.get(&(child_file, child_scope.clone())) {
                Some(&existing) => existing,
                None => {
                    let mut child_path = path.clone();
                    child_path.push(decl.name.clone());
                    let id = self.add_module(Module {
                        krate,
                        file: child_file,
                        scope: child_scope,
                        path: child_path,
                        parent: Some(module),
                        children_dir: child_dir,
                        cfg: cfg.and(decl.cfg),
                        children: BTreeMap::new(),
                        items: BTreeMap::new(),
                        uses: BTreeMap::new(),
                        globs: false,
                    });
                    added.push(id);
                    id
                }
            };
            self.modules[module]
                .children
                .entry(decl.name.clone())
                .or_default()
                .push(child);
        }
        added
    }

    fn file_of(&self, res: &Res) -> FileId {
        match res {
            Res::Module(module) => self.modules[*module].file,
            Res::Item(item) => self.modules[item.module].file,
        }
    }

    fn extern_crate(&self, from: usize, name: &str) -> Option<usize> {
        let key = self.crates[self.modules[from].krate].deps.get(name)?;
        let index = *self.libs.get(key)?;
        self.roots[index]
    }

    fn root(&self, module: usize) -> Option<usize> {
        self.roots[self.modules[module].krate]
    }

    /// What `name` means in module `module`: its child modules, its items
    /// and its `use` bindings, followed. A name bound in several files is a
    /// tie, broken as the module comment says.
    ///
    /// `intermediate` is a segment more segments follow, which Rust looks up
    /// in the type namespace only: modules, types and traits. Where the
    /// module declares one of those, its `use` bindings are not consulted.
    /// Without that, `mod escape; pub use crate::escape::escape;` (a module
    /// and a function re-exported from it under one name, which ripgrep
    /// does in five crates) sent `crate::escape` round the `use` binding it
    /// was resolving until the hop limit, and every path through the module
    /// stopped as uncertain.
    fn lookup(
        &self,
        module: usize,
        name: &str,
        intermediate: bool,
        depth: usize,
        reach: &mut Reach,
    ) -> std::result::Result<Vec<Res>, Missing> {
        let m = &self.modules[module];
        let mut found = Vec::<(Res, Cfg)>::new();
        for &child in m.children.get(name).into_iter().flatten() {
            found.push((Res::Module(child), self.modules[child].cfg));
        }
        for &(kind, cfg) in m.items.get(name).into_iter().flatten() {
            if intermediate && !matches!(kind, ItemKind::Type | ItemKind::Trait) {
                continue;
            }
            found.push((
                Res::Item(ItemRef {
                    module,
                    name: name.to_owned(),
                    kind,
                }),
                cfg,
            ));
        }
        let bindings = if intermediate && !found.is_empty() {
            None
        } else {
            m.uses.get(name)
        };
        for &(use_index, leaf_index) in bindings.into_iter().flatten() {
            if reach.skip == Some((module, use_index, leaf_index)) {
                continue;
            }
            if depth >= RUST_REEXPORT_HOPS {
                return Err(Missing::Uncertain);
            }
            let decl = &self.syntax[&m.file].uses[use_index];
            let leaf = &decl.leaves[leaf_index];
            let mut sub = Reach::for_leaf(reach.importer_in, (module, use_index, leaf_index));
            self.follow(
                module,
                &leaf.segments,
                decl.extern_crate,
                true,
                depth + 1,
                &mut sub,
            );
            if sub.external || sub.unresolved || sub.glob || !sub.uncertain.is_empty() {
                return Err(Missing::Uncertain);
            }
            reach.tie_kept |= sub.tie_kept;
            reach.tie_broken |= sub.tie_broken;
            let cfg = m.cfg.and(decl.cfg);
            for &target in &sub.modules {
                found.push((Res::Module(target), cfg));
            }
            for item in sub.items {
                found.push((Res::Item(item), cfg));
            }
        }
        if found.is_empty() {
            return Err(if m.globs {
                Missing::Glob
            } else {
                Missing::Undeclared
            });
        }
        let files = found
            .iter()
            .map(|(res, _)| self.file_of(res))
            .collect::<BTreeSet<_>>();
        if files.len() > 1 {
            let known = found.iter().all(|(_, cfg)| *cfg != Cfg::Unknown);
            let in_files = found
                .iter()
                .filter(|(_, cfg)| *cfg == Cfg::In)
                .map(|(res, _)| self.file_of(res))
                .collect::<BTreeSet<_>>();
            if reach.importer_in && known && in_files.len() == 1 {
                found.retain(|(res, _)| in_files.contains(&self.file_of(res)));
                reach.tie_broken = true;
            } else {
                reach.tie_kept = true;
            }
        }
        let mut result = found.into_iter().map(|(res, _)| res).collect::<Vec<_>>();
        result.sort();
        result.dedup();
        Ok(result)
    }

    /// Follows `segments` from module `from`. `extern_only` is an `extern
    /// crate` name; `in_use` a `use` path, which a 2015-edition crate
    /// resolves from its root.
    fn follow(
        &self,
        from: usize,
        segments: &[String],
        extern_only: bool,
        in_use: bool,
        depth: usize,
        reach: &mut Reach,
    ) {
        let skip = reach.skip.take();
        let Some((head, rest)) = segments.split_first() else {
            return;
        };
        let starts = match head.as_str() {
            "crate" => match self.root(from) {
                Some(root) => vec![Res::Module(root)],
                None => {
                    reach.unresolved = true;
                    return;
                }
            },
            "self" if !extern_only => vec![Res::Module(from)],
            "super" if !extern_only => match self.modules[from].parent {
                Some(parent) => vec![Res::Module(parent)],
                None => {
                    reach.unresolved = true;
                    return;
                }
            },
            "Self" | "self" | "super" => {
                reach.unresolved = true;
                return;
            }
            name => {
                let base = if in_use && self.crates[self.modules[from].krate].edition_2015 {
                    self.root(from)
                } else {
                    Some(from)
                };
                reach.skip = skip;
                let local = match base {
                    Some(base) if !extern_only => {
                        self.lookup(base, name, !rest.is_empty(), depth, reach)
                    }
                    _ => Err(Missing::Undeclared),
                };
                reach.skip = None;
                match local {
                    Ok(found) => found,
                    Err(Missing::Uncertain) => {
                        if let Some(base) = base {
                            reach.uncertain.insert(base);
                        }
                        return;
                    }
                    Err(missing) => match self.extern_crate(from, name) {
                        Some(root) => vec![Res::Module(root)],
                        None => {
                            // A glob could bind the head, unless it names a
                            // crate: std, core, alloc, or a dependency.
                            let known_crate =
                                self.crates[self.modules[from].krate].externs.contains(name);
                            if missing == Missing::Glob && !known_crate {
                                reach.glob = true;
                            } else {
                                reach.external = true;
                            }
                            return;
                        }
                    },
                }
            }
        };
        for start in starts {
            self.descend(start, rest, None, depth, reach);
        }
    }

    fn descend(
        &self,
        res: Res,
        rest: &[String],
        via: Option<usize>,
        depth: usize,
        reach: &mut Reach,
    ) {
        match res {
            Res::Module(module) => {
                let Some((name, tail)) = rest.split_first() else {
                    reach.modules.insert(module);
                    return;
                };
                if name == "super" {
                    match self.modules[module].parent {
                        Some(parent) => self.descend(Res::Module(parent), tail, via, depth, reach),
                        None => reach.unresolved = true,
                    }
                    return;
                }
                match self.lookup(module, name, !tail.is_empty(), depth, reach) {
                    Ok(found) => {
                        for next in found {
                            self.descend(next, tail, Some(module), depth, reach);
                        }
                    }
                    Err(missing) => {
                        reach.glob |= missing == Missing::Glob;
                        reach.uncertain.insert(module);
                    }
                }
            }
            Res::Item(item) => {
                let file = self.modules[item.module].file;
                let via = via.unwrap_or(item.module);
                reach.targets.insert((file, item.name.clone(), via));
                // `Type::method`: the one inherent impl declaring it.
                if let (ItemKind::Type, Some(method)) = (item.kind, rest.first()) {
                    if let Some(files) = self
                        .impls
                        .get(&(item.module, item.name.clone()))
                        .and_then(|methods| methods.get(method))
                    {
                        if files.len() == 1 {
                            let impl_file = *files.iter().next().expect("one file");
                            reach.targets.insert((impl_file, method.clone(), via));
                        }
                    }
                }
                reach.items.insert(item);
            }
        }
    }

    fn module_path(&self, module: usize) -> String {
        let m = &self.modules[module];
        let mut path = self.crates[m.krate].key.clone();
        for segment in &m.path {
            path.push_str("::");
            path.push_str(segment);
        }
        path
    }
}

/// How one `use` leaf or one written-out path resolved. Recorded only for
/// `TOLMAP_RUST_IMPORT_REPORT`; the graph needs only its targets.
fn outcome(reach: &Reach, linked: bool) -> &'static str {
    if !reach.uncertain.is_empty() {
        if reach.glob {
            "uncertain_glob"
        } else {
            "uncertain"
        }
    } else if !reach.targets.is_empty() {
        "defined"
    } else if !reach.modules.is_empty() {
        if linked {
            "module"
        } else {
            "module_unselected"
        }
    } else if reach.glob {
        "glob_scope"
    } else if reach.external {
        "external"
    } else {
        "unresolved"
    }
}

fn tie(reach: &Reach) -> Option<&'static str> {
    if reach.tie_kept {
        Some("kept")
    } else if reach.tie_broken {
        Some("broken")
    } else {
        None
    }
}

type Seen<T> = BTreeMap<Vec<String>, BTreeSet<T>>;

/// The identifiers and paths each scope's `use` leaves can be referenced
/// by: its own, and those of every inline module below it that brings
/// everything in with `use super::*` (a `mod tests` does). What `super::*`
/// binds in the child is exactly what the parent binds, so a name the
/// child uses is one of the parent's `use` leaves in use. Only for judging
/// the parent's leaves: the child's own paths still resolve from the child.
fn seen_from_children(syntax: &RustSyntax) -> (Seen<String>, Seen<Vec<String>>) {
    let mut idents = syntax.idents.clone();
    let mut paths = syntax.paths.clone();
    let mut globbing = syntax
        .uses
        .iter()
        .filter(|decl| {
            !decl.scope.is_empty()
                && decl
                    .leaves
                    .iter()
                    .any(|leaf| leaf.glob && leaf.segments == ["super"])
        })
        .map(|decl| decl.scope.clone())
        .collect::<Vec<_>>();
    // Deepest first, so a grandchild's names reach the grandparent.
    globbing.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    globbing.dedup();
    for scope in globbing {
        let parent = scope[..scope.len() - 1].to_vec();
        if let Some(names) = idents.get(&scope).cloned() {
            idents.entry(parent.clone()).or_default().extend(names);
        }
        if let Some(selected) = paths.get(&scope).cloned() {
            paths.entry(parent).or_default().extend(selected);
        }
    }
    (idents, paths)
}

struct Unit {
    targets: BTreeMap<FileId, BTreeSet<String>>,
}

impl Unit {
    fn new() -> Self {
        Self {
            targets: BTreeMap::new(),
        }
    }

    fn add(&mut self, file: FileId, name: &str) {
        self.targets
            .entry(file)
            .or_default()
            .insert(name.to_owned());
    }
}

pub(crate) fn resolve(
    repo: &Path,
    pkg: &str,
    parsed: BTreeMap<String, ParsedFile>,
    raw: BTreeMap<String, FileRaw>,
    progress: &crate::progress::StageCounter,
) -> Result<SourceIntermediate> {
    let (files, ids) = file_ids(parsed.keys().cloned())?;
    let syntax = raw
        .iter()
        .filter_map(|(file, raw)| match raw {
            FileRaw::Rust(syntax) => ids.get(file).map(|&id| (id, syntax)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let crates = crate_targets(repo);
    let resolver = Resolver::new(&files, &ids, &syntax, &crates);

    let mut static_edges = BTreeMap::<(FileId, FileId), f64>::new();
    let mut directed = BTreeMap::<(FileId, FileId), f64>::new();
    let mut fanin = BTreeMap::<FileId, f64>::new();
    let mut uses = BTreeSet::<(FileId, FileId, String)>::new();
    let report_dir = std::env::var_os("TOLMAP_RUST_IMPORT_REPORT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let mut report = Vec::new();

    let mut emit = |source: FileId, unit: &Unit| {
        if unit.targets.is_empty() {
            return;
        }
        // The importer counts in the share, and the self-pair is skipped
        // (finding 52).
        let share = 1.0 / unit.targets.len() as f64;
        for (&target, names) in &unit.targets {
            if target == source {
                continue;
            }
            *static_edges
                .entry(ordered_file_pair(source, target))
                .or_default() += share;
            *directed.entry((source, target)).or_default() += share;
            *fanin.entry(target).or_default() += share;
            for name in names {
                uses.insert((source, target, name.clone()));
            }
        }
    };

    for (&file, file_syntax) in &syntax {
        let path_of = |target: FileId| files[target as usize].as_str();
        let (idents_seen, paths_seen) = seen_from_children(file_syntax);
        // 1. `use` declarations: one unit each.
        for (use_index, decl) in file_syntax.uses.iter().enumerate() {
            let Some(&module) = resolver.by_scope.get(&(file, decl.scope.clone())) else {
                if report_dir.is_some() {
                    for leaf in &decl.leaves {
                        report.push(serde_json::json!([
                            &files[file as usize],
                            "use",
                            leaf.segments.join("::"),
                            leaf.local,
                            "no_module",
                            null,
                            [],
                            decl.scope.join("::"),
                        ]));
                    }
                }
                continue;
            };
            let importer_in = resolver.modules[module].cfg == Cfg::In;
            let idents = idents_seen.get(&decl.scope);
            let selected = paths_seen.get(&decl.scope);
            let mut unit = Unit::new();
            for (leaf_index, leaf) in decl.leaves.iter().enumerate() {
                let this_leaf = (module, use_index, leaf_index);
                let mut row_targets = BTreeSet::new();
                let (outcome_name, tie_name) = if leaf.glob {
                    ("glob", None)
                } else if let Some(local) = &leaf.local {
                    if !decl.public && !idents.is_some_and(|names| names.contains(local)) {
                        // Not linked. Followed only to tell a name from
                        // this repository (a possible miss: a trait used
                        // for its methods, or a name only a macro expands
                        // to) from another crate's.
                        let mut reach = Reach::for_leaf(importer_in, this_leaf);
                        resolver.follow(
                            module,
                            &leaf.segments,
                            decl.extern_crate,
                            true,
                            0,
                            &mut reach,
                        );
                        let in_repo = !reach.targets.is_empty()
                            || !reach.modules.is_empty()
                            || !reach.uncertain.is_empty();
                        (if in_repo { "unused" } else { "unused_external" }, None)
                    } else {
                        let mut reach = Reach::for_leaf(importer_in, this_leaf);
                        resolver.follow(
                            module,
                            &leaf.segments,
                            decl.extern_crate,
                            true,
                            0,
                            &mut reach,
                        );
                        let paths = selected
                            .into_iter()
                            .flatten()
                            .filter(|path| path.first() == Some(local))
                            .collect::<Vec<_>>();
                        let mut linked = false;
                        // A module brought in: the names the file selects
                        // through it.
                        for &target in &reach.modules {
                            for path in &paths {
                                let mut sub = Reach::for_importer(importer_in);
                                resolver.descend(
                                    Res::Module(target),
                                    &path[1..],
                                    None,
                                    0,
                                    &mut sub,
                                );
                                for (file_id, name, _) in &sub.targets {
                                    unit.add(*file_id, name);
                                    row_targets.insert(*file_id);
                                    linked = true;
                                }
                                for &uncertain in &sub.uncertain {
                                    let module_file = resolver.modules[uncertain].file;
                                    unit.add(module_file, path.last().map_or("", String::as_str));
                                    row_targets.insert(module_file);
                                    linked = true;
                                }
                                reach.tie_kept |= sub.tie_kept;
                                reach.tie_broken |= sub.tie_broken;
                            }
                        }
                        // An item brought in, and `Type::method` through it.
                        for item in &reach.items {
                            if item.kind != ItemKind::Type {
                                continue;
                            }
                            for path in &paths {
                                let mut sub = Reach::for_importer(importer_in);
                                resolver.descend(
                                    Res::Item(item.clone()),
                                    &path[1..],
                                    None,
                                    0,
                                    &mut sub,
                                );
                                for (file_id, name, _) in &sub.targets {
                                    unit.add(*file_id, name);
                                    row_targets.insert(*file_id);
                                }
                            }
                        }
                        for (file_id, name, _) in &reach.targets {
                            unit.add(*file_id, name);
                            row_targets.insert(*file_id);
                        }
                        // An uncertain chain links the module the path named.
                        for &uncertain in &reach.uncertain {
                            let module_file = resolver.modules[uncertain].file;
                            unit.add(module_file, local);
                            row_targets.insert(module_file);
                        }
                        (outcome(&reach, linked), tie(&reach))
                    }
                } else {
                    ("unnamed", None)
                };
                if report_dir.is_some() {
                    report.push(serde_json::json!([
                        &files[file as usize],
                        if decl.extern_crate {
                            "extern_crate"
                        } else {
                            "use"
                        },
                        leaf.segments.join("::"),
                        leaf.local,
                        outcome_name,
                        tie_name,
                        row_targets
                            .iter()
                            .map(|&id| path_of(id))
                            .collect::<Vec<_>>(),
                        decl.scope.join("::"),
                    ]));
                }
            }
            emit(file, &unit);
        }
        // 2. Paths written out in full: one unit per module they go through.
        for (scope, paths) in &file_syntax.paths {
            let Some(&module) = resolver.by_scope.get(&(file, scope.clone())) else {
                continue;
            };
            let importer_in = resolver.modules[module].cfg == Cfg::In;
            let mut units = BTreeMap::<usize, Unit>::new();
            for path in paths {
                if resolver.modules[module].uses.contains_key(&path[0]) {
                    // Counted with the `use` that brought the head in.
                    continue;
                }
                let mut reach = Reach::for_importer(importer_in);
                resolver.follow(module, path, false, false, 0, &mut reach);
                if reach.external && reach.targets.is_empty() && reach.uncertain.is_empty() {
                    // std, core, an external crate, a prelude name or a
                    // generic parameter: nothing in the map. Not reported.
                    continue;
                }
                // One unit per module the path goes through. Alternatives a
                // cfg tie keeps are one unit too, keyed by the first of
                // them, so they share the mass as Go's package does.
                let Some(key) = reach
                    .targets
                    .iter()
                    .map(|(_, _, via)| *via)
                    .chain(reach.uncertain.iter().copied())
                    .min()
                else {
                    if report_dir.is_some() {
                        report.push(serde_json::json!([
                            &files[file as usize],
                            "path",
                            path.join("::"),
                            null,
                            outcome(&reach, false),
                            tie(&reach),
                            [],
                            scope.join("::"),
                        ]));
                    }
                    continue;
                };
                let unit = units.entry(key).or_insert_with(Unit::new);
                let mut row_targets = BTreeSet::new();
                for (file_id, name, _) in &reach.targets {
                    unit.add(*file_id, name);
                    row_targets.insert(*file_id);
                }
                for &uncertain in &reach.uncertain {
                    let module_file = resolver.modules[uncertain].file;
                    unit.add(module_file, path.last().map_or("", String::as_str));
                    row_targets.insert(module_file);
                }
                if report_dir.is_some() {
                    let linked = !row_targets.is_empty();
                    report.push(serde_json::json!([
                        &files[file as usize],
                        "path",
                        path.join("::"),
                        null,
                        outcome(&reach, linked),
                        tie(&reach),
                        row_targets
                            .iter()
                            .map(|&id| path_of(id))
                            .collect::<Vec<_>>(),
                        scope.join("::"),
                    ]));
                }
            }
            for unit in units.values() {
                emit(file, unit);
            }
        }
        progress.advance(1);
    }

    if let Some(directory) = report_dir {
        let slug = if pkg == "." {
            "root".to_owned()
        } else {
            pkg.replace('/', "_")
        };
        fs::create_dir_all(&directory)
            .with_context(|| format!("create {}", directory.display()))?;
        let path = directory.join(format!("rust-imports.{slug}.json"));
        // Rows are in file order, then source order: deterministic.
        fs::write(&path, serde_json::to_vec(&report)?)
            .with_context(|| format!("write {}", path.display()))?;
        // Every file's module path, or null for a file no crate reaches.
        let mut module_rows = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let module = resolver
                .by_scope
                .get(&(index as FileId, Vec::new()))
                .map(|&module| resolver.module_path(module));
            module_rows.push(serde_json::json!([file, module]));
        }
        let path = directory.join(format!("rust-modules.{slug}.json"));
        fs::write(&path, serde_json::to_vec(&module_rows)?)
            .with_context(|| format!("write {}", path.display()))?;
    }

    // A file's module path names it for the symbols document's resolver;
    // a file no crate reaches keeps its path, as Go and TypeScript files do.
    let module_for = files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let module = resolver
                .by_scope
                .get(&(index as FileId, Vec::new()))
                .map_or_else(|| file.clone(), |&module| resolver.module_path(module));
            (file.clone(), module)
        })
        .collect();

    Ok(SourceIntermediate {
        pkg: pkg.to_owned(),
        language: LanguageKind::Rust,
        parsed,
        files,
        static_edges,
        directed,
        fanin,
        uses,
        module_for,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn intermediate(root: &Path) -> SourceIntermediate {
        let (parsed, raw) = super::super::parse_files(root, ".", LanguageKind::Rust).unwrap();
        let progress = crate::progress::Progress::silent();
        let stage = progress.stage(crate::progress::StageId::Resolve, Some(parsed.len() as u64));
        resolve(root, ".", parsed, raw, &stage).unwrap()
    }

    fn edges(root: &Path) -> BTreeMap<(String, String), f64> {
        let intermediate = intermediate(root);
        intermediate
            .directed
            .iter()
            .map(|(&(a, b), &weight)| {
                (
                    (
                        intermediate.files[a as usize].clone(),
                        intermediate.files[b as usize].clone(),
                    ),
                    weight,
                )
            })
            .collect()
    }

    fn expected(rows: &[(&str, &str, f64)]) -> BTreeMap<(String, String), f64> {
        rows.iter()
            .map(|&(a, b, weight)| ((a.to_owned(), b.to_owned()), weight))
            .collect()
    }

    /// A two-crate Cargo workspace: a library with a 2018-style module
    /// (`model.rs` beside `model/`), a `mod.rs` module (`store/`), a pair of
    /// `#[path]` modules behind `cfg(unix)`/`cfg(windows)`, re-exports two
    /// hops deep, a glob import and an inherent `impl` in another file; and
    /// a binary that reaches the library through a `workspace = true` path
    /// dependency.
    fn write_workspace(root: &Path) {
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n\n\
             [workspace.package]\nedition = \"2021\"\n\n\
             [workspace.dependencies]\ndemo-core = { path = \"crates/core\" }\n",
        );
        write(
            root,
            "crates/core/Cargo.toml",
            "[package]\nname = \"demo-core\"\nversion = \"0.1.0\"\nedition.workspace = true\n",
        );
        write(
            root,
            "crates/core/src/lib.rs",
            "//! The core crate.\npub mod model;\npub mod store;\n\
             #[cfg(unix)]\n#[path = \"sys/unix.rs\"]\nmod platform;\n\
             #[cfg(windows)]\n#[path = \"sys/windows.rs\"]\nmod platform;\n\n\
             pub use model::Item;\npub use store::Store;\n\n\
             pub fn version() -> u32 {\n    platform::VALUE\n}\n",
        );
        write(
            root,
            "crates/core/src/model.rs",
            "mod detail;\n\npub use self::detail::Detail;\n\n\
             pub struct Item {\n    pub detail: Detail,\n}\n\n\
             impl Item {\n    pub fn new() -> Self {\n        Item { detail: Detail }\n    }\n}\n",
        );
        write(
            root,
            "crates/core/src/model/detail.rs",
            "pub struct Detail;\n",
        );
        write(
            root,
            "crates/core/src/store/mod.rs",
            "mod memory;\nmod ops;\n\npub use memory::Store;\nuse crate::model::Item;\n\n\
             pub fn keep(item: Item) -> Item {\n    item\n}\n",
        );
        write(
            root,
            "crates/core/src/store/memory.rs",
            "use super::super::model::Detail;\nuse crate::model::*;\n\n\
             pub struct Store {\n    detail: Detail,\n}\n",
        );
        write(
            root,
            "crates/core/src/store/ops.rs",
            "use super::memory::Store;\n\nimpl Store {\n    pub fn open() -> Self {\n        unimplemented!()\n    }\n}\n",
        );
        write(
            root,
            "crates/core/src/sys/unix.rs",
            "pub const VALUE: u32 = 1;\n",
        );
        write(
            root,
            "crates/core/src/sys/windows.rs",
            "pub const VALUE: u32 = 2;\n",
        );
        write(
            root,
            "crates/app/Cargo.toml",
            "[package]\nname = \"demo-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\ndemo-core = { workspace = true }\n",
        );
        write(
            root,
            "crates/app/src/main.rs",
            "mod cli;\n\nuse demo_core::store;\nuse demo_core::{Item, Store};\n\n\
             fn main() {\n    let item = Item::new();\n    let _store = Store::open();\n    \
             store::keep(item);\n    cli::run();\n    println!(\"{}\", demo_core::version());\n}\n\n\
             pub fn helper() {}\n",
        );
        write(
            root,
            "crates/app/src/cli.rs",
            "use crate::missing::Thing;\n\npub fn run() {\n    crate::helper();\n}\n",
        );
        // Cargo's build output is never walked.
        write(root, "target/debug/build/out.rs", "pub fn generated() {}\n");
    }

    #[test]
    fn synthetic_workspace_links_exactly_the_files_that_define_what_is_used() {
        let dir = tempfile::TempDir::new().unwrap();
        write_workspace(dir.path());
        let third = 1.0 / 3.0;
        assert_eq!(
            edges(dir.path()),
            expected(&[
                // `crate::helper()`.
                ("crates/app/src/cli.rs", "crates/app/src/main.rs", 1.0),
                // `cli::run()`, through `mod cli;`.
                ("crates/app/src/main.rs", "crates/app/src/cli.rs", 1.0),
                // `demo_core::version()`, inside `println!`.
                ("crates/app/src/main.rs", "crates/core/src/lib.rs", 1.0),
                // One `use demo_core::{Item, Store}` shares its mass of 1:
                // `Item` and `Item::new` are defined in model.rs, `Store`
                // two re-export hops away in memory.rs, and `Store::open` in
                // the one inherent impl that declares it, ops.rs.
                ("crates/app/src/main.rs", "crates/core/src/model.rs", third),
                (
                    "crates/app/src/main.rs",
                    "crates/core/src/store/memory.rs",
                    third
                ),
                (
                    "crates/app/src/main.rs",
                    "crates/core/src/store/ops.rs",
                    third
                ),
                // `use demo_core::store;` then `store::keep(..)`.
                (
                    "crates/app/src/main.rs",
                    "crates/core/src/store/mod.rs",
                    1.0
                ),
                // Re-exports link the defining files, used or not.
                ("crates/core/src/lib.rs", "crates/core/src/model.rs", 1.0),
                (
                    "crates/core/src/lib.rs",
                    "crates/core/src/store/memory.rs",
                    1.0
                ),
                // `platform::VALUE`: cfg(unix) breaks the tie.
                ("crates/core/src/lib.rs", "crates/core/src/sys/unix.rs", 1.0),
                (
                    "crates/core/src/model.rs",
                    "crates/core/src/model/detail.rs",
                    1.0
                ),
                // Through model.rs's re-export to the defining file; the glob
                // import adds nothing.
                (
                    "crates/core/src/store/memory.rs",
                    "crates/core/src/model/detail.rs",
                    1.0
                ),
                (
                    "crates/core/src/store/mod.rs",
                    "crates/core/src/model.rs",
                    1.0
                ),
                (
                    "crates/core/src/store/mod.rs",
                    "crates/core/src/store/memory.rs",
                    1.0
                ),
                (
                    "crates/core/src/store/ops.rs",
                    "crates/core/src/store/memory.rs",
                    1.0
                ),
            ])
        );
    }

    #[test]
    fn synthetic_workspace_uses_name_what_each_file_uses() {
        let dir = tempfile::TempDir::new().unwrap();
        write_workspace(dir.path());
        let intermediate = intermediate(dir.path());
        let uses = intermediate
            .uses
            .iter()
            .map(|(a, b, name)| {
                (
                    intermediate.files[*a as usize].as_str(),
                    intermediate.files[*b as usize].as_str(),
                    name.as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        for row in [
            ("crates/app/src/main.rs", "crates/core/src/model.rs", "Item"),
            ("crates/app/src/main.rs", "crates/core/src/model.rs", "new"),
            (
                "crates/app/src/main.rs",
                "crates/core/src/store/memory.rs",
                "Store",
            ),
            (
                "crates/app/src/main.rs",
                "crates/core/src/store/ops.rs",
                "open",
            ),
            (
                "crates/app/src/main.rs",
                "crates/core/src/store/mod.rs",
                "keep",
            ),
            (
                "crates/core/src/lib.rs",
                "crates/core/src/sys/unix.rs",
                "VALUE",
            ),
        ] {
            assert!(uses.contains(&row), "missing use {row:?} in {uses:?}");
        }
        assert!(!intermediate
            .files
            .iter()
            .any(|file| file.starts_with("target/")));
        // Module paths name each file for the symbols document.
        assert_eq!(
            intermediate.module_for["crates/core/src/store/memory.rs"],
            "demo_core::store::memory"
        );
        assert_eq!(
            intermediate.module_for["crates/app/src/cli.rs"],
            "demo-app[bin:demo-app]::cli"
        );
    }

    #[test]
    fn synthetic_workspace_resolution_is_identical_across_runs() {
        let dir = tempfile::TempDir::new().unwrap();
        write_workspace(dir.path());
        let first = intermediate(dir.path());
        for _ in 0..2 {
            let again = intermediate(dir.path());
            assert_eq!(first.files, again.files);
            assert_eq!(first.directed, again.directed);
            assert_eq!(first.static_edges, again.static_edges);
            assert_eq!(first.uses, again.uses);
            assert_eq!(first.module_for, again.module_for);
        }
    }

    #[test]
    fn a_cfg_tie_the_target_does_not_decide_keeps_every_alternative() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"tie\"\nedition = \"2021\"\n",
        );
        write(
            dir.path(),
            "src/lib.rs",
            "#[cfg(feature = \"fast\")]\n#[path = \"fast.rs\"]\nmod imp;\n\
             #[cfg(not(feature = \"fast\"))]\n#[path = \"slow.rs\"]\nmod imp;\n\n\
             pub fn run() {\n    imp::go();\n}\n",
        );
        write(dir.path(), "src/fast.rs", "pub fn go() {}\n");
        write(dir.path(), "src/slow.rs", "pub fn go() {}\n");
        assert_eq!(
            edges(dir.path()),
            expected(&[
                ("src/lib.rs", "src/fast.rs", 0.5),
                ("src/lib.rs", "src/slow.rs", 0.5),
            ])
        );
    }

    #[test]
    fn an_uncertain_chain_links_the_module_the_path_names_and_nothing_further() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"chain\"\nedition = \"2021\"\n",
        );
        write(
            dir.path(),
            "src/lib.rs",
            "mod facade;\nmod generated;\nmod user;\n",
        );
        // `Value` leaves the parsed set; `Made` is macro-generated.
        write(
            dir.path(),
            "src/facade.rs",
            "pub use serde_json::Value;\nmake_type!(Made);\n",
        );
        write(dir.path(), "src/generated.rs", "pub struct Real;\n");
        write(
            dir.path(),
            "src/user.rs",
            "use crate::facade::{Made, Value};\nuse crate::generated::*;\n\n\
             pub fn f(v: Value) -> Made {\n    let _ = Real;\n    todo!()\n}\n",
        );
        assert_eq!(
            edges(dir.path()),
            expected(&[("src/user.rs", "src/facade.rs", 1.0)])
        );
    }

    #[test]
    fn a_use_the_file_never_names_links_nothing_unless_it_re_exports() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"reexport\"\nedition = \"2021\"\n",
        );
        write(
            dir.path(),
            "src/lib.rs",
            "mod a;\nmod b;\npub(crate) use a::Shown;\nuse b::Hidden;\n",
        );
        write(dir.path(), "src/a.rs", "pub struct Shown;\n");
        write(dir.path(), "src/b.rs", "pub struct Hidden;\n");
        assert_eq!(
            edges(dir.path()),
            expected(&[("src/lib.rs", "src/a.rs", 1.0)])
        );
    }

    #[test]
    fn a_test_module_that_globs_super_uses_its_parents_imports() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"tested\"\nedition = \"2021\"\n",
        );
        write(
            dir.path(),
            "src/lib.rs",
            "mod a;\nmod b;\nuse a::Thing;\nuse b::Other;\n\n\
             #[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn t() {\n        Thing::new();\n    }\n}\n",
        );
        write(
            dir.path(),
            "src/a.rs",
            "pub struct Thing;\nimpl Thing {\n    pub fn new() -> Self {\n        Thing\n    }\n}\n",
        );
        write(dir.path(), "src/b.rs", "pub struct Other;\n");
        assert_eq!(
            edges(dir.path()),
            expected(&[("src/lib.rs", "src/a.rs", 1.0)])
        );
    }

    #[test]
    fn a_module_and_the_function_it_re_exports_under_one_name_both_resolve() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"cli\"\nedition = \"2021\"\n",
        );
        // ripgrep's crates/cli: `escape` is a module and the function it
        // re-exports.
        write(
            dir.path(),
            "src/lib.rs",
            "mod escape;\nmod pattern;\n\npub use crate::{escape::{escape, unescape}, pattern::pattern};\n",
        );
        write(
            dir.path(),
            "src/escape.rs",
            "pub fn escape() {}\npub fn unescape() {}\n",
        );
        write(
            dir.path(),
            "src/pattern.rs",
            "use crate::escape::unescape;\n\npub fn pattern() {\n    unescape();\n    crate::escape();\n}\n",
        );
        assert_eq!(
            edges(dir.path()),
            expected(&[
                ("src/lib.rs", "src/escape.rs", 0.5),
                ("src/lib.rs", "src/pattern.rs", 0.5),
                // The `use` weighs 1; `crate::escape()` is the function,
                // through lib.rs's re-export, in the same file.
                ("src/pattern.rs", "src/escape.rs", 2.0),
            ])
        );
    }

    #[test]
    fn a_use_naming_a_crate_and_its_item_alike_resolves_to_the_crate() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"util\", \"app\"]\n",
        );
        write(
            dir.path(),
            "util/Cargo.toml",
            "[package]\nname = \"util\"\nedition = \"2021\"\n",
        );
        write(dir.path(), "util/src/lib.rs", "pub fn util() {}\n");
        write(
            dir.path(),
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nedition = \"2021\"\n\n[dependencies]\nutil = { path = \"../util\" }\nmemchr = \"2\"\n",
        );
        // `use util::util;` binds `util`; its own head must be the crate.
        write(
            dir.path(),
            "app/src/main.rs",
            "use memchr::memchr;\nuse util::util;\n\nfn main() {\n    util();\n    memchr(b'a', b\"abc\");\n}\n",
        );
        assert_eq!(
            edges(dir.path()),
            expected(&[("app/src/main.rs", "util/src/lib.rs", 1.0)])
        );
    }

    #[test]
    fn cfg_predicates_evaluate_for_the_linux_target() {
        assert_eq!(eval_cfg("(unix)"), Cfg::In);
        assert_eq!(eval_cfg("(windows)"), Cfg::Out);
        assert_eq!(eval_cfg("(target_os = \"linux\")"), Cfg::In);
        assert_eq!(eval_cfg("(target_os = \"macos\")"), Cfg::Out);
        assert_eq!(eval_cfg("(all(unix, not(target_os = \"macos\")))"), Cfg::In);
        assert_eq!(
            eval_cfg("(any(windows, target_arch = \"x86_64\"))"),
            Cfg::In
        );
        assert_eq!(eval_cfg("(feature = \"x\")"), Cfg::Unknown);
        assert_eq!(eval_cfg("(test)"), Cfg::Unknown);
        // Out wins a conjunction whatever the unknown half says.
        assert_eq!(eval_cfg("(all(windows, feature = \"x\"))"), Cfg::Out);
        assert_eq!(eval_cfg("(any(unix, feature = \"x\"))"), Cfg::In);
        assert_eq!(eval_cfg("(not(feature = \"x\"))"), Cfg::Unknown);
        assert_eq!(eval_cfg("(unix"), Cfg::Unknown);
    }

    fn parse(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn use_trees_flatten_to_one_leaf_per_name() {
        let source = "pub use crate::a::{self, b::C as D, e::*, f::{g, h as i}};\nextern crate core as kore;\n";
        let tree = parse(source);
        let syntax = syntax(tree.root_node(), source.as_bytes());
        let leaves = syntax
            .uses
            .iter()
            .flat_map(|decl| decl.leaves.iter())
            .map(|leaf| (leaf.local.clone(), leaf.segments.join("::"), leaf.glob))
            .collect::<Vec<_>>();
        assert_eq!(
            leaves,
            vec![
                (Some("a".to_owned()), "crate::a".to_owned(), false),
                (Some("D".to_owned()), "crate::a::b::C".to_owned(), false),
                (None, "crate::a::e".to_owned(), true),
                (Some("g".to_owned()), "crate::a::f::g".to_owned(), false),
                (Some("i".to_owned()), "crate::a::f::h".to_owned(), false),
                (Some("kore".to_owned()), "core".to_owned(), false),
            ]
        );
        assert!(syntax.uses[0].public && !syntax.uses[0].extern_crate);
        assert!(syntax.uses[1].extern_crate);
    }

    #[test]
    fn metrics_name_each_symbol_with_the_existing_kinds() {
        let source = "pub struct S;\nimpl S {\n    fn method(&self) {}\n}\n\
                      pub trait T {\n    fn required(&self);\n    fn provided(&self) {}\n}\n\
                      pub enum E { A }\ntype Alias = S;\nconst C: u8 = 1;\nstatic V: u8 = 2;\n\
                      macro_rules! m { () => {} }\nmod inline {\n    pub fn inner() {}\n}\n\
                      fn free() {\n    fn nested() {}\n}\n";
        let tree = parse(source);
        let (_, _, symbols) = metrics(tree.root_node(), source.as_bytes());
        let rows = symbols
            .iter()
            .map(|row| (row.0 .0.as_str(), row.0 .1))
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            vec![
                ("S", KIND_CLASS),
                ("method", KIND_METHOD),
                ("T", KIND_INTERFACE),
                ("required", KIND_METHOD),
                ("provided", KIND_METHOD),
                ("E", KIND_CLASS),
                ("Alias", KIND_TYPE),
                ("C", KIND_CONST),
                ("V", KIND_CONST),
                ("m", KIND_FUNC),
                ("inline", KIND_TYPE),
                ("inner", KIND_FUNC),
                ("free", KIND_FUNC),
                ("nested", KIND_FUNC),
            ]
        );
    }
}
