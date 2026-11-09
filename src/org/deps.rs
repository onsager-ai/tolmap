//! Declared dependencies of one member tree (issue #255): what packages it
//! provides and what it depends on, read from Cargo, npm/pnpm, Go and Python
//! manifests at the member's commit. Nothing here resolves a dependency to a
//! repository; `org::resolve` does that against every member at once.
//!
//! Reuses the readers the extractor and detection already trust: Cargo
//! manifests and workspace inheritance from `extract::rust` (#101/#115's
//! workspace handling), `go.mod`'s module line from `extract`, and a Python
//! project's declared name from `detect` (#162). The walk skips exactly the
//! directories the Rust walk skips, so a vendored or example manifest adds no
//! dependency.
//!
//! Lower bound: only declared dependencies are read. `setup.py`'s
//! `install_requires` is not evaluated (it is Python), and an npm range with
//! `||`, a hyphen range or a PEP 440 local version is kept but not judged.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::{Ecosystem, Pin, PinKind, ProvidedPackage};
use crate::extract::{self, rust};

/// One declared dependency of a member.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Declared {
    pub ecosystem: Ecosystem,
    pub consumer_package: String,
    pub file: String,
    pub section: String,
    /// The provider's package name as the provider declares it (Python
    /// names normalised per PEP 503).
    pub name: String,
    /// `owner/repo`, lowercased, when the dependency names a GitHub
    /// repository (a git dependency).
    pub repo: Option<String>,
    pub pin: Pin,
}

/// A declaration that could not be read as a dependency at all.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Unreadable {
    pub ecosystem: Ecosystem,
    pub consumer_package: String,
    pub file: String,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub(crate) struct Tree {
    pub provided: Vec<ProvidedPackage>,
    pub declared: Vec<Declared>,
    pub unreadable: Vec<Unreadable>,
}

pub(crate) fn scan(root: &Path) -> Tree {
    let mut tree = Tree::default();
    let files = manifest_files(root);
    cargo(root, &files, &mut tree);
    for file in &files {
        let name = file.rsplit('/').next().unwrap_or(file.as_str());
        let ecosystem = match name {
            "package.json" => Ecosystem::Npm,
            "go.mod" => Ecosystem::Go,
            "pyproject.toml" | "setup.py" | "setup.cfg" => Ecosystem::Python,
            _ if is_requirements(name) => Ecosystem::Python,
            _ => continue,
        };
        let Ok(text) = fs::read_to_string(root.join(file)) else {
            tree.unreadable.push(Unreadable {
                ecosystem,
                consumer_package: directory_or_root(directory_of(file)),
                file: file.clone(),
                name: String::new(),
                reason: "manifest is not readable as UTF-8 text".to_owned(),
            });
            continue;
        };
        match name {
            "package.json" => npm(file, &text, &mut tree),
            "go.mod" => go(file, &text, &mut tree),
            "pyproject.toml" => pyproject(root, file, &text, &mut tree),
            _ if is_requirements(name) => requirements(root, file, &text, &mut tree),
            _ => {}
        }
    }
    python_provided(root, &files, &mut tree);
    tree.provided.sort();
    tree.provided.dedup();
    tree.declared.sort();
    tree.declared.dedup();
    tree.unreadable.sort();
    tree.unreadable.dedup();
    tree
}

fn is_requirements(name: &str) -> bool {
    name.starts_with("requirements") && name.ends_with(".txt")
}

/// Repo-relative paths of every manifest this module reads, in a fixed
/// order.
fn manifest_files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(here) = stack.pop() {
        let Ok(entries) = fs::read_dir(root.join(&here)) else {
            continue;
        };
        let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = join(&here, &name);
            if kind.is_dir() {
                if !rust::skip_dir(&name) {
                    stack.push(path);
                }
            } else if kind.is_file()
                && (matches!(
                    name.as_str(),
                    "Cargo.toml"
                        | "package.json"
                        | "go.mod"
                        | "pyproject.toml"
                        | "setup.py"
                        | "setup.cfg"
                ) || is_requirements(&name))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() || directory == "." {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

fn directory_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(directory, _)| directory)
}

