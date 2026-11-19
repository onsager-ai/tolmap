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
        },
        GeneratedFile {
            file: 1,
            generator: Some(4),
        },
        GeneratedFile {
            file: 2,
            generator: None,
        },
        GeneratedFile {
            file: 3,
            generator: None,
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
        (2, 2)
    );
    assert_eq!(projection.retained, [4, 5, 6]);
    assert_eq!(
        data.nodes
            .iter()
            .map(|n| n.file.as_str())
            .collect::<Vec<_>>(),
        ["src/model.rs", "web/app.ts", "web/other.ts"]
    );
    assert_eq!(data.imports, [(1, 0, 2.5)]);
    assert_eq!(data.uses, [(1, 0, "A".into())]);
    assert_eq!(data.edges.len(), 1);
    let edge = &data.edges[0];
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
        (cochange[&(0, 1)].count, cochange[&(0, 1)].weight),
        (5, 0.6)
    );
    assert_eq!(data.nodes[0].fanin, 2.5);
    assert_eq!(
        (
            data.nodes[0].loc,
            data.nodes[0].complexity,
            data.nodes[0].churn
        ),
        (20, 2, 3)
    );
    assert_eq!(
        data.symbols.keys().map(String::as_str).collect::<Vec<_>>(),
        ["src/model.rs"]
    );
    let tree = data.rust_mod.as_ref().unwrap();
    assert_eq!(tree.roots, BTreeSet::from([0]));
    assert_eq!(tree.pairs, BTreeSet::from([(0, 2)]));
    assert_eq!(data.build_entries, Some(BTreeSet::from([1])));
    assert_eq!(data.generated.as_ref().unwrap().len(), 0);

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
            assert_eq!(graph.node_count, 3);
            assert_eq!(initial, Some([8, 9, 10].as_slice()));
            Ok(tolmap::partition::PartitionResult {
                membership: vec![0; 3],
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
        (2, 2)
    );
    assert_eq!(output.weighted.nodes.len(), 3);
}

#[test]
fn extraction_matches_unique_ts_rs_generator_and_excludes_ambiguous_or_other_headers() {
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
        "bindings/Missing.ts",
        "// This file was generated by [ts-rs]\nexport type Missing = {};\n",
    );
    write("web/api.ts", "// @generated\nexport const value = 1;\n");
    write("web/app.ts", "import type { Thing } from '../bindings/Thing';\nexport function read(x: Thing) { return x.value; }\n");
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
    assert_eq!((coverage.collapsed, coverage.excluded), (1, 3));
    assert!(data
        .nodes
        .iter()
        .all(|node| !node.file.starts_with("bindings/") && node.file != "web/api.ts"));
    assert!(data
        .imports
        .iter()
        .any(|&(a, b, _)| data.nodes[a as usize].file == "web/app.ts"
            && data.nodes[b as usize].file == "src/lib.rs"));
}
