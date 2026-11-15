//! Entry evidence only: never an input to edges, weights or partitioning.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use crate::schema::SourceNode;

/// Inspect only mapped files and manifests in their ancestor directories.
/// No manifest/configuration code is executed. Reasons use repo-relative paths.
pub(crate) fn detect(repo: &Path, nodes: &[SourceNode]) -> BTreeMap<String, String> {
    let files: BTreeSet<&str> = nodes.iter().map(|node| node.file.as_str()).collect();
    let mut dirs = BTreeSet::new();
    for file in &files {
        let mut parent = Path::new(file).parent();
        while let Some(dir) = parent {
            dirs.insert(dir.to_string_lossy().replace('\\', "/"));
            parent = dir.parent();
        }
    }
    let mut entries = BTreeMap::new();
    for dir in dirs {
        let cargo_path = join(&dir, "Cargo.toml");
        if let Ok(text) = fs::read_to_string(repo.join(&cargo_path)) {
            if let Ok(manifest) = text.parse::<toml::Value>() {
                if manifest.get("package").is_some() {
                    let autobins = manifest
                        .get("package")
                        .and_then(|p| p.get("autobins"))
                        .and_then(toml::Value::as_bool)
                        .unwrap_or(true);
                    if autobins {
                        add(
                            &mut entries,
                            &files,
                            join(&dir, "src/main.rs"),
                            format!("{cargo_path}: src/main.rs"),
                        );
                        let prefix = join(&dir, "src/bin/");
                        for file in &files {
                            if let Some(rest) = file.strip_prefix(&prefix) {
                                if (rest.ends_with(".rs") && !rest.contains('/'))
                                    || rest
                                        .strip_suffix("/main.rs")
                                        .is_some_and(|name| !name.contains('/'))
                                {
                                    add(
                                        &mut entries,
                                        &files,
                                        (*file).to_owned(),
                                        format!("{cargo_path}: src/bin convention"),
                                    );
                                }
                            }
                        }
                    }
                    if let Some(bins) = manifest.get("bin").and_then(toml::Value::as_array) {
                        for bin in bins {
                            if let Some(path) = bin.get("path").and_then(toml::Value::as_str) {
                                if Path::new(path)
                                    .file_name()
                                    .is_some_and(|name| name == "build.rs")
                                {
                                    continue;
                                }
                                if let Some(path) = relative(&dir, path) {
                                    add(
                                        &mut entries,
                                        &files,
                                        path,
                                        format!("{cargo_path}: [[bin]].path"),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        let package_path = join(&dir, "package.json");
        if let Ok(text) = fs::read_to_string(repo.join(&package_path)) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) {
                for field in ["bin", "main", "module", "exports"] {
                    let mut targets = BTreeSet::new();
                    if let Some(value) = manifest.get(field) {
                        strings(value, &mut targets);
                    }
                    for target in targets {
                        // Patterns, external targets and declarations are not runtime entry files.
                        if target.contains('*')
                            || target.ends_with(".d.ts")
                            || target.ends_with(".d.mts")
                            || target.ends_with(".d.cts")
                        {
                            continue;
                        }
                        let Some(path) = relative(&dir, &target) else {
                            continue;
                        };
                        if files.contains(path.as_str()) {
                            add(
                                &mut entries,
                                &files,
                                path,
                                format!("{package_path}: {field} {target}"),
                            );
                        } else if let Some(source) = dist_source(&dir, &target, &files) {
                            add(
                                &mut entries,
                                &files,
                                source,
                                format!(
                                    "{package_path}: {field} {target} → unique dist/src source"
                                ),
                            );
                        } else {
                            eprintln!("entry skipped: {package_path} {field} {target:?}: no mapped target or unique dist/src source (bundled/renamed output needs an explicit source mapping)");
                        }
                    }
                }
                // Vite's default HTML root is unambiguous only without a custom config.
                let vite = ["dependencies", "devDependencies"]
                    .iter()
                    .any(|key| manifest.get(key).and_then(|v| v.get("vite")).is_some());
                let custom = ["js", "ts", "mjs", "mts", "cjs", "cts"].iter().any(|ext| {
                    repo.join(join(&dir, &format!("vite.config.{ext}")))
                        .exists()
                });
                if vite && !custom {
                    if let Ok(html) = fs::read_to_string(repo.join(join(&dir, "index.html"))) {
                        for target in module_scripts(&html) {
                            if let Some(path) = relative(&dir, target.trim_start_matches('/')) {
                                add(
                                    &mut entries,
                                    &files,
                                    path,
                                    format!("{}: module script", join(&dir, "index.html")),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    for file in files
        .iter()
        .filter(|file| file.ends_with(".go") && !file.ends_with("_test.go"))
    {
        if let Ok(text) = fs::read_to_string(repo.join(file)) {
            if go_main(&text) {
                add(
                    &mut entries,
                    &files,
                    (*file).to_owned(),
                    "Go package main with func main".into(),
                );
            }
        }
    }
    entries
}

fn join(dir: &str, path: &str) -> String {
    if dir.is_empty() {
        path.to_owned()
    } else {
        format!("{dir}/{path}")
    }
}

fn relative(dir: &str, target: &str) -> Option<String> {
    let path = Path::new(target);
    if path.is_absolute() || target.contains(':') || target.contains('?') || target.contains('#') {
        return None;
    }
    let mut parts = Vec::new();
    for part in Path::new(dir).join(path).components() {
        match part {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn add(entries: &mut BTreeMap<String, String>, files: &BTreeSet<&str>, path: String, why: String) {
    if files.contains(path.as_str()) {
        entries.entry(path).or_insert(why);
    }
}

fn strings(value: &serde_json::Value, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(value) => {
            out.insert(value.clone());
        }
        serde_json::Value::Array(values) => {
            for value in values {
                strings(value, out);
            }
        }
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if key != "types" {
                    strings(value, out);
                }
            }
        }
        _ => {}
    }
}

fn dist_source(dir: &str, target: &str, files: &BTreeSet<&str>) -> Option<String> {
    let rest = target.trim_start_matches("./").strip_prefix("dist/")?;
    let stem = [".js", ".mjs", ".cjs"]
        .into_iter()
        .find_map(|ext| rest.strip_suffix(ext))?;
    let mut found = BTreeSet::new();
    for ext in ["ts", "tsx", "js", "jsx", "mts", "cts"] {
        for path in [
            format!("src/{stem}.{ext}"),
            format!("src/{stem}/index.{ext}"),
        ] {
            let path = relative(dir, &path)?;
            if files.contains(path.as_str()) {
                found.insert(path);
            }
        }
    }
    (found.len() == 1).then(|| found.into_iter().next().unwrap())
}

// Read static HTML attributes only. Dynamic expressions and inline scripts cannot
// name mapped files; comments are removed before scanning script opening tags.
fn module_scripts(html: &str) -> Vec<String> {
    let mut clean = String::new();
    let mut rest = html;
    while let Some((before, comment)) = rest.split_once("<!--") {
        clean.push_str(before);
        let Some((_, after)) = comment.split_once("-->") else {
            return Vec::new();
        };
        rest = after;
    }
    clean.push_str(rest);
    let mut result = Vec::new();
    for script in clean.split("<script").skip(1) {
        let Some((tag, _)) = script.split_once('>') else {
            continue;
        };
        let mut attrs = BTreeMap::new();
        let mut rest = tag.trim();
        while !rest.is_empty() {
            let end = rest
                .find(|c: char| c.is_whitespace() || c == '=')
                .unwrap_or(rest.len());
            let name = &rest[..end];
            rest = rest[end..].trim_start();
            if !rest.starts_with('=') {
                if end == 0 {
                    break;
                }
                continue;
            }
            rest = rest[1..].trim_start();
            let Some(quote @ ('\'' | '"')) = rest.chars().next() else {
                break;
            };
            rest = &rest[1..];
            let Some(end) = rest.find(quote) else {
                break;
            };
            attrs.insert(name, &rest[..end]);
            rest = rest[end + 1..].trim_start();
        }
        if attrs.get("type") == Some(&"module") {
            if let Some(src) = attrs.get("src") {
                result.push((*src).to_owned());
            }
        }
    }
    result
}

fn go_main(text: &str) -> bool {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("Go parser");
    let Some(tree) = parser.parse(text, None) else {
        return false;
    };
    let mut package = false;
    let mut main = false;
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().named_children(&mut cursor) {
        if node.kind() == "package_clause" {
            let mut cursor = node.walk();
            package = node.named_children(&mut cursor).any(|child| {
                child.kind() == "package_identifier"
                    && child.utf8_text(text.as_bytes()).ok() == Some("main")
            });
        }
        if node.kind() == "function_declaration" {
            main |= node
                .child_by_field_name("name")
                .is_some_and(|name| name.utf8_text(text.as_bytes()).ok() == Some("main"));
        }
    }
    package && main
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SourceNode;
    fn entries(files: &[(&str, &str)]) -> BTreeMap<String, String> {
        let repo = tempfile::tempdir().unwrap();
        let mut nodes = Vec::new();
        for (path, content) in files {
            let dest = repo.path().join(path);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(dest, content).unwrap();
            if path.ends_with(".rs")
                || path.ends_with(".ts")
                || path.ends_with(".tsx")
                || path.ends_with(".go")
                || path.ends_with(".js")
            {
                nodes.push(SourceNode {
                    file: (*path).into(),
                    loc: 1,
                    code_lines: None,
                    complexity: 1,
                    churn: 0,
                    fanin: 0.0,
                    module: String::new(),
                    lang: String::new(),
                });
            }
        }
        let first = detect(repo.path(), &nodes);
        nodes.reverse();
        assert_eq!(first, detect(repo.path(), &nodes));
        first
    }
    #[test]
    fn cargo_bins_and_conventions_exclude_build_script() {
        let found = entries(&[("Cargo.toml", "[package]\nname='app'\n[[bin]]\nname='tool'\npath='tools/run.rs'\n[[bin]]\nname='build'\npath='build.rs'"), ("src/main.rs", ""), ("src/bin/a.rs", ""), ("src/bin/b/main.rs", ""), ("src/bin/b/helper.rs", ""), ("tools/run.rs", ""), ("build.rs", ""), ("src/lib.rs", "")]);
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "src/bin/a.rs",
                "src/bin/b/main.rs",
                "src/main.rs",
                "tools/run.rs"
            ]
        );
        assert!(found["tools/run.rs"].contains("[[bin]].path"));
    }
    #[test]
    fn package_bin_main_module_exports_and_unique_source() {
        let found = entries(&[
            (
                "package.json",
                r#"{"bin":{"tool":"cli.ts"},"main":"dist/main.js","module":"src/module.ts","exports":{".":{"import":"./src/export.ts","types":"./src/types.ts"},"./ambiguous":"./dist/ambiguous.js","./missing":"./dist/missing.js","./*":"./src/*.ts"}}"#,
            ),
            ("cli.ts", ""),
            ("src/main.ts", ""),
            ("src/module.ts", ""),
            ("src/export.ts", ""),
            ("src/types.ts", ""),
            ("src/ambiguous.ts", ""),
            ("src/ambiguous.tsx", ""),
        ]);
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["cli.ts", "src/export.ts", "src/main.ts", "src/module.ts"]
        );
        assert!(found["src/main.ts"].contains("unique dist/src"));
    }
    #[test]
    fn package_string_bin_and_array_exports() {
        let found = entries(&[
            (
                "pkg/package.json",
                r#"{"bin":"./cli.ts","exports":["./one.ts",{"default":"./two.ts"}]}"#,
            ),
            ("pkg/cli.ts", ""),
            ("pkg/one.ts", ""),
            ("pkg/two.ts", ""),
        ]);
        assert_eq!(found.len(), 3);
    }
    #[test]
    fn vite_static_module_script_only() {
        let found = entries(&[
            ("package.json", r#"{"devDependencies":{"vite":"1"}}"#),
            (
                "index.html",
                r#"<!-- <script type="module" src="/src/comment.ts"></script> --><script src='/src/main.ts' type = 'module'></script><script src='/src/ordinary.ts'></script>"#,
            ),
            ("src/main.ts", ""),
            ("src/comment.ts", ""),
            ("src/ordinary.ts", ""),
        ]);
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["src/main.ts"]
        );
    }
    #[test]
    fn go_requires_real_main_declarations() {
        let found = entries(&[
            ("cmd/main.go", "package main\nfunc main() {}"),
            ("lib/main.go", "package lib\nfunc main() {}"),
            (
                "cmd/fake.go",
                "package main\n// func main() {}\nvar text = `func main() {}`",
            ),
            ("cmd/main_test.go", "package main\nfunc main() {}"),
        ]);
        assert_eq!(
            found.keys().map(String::as_str).collect::<Vec<_>>(),
            ["cmd/main.go"]
        );
    }
}
