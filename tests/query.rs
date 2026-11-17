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
fn overview_discloses_measured_and_unknown_test_exclusions() {
    let mut s = fixture("tolmap");
    let old = s.overview(0);
    assert!(serde_json::to_value(&old).unwrap()["query"]["excluded_tests"].is_null());
    assert!(tolmap::query::text(&old).contains("excluded test files: unknown"));
    for count in [0, 17] {
        s.map.excluded_tests = Some(count);
        let report = s.overview(0);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["query"]["excluded_tests"], count);
        assert!(json["query"]["test_exclusion_rule"]
            .as_str()
            .unwrap()
            .contains(".test."));
        assert!(
            tolmap::query::text(&report).contains(&format!("excluded test files: {count}; rule:"))
        );
    }
}

#[test]
fn missing_paths_are_explicit_and_suggestions_have_stable_ties() {
    let mut s = fixture("tolmap");
    s.map.files = vec!["src/b.ts".into(), "src/a.ts".into(), "lib/z.ts".into()];
    // The fixture's symbols index the original file list.
    s.map.symbols.clear();
    let response = s.where_is("src/c.ts", 0).unwrap();
    assert_eq!(response.total, 0);
    assert!(!response.truncated);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["query"]["missing_path"], "src/c.ts");
    assert_eq!(
        json["query"]["suggestions"],
        serde_json::json!(["src/a.ts", "src/b.ts", "lib/z.ts"])
    );
    let text = tolmap::query::text(&response);
    assert!(text.contains("path is not in the map: src/c.ts"));
    assert!(text.contains("nearest mapped paths:"));
    assert!(text.find("src/a.ts").unwrap() < text.find("src/b.ts").unwrap());
    s.map.files.reverse();
    assert_eq!(
        serde_json::to_value(s.where_is("src/c.ts", 0).unwrap()).unwrap(),
        json
    );
    assert!(
        serde_json::to_value(fixture("tolmap").where_is("missing_symbol", 1).unwrap()).unwrap()
            ["query"]
            .get("missing_path")
            .is_none()
    );
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
            let totals = [
                expected.iter().filter(|(_, d)| *d == 0).count(),
                expected.iter().filter(|(_, d)| *d == 1).count(),
            ];
            let returned = totals.map(|n| n.min(1));
            let omitted = expected.len() - returned.iter().sum::<usize>();
            assert_eq!(report.total, expected.len());
            assert_eq!(report.truncated, omitted > 0);
            assert_eq!(report.omitted, omitted);
            assert_eq!(
                report.next_limit,
                (omitted > 0).then_some(totals[0].max(totals[1]))
            );
            let Query::Deps { directions, .. } = &report.query else {
                panic!("expected deps");
            };
            for (d, counts) in [&directions.inbound, &directions.out]
                .into_iter()
                .enumerate()
            {
                if (d == 0 && direction == Direction::Out) || (d == 1 && direction == Direction::In)
                {
                    assert!(counts.is_none());
                } else {
                    let counts = counts.as_ref().unwrap();
                    assert_eq!(counts.total, totals[d]);
                    assert_eq!(counts.returned, returned[d]);
                    assert_eq!(counts.omitted, totals[d] - returned[d]);
                }
            }
            if report.truncated {
                let text = tolmap::query::text(&report);
                assert!(text.contains(&format!("--limit {}", report.next_limit.unwrap())));
            }
            assert_eq!(report.results.len(), returned.iter().sum::<usize>());
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
fn pinned_both_directions_keep_smaller_boundary_and_account_for_zero_limit() {
    let s = fixture("tolmap");
    // Pinned src/parity.rs has three importers and one import. The inbound
    // boundary used to consume the shared limit and hide the outbound row.
    let path = "src/parity.rs";
    for limit in [0, 1, 3] {
        let report = s.deps(path, Direction::Both, limit);
        let Query::Deps { directions, .. } = &report.query else {
            panic!("expected deps");
        };
        let inbound = directions.inbound.as_ref().unwrap();
        let outbound = directions.out.as_ref().unwrap();
        assert_eq!((inbound.total, outbound.total), (3, 1));
        assert_eq!(inbound.returned, 3.min(limit));
        assert_eq!(outbound.returned, 1.min(limit));
        assert_eq!(inbound.omitted, 3usize.saturating_sub(limit));
        assert_eq!(outbound.omitted, 1usize.saturating_sub(limit));
        assert_eq!(report.total, 4);
        assert_eq!(report.omitted, inbound.omitted + outbound.omitted);
        assert_eq!(report.truncated, limit < 3);
        assert_eq!(report.next_limit, (limit < 3).then_some(3));
        for (direction, count) in [(Direction::In, inbound), (Direction::Out, outbound)] {
            assert_eq!(
                report
                    .results
                    .iter()
                    .filter(|r| matches!(r,
                        QueryResult::Dependency { direction: d, .. } if *d == direction
                    ))
                    .count(),
                count.returned
            );
        }
        assert_eq!(
            serde_json::to_vec(&report).unwrap(),
            serde_json::to_vec(&s.deps(path, Direction::Both, limit)).unwrap()
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema"], "tolmap.query/v3");
        assert_eq!(json["query"]["directions"]["in"]["total"], 3);
        assert_eq!(json["query"]["directions"]["out"]["total"], 1);
        if limit == 1 {
            let text = tolmap::query::text(&report);
            assert!(text.contains("in: 1 of 3 results, 2 rows omitted; repeat with --limit 3"));
            assert!(text.contains("out: 1 of 1 results, 0 rows omitted; repeat with --limit 3"));
            assert_eq!(
                text.lines()
                    .filter(|line| line.contains("rows omitted;"))
                    .count(),
                2
            );
        }
    }
    let missing = s.deps("missing", Direction::Both, 0);
    assert!(!missing.truncated);
    assert_eq!(missing.omitted, 0);
    assert_eq!(missing.next_limit, None);
    assert_eq!(missing.results.len(), 0);
}

#[test]
fn directory_layout_and_invalid_indices() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = include_bytes!("../data/flask.json");
    std::fs::write(temp.path().join("flask.json"), bytes).unwrap();
    std::fs::write(temp.path().join("flask.names.json"), b"{}").unwrap();
    // Known sidecars are never parsed during map-only directory discovery.
    std::fs::write(temp.path().join("flask.symbols.json"), b"not JSON").unwrap();
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
fn overview_and_routes_on_pinned_maps() {
    for name in ["tolmap", "flask"] {
        let s = fixture(name);
        let overview = s.overview(1000);
        assert_eq!(overview.total, s.map.districts.len());
        let files: usize = overview
            .results
            .iter()
            .map(|r| match r {
                QueryResult::District { files, .. } => *files,
                _ => panic!("expected district"),
            })
            .sum();
        assert_eq!(files, s.map.files.len());
        let [a, b] = s.map.edges[0];
        let route = s.route(&s.map.files[a], &s.map.files[b], 8);
        assert_eq!(route.total, 2);
        assert!(matches!(
            route.query,
            Query::Route {
                edge_kind: Some(tolmap::lens::RouteKind::Imports),
                ..
            }
        ));
        assert!(
            matches!(&route.results[0], QueryResult::Step { path, .. } if path == &s.map.files[a])
        );
        let short = s.route(&s.map.files[a], &s.map.files[b], 1);
        assert_eq!(short.total, 2);
        assert!(short.truncated);
        assert_eq!(short.omitted, 1);
        assert_eq!(short.next_limit, Some(2));
        let same = s.route(&s.map.files[a], &s.map.files[a], 8);
        assert_eq!(same.total, 1);
        assert!(matches!(
            same.query,
            Query::Route {
                edge_kind: Some(tolmap::lens::RouteKind::SameFile),
                ..
            }
        ));
        assert_eq!(s.route("missing", &s.map.files[a], 8).total, 0);
        assert!(serde_json::to_vec(&s.overview(8)).unwrap().len() < 2000);
    }
}

#[test]
fn route_direction_and_no_route_use_lens_semantics() {
    let mut s = fixture("tolmap");
    s.map.edges = vec![[0, 1], [2, 1]];
    let route = s.route(&s.map.files[1], &s.map.files[0], 8);
    assert!(matches!(
        route.query,
        Query::Route {
            edge_kind: Some(tolmap::lens::RouteKind::ImportedBy),
            ..
        }
    ));
    let route = s.route(&s.map.files[0], &s.map.files[2], 8);
    assert!(matches!(
        route.query,
        Query::Route {
            edge_kind: Some(tolmap::lens::RouteKind::Undirected),
            ..
        }
    ));
    assert_eq!(route.total, 3);
    assert_eq!(s.route(&s.map.files[0], &s.map.files[3], 8).total, 0);
}

#[test]
fn cli_exit_codes_and_text() {
    let binary = env!("CARGO_BIN_EXE_tolmap");
    let map = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/tolmap.json");
    for args in [
        vec!["overview"],
        vec!["where", "missing"],
        vec!["deps", "src/main.rs"],
        vec!["route", "src/main.rs", "src/lib.rs"],
    ] {
        let output = std::process::Command::new(binary)
            .args(args)
            .arg("--map")
            .arg(&map)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["schema"], QUERY_SCHEMA);
        if report["query"]["kind"] == "overview" {
            assert_eq!(
                report["results"].as_array().unwrap().len(),
                report["total"].as_u64().unwrap() as usize
            );
            assert_eq!(report["truncated"], false);
        } else {
            assert_eq!(report["limit"], 25);
        }
    }
    let limited = std::process::Command::new(binary)
        .args(["overview", "--limit", "0", "--format", "text", "--map"])
        .arg(&map)
        .output()
        .unwrap();
    assert_eq!(limited.status.code(), Some(0));
    let limited = String::from_utf8(limited.stdout).unwrap();
    assert!(limited.contains("rows omitted; repeat with --limit"));
    let text = std::process::Command::new(binary)
        .args(["where", "main", "--format", "text", "--map"])
        .arg(&map)
        .output()
        .unwrap();
    assert_eq!(text.status.code(), Some(0));
    assert!(String::from_utf8(text.stdout)
        .unwrap()
        .contains("ambiguous"));
    let bad = std::process::Command::new(binary)
        .args(["overview", "--map", "/nonexistent/tolmap-query-input"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(bad.stdout.is_empty());
    #[cfg(unix)]
    {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let internal = std::process::Command::new(binary)
            .args(["overview", "--map"])
            .arg(&map)
            .stdout(full)
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert_eq!(internal.code(), Some(3));
    }
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
    let mut malformed = symbols;
    malformed["symbols"][0][5] = serde_json::json!(1);
    std::fs::write(
        map.with_extension("symbols.json"),
        serde_json::to_vec(&malformed).unwrap(),
    )
    .unwrap();
    assert!(s
        .where_is("run", 8)
        .unwrap_err()
        .to_string()
        .contains("cycle"));
    assert_eq!(s.where_is(&s.map.files[0], 8).unwrap().total, 1);
}
