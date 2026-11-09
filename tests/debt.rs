//! Debt findings (issue #250): each finding kind on a hand-built map, the
//! entry exclusion, determinism, hazard coverage and the lens/CLI outputs.
use tolmap::debt::{self, DebtReport};
use tolmap::lens::{self, Lens};
use tolmap::schema::MapDocument;

const FLASK: &str = include_str!("../data/flask.json");

// Four districts a, b, c, d. Edges 1->3 (a->b), 4->5 (b->c) and 6->2 (c->a)
// close a three-district cycle; d only points into a, so it is not part of
// it. File 7 sits in d but sends three of its five edges to a. File 11 has
// the top fan-in and complexity, file 12 the top churn x complexity. File
// 10 has an outbound edge and no inbound one; files 0 (`main`) and 13
// (under `tests/`) likewise, but they are entries. File 14 has no edge.
const FILES: [(&str, usize, usize, usize, f64); 16] = [
    // path, district, complexity, churn, fan-in
    ("src/a/main.py", 0, 2, 1, 0.0),
    ("src/a/core.py", 0, 5, 1, 2.0),
    ("src/a/util.py", 0, 3, 1, 2.0),
    ("src/b/one.py", 1, 4, 1, 1.0),
    ("src/b/two.py", 1, 4, 1, 1.0),
    ("src/c/x.py", 2, 4, 1, 1.0),
    ("src/c/y.py", 2, 4, 1, 1.0),
    ("src/d/misplaced.py", 3, 6, 1, 1.0),
    ("src/d/helper.py", 3, 2, 1, 1.0),
    ("src/d/lonely.py", 3, 2, 1, 1.0),
    ("src/d/orphan.py", 3, 2, 1, 0.0),
    ("src/b/god.py", 1, 50, 5, 9.0),
    ("src/c/hot.py", 2, 30, 20, 1.0),
    ("tests/test_core.py", 0, 2, 1, 0.0),
    ("src/c/alone.py", 2, 1, 1, 0.0),
    ("src/a/more.py", 0, 1, 1, 1.0),
];
const EDGES: [[usize; 2]; 17] = [
    [0, 1],
    [13, 1],
    [1, 3],
    [3, 4],
    [4, 5],
    [5, 6],
    [6, 2],
    [3, 11],
    [4, 11],
    [5, 12],
    [7, 1],
    [7, 2],
    [7, 15],
    [8, 7],
    [7, 9],
    [9, 8],
    [10, 9],
];

fn synthetic() -> MapDocument {
    let nodes: Vec<_> = FILES
        .iter()
        .map(|&(_, d, cx, ch, fi)| {
            serde_json::json!([d, 0.0, 0.0, 10, cx, ch, fi, 0.0, 0.0, 0.0, 0.0])
        })
        .collect();
    let district = |size| serde_json::json!({"size": size, "c": [0.0, 0.0], "blob": []});
    serde_json::from_value(serde_json::json!({
        "repo": "synthetic",
        "q": 0.3,
        "names": {"0": "a", "1": "b", "2": "c", "3": "d"},
        "districts": {"0": district(5), "1": district(3), "2": district(4), "3": district(4)},
        "F": FILES.iter().map(|f| f.0).collect::<Vec<_>>(),
        "N": nodes,
        "E": EDGES,
        "L": [[12, "hazard", "churn 20 x cplx 30", 1], [11, "hazard", "churn 5 x cplx 50", 2]],
        "S": {},
        "U": {},
        "roads": [],
        "lang": "python",
        "coverage": {"zero_edge_files": 1, "total_files": 16, "by_language": {}}
    }))
    .unwrap()
}

fn paths<'a>(items: impl IntoIterator<Item = &'a String>) -> Vec<&'a str> {
    items.into_iter().map(String::as_str).collect()
}

#[test]
fn each_finding_kind_is_detected() {
    let map = synthetic();
    let report = debt::findings(&map);
    assert!(report.lower_bound);
    assert_eq!(report.coverage.as_ref().unwrap().zero_edge_files, 1);

    assert_eq!(report.cycles.len(), 1);
    let cycle = &report.cycles[0];
    assert_eq!(cycle.route, [0, 1, 2]);
    let closing: Vec<_> = cycle
        .closing_edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert_eq!(
        closing,
        [
            ("src/a/core.py", "src/b/one.py"),
            ("src/b/two.py", "src/c/x.py"),
            ("src/c/y.py", "src/a/util.py"),
        ]
    );
    assert_eq!(cycle.cross_edges, 3);

    assert_eq!(
        paths(report.misplaced.iter().map(|m| &m.path)),
        ["src/d/misplaced.py"]
    );
    let m = &report.misplaced[0];
    assert_eq!((m.edges, m.to_district, m.share), (5, 0, 0.6));

    assert_eq!(
        paths(report.god_files.iter().map(|g| &g.path)),
        ["src/b/god.py"]
    );
    assert_eq!(
        paths(report.hotspots.iter().map(|h| &h.path)),
        ["src/c/hot.py", "src/b/god.py"]
    );
    assert_eq!(report.hotspots[0].score, 600);

    // main.py and tests/test_core.py also have no inbound edge: entries.
    assert_eq!(
        paths(report.unreached.iter().map(|u| &u.path)),
        ["src/d/orphan.py"]
    );
    assert_eq!(report.isolated_files, 1);
}