pub(crate) fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn pin(kind: PinKind, value: impl Into<String>) -> Pin {
    Pin {
        kind,
        value: value.into(),
    }
}

/// A git reference as a pin: a hex string of at least 7 characters is a
/// commit; anything else is a ref whose kind (tag or branch) is unknown.
fn git_ref_pin(reference: &str) -> Pin {
    if reference.len() >= 7 && is_hex(reference) {
        pin(PinKind::Commit, reference.to_ascii_lowercase())
    } else {
        pin(PinKind::Ref, reference)
    }
}

/// `owner/repo`, lowercased, for a GitHub URL in any form a manifest uses:
/// `https://github.com/o/r(.git)`, `git+https://...`, `ssh://git@github.com/o/r`,
/// `git@github.com:o/r.git`, `github:o/r`. Anything after `#`, `?` or the
/// repository segment is ignored.
pub(crate) fn github_repo(url: &str) -> Option<String> {
    let url = url.trim();
    let url = url.strip_prefix("git+").unwrap_or(url);
    let rest = if let Some(rest) = url.strip_prefix("github:") {
        rest
    } else if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        let at = url.find("github.com/")?;
        let before = &url[..at];
        if !(before.ends_with("://") || before.ends_with('@') || before.ends_with("://www.")) {
            return None;
        }
        &url[at + "github.com/".len()..]
    };
    let rest = rest.split(['#', '?']).next()?;
    let mut segments = rest.split('/');
    let owner = segments.next()?;
    let repo = segments.next()?;
    let repo = repo.split('@').next()?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let valid = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    (valid(owner) && valid(repo)).then(|| format!("{owner}/{repo}").to_ascii_lowercase())
}

// ---------------------------------------------------------------------
// Cargo
// ---------------------------------------------------------------------

fn cargo(root: &Path, files: &[String], tree: &mut Tree) {
    let manifests = rust::manifests(root);
    for file in files.iter().filter(|f| f.ends_with("Cargo.toml")) {
        let directory = directory_of(file);
        if !manifests.contains_key(directory) {
            tree.unreadable.push(Unreadable {
                ecosystem: Ecosystem::Cargo,
                consumer_package: directory_or_root(directory),
                file: file.clone(),
                name: String::new(),
                reason: "Cargo.toml is not valid TOML".to_owned(),
            });
        }
    }
    for (directory, manifest) in &manifests {
        let file = join(directory, "Cargo.toml");
        let Some(package) = rust::str_at(manifest, &["package", "name"]) else {
            continue;
        };
        let version = match manifest.get("package").and_then(|p| p.get("version")) {
            Some(toml::Value::String(version)) => Some(version.clone()),
            Some(toml::Value::Table(table))
                if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) =>
            {
                rust::workspace_of(directory, &manifests).and_then(|(_, workspace)| {
                    rust::str_at(workspace, &["workspace", "package", "version"]).map(str::to_owned)
                })
            }
            _ => None,
        };
        tree.provided.push(ProvidedPackage {
            ecosystem: Ecosystem::Cargo,
            name: package.to_owned(),
            file: file.clone(),
            version,
        });
        let mut tables = Vec::new();
        for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(table) = manifest.get(key).and_then(toml::Value::as_table) {
                tables.push((key.to_owned(), table));
            }
        }
        if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
            for (platform, value) in targets {
                for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(table) = value.get(key).and_then(toml::Value::as_table) {
                        tables.push((format!("target.{platform}.{key}"), table));
                    }
                }
            }
        }
        for (section, table) in tables {
            for (key, spec) in table {
                let spec = if spec.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    let inherited =
                        rust::workspace_of(directory, &manifests).and_then(|(_, workspace)| {
                            workspace
                                .get("workspace")
                                .and_then(|w| w.get("dependencies"))
                                .and_then(|d| d.get(key))
                        });
                    match inherited {
                        Some(spec) => spec,
                        None => {
                            tree.unreadable.push(Unreadable {
                                ecosystem: Ecosystem::Cargo,
                                consumer_package: package.to_owned(),
                                file: file.clone(),
                                name: key.clone(),
                                reason:
                                    "inherits a workspace dependency the workspace does not declare"
                                        .to_owned(),
                            });
                            continue;
                        }
                    }
                } else {
                    spec
                };
                let (name, repo, pin) = cargo_spec(key, spec);
                tree.declared.push(Declared {
                    ecosystem: Ecosystem::Cargo,
                    consumer_package: package.to_owned(),
                    file: file.clone(),
                    section: section.clone(),
                    name,
                    repo,
                    pin,
                });
            }
        }
    }
}

