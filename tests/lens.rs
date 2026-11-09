//! Lenses (issue #241): schema, resolution, determinism and the CLI.
use tolmap::lens::{self, HopSource, Lens, LensError, LensReport, RouteKind, UnresolvedKind as K};
use tolmap::schema::MapDocument;

const FLASK: &str = include_str!("../data/flask.json");
const EXAMPLE: &str = include_str!("../data/lenses/flask-request.toml");

fn flask() -> MapDocument {
    serde_json::from_str(FLASK).unwrap()
}

fn resolve(toml: &str, map: &MapDocument) -> LensReport {
    lens::resolve(&Lens::parse(toml).unwrap(), map)
}

fn path_lens(stops: &[&str]) -> String {
    let stops: Vec<String> = stops.iter().map(|s| format!("{{ at = {s:?} }}")).collect();
    format!(
        "version = 1\n[[paths]]\nname = \"p\"\nstops = [{}]\n",
        stops.join(", ")
    )
}

#[test]
fn semon_lenses_round_trip() {
    // Finding 62's lenses; semon is not a fixture, so only the schema is checked here.
    for text in [
        include_str!("../data/lenses/semon/session-sync.toml"),
        include_str!("../data/lenses/semon/transcript-read.toml"),
    ] {
        let lens = Lens::parse(text).unwrap();
        assert_eq!(Lens::parse(&lens.to_toml()).unwrap(), lens);
        assert_eq!(lens.paths[0].stops.len(), 9);
    }
}

#[test]
fn example_lens_round_trips_and_resolves_fully() {
    let lens = Lens::parse(EXAMPLE).unwrap();
    assert_eq!(Lens::parse(&lens.to_toml()).unwrap(), lens);
    let json = serde_json::to_string(&lens).unwrap();
    assert_eq!(serde_json::from_str::<Lens>(&json).unwrap(), lens);

    let report = lens::resolve(&lens, &flask());
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    assert!(report.lower_bound);
    let kinds: Vec<_> = report.paths[0]
        .hops
        .iter()
        .map(|h| (h.source, h.edge_kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (HopSource::Computed, Some(RouteKind::Imports)),
            (HopSource::Computed, Some(RouteKind::ImportedBy)),
            (HopSource::Declared, None),
        ]
    );
    // sansio/** expands to its three files; Flask adds one symbol member.
    let core = &report.areas[0].members;
    assert_eq!(core.len(), 1 + 1 + 3);
    assert_eq!(report.notes[0].district, Some(1));
    assert_eq!(report.notes[1].target.as_ref().unwrap().district, 2);
}

#[test]
fn invalid_lenses_are_refused_with_a_reason() {
    let cases: [(&str, &str); 9] = [
        ("version = 2\n", "version"),
        (
            "version = 1\n[[areas]]\nlabel = \"a\"\ncolor = \"pink\"\nmembers = [\"x\"]\n",
            "pink",
        ),
        (
            "version = 1\n[[areas]]\nlabel = \"a\"\ncolor = \"red\"\nmembers = [\"../x\"]\n",
            "relative",
        ),
        (
            "version = 1\n[[paths]]\nname = \"p\"\nstops = [{ at = \"a\" }, { at = \"b\", hop = \"declared\" }]\n",
            "reason",
        ),
        (
            "version = 1\n[[paths]]\nname = \"p\"\nstops = [{ at = \"a\", hop = \"declared\", reason = \"r\" }, { at = \"b\" }]\n",
            "first stop",
        ),
        (
            "version = 1\n[[paths]]\nname = \"p\"\nstops = [{ at = \"a\" }]\n",
            "two stops",
        ),
        (
            "version = 1\n[[paths]]\nname = \"p\"\nstops = [{ at = \"src/*.rs\" }, { at = \"b\" }]\n",
            "glob",
        ),
        ("version = 1\nsurprise = true\n", "surprise"),
        (
            "version = 1\n[[notes]]\nanchor = \"district:\"\ntext = \"t\"\n",
            "district",
        ),
    ];
    for (text, needle) in cases {
        let err = Lens::parse(text).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains(needle), "{text:?} -> {shown}");
    }
    assert!(matches!(
        Lens::parse("version = ").unwrap_err(),
        LensError::Parse(_)
    ));
}

#[test]
fn every_unresolved_kind_is_reported_distinctly() {
    let mut map = flask();
    // Disconnect views.py so a computed hop to it has no route.
    let views = map
        .files
        .iter()
        .position(|f| f.ends_with("/views.py"))
        .unwrap();
    map.edges.retain(|e| !e.contains(&views));
    let text = r#"
version = 1
[[areas]]
label = "broken"
color = "red"
members = [
  "src/flask/gone.py",
  "src/nowhere/**",
  "src/flask/app.py#Missing",
  "src/flask/wrappers.py#Request.max_content_length",
  "src/flask/app.py",
]
[[paths]]
name = "p"
stops = [
  { at = "src/flask/app.py" },
  { at = "src/flask/views.py" },
  { at = "src/flask/gone.py" },
]
[[notes]]
anchor = "district:atlantis"
text = "t"
"#;
    let report = resolve(text, &map);
    let got: Vec<(K, &str)> = report
        .unresolved
        .iter()
        .map(|u| (u.kind, u.location.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (K::MissingPath, "areas.broken.members[0]"),
            (K::EmptyGlob, "areas.broken.members[1]"),
            (K::UnknownSymbol, "areas.broken.members[2]"),
            (K::Ambiguous, "areas.broken.members[3]"),
            (K::MissingPath, "paths.p.stops[2]"),
            (K::NoRoute, "paths.p.hops[0]"),
            (K::UnknownDistrict, "notes[0].anchor"),
        ]
    );
    // The reachable member is still there, nothing was dropped.
    assert_eq!(report.areas[0].members.len(), 1);
    assert!(!report.paths[0].hops[0].resolved);
    assert!(!report.paths[0].hops[1].resolved);
    assert_eq!(report.paths[0].stops[2].target, None);
}

