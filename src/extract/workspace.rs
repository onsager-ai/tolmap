use std::{fs, path::Path};

use super::join_slash;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct TsPrefix {
    pub(super) prefix: String,
    pub(super) target: String,
    pub(super) scope: String,
    /// `true` for a workspace `package.json` name, `false` for a tsconfig
    /// `paths` alias. Resolution branches on this: a package name is looked
    /// up through its manifest (`exports`, then `main`/`types`/`module`,
    /// then `src/index.ts`), while an alias is a plain directory
    /// substitution through the existing relative-import probe.
    pub(super) is_package: bool,
}

/// The subset of `package.json` that decides a workspace package's entry
/// point. Read once per matched package directory in [`super::module_index`] and
/// consulted by [`super::resolve_package_entry`] for every bare-specifier import
/// that names this package.
#[derive(Debug, Default, Clone)]
pub(super) struct PackageManifest {
    pub(super) exports: Option<serde_json::Value>,
    pub(super) types: Option<String>,
    pub(super) module: Option<String>,
    pub(super) main: Option<String>,
    /// Issue #115's guarded mirror rule. Kept raw, exactly as `exports` is:
    /// [`super::mirror_build_output`] reuses [`super::match_export_key`] unchanged to match
    /// a subpath's key against a version-range's map, `*`-pattern included.
    pub(super) types_versions: Option<serde_json::Value>,
}

/// Workspace-member globs from the repository root only: `pnpm-workspace.yaml`
/// `packages:`, root `package.json` `workspaces` (array or `{packages:[]}`),
/// and root `lerna.json` `packages`. Read directly rather than during the
/// tree walk -- these three files only matter at the root, so there is
/// nothing to gain from discovering them mid-walk, and reading them upfront
/// means every subsequent `package.json` can be gated against a complete
/// list.
pub(super) fn workspace_globs_from_root(repo: &Path) -> Vec<String> {
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
pub(super) fn is_workspace_member(globs: &[String], dir: &str) -> bool {
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

pub(super) fn package_manifest(path: &Path) -> Option<(String, PackageManifest)> {
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
        types_versions: value.get("typesVersions").cloned(),
    };
    Some((name, manifest))
}

/// Return `compilerOptions.paths` from one tsconfig as repo-relative roots.
///
/// `extends` is deliberately not followed. It can point into `node_modules`,
/// and a map whose aliases depend on whether dependencies happen to be
/// installed is not a map of the commit.
pub(super) fn tsconfig_aliases(path: &Path, here: &str) -> Vec<TsPrefix> {
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