fn directory_or_root(directory: &str) -> String {
    if directory.is_empty() {
        ".".to_owned()
    } else {
        directory.to_owned()
    }
}

/// `(provider package, repository, pin)` for one Cargo dependency spec. A
/// git source wins over `path`, and `path` over `version`: that is the
/// source the consumer builds from in its own tree.
fn cargo_spec(key: &str, spec: &toml::Value) -> (String, Option<String>, Pin) {
    let field = |name: &str| spec.get(name).and_then(toml::Value::as_str);
    let name = field("package").unwrap_or(key).to_owned();
    if let toml::Value::String(version) = spec {
        return (name, None, pin(PinKind::Version, version.clone()));
    }
    if let Some(git) = field("git") {
        let pin = if let Some(rev) = field("rev") {
            git_ref_pin(rev)
        } else if let Some(tag) = field("tag") {
            pin(PinKind::Tag, tag)
        } else if let Some(branch) = field("branch") {
            pin(PinKind::Branch, branch)
        } else {
            pin(PinKind::None, "")
        };
        return (name, github_repo(git), pin);
    }
    if let Some(path) = field("path") {
        return (name, None, pin(PinKind::Path, path));
    }
    match field("version") {
        Some(version) => (name, None, pin(PinKind::Version, version)),
        None => (name, None, pin(PinKind::None, "")),
    }
}

// ---------------------------------------------------------------------
// npm / pnpm
// ---------------------------------------------------------------------

const NPM_SECTIONS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

fn npm(file: &str, text: &str, tree: &mut Tree) {
    let consumer = directory_or_root(directory_of(file));
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        tree.unreadable.push(Unreadable {
            ecosystem: Ecosystem::Npm,
            consumer_package: consumer,
            file: file.to_owned(),
            name: String::new(),
            reason: "package.json is not valid JSON".to_owned(),
        });
        return;
    };
    let name = value
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty());
    if let Some(name) = name {
        tree.provided.push(ProvidedPackage {
            ecosystem: Ecosystem::Npm,
            name: name.to_owned(),
            file: file.to_owned(),
            version: value
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        });
    }
    let consumer = name.map_or(consumer, str::to_owned);
    for section in NPM_SECTIONS {
        let Some(table) = value.get(section).and_then(|s| s.as_object()) else {
            continue;
        };
        for (key, spec) in table {
            let Some(spec) = spec.as_str() else {
                tree.unreadable.push(Unreadable {
                    ecosystem: Ecosystem::Npm,
                    consumer_package: consumer.clone(),
                    file: file.to_owned(),
                    name: key.clone(),
                    reason: "dependency spec is not a string".to_owned(),
                });
                continue;
            };
            let (name, repo, pin) = npm_spec(key, spec);
            tree.declared.push(Declared {
                ecosystem: Ecosystem::Npm,
                consumer_package: consumer.clone(),
                file: file.to_owned(),
                section: section.to_owned(),
                name,
                repo,
                pin,
            });
        }
    }
}