#[test]
fn breaking_one_edge_breaks_the_cycle() {
    let mut map = synthetic();
    map.edges.retain(|e| *e != [6, 2]);
    assert!(debt::findings(&map).cycles.is_empty());
}

#[test]
fn entry_rule() {
    for path in [
        "src/main.rs",
        "pkg/__main__.py",
        "web/src/index.tsx",
        "src/bin/tool.rs",
        "tests/lens.rs",
        "pkg/test_x.py",
        "cmd/x_test.go",
        "web/a.test.ts",
        "web/vite.config.ts",
        "web/env.d.ts",
        "crates/x/src/model/tests.rs",
    ] {
        assert!(debt::is_entry_path(path), "{path}");
    }
    for path in [
        "src/lens.rs",
        "pkg/testing.py",
        "web/src/data.ts",
        "src/domain.rs",
        "crates/x/src/model/testing.rs",
        "crates/x/src/contests.rs",
    ] {
        assert!(!debt::is_entry_path(path), "{path}");
    }
}

#[test]
fn top_two_hotspots_include_the_hazard_landmarks() {
    for map in [
        synthetic(),
        serde_json::from_str::<MapDocument>(FLASK).unwrap(),
    ] {
        let report = debt::findings(&map);
        let top: Vec<&str> = report
            .hotspots
            .iter()
            .take(2)
            .map(|h| h.path.as_str())
            .collect();
        let hazards: Vec<_> = map
            .landmarks
            .iter()
            .filter(|l| l.0 .1 == "hazard")
            .collect();
        assert!(!hazards.is_empty());
        for l in hazards {
            assert!(
                top.contains(&map.files[l.0 .0].as_str()),
                "{} not in {top:?}",
                map.files[l.0 .0]
            );
        }
    }
}

fn outputs(map: &MapDocument) -> (String, String, String) {
    let report = debt::findings(map);
    (
        debt::render_json(&report),
        debt::render_text(&report),
        debt::to_lens(&report, map).to_toml(),
    )
}

#[test]
fn output_is_deterministic_and_the_map_is_untouched() {
    for map in [
        synthetic(),
        serde_json::from_str::<MapDocument>(FLASK).unwrap(),
    ] {
        let before = serde_json::to_vec(&map).unwrap();
        let first = outputs(&map);
        assert_eq!(first, outputs(&map));
        assert_eq!(before, serde_json::to_vec(&map).unwrap());
        let back: DebtReport = serde_json::from_str(&first.0).unwrap();
        assert_eq!(debt::render_json(&back), first.0);
    }
}

#[test]
fn lens_resolves_fully_with_stable_labels() {
    for map in [
        synthetic(),
        serde_json::from_str::<MapDocument>(FLASK).unwrap(),
    ] {
        let toml = outputs(&map).2;
        let lens = Lens::parse(&toml).unwrap();
        let resolved = lens::resolve(&lens, &map);
        assert!(resolved.unresolved.is_empty(), "{:?}", resolved.unresolved);
    }

    let map = synthetic();
    let lens = Lens::parse(&outputs(&map).2).unwrap();
    // One area per kind, labelled by the kind alone: a district is tinted at
    // most once per kind.
    let labels: Vec<&str> = lens.areas.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "district cycle",
            "misplaced file",
            "god file",
            "hotspot",
            "no inbound edge found",
        ]
    );
    let members = |kind: &str| -> Vec<&str> {
        lens.areas
            .iter()
            .find(|a| a.label == kind)
            .unwrap()
            .members
            .iter()
            .map(String::as_str)
            .collect()
    };
    assert_eq!(members("hotspot"), ["src/b/god.py", "src/c/hot.py"]);
    assert_eq!(members("district cycle").len(), 6);
    // The cycle path: closing edges computed, joined by declared hops.
    let stops: Vec<_> = lens.paths[0]
        .stops
        .iter()
        .map(|s| (s.at.as_str(), s.hop))
        .collect();
    use tolmap::lens::HopMode::{Computed as C, Declared as D};
    assert_eq!(
        stops,
        [
            ("src/a/core.py", C),
            ("src/b/one.py", C),
            ("src/b/two.py", D),
            ("src/c/x.py", C),
            ("src/c/y.py", D),
            ("src/a/util.py", C),
            ("src/a/core.py", D),
        ]
    );

    // A declared hop joins two files of one district; it claims no edge.
    assert_eq!(
        lens.paths[0].stops[2].reason.as_deref(),
        Some("no edge claimed: both files are in district b, where the cycle continues")
    );

    // A metric change moves notes, never labels or path names.
    let mut hotter = synthetic();
    hotter.nodes[12].0 .5 = 40;
    hotter.nodes[11].0 .6 = 12.0;
    let other = Lens::parse(&outputs(&hotter).2).unwrap();
    assert_eq!(
        other.areas.iter().map(|a| &a.label).collect::<Vec<_>>(),
        lens.areas.iter().map(|a| &a.label).collect::<Vec<_>>()
    );
    assert_eq!(other.paths, lens.paths);
    assert_ne!(other.notes, lens.notes);
}

