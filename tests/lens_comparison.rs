use tolmap::{
    lens::*,
    lens_comparison::{changes, LensChange},
    schema::MapDocument,
};

fn frame() -> MapDocument {
    serde_json::from_str(include_str!("../data/flask.json")).unwrap()
}
fn lens() -> Lens {
    Lens::parse("version=1\n[[areas]]\nlabel='Core'\ncolor='blue'\nmembers=['src/flask/**']\n[[paths]]\nname='request'\nstops=[{at='src/flask/app.py'},{at='src/flask/views.py'}]\n").unwrap()
}

#[test]
fn deletion_breaks_hop_rename_stays_explicit_and_growth_uses_base_frame() {
    let map = frame();
    let bytes = serde_json::to_vec(&map).unwrap();
    let before = resolve(&lens(), &map);
    let mut head = map.clone();
    let app = head
        .files
        .iter()
        .position(|p| p == "src/flask/app.py")
        .unwrap();
    let views = head
        .files
        .iter()
        .position(|p| p == "src/flask/views.py")
        .unwrap();
    head.files[app] = "src/flask/renamed.py".into();
    head.files[views] = "src/flask/new.py".into();
    head.edges.clear();
    let renames = vec![FileRename {
        from: "src/flask/app.py".into(),
        to: "src/flask/renamed.py".into(),
        commit: "a".repeat(40),
    }];
    let mut after = resolve(&lens(), &head);
    report_renames(&mut after, &renames);
    assert!(after
        .unresolved
        .iter()
        .any(|u| u.kind == UnresolvedKind::RenamedFile && u.renamed == Some(renames[0].clone())));
    assert!(after
        .unresolved
        .iter()
        .any(|u| u.kind == UnresolvedKind::MissingPath));
    assert!(
        after.paths[0].stops[0].target.is_none(),
        "rename is never silently substituted"
    );
    let result = changes(&before, &after, &map, &renames);
    assert!(result
        .iter()
        .any(|d| d.kind == "hop" && d.status == LensChange::Broken && d.targets.len() == 2));
    assert!(result
        .iter()
        .any(|d| d.kind == "stop" && d.status == LensChange::Renamed));
    assert!(result.iter().any(|d| d.kind == "area"
        && d.status == LensChange::Added
        && d.after == ["src/flask/new.py"]
        && d.targets.is_empty()));
    assert_eq!(
        serde_json::to_vec(&result).unwrap(),
        serde_json::to_vec(&changes(&before, &after, &map, &renames)).unwrap()
    );
    assert_eq!(serde_json::to_vec(&map).unwrap(), bytes);
}

#[test]
fn overlapping_lenses_share_members_stops_and_hops_without_matching_names() {
    let map = frame();
    let a = lens();
    let mut b = a.clone();
    b.areas[0].label = "Other area".into();
    b.areas[0].members = vec!["src/flask/app.py".into()];
    b.paths[0].name = "other-path".into();
    let result = changes(&resolve(&a, &map), &resolve(&b, &map), &map, &[]);
    assert!(result
        .iter()
        .any(|d| d.kind == "area" && d.status == LensChange::Shared));
    assert!(result
        .iter()
        .any(|d| d.kind == "area" && d.status == LensChange::Removed));
    assert!(result
        .iter()
        .any(|d| d.kind == "hop" && d.status == LensChange::Shared));
    assert!(result
        .iter()
        .any(|d| d.kind == "stop" && d.status == LensChange::Shared));
}

#[test]
fn updated_renamed_stop_and_moved_symbol_use_old_base_footprint() {
    let map = frame();
    let a = lens();
    let mut b = a.clone();
    b.paths[0].stops[0].at = "src/flask/renamed.py".into();
    let mut head = map.clone();
    let app = head
        .files
        .iter()
        .position(|p| p == "src/flask/app.py")
        .unwrap();
    head.files[app] = "src/flask/renamed.py".into();
    let rename = FileRename {
        from: "src/flask/app.py".into(),
        to: "src/flask/renamed.py".into(),
        commit: "b".repeat(40),
    };
    let result = changes(&resolve(&a, &map), &resolve(&b, &head), &map, &[rename]);
    let renamed = result
        .iter()
        .find(|d| d.kind == "stop" && d.status == LensChange::Renamed)
        .unwrap();
    assert!(renamed
        .targets
        .iter()
        .all(|t| t.file == app && t.path == "src/flask/app.py"));
}