fn npm_spec(key: &str, spec: &str) -> (String, Option<String>, Pin) {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("workspace:") {
        return (key.to_owned(), None, pin(PinKind::Workspace, rest));
    }
    for prefix in ["file:", "link:", "portal:"] {
        if let Some(rest) = spec.strip_prefix(prefix) {
            return (key.to_owned(), None, pin(PinKind::Path, rest));
        }
    }
    if let Some(alias) = spec.strip_prefix("npm:") {
        // `npm:real-name@range`, the name possibly scoped (`@s/n@range`).
        return match alias
            .get(1..)
            .and_then(|rest| rest.find('@'))
            .map(|i| i + 1)
        {
            Some(at) => (
                alias[..at].to_owned(),
                None,
                pin(PinKind::Version, &alias[at + 1..]),
            ),
            None => (alias.to_owned(), None, pin(PinKind::None, "")),
        };
    }
    let shorthand =
        !spec.contains(':') && spec.contains('/') && !spec.starts_with(['.', '/', '@', '~']);
    let is_git = spec.starts_with("git+")
        || spec.starts_with("git://")
        || spec.starts_with("git@")
        || spec.starts_with("github:")
        || shorthand
        || (spec.starts_with("http") && spec.contains("github.com/") && !spec.ends_with(".tgz"));
    if is_git {
        let repo = if shorthand {
            github_repo(&format!("github:{spec}"))
        } else {
            github_repo(spec)
        };
        let reference = spec.split_once('#').map(|(_, r)| r).unwrap_or("");
        let pin = if reference.is_empty() {
            pin(PinKind::None, "")
        } else if let Some(range) = reference.strip_prefix("semver:") {
            pin(PinKind::Version, range)
        } else {
            git_ref_pin(reference)
        };
        return (key.to_owned(), repo, pin);
    }
    (key.to_owned(), None, pin(PinKind::Version, spec))
}

// ---------------------------------------------------------------------
// Go
// ---------------------------------------------------------------------

fn go(file: &str, text: &str, tree: &mut Tree) {
    let directory = directory_of(file);
    let Some(module) = extract::go_module_in(text) else {
        tree.unreadable.push(Unreadable {
            ecosystem: Ecosystem::Go,
            consumer_package: directory_or_root(directory),
            file: file.to_owned(),
            name: String::new(),
            reason: "go.mod declares no module".to_owned(),
        });
        return;
    };
    tree.provided.push(ProvidedPackage {
        ecosystem: Ecosystem::Go,
        name: module.clone(),
        file: file.to_owned(),
        version: None,
    });
    let mut requires = Vec::new();
    let mut replaces = BTreeMap::new();
    let mut block: Option<&str> = None;
    for raw in text.lines() {
        let (line, comment) = match raw.split_once("//") {
            Some((line, comment)) => (line.trim(), comment.trim()),
            None => (raw.trim(), ""),
        };
        if let Some(open) = block {
            if line == ")" {
                block = None;
            } else if !line.is_empty() {
                go_directive(open, line, comment, &mut requires, &mut replaces);
            }
            continue;
        }
        for directive in ["require", "replace"] {
            let Some(rest) = line
                .strip_prefix(directive)
                .filter(|rest| rest.starts_with([' ', '\t', '(']))
            else {
                continue;
            };
            let rest = rest.trim();
            if rest == "(" {
                block = Some(directive);
            } else {
                go_directive(directive, rest, comment, &mut requires, &mut replaces);
            }
        }
    }
    for (required, version, section) in requires {
        let (name, pin) = match replaces.get(&required) {
            Some(GoReplace::Path(path)) => (required.clone(), pin(PinKind::Path, path)),
            Some(GoReplace::Module(name, version)) => {
                (name.clone(), pin(PinKind::Version, version))
            }
            None => (required.clone(), pin(PinKind::Version, version)),
        };
        tree.declared.push(Declared {
            ecosystem: Ecosystem::Go,
            consumer_package: module.clone(),
            file: file.to_owned(),
            section,
            name,
            // A Go module path names its repository only by convention;
            // resolution goes by module path alone.
            repo: None,
            pin,
        });
    }
}

enum GoReplace {
    Path(String),
    Module(String, String),
}