#[test]
fn debt_cli_formats_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.json");
    std::fs::write(&map, serde_json::to_vec(&synthetic()).unwrap()).unwrap();
    let before = std::fs::read(&map).unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_tolmap"))
            .arg("debt")
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout).unwrap(),
        )
    };
    let m = map.to_str().unwrap();
    let (code, text) = run(&["--map", m]);
    assert_eq!(code, 0);
    assert!(text.contains("lower bounds"), "{text}");
    let (code, json) = run(&["--map", m, "--format", "json"]);
    assert_eq!(code, 0);
    assert_eq!(json, debt::render_json(&debt::findings(&synthetic())));
    let (code, toml) = run(&["--map", m, "--format", "lens"]);
    assert_eq!(code, 0);
    let lens_file = dir.path().join("debt.toml");
    std::fs::write(&lens_file, &toml).unwrap();
    let check = std::process::Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .args(["lens", "check"])
        .arg(&lens_file)
        .args(["--map", m])
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
    assert_eq!(before, std::fs::read(&map).unwrap());
    assert_eq!(
        run(&["--map", dir.path().join("absent.json").to_str().unwrap()]).0,
        2
    );
}

// Issue #250's comparison test: compared on the base map, a cycle broken by
// a removed edge loses its members and path, a new god file is an added
// member of the god area even though the file was already in the cycle, and
// a metric change on an unchanged finding is `shared`, not removed + added.
#[test]
fn comparison_reads_findings_by_identity() {
    use tolmap::lens_comparison::LensChange::{self, Added, Removed, Shared};
    let base = synthetic();
    let mut head = synthetic();
    head.edges.retain(|e| *e != [6, 2]); // breaks the a -> b -> c -> a cycle
    head.nodes[1].0 .4 = 60; // core.py: now top complexity...
    head.nodes[1].0 .6 = 20.0; // ...and top fan-in, so a god file
    head.nodes[12].0 .5 = 40; // hot.py: churn up, still a hotspot
    let (before, after, changes) = debt::compare(&base, &head, &head, &[]);
    assert!(before.unresolved.is_empty() && after.unresolved.is_empty());
    // An area item is one member file of one kind's area.
    let status = |kind: &str, file: &str| -> Vec<LensChange> {
        changes
            .iter()
            .filter(|d| d.kind == "area" && d.label == kind)
            .filter(|d| d.before.iter().chain(&d.after).any(|s| s == file))
            .map(|d| d.status)
            .collect()
    };
    for file in [
        "src/a/core.py",
        "src/a/util.py",
        "src/b/one.py",
        "src/b/two.py",
        "src/c/x.py",
        "src/c/y.py",
    ] {
        assert_eq!(status("district cycle", file), [Removed], "{file}");
    }
    let cycle_path = &before.paths[0].name;
    assert!(changes
        .iter()
        .filter(|d| d.kind != "area")
        .all(|d| d.status == Removed && d.label == *cycle_path));
    assert_eq!(status("god file", "src/a/core.py"), [Added]);
    assert_eq!(status("god file", "src/b/god.py"), [Removed]);
    assert_eq!(status("hotspot", "src/c/hot.py"), [Shared]);
    assert_eq!(status("hotspot", "src/b/god.py"), [Shared]);
    assert_eq!(changes.iter().filter(|d| d.status == Added).count(), 1);
    // Same inputs, same output.
    let again = debt::compare(&base, &head, &head, &[]);
    assert_eq!(
        serde_json::to_string(&again.2).unwrap(),
        serde_json::to_string(&changes).unwrap()
    );
}
