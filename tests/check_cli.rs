//! `tolmap check` end to end (issue #170, docs/CHECK.md): a small Python
//! repository with three packages that do not import each other -- three
//! districts by construction -- and scripted changes against it, run through
//! the real binary so the exit code, which is the contract, is what is
//! tested.
//!
//! Each package is committed on its own and then edited on its own, so the
//! co-change signal agrees with the imports and directories: nothing here
//! should leave Leiden a reason to merge two packages into one district.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const TOLMAP: &str = env!("CARGO_BIN_EXE_tolmap");

/// Settings a CI or developer environment might carry that would make a
/// `tolmap build` here differ from the check's own base build.
const DEFAULTS_ONLY: &[&str] = &[
    "TOLMAP_REFS",
    "TOLMAP_SCIP_INSTALL",
    "TOLMAP_NAMER",
    "TOLMAP_NAMER_MODEL",
    "TOLMAP_PRUNE_VARIANT",
];

/// Three packages of five modules. Module `i` imports modules `i + 1` and
/// `i + 2` of its own package (mod 5), and each package has its own
/// vocabulary, so every signal the blend reads keeps the packages apart.
const PACKAGES: [(&str, [&str; 5]); 3] = [
    ("billing", ["invoice", "ledger", "payment", "tariff", "tax"]),
    (
        "catalog",
        ["product", "price", "stock", "supplier", "category"],
    ),
    (
        "people",
        ["account", "profile", "session", "address", "contact"],
    ),
];

fn class_name(module: &str) -> String {
    let mut chars = module.chars();
    let first = chars.next().unwrap().to_ascii_uppercase();
    format!("{first}{}", chars.as_str())
}

fn module_source(package: &str, modules: &[&str; 5], index: usize, constant: u32) -> String {
    let module = modules[index];
    let first = modules[(index + 1) % 5];
    let second = modules[(index + 2) % 5];
    let (class, first_class, second_class) =
        (class_name(module), class_name(first), class_name(second));
    format!(
        "from .{first} import {first_class}\n\
         from .{second} import {second_class}\n\
         \n\
         {upper}_{module}_LIMIT = {constant}\n\
         \n\
         \n\
         class {class}:\n\
         \x20   def __init__(self):\n\
         \x20       self.{module}_{first} = {first_class}()\n\
         \x20       self.{module}_{second} = {second_class}()\n\
         \n\
         \x20   def {package}_{module}_total(self):\n\
         \x20       return {upper}_{module}_LIMIT\n",
        upper = package.to_ascii_uppercase(),
    )
}

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

/// The fixture repository, committed. Returns the temporary directory (kept
/// alive by the caller), the repository path and the base commit.
fn fixture() -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("town");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    write(
        &repo,
        "pyproject.toml",
        "[project]\nname = \"town\"\nversion = \"0.1.0\"\n",
    );
    write(&repo, "town/__init__.py", "");
    for (package, modules) in PACKAGES {
        write(&repo, &format!("town/{package}/__init__.py"), "");
        for index in 0..5 {
            write(
                &repo,
                &format!("town/{package}/{}.py", modules[index]),
                &module_source(package, &modules, index, 1),
            );
        }
        // A sixth module that imports two others and that nothing imports:
        // the one the deletion test removes.
        write(
            &repo,
            &format!("town/{package}/audit.py"),
            &format!(
                "from .{} import {}\nfrom .{} import {}\n\n\nclass {}Audit:\n    pass\n",
                modules[0],
                class_name(modules[0]),
                modules[1],
                class_name(modules[1]),
                class_name(package),
            ),
        );
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", &format!("add {package}")]);
    }
    // One more commit per package, each touching two of its own modules,
    // so co-change repeats the package boundary.
    for (package, modules) in PACKAGES {
        for index in [0, 1] {
            write(
                &repo,
                &format!("town/{package}/{}.py", modules[index]),
                &module_source(package, &modules, index, 2),
            );
        }
        git(&repo, &["commit", "-qam", &format!("tune {package}")]);
    }
    let base = git(&repo, &["rev-parse", "HEAD"]);
    (dir, repo, base)
}