fn go_directive(
    directive: &str,
    line: &str,
    comment: &str,
    requires: &mut Vec<(String, String, String)>,
    replaces: &mut BTreeMap<String, GoReplace>,
) {
    if directive == "require" {
        let mut parts = line.split_whitespace();
        if let (Some(module), Some(version)) = (parts.next(), parts.next()) {
            let section = if comment == "indirect" {
                "require (indirect)"
            } else {
                "require"
            };
            requires.push((module.to_owned(), version.to_owned(), section.to_owned()));
        }
        return;
    }
    let Some((old, new)) = line.split_once("=>") else {
        return;
    };
    let Some(old) = old.split_whitespace().next() else {
        return;
    };
    let new = new.split_whitespace().collect::<Vec<_>>();
    match new.as_slice() {
        [path] if path.starts_with('.') || path.starts_with('/') => {
            replaces.insert(old.to_owned(), GoReplace::Path((*path).to_owned()));
        }
        [module, version] => {
            replaces.insert(
                old.to_owned(),
                GoReplace::Module((*module).to_owned(), (*version).to_owned()),
            );
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------

/// PEP 503 normalisation: lowercase, runs of `-`, `_`, `.` become `-`.
pub(crate) fn normalize_python(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut separator = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            separator = true;
        } else {
            if separator && !out.is_empty() {
                out.push('-');
            }
            separator = false;
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

/// Every Python project directory and its declared name, nearest first
/// when looked up by [`python_consumer`].
fn python_projects(root: &Path, files: &[String]) -> BTreeMap<String, String> {
    let mut projects = BTreeMap::new();
    for file in files {
        let name = file.rsplit('/').next().unwrap_or(file.as_str());
        if !matches!(name, "pyproject.toml" | "setup.py") {
            continue;
        }
        let directory = directory_of(file);
        let path = root.join(directory);
        if !crate::detect::is_python_project_dir(&path) {
            continue;
        }
        if let Some(declared) = crate::detect::python_declared_name(&path) {
            projects.insert(directory.to_owned(), declared);
        }
    }
    projects
}

fn python_consumer(projects: &BTreeMap<String, String>, file: &str) -> String {
    let mut directory = directory_of(file);
    loop {
        if let Some(name) = projects.get(directory) {
            return name.clone();
        }
        if directory.is_empty() {
            return directory_or_root(directory_of(file));
        }
        directory = directory_of(directory);
    }
}

fn python_provided(root: &Path, files: &[String], tree: &mut Tree) {
    for (directory, name) in python_projects(root, files) {
        let pyproject = join(&directory, "pyproject.toml");
        let version = fs::read_to_string(root.join(&pyproject))
            .ok()
            .and_then(|text| text.parse::<toml::Value>().ok())
            .and_then(|value| {
                rust::str_at(&value, &["project", "version"])
                    .or_else(|| rust::str_at(&value, &["tool", "poetry", "version"]))
                    .map(str::to_owned)
            });
        let file = ["pyproject.toml", "setup.py", "setup.cfg"]
            .into_iter()
            .map(|name| join(&directory, name))
            .find(|file| files.contains(file))
            .unwrap_or(pyproject);
        tree.provided.push(ProvidedPackage {
            ecosystem: Ecosystem::Python,
            name: normalize_python(&name),
            file,
            version,
        });
    }
}

fn pyproject(root: &Path, file: &str, text: &str, tree: &mut Tree) {
    let files = vec![file.to_owned()];
    let projects = python_projects(root, &files);
    let consumer = python_consumer(&projects, file);
    let value = match text.parse::<toml::Value>() {
        Ok(value) => value,
        Err(_) => {
            tree.unreadable.push(Unreadable {
                ecosystem: Ecosystem::Python,
                consumer_package: consumer,
                file: file.to_owned(),
                name: String::new(),
                reason: "pyproject.toml is not valid TOML".to_owned(),
            });
            return;
        }
    };
    // `[tool.uv.sources]` says where a PEP 508 requirement actually comes
    // from (git, path, workspace); it overrides the requirement's pin.
    let uv_sources = value
        .get("tool")
        .and_then(|t| t.get("uv"))
        .and_then(|u| u.get("sources"))
        .and_then(toml::Value::as_table);
    let push = |section: String, requirement: &str, tree: &mut Tree| {
        let Some((name, mut repo, mut pin)) = pep508(requirement) else {
            tree.unreadable.push(Unreadable {
                ecosystem: Ecosystem::Python,
                consumer_package: consumer.clone(),
                file: file.to_owned(),
                name: requirement.to_owned(),
                reason: "not a PEP 508 requirement".to_owned(),
            });
            return;
        };
        if let Some(source) = uv_sources.and_then(|sources| {
            sources
                .iter()
                .find(|(key, _)| normalize_python(key) == name)
                .map(|(_, source)| source)
        }) {
            (repo, pin) = python_table_source(source).unwrap_or((repo, pin));
        }
        tree.declared.push(Declared {
            ecosystem: Ecosystem::Python,
            consumer_package: consumer.clone(),
            file: file.to_owned(),
            section,
            name,
            repo,
            pin,
        });
    };
    let strings = |value: Option<&toml::Value>| -> Vec<String> {
        value
            .and_then(toml::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let project = value.get("project");
    for requirement in strings(project.and_then(|p| p.get("dependencies"))) {
        push("project.dependencies".to_owned(), &requirement, tree);
    }
    if let Some(extras) = project
        .and_then(|p| p.get("optional-dependencies"))
        .and_then(toml::Value::as_table)
    {
        for (extra, items) in extras {
            for requirement in strings(Some(items)) {
                push(
                    format!("project.optional-dependencies.{extra}"),
                    &requirement,
                    tree,
                );
            }
        }
    }
    if let Some(groups) = value
        .get("dependency-groups")
        .and_then(toml::Value::as_table)
    {
        for (group, items) in groups {
            // `{include-group = ...}` entries are tables, not requirements.
            for requirement in strings(Some(items)) {
                push(format!("dependency-groups.{group}"), &requirement, tree);
            }
        }
    }
    let poetry = value.get("tool").and_then(|t| t.get("poetry"));
    let mut poetry_tables = Vec::new();
    if let Some(table) = poetry
        .and_then(|p| p.get("dependencies"))
        .and_then(toml::Value::as_table)
    {
        poetry_tables.push(("tool.poetry.dependencies".to_owned(), table));
    }
    if let Some(table) = poetry
        .and_then(|p| p.get("dev-dependencies"))
        .and_then(toml::Value::as_table)
    {
        poetry_tables.push(("tool.poetry.dev-dependencies".to_owned(), table));
    }
    if let Some(groups) = poetry
        .and_then(|p| p.get("group"))
        .and_then(toml::Value::as_table)
    {
        for (group, value) in groups {
            if let Some(table) = value.get("dependencies").and_then(toml::Value::as_table) {
                poetry_tables.push((format!("tool.poetry.group.{group}.dependencies"), table));
            }
        }
    }
    for (section, table) in poetry_tables {
        for (key, spec) in table {
            if key == "python" {
                continue;
            }
            let (repo, pin) = match spec {
                toml::Value::String(version) => (None, pin(PinKind::Version, version.clone())),
                other => python_table_source(other).unwrap_or_else(|| {
                    match other.get("version").and_then(toml::Value::as_str) {
                        Some(version) => (None, pin(PinKind::Version, version)),
                        None => (None, pin(PinKind::None, "")),
                    }
                }),
            };
            tree.declared.push(Declared {
                ecosystem: Ecosystem::Python,
                consumer_package: consumer.clone(),
                file: file.to_owned(),
                section: section.clone(),
                name: normalize_python(key),
                repo,
                pin,
            });
        }
    }
}

/// A poetry or uv source table: `{git, rev|tag|branch}`, `{path}` or
/// `{workspace = true}`. `None` for a plain version table.
fn python_table_source(source: &toml::Value) -> Option<(Option<String>, Pin)> {
    let field = |name: &str| source.get(name).and_then(toml::Value::as_str);
    if let Some(git) = field("git") {
        let pin = if let Some(rev) = field("rev") {
            git_ref_pin(rev)
        } else if let Some(tag) = field("tag") {
            pin(PinKind::Tag, tag)
        } else if let Some(branch) = field("branch") {
            pin(PinKind::Branch, branch)
        } else {
            pin(PinKind::None, "")
        };
        return Some((github_repo(git), pin));
    }
    if let Some(path) = field("path") {
        return Some((None, pin(PinKind::Path, path)));
    }
    if source.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        return Some((None, pin(PinKind::Workspace, "true")));
    }
    None
}

/// `(normalised name, repository, pin)` for one PEP 508 requirement.
pub(crate) fn pep508(requirement: &str) -> Option<(String, Option<String>, Pin)> {
    let requirement = requirement.trim();
    let end = requirement
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(requirement.len());
    let name = &requirement[..end];
    if name.is_empty() {
        return None;
    }
    let mut rest = requirement[end..].trim_start();
    if rest.starts_with('[') {
        rest = rest[rest.find(']')? + 1..].trim_start();
    }
    if let Some(url) = rest.strip_prefix('@') {
        let url = url.split(" ;").next().unwrap_or(url).trim();
        let (repo, pin) = python_url(url);
        return Some((normalize_python(name), repo, pin));
    }
    let spec = rest.split(';').next().unwrap_or("").trim();
    let spec = spec
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(spec)
        .trim();
    let pin = if spec.is_empty() {
        pin(PinKind::None, "")
    } else {
        pin(PinKind::Version, spec)
    };
    Some((normalize_python(name), None, pin))
}

/// A direct-reference URL: `git+https://github.com/o/r(.git)@ref#...`, or
/// `file://...`.
fn python_url(url: &str) -> (Option<String>, Pin) {
    if let Some(path) = url.strip_prefix("file://") {
        return (None, pin(PinKind::Path, path));
    }
    let without_fragment = url.split('#').next().unwrap_or(url);
    let repo = github_repo(without_fragment);
    let host = without_fragment.find("github.com").unwrap_or(0);
    let reference = without_fragment
        .rfind('@')
        .filter(|&at| at > host)
        .map(|at| &without_fragment[at + 1..]);
    let pin = match reference {
        Some(reference) if !reference.is_empty() => git_ref_pin(reference),
        _ if repo.is_some() => pin(PinKind::None, ""),
        _ => pin(PinKind::Ref, url),
    };
    (repo, pin)
}

fn requirements(root: &Path, file: &str, text: &str, tree: &mut Tree) {
    let files = vec![file.to_owned()];
    let mut projects = python_projects(root, &files);
    // A requirements file belongs to the nearest project above it.
    let mut directory = directory_of(file);
    loop {
        let manifest = ["pyproject.toml", "setup.py"]
            .into_iter()
            .map(|name| join(directory, name))
            .find(|candidate| root.join(candidate).is_file());
        if let Some(candidate) = manifest {
            projects.extend(python_projects(root, &[candidate]));
            break;
        }
        if directory.is_empty() {
            break;
        }
        directory = directory_of(directory);
    }
    let consumer = python_consumer(&projects, file);
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let editable = line
            .strip_prefix("-e ")
            .or_else(|| line.strip_prefix("--editable "))
            .map(str::trim);
        if editable.is_none() && line.starts_with('-') {
            continue; // -r, -c, --index-url and other options
        }
        let target = editable.unwrap_or(line);
        let egg = target
            .split_once("#egg=")
            .map(|(_, egg)| egg.split('&').next().unwrap_or(egg).to_owned());
        let looks_like_url = target.contains("://") || target.starts_with("git+");
        let parsed = if looks_like_url || editable.is_some() {
            match egg {
                Some(egg) if looks_like_url => {
                    let (repo, pin) = python_url(target);
                    Some((normalize_python(&egg), repo, pin))
                }
                Some(egg) => Some((normalize_python(&egg), None, pin(PinKind::Path, target))),
                None => None,
            }
        } else {
            pep508(target)
        };
        match parsed {
            Some((name, repo, pin)) => tree.declared.push(Declared {
                ecosystem: Ecosystem::Python,
                consumer_package: consumer.clone(),
                file: file.to_owned(),
                section: "requirements".to_owned(),
                name,
                repo,
                pin,
            }),
            None => tree.unreadable.push(Unreadable {
                ecosystem: Ecosystem::Python,
                consumer_package: consumer.clone(),
                file: file.to_owned(),
                name: target.to_owned(),
                reason: "requirement names no project (no #egg= and not PEP 508)".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_manifest_is_retained_as_unresolved_evidence() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("package.json"), [0xff]).unwrap();
        let tree = scan(root.path());
        assert_eq!(tree.unreadable.len(), 1);
        assert_eq!(tree.unreadable[0].ecosystem, Ecosystem::Npm);
        assert_eq!(tree.unreadable[0].file, "package.json");
        assert_eq!(
            tree.unreadable[0].reason,
            "manifest is not readable as UTF-8 text"
        );
    }

    #[test]
    fn github_urls_in_every_manifest_form() {
        for url in [
            "https://github.com/Acme/Core",
            "https://github.com/acme/core.git",
            "git+https://github.com/acme/core.git#abc",
            "ssh://git@github.com/acme/core",
            "git@github.com:acme/core.git",
            "github:acme/core#v1",
            "git+https://github.com/acme/core@deadbeef",
        ] {
            assert_eq!(github_repo(url).as_deref(), Some("acme/core"), "{url}");
        }
        assert_eq!(github_repo("https://gitlab.com/acme/core"), None);
        assert_eq!(github_repo("https://notgithub.com/acme/core"), None);
    }

    #[test]
    fn pep508_requirements() {
        let (name, repo, p) = pep508("Acme_Core.Py[extra] >=0.3 ; python_version>'3'").unwrap();
        assert_eq!((name.as_str(), repo), ("acme-core-py", None));
        assert_eq!(p, pin(PinKind::Version, ">=0.3"));
        let (name, repo, p) =
            pep508("acme-core @ git+https://github.com/acme/core@0123456789abcdef").unwrap();
        assert_eq!(name, "acme-core");
        assert_eq!(repo.as_deref(), Some("acme/core"));
        assert_eq!(p, pin(PinKind::Commit, "0123456789abcdef"));
        assert_eq!(pep508("requests").unwrap().2, pin(PinKind::None, ""));
        assert!(pep508(">=1").is_none());
    }

    #[test]
    fn npm_specs() {
        assert_eq!(
            npm_spec("a", "^1.2.0"),
            ("a".into(), None, pin(PinKind::Version, "^1.2.0"))
        );
        assert_eq!(
            npm_spec("a", "workspace:*"),
            ("a".into(), None, pin(PinKind::Workspace, "*"))
        );
        assert_eq!(
            npm_spec("a", "acme/core#v1.0.0"),
            (
                "a".into(),
                Some("acme/core".into()),
                pin(PinKind::Ref, "v1.0.0")
            )
        );
        assert_eq!(
            npm_spec("a", "npm:@acme/real@^2"),
            ("@acme/real".into(), None, pin(PinKind::Version, "^2"))
        );
    }

    #[test]
    fn go_mod_requires_and_replaces() {
        let mut tree = Tree::default();
        go(
            "go.mod",
            "module github.com/acme/app\n\nrequire (\n\tgithub.com/acme/core v1.0.0\n\tgolang.org/x/mod v0.1.0 // indirect\n)\nrequire github.com/acme/util v0.0.0-20240101000000-0123456789ab\nreplace github.com/acme/util => ../util\n",
            &mut tree,
        );
        assert_eq!(tree.provided[0].name, "github.com/acme/app");
        let found = tree
            .declared
            .iter()
            .map(|d| (d.name.as_str(), d.section.as_str(), d.pin.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            vec![
                (
                    "github.com/acme/core",
                    "require",
                    pin(PinKind::Version, "v1.0.0")
                ),
                (
                    "golang.org/x/mod",
                    "require (indirect)",
                    pin(PinKind::Version, "v0.1.0")
                ),
                (
                    "github.com/acme/util",
                    "require",
                    pin(PinKind::Path, "../util")
                ),
            ]
        );
    }
}
