//! Auto-detection of language and source root from a bare clone.
//!
//! `tolmap build` used to require `--pkg`/`--lang`, which is fine for a CLI
//! run against a repository you already know, but nobody pasting a URL at a
//! public endpoint will supply either -- and getting `--pkg` wrong does not
//! fail loudly, it produces a sparse graph and a plausible-looking wrong map
//! (`docs/FINDINGS.md` finding 7). So detection carries its own evidence:
//! every candidate root it considered, which marker file decided it, how
//! many files it counted, and an explicit confidence -- never a bare guess.
//!
//! **This module does not merge polyglot repositories itself.** Most real
//! repositories are not one language, and `docs/ARCHITECTURE.md` argues
//! merging is the right long-term answer (a concern crosses languages, so
//! districts should too) -- see `docs/FINDINGS.md` finding 13 for what was
//! actually measured about which signals do and don't bridge languages. This
//! module's job stays narrower: detect and report *every* plausible source
//! -- one candidate per language that has any -- and return the one with the
//! most files as `chosen`. [`all_sources`] filters that same candidate list
//! down to the ones worth merging (`--all-sources`); `extract::build_multi_source`
//! does the actual merge. A caller (a human via `tolmap detect`, or
//! `tolmap build` reading `--pkg`/`--lang`) can always override either.
//!
//! Determinism (`CLAUDE.md`, finding 9): every candidate list is sorted by
//! file count and then by a stable key (path or language name) before the
//! winner is picked, so the choice never depends on directory-iteration
//! order or a `HashMap`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Result};

use crate::extract::{self, LanguageKind, MULTI_SKIP_DIR};

/// How sure `detect` is about a candidate. Never inflated: a guess that had
/// to fall back past every ecosystem marker is `Low`, not `High` dressed up
/// to look confident (numbers are a lower bound; so is confidence).
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// One plausible `(language, source root)` pair, with the evidence that
/// produced it.
#[derive(Clone, Debug)]
pub struct SourceCandidate {
    pub language: LanguageKind,
    /// Source root relative to the repository, in the same shape `--pkg`
    /// takes today (`"."`, `"celery"`, `"src/flask"`, `"lib/sqlalchemy"`).
    pub pkg: String,
    pub confidence: Confidence,
    /// Files `extract::source_files` would actually walk under `pkg` for
    /// `language` -- the same walk `build` runs, so this count is never
    /// optimistic about what a real build would find.
    pub file_count: usize,
    /// Human-readable reason: which marker file, what it said, what else
    /// was considered.
    pub evidence: String,
}

impl SourceCandidate {
    pub fn describe(&self) -> String {
        format!(
            "{} at {} ({} files, {} confidence) -- {}",
            self.language.as_str(),
            self.pkg,
            self.file_count,
            self.confidence.as_str(),
            self.evidence
        )
    }
}

/// The result of detecting a whole repository: the chosen source, plus every
/// other plausible source that was found (across all three languages),
/// `chosen` included, sorted by file count descending.
#[derive(Clone, Debug)]
pub struct Detection {
    pub chosen: SourceCandidate,
    pub candidates: Vec<SourceCandidate>,
}

/// The floor `--all-sources` (`tolmap build --all-sources` and friends;
/// `src/main.rs`) applies to `detect()`'s candidates before treating one as
/// a real second source to merge in: below this many files, or below this
/// share of every detected source file across all languages, a language is
/// noise (a stray generated `.ts` file checked into a Python repo, a single
/// vendored `.go` shim) rather than a second concern worth extracting and
/// blending. Both numbers live in this one place because
/// `tolmap polyglot-report` (step 2 of the polyglot work) measures whether
/// they are right on real repositories -- see `docs/FINDINGS.md` finding 13
/// -- and the pipeline is deliberately not retuned from what that measured;
/// changing either constant is a decision for whoever reads that finding,
/// not something this module should do on its own.
pub const ALL_SOURCES_MIN_FILES: usize = 25;
pub const ALL_SOURCES_MIN_SHARE: f64 = 0.05;

/// `detect()`'s candidates that clear the `--all-sources` floor above,
/// sorted by `(language.as_str(), pkg)` -- the same order
/// `extract::build_multi_source` merges in, so a caller can pass this
/// straight through with no further sorting. The share denominator is the
/// sum of every detected candidate's `file_count` (one per language that has
/// any source at all), not the repository's total file count -- vendored or
/// otherwise-excluded files were never a candidate to begin with, and
/// counting them would make the 5% floor stricter for no reason tied to
/// what `--all-sources` is actually choosing between.
pub fn all_sources(repo: &Path) -> Result<Vec<SourceCandidate>> {
    let detection = detect(repo)?;
    let total_detected_files: usize = detection.candidates.iter().map(|c| c.file_count).sum();
    let mut selected = detection
        .candidates
        .into_iter()
        .filter(|candidate| {
            candidate.file_count >= ALL_SOURCES_MIN_FILES
                && candidate.file_count as f64
                    >= ALL_SOURCES_MIN_SHARE * total_detected_files as f64
        })
        .collect::<Vec<_>>();
    selected.sort_by(|a, b| {
        a.language
            .as_str()
            .cmp(b.language.as_str())
            .then_with(|| a.pkg.cmp(&b.pkg))
    });
    Ok(selected)
}

