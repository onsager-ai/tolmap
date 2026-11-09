//! Organization maps end to end (issue #255): two member repositories whose
//! Cargo, npm, Go and Python manifests name each other, read through the
//! real binary at pinned commits. `acme/core` provides one package per
//! ecosystem and is tagged `v1.0.0` at its first commit; the organization
//! pins its second. `acme/app` depends on each: a Cargo git dependency and
//! a Go module both pinned at the first commit (pin skew), an npm and a
//! Python requirement its current version satisfies, and one external
//! dependency per ecosystem.

use std::path::{Path, PathBuf};
use std::process::Command;

const TOLMAP: &str = env!("CARGO_BIN_EXE_tolmap");

const DEFAULTS_ONLY: &[&str] = &[
    "TOLMAP_REFS",
    "TOLMAP_SCIP_INSTALL",
    "TOLMAP_NAMER",
    "TOLMAP_NAMER_MODEL",
    "TOLMAP_PRUNE_VARIANT",
];

fn write(repo: &Path, path: &str, content: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit_all(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", message]);
    git(repo, &["rev-parse", "HEAD"])
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    core: PathBuf,
    app: PathBuf,
    core_first: String,
    core_commit: String,
    app_commit: String,
    app_map: PathBuf,
}

const PY_MODULES: [&str; 6] = [
    "ledger", "invoice", "payment", "tariff", "account", "profile",
];

/// A minimal stored map: two districts of two files each.
fn stored_map() -> String {
    let node = |district: usize| {
        serde_json::json!([district, 0.0, 0.0, 10, 1, 1, 1.0, 0.0, 0.0, 0.0, 0.0])
    };
    let district = |size| serde_json::json!({"size": size, "c": [0.0, 0.0], "blob": []});
    serde_json::to_string(&serde_json::json!({
        "repo": "app",
        "q": 0.25,
        "names": {"0": "cli", "1": "web"},
        "districts": {"0": district(2), "1": district(2)},
        "F": ["src/main.rs", "src/cli.rs", "web/a.ts", "web/b.ts"],
        "N": [node(0), node(0), node(1), node(1)],
        "E": [[0, 1], [2, 3]],
        "L": [],
        "S": {},
        "U": {},
        "roads": [],
        "lang": "rust"
    }))
    .unwrap()
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let core = root.join("core");
    std::fs::create_dir_all(&core).unwrap();
    git(&core, &["init", "-q"]);
    write(
        &core,
        "Cargo.toml",
        "[package]\nname = \"acme-core\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    );
    write(&core, "src/lib.rs", "pub fn core() {}\n");
    write(
        &core,
        "package.json",
        "{\"name\": \"@acme/core\", \"version\": \"1.4.0\"}\n",
    );
    write(&core, "go.mod", "module github.com/acme/core\n\ngo 1.21\n");
    write(
        &core,
        "pyproject.toml",
        "[project]\nname = \"acme-core-py\"\nversion = \"0.3.0\"\n",
    );
    write(&core, "acme_core_py/__init__.py", "");
    for (index, module) in PY_MODULES.iter().enumerate() {
        let next = PY_MODULES[(index + 1) % PY_MODULES.len()];
        write(
            &core,
            &format!("acme_core_py/{module}.py"),
            &format!(
                "from .{next} import {next}_total\n\n\ndef {module}_total(items):\n    return sum(items) + len(items)\n"
            ),
        );
    }
    let core_first = commit_all(&core, "core v1");
    git(&core, &["tag", "v1.0.0"]);
    write(
        &core,
        "acme_core_py/ledger.py",
        "from .invoice import invoice_total\n\n\ndef ledger_total(items):\n    return sum(items) * 2\n",
    );
    let core_commit = commit_all(&core, "core v1.1");

    let app = root.join("clones/acme/app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q"]);
    write(
        &app,
        "Cargo.toml",
        &format!(
            "[package]\nname = \"acme-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nacme-core = {{ git = \"https://github.com/acme/core\", rev = \"{core_first}\" }}\nserde = \"1\"\n"
        ),
    );
    write(&app, "src/main.rs", "fn main() {}\n");
    write(
        &app,
        "web/package.json",
        "{\"name\": \"@acme/app\", \"dependencies\": {\"@acme/core\": \"^1.2.0\", \"left-pad\": \"1.3.0\"}}\n",
    );
    write(
        &app,
        "go.mod",
        "module github.com/acme/app\n\ngo 1.21\n\nrequire (\n\tgithub.com/acme/core v1.0.0\n\tgolang.org/x/mod v0.17.0 // indirect\n)\n",
    );
    write(
        &app,
        "pyproject.toml",
        "[project]\nname = \"acme-app\"\nversion = \"0.1.0\"\ndependencies = [\"acme-core-py>=0.3\", \"requests>=2\"]\n",
    );
    write(&app, "requirements.txt", "-e ./vendored\n");
    let app_commit = commit_all(&app, "app");

    let maps = root.join("maps/acme/app");
    std::fs::create_dir_all(&maps).unwrap();
    let app_map = maps.join(format!("{app_commit}.json"));
    std::fs::write(&app_map, stored_map()).unwrap();

    write(
        &root,
        "acme.org.toml",
        &format!(
            "version = 1\nname = \"acme\"\n\n[[members]]\nrepo = \"acme/core\"\ncommit = \"{core_commit}\"\npath = \"core\"\n\n[[members]]\nrepo = \"acme/app\"\ncommit = \"{app_commit}\"\n"
        ),
    );
    Fixture {
        _dir: dir,
        root,
        core,
        app,
        core_first,
        core_commit,
        app_commit,
        app_map,
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn tolmap(dir: &Path, args: &[&str]) -> Run {
    let mut command = Command::new(TOLMAP);
    command.current_dir(dir).args(args);
    for key in DEFAULTS_ONLY {
        command.env_remove(key);
    }
    let output = command.output().unwrap();
    Run {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn build(fixture: &Fixture) -> Run {
    tolmap(
        &fixture.root,
        &[
            "org",
            "build",
            "acme.org.toml",
            "--clones",
            "clones",
            "--maps",
            "maps",
        ],
    )
}

fn keys(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                out.push(key.clone());
                keys(value, out);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| keys(item, out)),
        _ => {}
    }
}

#[test]
fn org_check_accepts_a_valid_manifest_and_rejects_each_rule_distinctly() {
    let fixture = fixture();
    let run = tolmap(&fixture.root, &["org", "check", "acme.org.toml"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.contains("organization acme: 2 members"));

    let member =
        |commit: &str| format!("[[members]]\nrepo = \"acme/core\"\ncommit = \"{commit}\"\n");
    let cases = [
        (
            format!(
                "version = 1\nname = \"acme\"\nowner = \"x\"\n{}",
                member(&fixture.core_commit)
            ),
            "unknown key",
        ),
        (
            format!(
                "version = 1\nname = \"acme\"\n{}{}",
                member(&fixture.core_commit),
                member(&fixture.core_commit)
            ),
            "listed more than once",
        ),
        (
            format!(
                "version = 1\nname = \"acme\"\n{}",
                member(&fixture.core_commit[..12])
            ),
            "is abbreviated",
        ),
    ];
    for (text, message) in cases {
        write(&fixture.root, "bad.org.toml", &text);
        let run = tolmap(&fixture.root, &["org", "check", "bad.org.toml"]);
        assert_eq!(run.code, 2, "{text}");
        assert!(run.stderr.contains(message), "{message}: {}", run.stderr);
    }
}

#[test]
fn org_build_reports_edges_pin_skew_external_and_unresolved() {
    let fixture = fixture();
    let map_before = std::fs::read(&fixture.app_map).unwrap();
    let first = build(&fixture);
    assert_eq!(first.code, 0, "{}", first.stderr);
    let document: serde_json::Value = serde_json::from_str(&first.stdout).unwrap();

    // Members, sorted by repository; app's map is the stored one, read only.
    let members = document["members"].as_array().unwrap();
    assert_eq!(members[0]["repo"], "acme/app");
    assert_eq!(members[0]["commit"], fixture.app_commit.as_str());
    assert_eq!(members[0]["map"], "stored");
    assert_eq!(members[0]["files"], 4);
    assert_eq!(members[0]["districts"], 2);
    assert_eq!(members[0]["path"], serde_json::Value::Null);
    assert_eq!(members[1]["repo"], "acme/core");
    assert_eq!(members[1]["map"], "built");
    assert_eq!(members[1]["path"], "core");
    assert!(members[1]["map_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(std::fs::read(&fixture.app_map).unwrap(), map_before);

    // One repository edge, with one piece of evidence per ecosystem.
    let edges = document["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["consumer"], "acme/app");
    assert_eq!(edges[0]["provider"], "acme/core");
    let evidence = edges[0]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["ecosystem"].as_str().unwrap().to_owned(),
                e["provider_package"].as_str().unwrap().to_owned(),
                e["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let expected = [
        ("cargo", "acme-core", "skew"),
        ("npm", "@acme/core", "match"),
        ("go", "github.com/acme/core", "skew"),
        ("python", "acme-core-py", "match"),
    ]
    .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()));
    assert_eq!(evidence, expected);

    // Pin skew names the pinned commit and the organization's commit.
    let skew = document["pin_skew"].as_array().unwrap();
    assert_eq!(skew.len(), 2);
    assert_eq!(skew[0]["pin"]["kind"], "commit");
    assert_eq!(skew[0]["pin"]["value"], fixture.core_first.as_str());
    assert_eq!(skew[0]["member_commit"], fixture.core_commit.as_str());
    assert_eq!(skew[1]["provider_package"], "github.com/acme/core");
    assert_eq!(skew[1]["pin"]["value"], "v1.0.0");

    // Dependencies no member provides are listed, not dropped.
    let external = document["external"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        external,
        // Sorted by consumer, consumer package, ecosystem.
        ["left-pad", "serde", "requests", "golang.org/x/mod"].map(str::to_owned)
    );
    let unresolved = document["unresolved"].as_array().unwrap();
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0]["name"], "./vendored");

    // No modularity of any kind appears in the organization document.
    let mut all = Vec::new();
    keys(&document, &mut all);
    assert!(!all.iter().any(|k| k == "q" || k.contains("modularity")));

    // Byte-identical for the same manifest and member commits, and the
    // member clones are left as they were.
    let second = build(&fixture);
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert_eq!(first.stdout, second.stdout);
    for repo in [&fixture.core, &fixture.app] {
        assert_eq!(
            git(repo, &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1
        );
        assert_eq!(git(repo, &["status", "--porcelain"]), "");
    }
}

#[test]
fn org_build_refuses_a_commit_missing_from_the_clone() {
    let fixture = fixture();
    write(
        &fixture.root,
        "missing.org.toml",
        "version = 1\nname = \"acme\"\n\n[[members]]\nrepo = \"acme/core\"\ncommit = \"0123456789abcdef0123456789abcdef01234567\"\npath = \"core\"\n",
    );
    let run = tolmap(&fixture.root, &["org", "build", "missing.org.toml"]);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("is not in the clone"), "{}", run.stderr);
}