#[test]
fn output_is_deterministic_and_the_map_is_untouched() {
    let map = flask();
    let before = serde_json::to_vec(&map).unwrap();
    let a = lens::render_json(&resolve(EXAMPLE, &map));
    let b = lens::render_json(&resolve(EXAMPLE, &flask()));
    assert_eq!(a, b);
    assert_eq!(before, serde_json::to_vec(&map).unwrap());
    // Member order does not depend on how the lens lists them.
    let one = "version = 1\n[[areas]]\nlabel = \"a\"\ncolor = \"red\"\nmembers = [\"src/flask/views.py\", \"src/flask/app.py\", \"src/flask/*.py\"]\n";
    let two = "version = 1\n[[areas]]\nlabel = \"a\"\ncolor = \"red\"\nmembers = [\"src/flask/*.py\", \"src/flask/app.py\", \"src/flask/views.py\"]\n";
    let paths = |t: &str| -> Vec<String> {
        resolve(t, &map).areas[0]
            .members
            .iter()
            .map(|m| m.target.path.clone())
            .collect()
    };
    let sorted = paths(one);
    assert_eq!(sorted, paths(two));
    assert!(
        sorted.windows(2).all(|w| w[0] < w[1]),
        "sorted and distinct"
    );
}

// Expected values come from web/src/map/graph.ts `findRoute` run on the same
// edges: directed a->b first, then b->a reported reversed, then undirected.
#[test]
fn computed_hops_agree_with_the_viewer_route() {
    let map = flask();
    let cases: [(&str, &str, RouteKind, &[&str]); 5] = [
        ("__init__", "app", RouteKind::Imports, &["__init__", "app"]),
        (
            "__init__",
            "cli",
            RouteKind::Imports,
            &["__init__", "app", "cli"],
        ),
        (
            "app",
            "__init__",
            RouteKind::ImportedBy,
            &["app", "__init__"],
        ),
        (
            "__init__",
            "__main__",
            RouteKind::Undirected,
            &["__init__", "app", "cli", "__main__"],
        ),
        (
            "__init__",
            "views",
            RouteKind::Undirected,
            &["__init__", "globals", "views"],
        ),
    ];
    for (a, b, kind, via) in cases {
        let file = |n: &str| format!("src/flask/{n}.py");
        let report = resolve(&path_lens(&[&file(a), &file(b)]), &map);
        let hop = &report.paths[0].hops[0];
        assert_eq!(hop.source, HopSource::Computed);
        assert_eq!(hop.edge_kind, Some(kind), "{a} -> {b}");
        let want: Vec<String> = via.iter().map(|n| file(n)).collect();
        assert_eq!(hop.via, want, "{a} -> {b}");
        assert!(hop.resolved && report.unresolved.is_empty());
    }
    // Two symbols of one file: containment, labelled as such.
    let report = resolve(
        &path_lens(&["src/flask/app.py#Flask", "src/flask/app.py#remove_ctx"]),
        &map,
    );
    assert_eq!(report.paths[0].hops[0].edge_kind, Some(RouteKind::SameFile));
}

#[test]
fn declared_hops_are_never_computed() {
    let map = flask();
    // Connected files: a route exists, but the author declared the hop.
    let text = "version = 1\n[[paths]]\nname = \"p\"\nstops = [{ at = \"src/flask/app.py\" }, { at = \"src/flask/cli.py\", hop = \"declared\", reason = \"by hand\" }]\n";
    let hop = &resolve(text, &map).paths[0].hops[0];
    assert_eq!(hop.source, HopSource::Declared);
    assert_eq!(hop.edge_kind, None);
    assert!(hop.via.is_empty());
    assert_eq!(hop.reason.as_deref(), Some("by hand"));
    // Unconnected files: a declared hop is not a missing route.
    let mut cut = flask();
    let views = cut
        .files
        .iter()
        .position(|f| f.ends_with("/views.py"))
        .unwrap();
    cut.edges.retain(|e| !e.contains(&views));
    let text = text.replace("cli.py", "views.py");
    let report = resolve(&text, &cut);
    assert!(report.unresolved.is_empty());
    assert!(report.paths[0].hops[0].resolved);
}

fn run_cli(lens: &std::path::Path, extra: &[&str]) -> (i32, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_tolmap"))
        .args(["lens", "check"])
        .arg(lens)
        .arg("--map")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/data/flask.json"))
        .args(extra)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn lens_check_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let example = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/data/lenses/flask-request.toml"
    );
    let (code, out) = run_cli(example.as_ref(), &[]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("all members, stops and anchors resolved"));

    let broken = dir.path().join("broken.toml");
    std::fs::write(
        &broken,
        path_lens(&["src/flask/app.py", "src/flask/gone.py"]),
    )
    .unwrap();
    let (code, out) = run_cli(&broken, &["--format", "json"]);
    assert_eq!(code, 1);
    let report: LensReport = serde_json::from_str(&out).unwrap();
    assert_eq!(report.unresolved[0].kind, K::MissingPath);

    let invalid = dir.path().join("invalid.toml");
    std::fs::write(&invalid, "version = 9\n").unwrap();
    assert_eq!(run_cli(&invalid, &[]).0, 2);
    assert_eq!(run_cli(&dir.path().join("absent.toml"), &[]).0, 2);
}