/// Detects language and source root for `repo`, across all supported
/// languages, and picks the one with the most files as `chosen`.
pub fn detect(repo: &Path) -> Result<Detection> {
    ensure!(
        repo.is_dir(),
        "repository {} is not a directory",
        repo.display()
    );
    let mut candidates = Vec::new();
    for language in [
        LanguageKind::Python,
        LanguageKind::Go,
        LanguageKind::TypeScript,
        LanguageKind::Rust,
    ] {
        if let Some(candidate) = detect_language(repo, language)? {
            candidates.push(candidate);
        }
    }
    ensure!(
        !candidates.is_empty(),
        "no supported source (.py, .go, .ts, or .rs) found in {}",
        repo.display()
    );
    sort_candidates(&mut candidates);
    let chosen = candidates[0].clone();
    Ok(Detection { chosen, candidates })
}

/// Detects a source root for one specific language, without considering the
/// others. This is what `tolmap build --lang go` (with `--pkg` omitted)
/// uses: the caller already pinned the language, so only that language's
/// root needs resolving. Returns `Ok(None)` if `language` has no files in
/// `repo` at all.
pub fn detect_language(repo: &Path, language: LanguageKind) -> Result<Option<SourceCandidate>> {
    match language {
        LanguageKind::Python => detect_python(repo),
        LanguageKind::Go => detect_go(repo),
        LanguageKind::TypeScript => detect_typescript(repo),
        LanguageKind::Rust => detect_rust(repo),
    }
}

fn sort_candidates(candidates: &mut [SourceCandidate]) {
    candidates.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| a.language.as_str().cmp(b.language.as_str()))
            .then_with(|| a.pkg.cmp(&b.pkg))
    });
}

