use std::path::Path;

use tolmap::query::Snapshot;
use tolmap::query_contract::{Direction, Query, QueryResult, QUERY_SCHEMA};

fn fixture(name: &str) -> Snapshot {
    Snapshot::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("data/{name}.json")))
        .unwrap()
}

#[test]
fn pinned_file_symbol_and_ambiguity() {
    let s = fixture("tolmap");
    let file = s.where_is("src/main.rs", 8).unwrap();
    assert_eq!(file.schema, QUERY_SCHEMA);
    assert_eq!(file.total, 1);
    assert_eq!(file.map.sha256.len(), 64);
    assert!(file.map.commit.is_none());
    assert!(file.coverage.unresolved.is_none());
    assert!(serde_json::to_vec(&file).unwrap().len() < 2000);
    let symbol = s.where_is("main", 1).unwrap();
    assert!(matches!(
        symbol.query,
        Query::Where {
            ambiguous: true,
            ..
        }
    ));
    assert!(symbol.total > 1);
    assert!(symbol.truncated);
    let exact = s.where_is("src/bin/generate_types.rs::main", 8).unwrap();
    assert_eq!(exact.total, 1);
    assert!(
        matches!(&exact.results[0], QueryResult::File { symbol: Some(symbol), .. } if symbol.name == "main")
    );
    assert_eq!(s.where_is("no-such-symbol", 8).unwrap().total, 0);
}

#[test]
fn pinned_deps_match_all_edges_and_keep_total() {
    for name in ["tolmap", "flask"] {
        let s = fixture(name);
        let path = &s.map.files[1];
        for direction in [Direction::In, Direction::Out, Direction::Both] {
            let report = s.deps(path, direction, 1);
            let expected: std::collections::BTreeSet<_> = s
                .map
                .edges
                .iter()
                .flat_map(|&[a, b]| {
                    let mut rows = Vec::new();
                    if a == 1 && direction != Direction::In {
                        rows.push((b, 1));
                    }
                    if b == 1 && direction != Direction::Out {
                        rows.push((a, 0));
                    }
                    rows
                })
                .collect();
            assert_eq!(report.total, expected.len());
            assert_eq!(report.truncated, expected.len() > 1);
            assert_eq!(report.results.len(), expected.len().min(1));
            assert_eq!(
                serde_json::to_vec(&report).unwrap(),
                serde_json::to_vec(&s.deps(path, direction, 1)).unwrap()
            );
        }
    }
    let s = fixture("tolmap");
    let report = s.deps("src/main.rs", Direction::Both, 8);
    assert!(serde_json::to_vec(&report).unwrap().len() < 2000);
    assert_eq!(s.deps("missing", Direction::Both, 8).total, 0);
}

#[test]
fn directory_layout_and_invalid_indices() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = include_bytes!("../data/flask.json");
    std::fs::write(temp.path().join("flask.json"), bytes).unwrap();
    std::fs::write(temp.path().join("flask.names.json"), b"{}").unwrap();
    assert_eq!(Snapshot::load(temp.path()).unwrap().map.repo, "flask");
    std::fs::write(temp.path().join("other.json"), bytes).unwrap();
    assert!(Snapshot::load(temp.path()).is_err());
    let mut map: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    map["E"] = serde_json::json!([[999999, 0]]);
    std::fs::write(
        temp.path().join("bad.json"),
        serde_json::to_vec(&map).unwrap(),
    )
    .unwrap();
    assert!(Snapshot::load(&temp.path().join("bad.json")).is_err());
}

#[test]
fn hierarchical_symbols_and_unknown_coverage() {
    let temp = tempfile::tempdir().unwrap();
    let map = temp.path().join("flask.json");
    std::fs::write(&map, include_bytes!("../data/flask.json")).unwrap();
    let symbols = serde_json::json!({
        "files": [0], "symbols": [[0,"App",0,1,20,-1,5,false], [0,"run",1,2,5,0,3,false], [0,"run",1,22,25,-1,3,false]],
        "edges": [], "module_code_lines": {},
        "coverage": {"calls_total": 7, "calls_resolved": 3, "unresolved": {"dynamic": 4}}
    });
    std::fs::write(
        map.with_extension("symbols.json"),
        serde_json::to_vec(&symbols).unwrap(),
    )
    .unwrap();
    let s = Snapshot::load(&map).unwrap();
    let report = s.where_is("run", 0).unwrap();
    assert_eq!(report.total, 2);
    assert!(report.truncated);
    assert!(matches!(
        report.query,
        Query::Where {
            ambiguous: true,
            ..
        }
    ));
    let report = s.where_is("App::run", 8).unwrap();
    assert_eq!(report.total, 1);
    assert_eq!(report.coverage.calls_total, Some(7));
    assert_eq!(report.coverage.unresolved.unwrap()["dynamic"], 4);
    assert!(s
        .where_is(&s.map.files[0], 8)
        .unwrap()
        .coverage
        .calls_total
        .is_none());
}
