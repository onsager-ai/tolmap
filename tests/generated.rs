use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use tolmap::generated;
use tolmap::schema::{CochangeSignal, GeneratedFile, GraphData, RustModTree};

fn graph() -> GraphData {
    let files = [
        "bindings/A.ts",
        "bindings/B.ts",
        "bindings/Unknown.ts",
        "generated/api.ts",
        "src/model.rs",
        "web/app.ts",
        "web/other.ts",
    ];
    let nodes = files
        .iter()
        .map(|file| {
            json!({
                "file": file, "loc": 20, "complexity": 2, "churn": 3,
                "fanin": 0.0, "module": file, "lang": "ts"
            })
        })
        .collect::<Vec<_>>();
    let edge = |a: usize, b: usize, weight: f64| {
        json!({
            "a": files[a], "b": files[b], "weight": weight,
            "static_signal": weight, "cochange": weight,
            "proximity": weight, "semantic": weight
        })
    };
    let mut data: GraphData = serde_json::from_value(json!({
        "repo": "generated", "pkg": ".", "lang": "ts", "nodes": nodes,
        "imports": [[files[5], files[0], 1.0], [files[5], files[1], 0.5],
            [files[5], files[4], 1.0], [files[0], files[1], 1.0],
            [files[5], files[2], 1.0], [files[3], files[6], 1.0]],
        "edges": [edge(0, 5, 1.0), edge(1, 5, 0.5), edge(4, 5, 1.0),
            edge(0, 1, 1.0), edge(2, 5, 1.0), edge(3, 6, 1.0)],
        "symbols": {files[0]: [["A", 1, 1, 1]], files[4]: [["A", 1, 1, 1]]},
        "uses": [[files[5], files[0], "A"], [files[5], files[4], "A"],
            [files[2], files[4], "A"]], "commits_scanned": 12
    }))
    .unwrap();
    data.generated = Some(vec![
        GeneratedFile {
            file: 0,
            generator: Some(4),
            ts_rs: true,
        },
        GeneratedFile {
            file: 1,
            generator: Some(4),
            ts_rs: true,
        },
        GeneratedFile {
            file: 2,
            generator: None,
            ts_rs: true,
        },
        GeneratedFile {
            file: 3,
            generator: None,
            ts_rs: false,
        },
    ]);
    data.cochange = Some(BTreeMap::from([
        (
            (0, 5),
            CochangeSignal {
                count: 4,
                weight: 0.5,
            },
        ),
        (
            (1, 5),
            CochangeSignal {
                count: 5,
                weight: 0.6,
            },
        ),
        (
            (0, 1),
            CochangeSignal {
                count: 7,
                weight: 0.9,
            },
        ),
        (
            (2, 5),
            CochangeSignal {
                count: 9,
                weight: 1.0,
            },
        ),
    ]));
    data.rust_mod = Some(RustModTree {
        roots: BTreeSet::from([4]),
        pairs: BTreeSet::from([(4, 6)]),
        descendant_fanin: BTreeMap::new(),
    });
    data.build_entries = Some(BTreeSet::from([2, 5]));
    data.entry_points.insert(files[5].into(), "entry".into());
    data
}