fn sort_pkg_counts(items: &mut [(String, usize)]) {
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

// ---------------------------------------------------------------------
// Python: docs/ARCHITECTURE.md -- "pyproject.toml or setup.py points at
// src/<pkg> or a top-level package directory".
// ---------------------------------------------------------------------

struct PyMeta {
    name: Option<String>,
    /// `[tool.setuptools.packages.find].where`, when pyproject.toml declares
    /// it explicitly (sqlalchemy: `where = ["lib"]`). Ground truth when
    /// present -- takes precedence over the src/root default guess.
    where_dirs: Vec<String>,
    source_file: &'static str,
}

fn detect_python(repo: &Path) -> Result<Option<SourceCandidate>> {
    let total = extract::source_files(repo, ".", LanguageKind::Python)?.len();
    if total == 0 {
        return Ok(None);
    }

    if let Some(meta) = read_python_metadata(repo) {
        let where_dirs = if meta.where_dirs.is_empty() {
            vec!["src".to_owned(), ".".to_owned()]
        } else {
            meta.where_dirs.clone()
        };
        if let Some(name) = &meta.name {
            for where_dir in &where_dirs {
                for candidate_name in name_variants(name) {
                    let candidate_pkg = if where_dir == "." {
                        candidate_name
                    } else {
                        format!("{where_dir}/{candidate_name}")
                    };
                    if let Ok(files) =
                        extract::source_files(repo, &candidate_pkg, LanguageKind::Python)
                    {
                        if !files.is_empty() {
                            let where_note = if meta.where_dirs.is_empty() {
                                String::new()
                            } else {
                                format!(", where={:?}", meta.where_dirs)
                            };
                            // Report the path that actually matched on disk,
                            // not the declared name reconstructed -- PyPI
                            // names are case/separator-insensitive (PEP
                            // 503), so the two can differ (Django -> django)
                            // and the evidence must describe reality.
                            let evidence = format!(
                                "{} declares name {name:?}{where_note}; matched {candidate_pkg}",
                                meta.source_file
                            );
                            return Ok(Some(SourceCandidate {
                                language: LanguageKind::Python,
                                pkg: candidate_pkg,
                                confidence: Confidence::High,
                                file_count: files.len(),
                                evidence,
                            }));
                        }
                    }
                }
            }
        }
    }

    // No metadata, or the declared name didn't match anything on disk (e.g.
    // the PyPI name and the package directory diverge in a way this doesn't
    // know about). Fall back to structure: a single directory with an
    // `__init__.py` is a real package even with no metadata to name it --
    // this is the "src/ and no package metadata at all" shape.
    if let Some(candidate) = structural_python_guess(repo)? {
        return Ok(Some(candidate));
    }

    // Last resort: no package layout recognised at all. Map the repository
    // root directly rather than refusing -- silence is the failure mode
    // this module exists to avoid -- but say plainly that it's a guess.
    Ok(Some(SourceCandidate {
        language: LanguageKind::Python,
        pkg: ".".to_owned(),
        confidence: Confidence::Low,
        file_count: total,
        evidence: "no pyproject.toml/setup.py package match and no directory with __init__.py; mapping the repository root directly".to_owned(),
    }))
}

/// Name-matching variants tried against directories on disk. PyPI project
/// names are case-insensitive and treat `-`/`_`/`.` as equivalent (PEP 503);
/// the directory underneath is conventionally lowercase with underscores
/// (`Flask` -> `flask`, `SQLAlchemy` -> `sqlalchemy`), so normalise before
/// checking rather than requiring an exact match against the declared name.
fn name_variants(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let underscored = lower.replace(['-', '.'], "_");
    let hyphenated = lower.replace(['_', '.'], "-");
    let mut variants = vec![underscored, hyphenated, lower, name.to_owned()];
    variants.sort();
    variants.dedup();
    variants
}

fn read_python_metadata(repo: &Path) -> Option<PyMeta> {
    if let Ok(text) = fs::read_to_string(repo.join("pyproject.toml")) {
        if let Ok(value) = text.parse::<toml::Value>() {
            let name = value
                .get("project")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .or_else(|| {
                    value
                        .get("tool")
                        .and_then(|t| t.get("poetry"))
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                })
                .map(str::to_owned);
            let where_dirs = value
                .get("tool")
                .and_then(|t| t.get("setuptools"))
                .and_then(|s| s.get("packages"))
                .and_then(|p| p.get("find"))
                .and_then(|f| f.get("where"))
                .and_then(|w| w.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if name.is_some() || !where_dirs.is_empty() {
                return Some(PyMeta {
                    name,
                    where_dirs,
                    source_file: "pyproject.toml",
                });
            }
        }
    }
    // setup.py is Python, not the ported language -- no interpreter here, so
    // this is a text scan for the first `name = "..."` literal (case
    // insensitive, so a `NAME = 'celery'` constant later passed as
    // `setup(name=NAME)` is still recovered) rather than an attempt to
    // evaluate the file.
    if let Ok(text) = fs::read_to_string(repo.join("setup.py")) {
        if let Some(name) = first_name_literal(&text) {
            return Some(PyMeta {
                name: Some(name),
                where_dirs: Vec::new(),
                source_file: "setup.py",
            });
        }
    }
    if let Ok(text) = fs::read_to_string(repo.join("setup.cfg")) {
        if let Some(name) = first_name_literal(&text) {
            return Some(PyMeta {
                name: Some(name),
                where_dirs: Vec::new(),
                source_file: "setup.cfg",
            });
        }
    }
    None
}

/// First `name = "<value>"` (or `NAME = '<value>'`, etc.) in `text`, matched
/// as a whole word so `package_dir`/`long_name`/`namespace` don't trip it,
/// and `==` doesn't get mistaken for the assignment. Good enough to recover
/// a declared package name from `setup.py`/`setup.cfg`; anything more
/// ambitious means parsing Python, which is out of scope for a Rust port.
fn first_name_literal(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("name") {
        let idx = search_from + rel;
        let before_ok = idx == 0 || !is_ident_byte(bytes[idx - 1]);
        let after = idx + 4;
        search_from = after;
        if !before_ok || after >= bytes.len() || is_ident_byte(bytes[after]) {
            continue;
        }
        let rest = text[after..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        if rest.starts_with('=') {
            continue; // `==` comparison, not assignment
        }
        let rest = rest.trim_start();
        let mut chars = rest.chars();
        let Some(quote) = chars.next().filter(|c| *c == '\'' || *c == '"') else {
            continue;
        };
        let body = &rest[quote.len_utf8()..];
        if let Some(end) = body.find(quote) {
            let value = &body[..end];
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Non-source top-level directory names, beyond `extract::PY_SKIP_DIR`,
/// worth excluding specifically when *guessing* a package root (a `tools/`
/// or `docs/` directory is never the package even if it happens to hold a
/// stray `.py` file with an `__init__.py`-shaped sibling).
const PY_GUESS_EXCLUDE: &[&str] = &[
    "docs",
    "doc",
    "documentation",
    "examples",
    "example",
    "scripts",
    "script",
    "tools",
    "tool",
    "bench",
    "benchmarks",
    "build",
    "dist",
    "site-packages",
    "extra",
    "extras",
];

fn structural_python_guess(repo: &Path) -> Result<Option<SourceCandidate>> {
    for sub in ["src", "."] {
        let candidates = candidate_python_packages(repo, sub)?;
        if candidates.is_empty() {
            continue;
        }
        let mut scored = candidates.clone();
        sort_pkg_counts(&mut scored);
        let (pkg, count) = scored[0].clone();
        let confidence = if scored.len() == 1 {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        let evidence = if scored.len() == 1 {
            format!("no pyproject.toml/setup.py name match; single package directory {pkg} (has __init__.py)")
        } else {
            let others = scored[1..]
                .iter()
                .map(|(p, c)| format!("{p} ({c} files)"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "no pyproject.toml/setup.py name match; {} candidate package directories under {sub}/ (has __init__.py), chose {pkg} by file count over {others}",
                scored.len()
            )
        };
        return Ok(Some(SourceCandidate {
            language: LanguageKind::Python,
            pkg,
            confidence,
            file_count: count,
            evidence,
        }));
    }
    Ok(None)
}

fn candidate_python_packages(repo: &Path, sub: &str) -> Result<Vec<(String, usize)>> {
    let base = if sub == "." {
        repo.to_path_buf()
    } else {
        repo.join(sub)
    };
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(&base)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    let mut result = Vec::new();
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')
            || extract::PY_SKIP_DIR.contains(&name.as_str())
            || PY_GUESS_EXCLUDE.contains(&name.as_str())
        {
            continue;
        }
        if !entry.path().join("__init__.py").is_file() {
            continue;
        }
        let pkg = if sub == "." {
            name
        } else {
            format!("{sub}/{name}")
        };
        let files = extract::source_files(repo, &pkg, LanguageKind::Python)?;
        if !files.is_empty() {
            result.push((pkg, files.len()));
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------
// Go: docs/ARCHITECTURE.md -- "go.mod puts it at the repo root".
// ---------------------------------------------------------------------

fn detect_go(repo: &Path) -> Result<Option<SourceCandidate>> {
    let total = extract::source_files(repo, ".", LanguageKind::Go)?.len();
    if total == 0 {
        return Ok(None);
    }

    if repo.join("go.mod").is_file() {
        return Ok(Some(SourceCandidate {
            language: LanguageKind::Go,
            pkg: ".".to_owned(),
            confidence: Confidence::High,
            file_count: total,
            evidence: "go.mod at repository root".to_owned(),
        }));
    }

    // No root go.mod. Either the module lives in a subdirectory, or this is
    // a Go monorepo with several -- the "monorepo with several plausible
    // roots" shape, for Go. Report every go.mod found; pick by file count.
    let mods = find_go_mod_dirs(repo)?;
    if mods.is_empty() {
        return Ok(Some(SourceCandidate {
            language: LanguageKind::Go,
            pkg: ".".to_owned(),
            confidence: Confidence::Low,
            file_count: total,
            evidence:
                "no go.mod found anywhere in the repository; mapping the repository root directly"
                    .to_owned(),
        }));
    }
    let mut scored = mods;
    sort_pkg_counts(&mut scored);
    let (pkg, count) = scored[0].clone();
    let confidence = if scored.len() == 1 {
        Confidence::High
    } else {
        Confidence::Medium
    };
    let evidence = if scored.len() == 1 {
        format!("go.mod found at {pkg} (not the repository root)")
    } else {
        let others = scored[1..]
            .iter()
            .map(|(p, c)| format!("{p} ({c} files)"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} go.mod files found; chose {pkg} by file count over {others}",
            scored.len()
        )
    };
    Ok(Some(SourceCandidate {
        language: LanguageKind::Go,
        pkg,
        confidence,
        file_count: count,
        evidence,
    }))
}

fn find_go_mod_dirs(repo: &Path) -> Result<Vec<(String, usize)>> {
    let mut dirs = Vec::new();
    collect_go_mod_dirs(repo, repo, &mut dirs)?;
    dirs.sort();
    let mut result = Vec::new();
    for dir in dirs {
        let rel = if dir.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            dir.to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        };
        let files = extract::source_files(repo, &rel, LanguageKind::Go)?;
        result.push((rel, files.len()));
    }
    Ok(result)
}

fn collect_go_mod_dirs(repo: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            if MULTI_SKIP_DIR.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            collect_go_mod_dirs(repo, &path, out)?;
        } else if file_type.is_file() && name == "go.mod" {
            out.push(dir.strip_prefix(repo)?.to_path_buf());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Rust (issue #126): a Cargo.toml at the root, as a workspace (`[workspace]
// members`) or a package (`[package]`), covers every crate under it; the
// resolver finds the crates themselves (`extract::rust::crate_targets`,
// which follows `path =` dependencies between them). `target/` is cargo's
// build output and is never walked.
// ---------------------------------------------------------------------

fn detect_rust(repo: &Path) -> Result<Option<SourceCandidate>> {
    let total = extract::source_files(repo, ".", LanguageKind::Rust)?.len();
    if total == 0 {
        return Ok(None);
    }
    if let Some(manifest) = fs::read_to_string(repo.join("Cargo.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok())
    {
        let members = manifest
            .get("workspace")
            .and_then(|workspace| workspace.get("members"))
            .and_then(toml::Value::as_array)
            .map(|members| {
                members
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            });
        let package = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str);
        let evidence = match (members, package) {
            (Some(members), Some(package)) => format!(
                "Cargo.toml at repository root: package {package} and [workspace] members {members}"
            ),
            (Some(members), None) => {
                format!("Cargo.toml at repository root: [workspace] members {members}")
            }
            (None, Some(package)) => format!("Cargo.toml at repository root: package {package}"),
            (None, None) => "Cargo.toml at repository root".to_owned(),
        };
        return Ok(Some(SourceCandidate {
            language: LanguageKind::Rust,
            pkg: ".".to_owned(),
            confidence: Confidence::High,
            file_count: total,
            evidence,
        }));
    }
    // No root manifest: every Cargo.toml found, chosen by file count, as Go
    // does for go.mod.
    let mut roots = Vec::new();
    collect_named_dirs(repo, repo, "Cargo.toml", &mut roots)?;
    roots.sort();
    let mut scored = Vec::new();
    for dir in roots {
        let rel = dir
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        // An unreadable root manifest: the root still covers everything.
        let rel = if rel.is_empty() { ".".to_owned() } else { rel };
        let files = extract::source_files(repo, &rel, LanguageKind::Rust)?;
        scored.push((rel, files.len()));
    }
    // A nested package inside another's directory is counted in both;
    // only the outermost ones are roots.
    let outer = scored
        .iter()
        .filter(|(dir, _)| {
            !scored.iter().any(|(other, _)| {
                other != dir && (other == "." || dir.starts_with(&format!("{other}/")))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut scored = outer;
    if scored.is_empty() {
        return Ok(Some(SourceCandidate {
            language: LanguageKind::Rust,
            pkg: ".".to_owned(),
            confidence: Confidence::Low,
            file_count: total,
            evidence:
                "no Cargo.toml found anywhere in the repository; mapping the repository root directly"
                    .to_owned(),
        }));
    }
    sort_pkg_counts(&mut scored);
    let (pkg, count) = scored[0].clone();
    let (confidence, evidence) = if scored.len() == 1 {
        (
            Confidence::High,
            format!("Cargo.toml found at {pkg} (not the repository root)"),
        )
    } else {
        let others = scored[1..]
            .iter()
            .map(|(p, c)| format!("{p} ({c} files)"))
            .collect::<Vec<_>>()
            .join(", ");
        (
            Confidence::Medium,
            format!(
                "{} Cargo.toml roots found; chose {pkg} by file count over {others}",
                scored.len()
            ),
        )
    };
    Ok(Some(SourceCandidate {
        language: LanguageKind::Rust,
        pkg,
        confidence,
        file_count: count,
        evidence,
    }))
}

fn collect_named_dirs(repo: &Path, dir: &Path, marker: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            if extract::rust::skip_dir(&name) {
                continue;
            }
            collect_named_dirs(repo, &entry.path(), marker, out)?;
        } else if file_type.is_file() && name == marker {
            out.push(dir.strip_prefix(repo)?.to_path_buf());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// TypeScript: docs/ARCHITECTURE.md -- "package.json plus tsconfig.json
// points at src or a packages/* workspace".
// ---------------------------------------------------------------------

fn detect_typescript(repo: &Path) -> Result<Option<SourceCandidate>> {
    let total = extract::source_files(repo, ".", LanguageKind::TypeScript)?.len();
    if total == 0 {
        return Ok(None);
    }

    let has_package_json = repo.join("package.json").is_file();
    let has_tsconfig =
        repo.join("tsconfig.json").is_file() || repo.join("src/tsconfig.json").is_file();
    if !has_package_json || !has_tsconfig {
        return Ok(Some(SourceCandidate {
            language: LanguageKind::TypeScript,
            pkg: ".".to_owned(),
            confidence: Confidence::Low,
            file_count: total,
            evidence:
                "no package.json+tsconfig.json pair found; mapping the repository root directly"
                    .to_owned(),
        }));
    }

    let mut workspace_globs = read_pnpm_workspace_globs(repo);
    if workspace_globs.is_empty() {
        workspace_globs = read_package_json_workspaces(repo)?;
    }
    if !workspace_globs.is_empty() {
        let mut candidates = Vec::new();
        for dir in &workspace_globs {
            if repo.join(dir).is_dir() {
                let files = extract::source_files(repo, dir, LanguageKind::TypeScript)?;
                if !files.is_empty() {
                    candidates.push((dir.clone(), files.len()));
                }
            }
        }
        if !candidates.is_empty() {
            sort_pkg_counts(&mut candidates);
            let (pkg, count) = candidates[0].clone();
            let evidence = if candidates.len() == 1 {
                format!("package.json+tsconfig.json, workspace glob resolved to {pkg}")
            } else {
                let others = candidates[1..]
                    .iter()
                    .map(|(p, c)| format!("{p} ({c} files)"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "package.json+tsconfig.json, {} workspace roots declared; chose {pkg} by file count over {others}",
                    candidates.len()
                )
            };
            return Ok(Some(SourceCandidate {
                language: LanguageKind::TypeScript,
                pkg,
                confidence: Confidence::High,
                file_count: count,
                evidence,
            }));
        }
    }

    if repo.join("src").is_dir() {
        let files = extract::source_files(repo, "src", LanguageKind::TypeScript)?;
        if !files.is_empty() {
            return Ok(Some(SourceCandidate {
                language: LanguageKind::TypeScript,
                pkg: "src".to_owned(),
                confidence: Confidence::High,
                file_count: files.len(),
                evidence: "package.json+tsconfig.json, conventional src/ layout".to_owned(),
            }));
        }
    }

    Ok(Some(SourceCandidate {
        language: LanguageKind::TypeScript,
        pkg: ".".to_owned(),
        confidence: Confidence::Medium,
        file_count: total,
        evidence: "package.json+tsconfig.json present but no src/ or workspace layout; mapping the repository root directly".to_owned(),
    }))
}

/// `pnpm-workspace.yaml`'s `packages:` list. Hand-rolled rather than a YAML
/// dependency: the file is a flat list of quoted globs under one key, and
/// that shape needs no general YAML parser.
fn read_pnpm_workspace_globs(repo: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(repo.join("pnpm-workspace.yaml")) else {
        return Vec::new();
    };
    parse_yaml_string_list(&text, "packages")
}

fn parse_yaml_string_list(text: &str, key: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut in_list = false;
    let needle = format!("{key}:");
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !in_list {
            if trimmed.starts_with(&needle) {
                in_list = true;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('-') {
            let value = rest.trim().trim_matches(|c| c == '\'' || c == '"');
            if let Some(prefix) = glob_static_prefix(value) {
                result.push(prefix);
            }
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        break; // dedented past the list -- stop collecting
    }
    result
}

fn read_package_json_workspaces(repo: &Path) -> Result<Vec<String>> {
    let Ok(text) = fs::read_to_string(repo.join("package.json")) else {
        return Ok(Vec::new());
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(Vec::new());
    };
    let globs: Vec<String> = match value.get("workspaces") {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(serde_json::Value::Object(obj)) => obj
            .get("packages")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(globs.iter().filter_map(|g| glob_static_prefix(g)).collect())
}

/// The static directory a glob like `"packages/*"` or `"packages/**"`
/// names, or `None` if the glob has a wildcard anywhere but the trailing
/// path segment (that shape names no single directory to point `--pkg` at).
fn glob_static_prefix(glob: &str) -> Option<String> {
    let stripped = glob.trim_end_matches('*').trim_end_matches('/');
    if stripped.is_empty() || stripped.contains('*') {
        None
    } else {
        Some(stripped.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    // -- first_name_literal ------------------------------------------------

    #[test]
    fn name_literal_direct_kwarg() {
        assert_eq!(
            first_name_literal("import setuptools\nsetuptools.setup(name=\"rich\")\n"),
            Some("rich".to_owned())
        );
    }

    #[test]
    fn name_literal_constant_indirection() {
        // celery's setup.py: NAME = 'celery' ... setup(name=NAME, ...)
        let text = "NAME = 'celery'\nVERSION = '5.0'\nsetuptools.setup(\n    name=NAME,\n)\n";
        assert_eq!(first_name_literal(text), Some("celery".to_owned()));
    }

    #[test]
    fn name_literal_ignores_similar_identifiers() {
        let text =
            "package_dir = {'': 'src'}\nlong_name_field = 'nope'\nname == other\nname = 'flask'\n";
        assert_eq!(first_name_literal(text), Some("flask".to_owned()));
    }

    #[test]
    fn name_literal_absent() {
        assert_eq!(first_name_literal("setuptools.setup()\n"), None);
    }

    // -- name_variants -------------------------------------------------

    #[test]
    fn variants_cover_case_and_separators() {
        let variants = name_variants("SQLAlchemy");
        assert!(variants.contains(&"sqlalchemy".to_owned()));
        let variants = name_variants("Flask-Login");
        assert!(variants.contains(&"flask_login".to_owned()));
        assert!(variants.contains(&"flask-login".to_owned()));
    }

    // -- pnpm workspace glob parsing ----------------------------------

    #[test]
    fn pnpm_workspace_globs_stop_at_next_key() {
        let yaml = "packages:\n  - 'packages/*'\n  - 'packages-private/*'\n\ncatalog:\n  foo: 1\n";
        assert_eq!(
            parse_yaml_string_list(yaml, "packages"),
            vec!["packages".to_owned(), "packages-private".to_owned()]
        );
    }

    #[test]
    fn glob_prefix_rejects_mid_path_wildcards() {
        assert_eq!(
            glob_static_prefix("packages/*"),
            Some("packages".to_owned())
        );
        assert_eq!(
            glob_static_prefix("packages/**"),
            Some("packages".to_owned())
        );
        assert_eq!(glob_static_prefix("apps/*/src"), None);
    }

    // -- structural detection on constructed fixtures -----------------

    #[test]
    fn python_src_layout_no_metadata() {
        // The "src/ and no package metadata at all" awkward case: a single
        // package directory under src/, nothing declaring its name.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "src/widget/__init__.py", "");
        write(dir.path(), "src/widget/core.py", "x = 1\n");
        write(dir.path(), "README.md", "");
        let candidate = detect_python(dir.path()).unwrap().unwrap();
        assert_eq!(candidate.pkg, "src/widget");
        assert_eq!(candidate.confidence, Confidence::Medium);
        assert_eq!(candidate.file_count, 2);
    }

    #[test]
    fn python_no_layout_at_all_falls_back_to_root() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "one.py", "");
        write(dir.path(), "two.py", "");
        let candidate = detect_python(dir.path()).unwrap().unwrap();
        assert_eq!(candidate.pkg, ".");
        assert_eq!(candidate.confidence, Confidence::Low);
    }

    #[test]
    fn python_absent_returns_none() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "main.go", "package main\n");
        assert!(detect_python(dir.path()).unwrap().is_none());
    }

    #[test]
    fn go_monorepo_several_plausible_roots() {
        // Awkward case: a monorepo with more than one go.mod and no root
        // one -- must not silently pick either; must report both.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "service-a/go.mod", "module example.com/a\n");
        write(dir.path(), "service-a/main.go", "package main\n");
        write(dir.path(), "service-a/handler.go", "package main\n");
        write(dir.path(), "service-b/go.mod", "module example.com/b\n");
        write(dir.path(), "service-b/main.go", "package main\n");
        let candidate = detect_go(dir.path()).unwrap().unwrap();
        assert_eq!(candidate.pkg, "service-a");
        assert_eq!(candidate.confidence, Confidence::Medium);
        assert!(candidate.evidence.contains("service-b"));
    }

    #[test]
    fn go_vendored_copy_excluded() {
        // A vendored copy of something large at the repo root must not
        // out-count, or be mistaken for, the real module.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "go.mod", "module example.com/real\n");
        write(dir.path(), "main.go", "package main\n");
        for i in 0..50 {
            write(
                dir.path(),
                &format!("vendor/bigdep/file{i}.go"),
                "package bigdep\n",
            );
        }
        let candidate = detect_go(dir.path()).unwrap().unwrap();
        assert_eq!(candidate.pkg, ".");
        assert_eq!(candidate.confidence, Confidence::High);
        // root go.mod short-circuits before vendor is ever walked for
        // go.mod files, but the file count must still exclude vendor/.
        assert_eq!(candidate.file_count, 1);
    }

    #[test]
    fn typescript_pnpm_workspace_picks_dominant_package() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "package.json", "{}\n");
        write(dir.path(), "tsconfig.json", "{}\n");
        write(
            dir.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/*'\n  - 'packages-private/*'\n",
        );
        write(dir.path(), "packages/core/index.ts", "export {}\n");
        write(dir.path(), "packages/core/util.ts", "export {}\n");
        write(dir.path(), "packages-private/tool/index.ts", "export {}\n");
        let candidate = detect_typescript(dir.path()).unwrap().unwrap();
        assert_eq!(candidate.pkg, "packages");
        assert_eq!(candidate.confidence, Confidence::High);
        assert!(candidate.evidence.contains("packages-private"));
    }

    #[test]
    fn polyglot_dominant_language_may_not_be_the_interesting_one() {
        // A repo where a large generated/foreign-language tree outweighs a
        // small, properly-configured package in another language. detect()
        // is documented to report every candidate and choose by file count,
        // not by which one "looks interesting" -- this pins that behaviour
        // rather than silently trying to be clever about it.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pyproject.toml", "[project]\nname = \"tiny\"\n");
        write(dir.path(), "tiny/__init__.py", "");
        write(dir.path(), "tiny/core.py", "");
        write(dir.path(), "go.mod", "module example.com/big\n");
        for i in 0..20 {
            write(dir.path(), &format!("gen/file{i}.go"), "package gen\n");
        }
        let detection = detect(dir.path()).unwrap();
        assert_eq!(detection.chosen.language, LanguageKind::Go);
        assert_eq!(detection.candidates.len(), 2);
        let python = detection
            .candidates
            .iter()
            .find(|c| c.language == LanguageKind::Python)
            .unwrap();
        assert_eq!(python.pkg, "tiny");
        assert_eq!(python.confidence, Confidence::High);
    }

    #[test]
    fn all_sources_drops_a_language_below_either_floor_and_keeps_the_rest() {
        // Go: 60 files (root go.mod) -- well over both floors.
        // TypeScript: 40 files under src/ -- also over both floors.
        // Python: 2 files -- under the 25-file floor AND under 5% of the
        // 102 total detected files (2/102 ~= 2%), so it must be excluded
        // even though it is a real, correctly-detected package.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "go.mod", "module example.com/big\n");
        for i in 0..60 {
            write(dir.path(), &format!("svc/file{i}.go"), "package svc\n");
        }
        write(dir.path(), "package.json", "{}\n");
        write(dir.path(), "tsconfig.json", "{}\n");
        for i in 0..40 {
            write(dir.path(), &format!("src/file{i}.ts"), "export {{}}\n");
        }
        write(dir.path(), "pyproject.toml", "[project]\nname = \"tiny\"\n");
        write(dir.path(), "tiny/__init__.py", "");
        write(dir.path(), "tiny/core.py", "");

        let selected = all_sources(dir.path()).unwrap();
        let languages = selected.iter().map(|c| c.language).collect::<Vec<_>>();
        assert_eq!(
            languages,
            vec![LanguageKind::Go, LanguageKind::TypeScript],
            "python excluded (2 files: under 25 and under 5% of 102 detected); \
             go before typescript by the (lang, pkg) sort all_sources documents"
        );
        for candidate in &selected {
            assert!(candidate.file_count >= ALL_SOURCES_MIN_FILES);
        }
    }

    #[test]
    fn all_sources_returns_every_candidate_when_the_repo_is_small_and_balanced() {
        // Two languages, each comfortably over 25 files and each holding
        // well over half the total -- both must clear the share floor even
        // though it's the smaller one being checked against the total.
        let dir = TempDir::new().unwrap();
        write(dir.path(), "go.mod", "module example.com/mixed\n");
        for i in 0..30 {
            write(dir.path(), &format!("svc/file{i}.go"), "package svc\n");
        }
        write(dir.path(), "pyproject.toml", "[project]\nname = \"tiny\"\n");
        write(dir.path(), "tiny/__init__.py", "");
        for i in 0..30 {
            write(dir.path(), &format!("tiny/mod{i}.py"), "x = 1\n");
        }
        let selected = all_sources(dir.path()).unwrap();
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn rust_workspace_at_the_root_maps_the_root_and_skips_target() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n",
        );
        write(
            dir.path(),
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\n",
        );
        write(dir.path(), "crates/a/src/lib.rs", "pub fn a() {}\n");
        write(
            dir.path(),
            "crates/b/Cargo.toml",
            "[package]\nname = \"b\"\n",
        );
        write(dir.path(), "crates/b/src/main.rs", "fn main() {}\n");
        write(dir.path(), "target/debug/build/gen.rs", "pub fn gen() {}\n");
        let candidate = detect_language(dir.path(), LanguageKind::Rust)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.pkg, ".");
        assert_eq!(candidate.confidence, Confidence::High);
        assert_eq!(candidate.file_count, 2);
        assert!(candidate.evidence.contains("crates/a, crates/b"));
        assert_eq!(
            detect(dir.path()).unwrap().chosen.language,
            LanguageKind::Rust
        );
    }

    #[test]
    fn rust_package_below_the_root_is_found_by_its_manifest() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "tools/cli/Cargo.toml",
            "[package]\nname = \"cli\"\n",
        );
        write(dir.path(), "tools/cli/src/main.rs", "fn main() {}\n");
        write(dir.path(), "tools/cli/src/args.rs", "pub struct Args;\n");
        write(dir.path(), "scripts/one_off.rs", "fn main() {}\n");
        let candidate = detect_language(dir.path(), LanguageKind::Rust)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.pkg, "tools/cli");
        assert_eq!(candidate.file_count, 2);
    }

    #[test]
    fn detect_is_deterministic_across_runs() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[project]\nname = \"widget\"\n",
        );
        write(dir.path(), "widget/__init__.py", "");
        write(dir.path(), "widget/core.py", "");
        let first = detect(dir.path()).unwrap();
        let second = detect(dir.path()).unwrap();
        assert_eq!(first.chosen.pkg, second.chosen.pkg);
        assert_eq!(first.chosen.file_count, second.chosen.file_count);
    }
}