/// Rewrites one module with a different constant: an edit that changes no
/// import and no identifier.
fn touch(repo: &Path, package: &str, index: usize) {
    let modules = PACKAGES
        .iter()
        .find(|(name, _)| *name == package)
        .unwrap()
        .1;
    write(
        repo,
        &format!("town/{package}/{}.py", modules[index]),
        &module_source(package, &modules, index, 99),
    );
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
            panic!(
                "not JSON ({error}):\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        })
    }
}

fn check(repo: &Path, args: &[&str]) -> Run {
    let mut command = Command::new(TOLMAP);
    command.arg("check").arg(repo).args(args);
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

fn expect_code(run: &Run, code: i32) {
    assert_eq!(
        run.code, code,
        "exit code\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
}

fn file_entry<'a>(report: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("{path} not among the changed files: {report:#}"))
}

#[test]
fn an_in_district_edit_crosses_one_district_and_leaves_q_alone() {
    let (_dir, repo, base) = fixture();
    touch(&repo, "billing", 4);

    let run = check(&repo, &["--base", &base, "--format", "json"]);
    expect_code(&run, 0);
    let report = run.json();
    assert_eq!(report["version"], 1);
    assert_eq!(report["base"], base.as_str());
    assert!(report["head"].is_null(), "head is the working tree");
    // Three packages, three districts by construction; every module is on
    // the map.
    assert!(
        report["base_districts"].as_u64().unwrap() >= 3,
        "{report:#}"
    );
    assert!(report["base_files"].as_u64().unwrap() >= 18, "{report:#}");
    assert_eq!(report["districts_crossed"], 1, "{report:#}");
    assert_eq!(report["delta_q"].as_f64(), Some(0.0), "{report:#}");
    assert_eq!(report["modularity_base"], report["modularity_head"]);
    assert!(report["modularity_base"].as_f64().unwrap() > 0.0);
    assert_eq!(report["verdict"], "pass");
    assert_eq!(
        report["thresholds"],
        serde_json::json!({"max_districts": 4, "max_dq": 0.01, "source": "default"}),
        "no threshold flag: the calibrated defaults apply"
    );
    assert_eq!(report["lower_bound"], true);
    assert!(report["edges_added"].as_array().unwrap().is_empty());
    assert!(report["edges_removed"].as_array().unwrap().is_empty());
    let file = file_entry(&report, "town/billing/tax.py");
    assert_eq!(file["status"], "modified");
    assert_eq!(file["placed"], false);
    assert!(file["district"].is_u64());

    // The text format: three lines, the edge list, the lower-bound note.
    let run = check(&repo, &["--base", &base]);
    expect_code(&run, 0);
    let lines = run.stdout.lines().collect::<Vec<_>>();
    assert!(
        lines[0].starts_with("districts crossed: 1"),
        "{}",
        run.stdout
    );
    assert!(lines[1].starts_with("delta q: +0.000000"), "{}", run.stdout);
    assert_eq!(
        lines[2],
        "verdict: pass (districts 1 <= 4, delta q +0.000000 >= -0.010000; default thresholds)",
        "{}",
        run.stdout
    );
    assert!(
        lines
            .last()
            .unwrap()
            .starts_with("numbers are a lower bound"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_new_cross_district_import_lowers_q_and_fails_only_past_max_dq() {
    let (_dir, repo, base) = fixture();
    let modules = PACKAGES[0].1;
    let mut source = module_source("billing", &modules, 0, 1);
    source.insert_str(0, "from ..people.account import Account\n");
    source.push_str("\n    def holder(self):\n        return Account()\n");
    write(&repo, "town/billing/invoice.py", &source);

    let run = check(
        &repo,
        &["--base", &base, "--format", "json", "--report-only"],
    );
    expect_code(&run, 0);
    let report = run.json();
    let delta_q = report["delta_q"].as_f64().unwrap();
    assert!(
        delta_q < 0.0,
        "a cross-district import must lower q: {report:#}"
    );
    assert_eq!(report["verdict"], "pass", "--report-only: report only");
    assert_eq!(
        report["thresholds"],
        serde_json::json!({"max_districts": null, "max_dq": null, "source": "report_only"})
    );
    let edges = report["edges_added"].as_array().unwrap();
    let edge = edges
        .iter()
        .find(|edge| {
            edge["source"] == "town/billing/invoice.py"
                && edge["target"] == "town/people/account.py"
        })
        .unwrap_or_else(|| panic!("the new import is not listed: {report:#}"));
    assert_eq!(edge["static_import"], true);
    assert_ne!(edge["source_district"], edge["target_district"]);
    assert_eq!(
        edge["source_district"],
        file_entry(&report, "town/billing/invoice.py")["district"]
    );

    // Past the threshold: exit 1. A threshold the drop stays inside: 0.
    let run = check(
        &repo,
        &["--base", &base, "--format", "json", "--max-dq", "0"],
    );
    expect_code(&run, 1);
    assert_eq!(run.json()["verdict"], "fail");
    assert_eq!(
        run.json()["thresholds"],
        serde_json::json!({"max_districts": null, "max_dq": 0.0, "source": "flags"}),
        "a flag replaces the defaults as a set"
    );
    let run = check(&repo, &["--base", &base, "--max-dq", "1"]);
    expect_code(&run, 0);
    assert!(run
        .stdout
        .lines()
        .nth(2)
        .unwrap()
        .starts_with("verdict: pass ("));
    let run = check(&repo, &["--base", &base, "--max-dq", "0"]);
    expect_code(&run, 1);
    assert!(run
        .stdout
        .contains("town/billing/invoice.py -> town/people/account.py"));

    // Determinism: three runs, one sha256.
    let hashes = (0..3)
        .map(|_| {
            let run = check(
                &repo,
                &["--base", &base, "--format", "json", "--report-only"],
            );
            expect_code(&run, 0);
            format!("{:x}", Sha256::digest(run.stdout.as_bytes()))
        })
        .collect::<Vec<_>>();
    assert!(
        hashes.iter().all(|hash| *hash == hashes[0]),
        "three runs, different reports: {hashes:?}"
    );
}

/// Issue #170 PR 2: the calibrated defaults fail a change by themselves,
/// `--report-only` turns them off, and any threshold flag replaces them.
#[test]
fn the_default_thresholds_fail_a_heavy_coupling_change_without_any_flag() {
    let (_dir, repo, base) = fixture();
    // billing's invoice imports every module of the two other packages: on
    // a graph this small, far more cross-district weight than 0.01 of q.
    let modules = PACKAGES[0].1;
    let mut source = module_source("billing", &modules, 0, 1);
    let mut uses = String::new();
    for (package, others) in &PACKAGES[1..] {
        for other in others {
            let class = class_name(other);
            source.insert_str(0, &format!("from ..{package}.{other} import {class}\n"));
            uses.push_str(&format!("        {class}()\n"));
        }
    }
    source.push_str(&format!("\n    def reach(self):\n{uses}"));
    write(&repo, "town/billing/invoice.py", &source);

    let run = check(&repo, &["--base", &base, "--format", "json"]);
    expect_code(&run, 1);
    let report = run.json();
    assert!(
        report["delta_q"].as_f64().unwrap() < -0.01,
        "the change must drop q past the default: {report:#}"
    );
    assert_eq!(report["verdict"], "fail");
    assert_eq!(report["thresholds"]["source"], "default");
    let run = check(&repo, &["--base", &base]);
    expect_code(&run, 1);
    assert!(
        run.stdout
            .lines()
            .nth(2)
            .unwrap()
            .starts_with("verdict: fail (districts 1 <= 4, delta q -"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("; default thresholds)"),
        "{}",
        run.stdout
    );

    let run = check(&repo, &["--base", &base, "--report-only"]);
    expect_code(&run, 0);
    assert_eq!(
        run.stdout.lines().nth(2).unwrap(),
        "verdict: pass (report only: --report-only)"
    );
    // A flag the change stays inside replaces both defaults: pass.
    let run = check(&repo, &["--base", &base, "--max-dq", "1"]);
    expect_code(&run, 0);
}

#[test]
fn max_districts_trips_when_a_change_spans_packages() {
    let (_dir, repo, base) = fixture();
    touch(&repo, "billing", 4);
    touch(&repo, "catalog", 2);

    let run = check(&repo, &["--base", &base, "--format", "json"]);
    expect_code(&run, 0);
    let report = run.json();
    assert_eq!(report["districts_crossed"], 2, "{report:#}");
    let districts = report["districts"].as_array().unwrap();
    assert_eq!(districts.len(), 2);
    assert!(districts
        .iter()
        .all(|district| district["changed_files"] == 1));

    let run = check(&repo, &["--base", &base, "--max-districts", "1"]);
    expect_code(&run, 1);
    assert!(
        run.stdout.contains("verdict: fail (districts 2 > 1)"),
        "{}",
        run.stdout
    );
    let run = check(&repo, &["--base", &base, "--max-districts", "2"]);
    expect_code(&run, 0);
}

#[test]
fn a_new_file_is_placed_by_its_imports_and_one_without_imports_is_unplaced() {
    let (_dir, repo, base) = fixture();
    touch(&repo, "billing", 0);
    write(
        &repo,
        "town/billing/refund.py",
        "from .invoice import Invoice\nfrom .ledger import Ledger\n\n\n\
         class Refund:\n    def __init__(self):\n        self.refund_invoice = Invoice()\n\
         \x20       self.refund_ledger = Ledger()\n",
    );
    write(&repo, "town/billing/notes.py", "NOTES_HEADER = \"notes\"\n");

    let run = check(
        &repo,
        &["--base", &base, "--format", "json", "--report-only"],
    );
    expect_code(&run, 0);
    let report = run.json();
    let refund = file_entry(&report, "town/billing/refund.py");
    assert_eq!(refund["status"], "added");
    assert_eq!(refund["placed"], true, "{report:#}");
    assert_eq!(
        refund["district"],
        file_entry(&report, "town/billing/invoice.py")["district"],
        "placed where its imports point: {report:#}"
    );
    let notes = file_entry(&report, "town/billing/notes.py");
    assert!(notes["district"].is_null());
    assert_eq!(notes["placed"], false);
    assert_eq!(
        report["unplaced_files"],
        serde_json::json!(["town/billing/notes.py"])
    );
    // The unplaced file counts toward no district; refund.py joins
    // invoice.py's.
    assert_eq!(report["districts_crossed"], 1, "{report:#}");
    assert!(run.stdout.contains("\"town/billing/notes.py\""));

    let run = check(&repo, &["--base", &base, "--report-only"]);
    expect_code(&run, 0);
    assert!(run
        .stdout
        .lines()
        .next()
        .unwrap()
        .ends_with("; 1 new file(s) unplaced"));
}

#[test]
fn a_rename_is_followed_and_a_deletion_is_handled() {
    let (_dir, repo, base) = fixture();
    // catalog/supplier.py becomes vendor.py; its two importers follow.
    git(
        &repo,
        &["mv", "town/catalog/supplier.py", "town/catalog/vendor.py"],
    );
    for importer in ["price", "stock"] {
        let path = repo.join(format!("town/catalog/{importer}.py"));
        let source = std::fs::read_to_string(&path)
            .unwrap()
            .replace("from .supplier import", "from .vendor import");
        std::fs::write(&path, source).unwrap();
    }
    git(&repo, &["rm", "-q", "town/people/audit.py"]);
    git(
        &repo,
        &["commit", "-qam", "rename supplier, drop the people audit"],
    );
    let head = git(&repo, &["rev-parse", "HEAD"]);

    let run = check(
        &repo,
        &[
            "--base",
            &base,
            "--head",
            "HEAD",
            "--format",
            "json",
            "--report-only",
        ],
    );
    expect_code(&run, 0);
    let report = run.json();
    assert_eq!(report["head"], head.as_str());
    let vendor = file_entry(&report, "town/catalog/vendor.py");
    assert_eq!(vendor["status"], "renamed", "{report:#}");
    assert_eq!(vendor["base_path"], "town/catalog/supplier.py");
    assert_eq!(vendor["placed"], false, "a rename keeps its base district");
    assert_eq!(
        vendor["district"],
        file_entry(&report, "town/catalog/price.py")["district"]
    );
    let audit = file_entry(&report, "town/people/audit.py");
    assert_eq!(audit["status"], "deleted");
    assert!(audit["district"].is_u64());
    assert!(report["unplaced_files"].as_array().unwrap().is_empty());
    assert_eq!(report["districts_crossed"], 2, "{report:#}");
}

#[test]
fn a_bad_ref_and_an_unverifiable_or_mismatched_base_map_exit_2() {
    let (dir, repo, base) = fixture();
    let run = check(&repo, &["--base", "no-such-ref"]);
    expect_code(&run, 2);
    assert!(run.stdout.is_empty());
    let run = check(&repo, &["--base", &base, "--head", "no-such-ref"]);
    expect_code(&run, 2);
    let run = check(&repo, &["--base", &base, "--max-dq", "NaN"]);
    expect_code(&run, 2);
    let run = check(&repo, &["--base", &base, "--report-only", "--max-dq", "0"]);
    expect_code(&run, 2);
    assert!(run.stdout.is_empty());
    let run = check(&dir.path().join("not-a-repo"), &["--base", &base]);
    expect_code(&run, 2);

    // A stored map of the base commit, named as the service store names it.
    let out = dir.path().join("maps");
    let mut build = Command::new(TOLMAP);
    build
        .arg("build")
        .arg(&repo)
        .arg("--name")
        .arg(&base)
        .arg("--out")
        .arg(&out);
    for key in DEFAULTS_ONLY {
        build.env_remove(key);
    }
    let built = build.output().unwrap();
    assert!(
        built.status.success(),
        "tolmap build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let stored = out.join(format!("{base}.json"));
    assert!(stored.is_file());

    // A later commit: the stored map is not its map.
    touch(&repo, "people", 1);
    git(&repo, &["commit", "-qam", "later"]);
    let later = git(&repo, &["rev-parse", "HEAD"]);
    let stored_arg = stored.to_string_lossy().into_owned();
    let run = check(&repo, &["--base", &later, "--base-map", &stored_arg]);
    expect_code(&run, 2);
    assert!(run.stderr.contains("was built from"), "{}", run.stderr);

    // A map whose name records no commit cannot be verified.
    let unnamed = dir.path().join("map.json");
    std::fs::copy(&stored, &unnamed).unwrap();
    let run = check(
        &repo,
        &["--base", &base, "--base-map", &unnamed.to_string_lossy()],
    );
    expect_code(&run, 2);
    let missing = dir.path().join(format!("{later}.json"));
    let run = check(
        &repo,
        &["--base", &later, "--base-map", &missing.to_string_lossy()],
    );
    expect_code(&run, 2);

    // The matching stored map gives the same report the check's own base
    // build gives: the stored map was built from the same graph, cold.
    touch(&repo, "billing", 3);
    let built_here = check(
        &repo,
        &["--base", &base, "--format", "json", "--report-only"],
    );
    expect_code(&built_here, 0);
    let from_store = check(
        &repo,
        &[
            "--base",
            &base,
            "--base-map",
            &stored_arg,
            "--format",
            "json",
            "--report-only",
        ],
    );
    expect_code(&from_store, 0);
    assert_eq!(from_store.stdout, built_here.stdout);
    assert_eq!(built_here.json()["districts_crossed"], 2);
}

#[test]
fn the_check_leaves_no_worktree_behind() {
    let (_dir, repo, base) = fixture();
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let run = check(&repo, &["--base", &base, "--head", "HEAD"]);
    expect_code(&run, 0);
    let worktrees = git(&repo, &["worktree", "list", "--porcelain"]);
    assert_eq!(
        worktrees
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1,
        "{worktrees}"
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
}
