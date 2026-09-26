//! Acceptance test for issue #4: detection must reproduce the known-correct
//! `pkg`/`lang` for every fixture repository in `data/fixtures.toml` (nine,
//! plus tolmap itself as the Rust fixture since issue #126).
//!
//! Reads the answers from the manifest rather than hardcoding a second copy
//! (the same "one source of truth" rule the schema follows -- CLAUDE.md).
//! Runs against real clones when present; skips with a clear message when
//! they are not, since CI has no clones (mirrors `eval/verify_fixtures.py`'s
//! `TOLMAP_FIXTURE_REPOS` convention: `TOLMAP_FIXTURE_REPOS/<name>`).

use std::path::{Path, PathBuf};

use tolmap::detect;
use tolmap::extract::LanguageKind;

struct Fixture {
    name: String,
    pkg: String,
    lang: String,
}

/// Hand-parses just the four keys this test needs (`pkg`, `lang`, plus the
/// `[name]` section headers) out of `data/fixtures.toml`, rather than adding
/// the `toml` crate's full document model here -- `detect.rs` already needs
/// `toml::Value` for pyproject.toml, so this stays a plain line scan to keep
/// the two uses independent (this one is a test reading a fixed manifest
/// shape, not a general parser).
fn load_fixtures(manifest_path: &Path) -> Vec<Fixture> {
    let text = std::fs::read_to_string(manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
    let mut fixtures = Vec::new();
    let mut current: Option<(String, Option<String>, Option<String>)> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some((name, pkg, lang)) = current.take() {
                if let (Some(pkg), Some(lang)) = (pkg, lang) {
                    fixtures.push(Fixture { name, pkg, lang });
                }
            }
            current = Some((name.to_owned(), None, None));
            continue;
        }
        let Some((_, pkg, lang)) = current.as_mut() else {
            continue;
        };
        if let Some(value) = line.strip_prefix("pkg") {
            if let Some(value) = value.trim_start().strip_prefix('=') {
                *pkg = Some(unquote(value.trim()));
            }
        } else if let Some(value) = line.strip_prefix("lang") {
            if let Some(value) = value.trim_start().strip_prefix('=') {
                *lang = Some(unquote(value.trim()));
            }
        }
    }
    if let Some((name, pkg, lang)) = current.take() {
        if let (Some(pkg), Some(lang)) = (pkg, lang) {
            fixtures.push(Fixture { name, pkg, lang });
        }
    }
    assert_eq!(
        fixtures.len(),
        10,
        "expected 10 fixtures in {}, parsed {}",
        manifest_path.display(),
        fixtures.len()
    );
    fixtures
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').to_owned()
}

#[test]
fn detection_reproduces_all_nine_fixtures() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/fixtures.toml");
    let fixtures = load_fixtures(&manifest);

    let repos_dir = std::env::var("TOLMAP_FIXTURE_REPOS")
        .ok()
        .map(PathBuf::from);
    let Some(repos_dir) = repos_dir.filter(|d| d.is_dir()) else {
        eprintln!(
            "skipping: TOLMAP_FIXTURE_REPOS is not set (or not a directory) -- \
             point it at a directory of clones (<dir>/<name>) to run this test, \
             e.g. TOLMAP_FIXTURE_REPOS=/tmp/tolmap-fixtures.XXXXXX cargo test \
             detection_reproduces_all_nine_fixtures"
        );
        return;
    };

    let mut failures = Vec::new();
    for fixture in &fixtures {
        let repo = repos_dir.join(&fixture.name);
        if !repo.is_dir() {
            eprintln!("{}: skipped, no clone at {}", fixture.name, repo.display());
            continue;
        }
        let expected_lang = LanguageKind::parse(&fixture.lang).unwrap();
        match detect::detect_language(&repo, expected_lang) {
            Ok(Some(candidate)) if candidate.pkg == fixture.pkg => {
                println!("{}: ok -- {}", fixture.name, candidate.describe());
            }
            Ok(Some(candidate)) => {
                let msg = format!(
                    "{}: expected pkg={:?} lang={:?}, detected {}",
                    fixture.name,
                    fixture.pkg,
                    fixture.lang,
                    candidate.describe()
                );
                eprintln!("{msg}");
                failures.push(msg);
            }
            Ok(None) => {
                let msg = format!(
                    "{}: expected pkg={:?} lang={:?}, detected nothing",
                    fixture.name, fixture.pkg, fixture.lang
                );
                eprintln!("{msg}");
                failures.push(msg);
            }
            Err(e) => {
                let msg = format!("{}: detect_language errored: {e}", fixture.name);
                eprintln!("{msg}");
                failures.push(msg);
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of 10 fixtures mismatched:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