#[test]
fn collapse_redirects_and_sums_signal_mass_without_inventing_cochange_counts() {
    let mut data = graph();
    let projection = generated::collapse(&mut data);
    assert_eq!(
        (projection.coverage.collapsed, projection.coverage.excluded),
        (2, 1)
    );
    assert_eq!(projection.retained, [3, 4, 5, 6]);
    assert_eq!(
        data.nodes
            .iter()
            .map(|n| n.file.as_str())
            .collect::<Vec<_>>(),
        [
            "generated/api.ts",
            "src/model.rs",
            "web/app.ts",
            "web/other.ts"
        ]
    );
    assert_eq!(data.imports, [(0, 3, 1.0), (2, 1, 2.5)]);
    assert_eq!(data.uses, [(2, 1, "A".into())]);
    assert_eq!(data.edges.len(), 2);
    let edge = data
        .edges
        .iter()
        .find(|edge| edge.a == "src/model.rs")
        .unwrap();
    assert_eq!((&*edge.a, &*edge.b), ("src/model.rs", "web/app.ts"));
    assert_eq!(
        (
            edge.static_signal,
            edge.cochange,
            edge.proximity,
            edge.semantic
        ),
        (2.5, 2.5, 2.5, 2.5)
    );
    let cochange = data.cochange.as_ref().unwrap();
    assert_eq!(cochange.len(), 1);
    assert_eq!(
        (cochange[&(1, 2)].count, cochange[&(1, 2)].weight),
        (5, 0.6)
    );
    assert_eq!(data.nodes[1].fanin, 2.5);
    assert_eq!(
        (
            data.nodes[1].loc,
            data.nodes[1].complexity,
            data.nodes[1].churn
        ),
        (20, 2, 3)
    );
    assert_eq!(
        data.symbols.keys().map(String::as_str).collect::<Vec<_>>(),
        ["src/model.rs"]
    );
    let tree = data.rust_mod.as_ref().unwrap();
    assert_eq!(tree.roots, BTreeSet::from([1]));
    assert_eq!(tree.pairs, BTreeSet::from([(1, 3)]));
    assert_eq!(data.build_entries, Some(BTreeSet::from([2])));
    let retained_generated = data.generated.as_ref().unwrap();
    assert_eq!(retained_generated.len(), 1);
    assert_eq!(retained_generated[0].file, 0);
    assert_eq!(retained_generated[0].generator, None);
    assert!(!retained_generated[0].ts_rs);

    let once = serde_json::to_vec(&data).unwrap();
    assert_eq!(generated::collapse(&mut data).coverage, Default::default());
    assert_eq!(serde_json::to_vec(&data).unwrap(), once);
    let mut repeated = graph();
    generated::collapse(&mut repeated);
    assert_eq!(serde_json::to_vec(&repeated).unwrap(), once);
}

#[test]
fn old_graph_without_generated_evidence_is_byte_identical() {
    let mut data = graph();
    data.generated = None;
    let before = serde_json::to_vec(&data).unwrap();
    assert_eq!(generated::collapse(&mut data).coverage, Default::default());
    assert_eq!(serde_json::to_vec(&data).unwrap(), before);
}

#[test]
fn generic_generated_files_leave_the_entire_graph_and_debt_evidence_unchanged() {
    let mut data = graph();
    data.generated.as_mut().unwrap().retain(|row| !row.ts_rs);
    data.nodes[6].fanin = 1.234567;
    let before = serde_json::to_vec(&data).unwrap();
    let debt_before = serde_json::to_vec(&data.generated).unwrap();
    assert_eq!(generated::collapse(&mut data).coverage, Default::default());
    assert_eq!(serde_json::to_vec(&data).unwrap(), before);
    assert_eq!(serde_json::to_vec(&data.generated).unwrap(), debt_before);
}

#[test]
fn collapse_preserves_untouched_fanin_and_rounds_changed_non_python_fanin() {
    for language in ["ts", "py"] {
        let mut data = graph();
        data.nodes[4].lang = language.into();
        data.nodes[6].fanin = 1.23;
        // Rounding each import is lossy; an untouched node must retain the
        // extractor's fan-in rather than summing its rounded import rows.
        data.imports.push((5, 6, 0.23));
        data.imports[0].2 = 1.234;
        data.imports[1].2 = 0.117;
        let expected = if language == "py" {
            1.234 + 0.117 + 1.0
        } else {
            2.35
        };
        generated::collapse(&mut data);
        assert_eq!(data.nodes[3].fanin, 1.23);
        assert_eq!(data.nodes[1].fanin, expected);
    }
}

#[test]
fn generated_coverage_counts_are_optional_and_round_trip() {
    use tolmap::schema::CoverageReport;
    let legacy = json!({"zero_edge_files": 0, "total_files": 4, "by_language": {}});
    let mut coverage: CoverageReport = serde_json::from_value(legacy).unwrap();
    let absent = serde_json::to_value(&coverage).unwrap();
    assert!(absent.get("generated_collapsed").is_none());
    assert!(absent.get("generated_excluded").is_none());
    coverage.generated_collapsed = 2;
    coverage.generated_excluded = 1;
    let value = serde_json::to_value(coverage).unwrap();
    assert_eq!(value["generated_collapsed"], 2);
    assert_eq!(value["generated_excluded"], 1);
    let restored: CoverageReport = serde_json::from_value(value).unwrap();
    assert_eq!(restored.generated_collapsed, 2);
    assert_eq!(restored.generated_excluded, 1);
}

#[test]
fn warm_start_membership_tracks_retained_files_before_partition() {
    struct Capture;
    impl tolmap::partition::Partitioner for Capture {
        fn partition(
            &self,
            graph: &tolmap::schema::WeightedGraph,
            _: f64,
            seed: u64,
            initial: Option<&[usize]>,
        ) -> anyhow::Result<tolmap::partition::PartitionResult> {
            assert_eq!(seed, 7);
            assert_eq!(graph.node_count, 4);
            assert_eq!(initial, Some([4, 8, 9, 10].as_slice()));
            Ok(tolmap::partition::PartitionResult {
                membership: vec![0; 4],
                modularity: 0.0,
            })
        }
    }
    let output =
        tolmap::pipeline::run(graph(), 1.1, &Capture, Some(&[1, 2, 3, 4, 8, 9, 10])).unwrap();
    assert_eq!(
        (
            output.generated_coverage.collapsed,
            output.generated_coverage.excluded
        ),
        (2, 1)
    );
    assert_eq!(output.weighted.nodes.len(), 4);
}

#[test]
fn extraction_collapses_only_ts_rs_and_retains_other_generated_headers() {
    let repo = tempfile::tempdir().unwrap();
    let write = |path: &str, source: &str| {
        let path = repo.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    };
    write(
        "Cargo.toml",
        "[package]\nname = 'sample'\nversion = '0.1.0'\nedition = '2021'\n",
    );
    write("src/lib.rs", "mod other;\n#[derive(TS)]\npub struct Thing { pub value: u32 }\n#[derive(TS)]\npub struct Duplicate {}\n");
    write("src/other.rs", "#[derive(TS)]\npub struct Duplicate {}\n");
    write(
        "bindings/Thing.ts",
        "// This file was generated by [ts-rs]\nexport type Thing = { value: number };\n",
    );
    write(
        "bindings/Duplicate.ts",
        "// This file was generated by [ts-rs]\nexport type Duplicate = {};\n",
    );
    write(
        "elsewhere/Missing.ts",
        "// This file was generated by [ts-rs]\nexport type Missing = {};\n",
    );
    write("web/api.ts", "// @generated\nexport const value = 1;\n");
    write(
        "web/proto.ts",
        "// Code generated by protoc. DO NOT EDIT.\nexport const message = 1;\n",
    );
    write("web/app.ts", "import type { Thing } from '../bindings/Thing';\nexport function read(x: Thing) { return x.value; }\n");
    // Extraction reads history as well as source, so this fixture must be a
    // committed repository, just like the repositories mapped by the CLI.
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "generated extraction fixture"]);
    use tolmap::extract::LanguageKind;
    let mut data = tolmap::extract::build_multi_source(
        repo.path(),
        &[
            (".".into(), LanguageKind::Rust),
            (".".into(), LanguageKind::TypeScript),
        ],
    )
    .unwrap();
    let coverage = generated::collapse(&mut data).coverage;
    assert_eq!((coverage.collapsed, coverage.excluded), (1, 2));
    assert!(data
        .nodes
        .iter()
        .all(|node| !node.file.starts_with("bindings/") && node.file != "elsewhere/Missing.ts"));
    for path in ["web/api.ts", "web/proto.ts"] {
        assert!(data.nodes.iter().any(|node| node.file == path));
        assert!(data
            .generated
            .as_ref()
            .unwrap()
            .iter()
            .any(|row| data.nodes[row.file as usize].file == path && !row.ts_rs));
    }
    assert!(data
        .imports
        .iter()
        .any(|&(a, b, _)| data.nodes[a as usize].file == "web/app.ts"
            && data.nodes[b as usize].file == "src/lib.rs"));
}
