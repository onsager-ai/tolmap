use tolmap::{
    lens::*,
    lens_comparison::{changes, ComparisonMode, LensChange},
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
    let result = changes(&before, &after, &map, &renames, ComparisonMode::Commits);
    assert!(result
        .iter()
        .any(|d| d.kind == "hop" && d.status == LensChange::Broken && d.targets.len() == 2));
    for renamed in result
        .iter()
        .filter(|delta| delta.status == LensChange::Renamed)
    {
        assert_eq!(renamed.before, ["src/flask/app.py"]);
        assert_eq!(renamed.after, ["src/flask/renamed.py"]);
    }
    assert!(result
        .iter()
        .any(|d| d.kind == "stop" && d.status == LensChange::Renamed));
    assert!(result.iter().any(|d| d.kind == "area"
        && d.status == LensChange::Added
        && d.after == ["src/flask/new.py"]
        && d.targets.is_empty()));
    assert_eq!(
        serde_json::to_vec(&result).unwrap(),
        serde_json::to_vec(&changes(
            &before,
            &after,
            &map,
            &renames,
            ComparisonMode::Commits
        ))
        .unwrap()
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
    // The shared ordered hop is shifted to positions 1 -> 2 in B.
    b.paths[0].stops.insert(
        0,
        LensStop {
            at: "src/flask/helpers.py".into(),
            hop: HopMode::Computed,
            reason: None,
        },
    );
    let result = changes(
        &resolve(&a, &map),
        &resolve(&b, &map),
        &map,
        &[],
        ComparisonMode::Membership,
    );
    assert!(result
        .iter()
        .any(|d| d.kind == "area" && d.status == LensChange::Shared));
    assert!(result
        .iter()
        .any(|d| d.kind == "area" && d.status == LensChange::Removed));
    assert!(result
        .iter()
        .any(|d| d.kind == "hop" && d.status == LensChange::Shared));
    assert_eq!(
        result
            .iter()
            .filter(|d| d.kind == "stop" && d.status == LensChange::Shared)
            .count(),
        2
    );
    assert!(result.iter().all(|d| d.status != LensChange::Moved));
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
    let result = changes(
        &resolve(&a, &map),
        &resolve(&b, &head),
        &map,
        &[rename],
        ComparisonMode::Commits,
    );
    let renamed = result
        .iter()
        .find(|d| d.kind == "stop" && d.status == LensChange::Renamed)
        .unwrap();
    assert!(renamed
        .targets
        .iter()
        .all(|t| t.file == app && t.path == "src/flask/app.py"));
}

#[test]
fn resolution_transition_table_and_movement_precedence() {
    let map = frame();
    let file = "src/flask/app.py";
    let renamed = "src/flask/renamed.py";
    let definition = lens();
    for base_resolves in [false, true] {
        for head_resolves in [false, true] {
            for has_rename in [false, true] {
                for updated in [false, true] {
                    let mut base_map = map.clone();
                    let mut head_map = map.clone();
                    let index = map.files.iter().position(|p| p == file).unwrap();
                    if !base_resolves {
                        base_map.files[index] = "absent.py".into();
                    }
                    head_map.files[index] = if head_resolves {
                        if has_rename {
                            renamed.into()
                        } else {
                            file.into()
                        }
                    } else {
                        "absent.py".into()
                    };
                    let mut head_lens = definition.clone();
                    if has_rename && updated {
                        head_lens.paths[0].stops[0].at = renamed.into();
                    }
                    let renames = if has_rename {
                        vec![FileRename {
                            from: file.into(),
                            to: renamed.into(),
                            commit: "a".repeat(40),
                        }]
                    } else {
                        vec![]
                    };
                    let before = resolve(&definition, &base_map);
                    let mut after = resolve(&head_lens, &head_map);
                    report_renames(&mut after, &renames);
                    let changes = changes(&before, &after, &map, &renames, ComparisonMode::Commits);
                    let stop = changes
                        .iter()
                        .find(|d| d.kind == "stop" && d.before == [file])
                        .unwrap();
                    let expected = if has_rename && !updated {
                        LensChange::Renamed
                    } else if !base_resolves && head_resolves {
                        LensChange::Added
                    } else if base_resolves && !head_resolves {
                        LensChange::Broken
                    } else if has_rename {
                        LensChange::Renamed
                    } else {
                        LensChange::Shared
                    };
                    assert_eq!(stop.status, expected, "base={base_resolves}, head={head_resolves}, rename={has_rename}, updated={updated}");
                }
            }
        }
    }
    let before = resolve(&definition, &map);
    let mut reordered = definition.clone();
    reordered.paths[0].stops.reverse();
    let after = resolve(&reordered, &map);
    assert!(changes(&before, &after, &map, &[], ComparisonMode::Commits)
        .iter()
        .filter(|d| d.kind == "stop")
        .all(|d| d.status == LensChange::Moved));
    let mut relocated = before.clone();
    relocated.paths[0].stops[0]
        .target
        .as_mut()
        .unwrap()
        .district += 1;
    assert!(
        changes(&before, &relocated, &map, &[], ComparisonMode::Commits)
            .iter()
            .any(|d| d.kind == "stop" && d.status == LensChange::Moved)
    );
}

#[test]
fn symbol_membership_uses_resolved_identity_and_line_edits_are_not_movement() {
    let mut map = frame();
    let index = map
        .files
        .iter()
        .position(|p| p == "src/flask/app.py")
        .unwrap();
    map.symbols.insert(
        index.to_string(),
        vec![tolmap::schema::SymbolRow(("handler".into(), 1, 10, 20))],
    );
    let mut a = lens();
    a.paths[0].stops[0].at = "src/flask/app.py#handler".into();
    let mut b = a.clone();
    b.paths[0].stops[0].at = "src/flask/app.py#handler@10".into();
    b.paths[0].name = "another".into();
    b.paths[0].stops.reverse();
    let before = resolve(&a, &map);
    let overlap = changes(
        &before,
        &resolve(&b, &map),
        &map,
        &[],
        ComparisonMode::Membership,
    );
    assert_eq!(
        overlap
            .iter()
            .filter(|d| d.kind == "stop" && d.status == LensChange::Shared)
            .count(),
        2
    );
    map.symbols.get_mut(&index.to_string()).unwrap()[0].0 .2 = 12;
    map.symbols.get_mut(&index.to_string()).unwrap()[0].0 .3 = 22;
    let commit = changes(
        &before,
        &resolve(&a, &map),
        &map,
        &[],
        ComparisonMode::Commits,
    );
    assert!(commit
        .iter()
        .filter(|d| d.kind == "stop")
        .all(|d| d.status == LensChange::Shared));
}

#[test]
fn hop_resolution_transitions_and_one_sided_stops() {
    let map = frame();
    let mut unresolved = resolve(&lens(), &map);
    unresolved.paths[0].hops[0].resolved = false;
    let mut resolved = unresolved.clone();
    resolved.paths[0].hops[0].resolved = true;
    for (before, after, expected) in [
        (&unresolved, &unresolved, LensChange::Shared),
        (&unresolved, &resolved, LensChange::Added),
        (&resolved, &unresolved, LensChange::Broken),
        (&resolved, &resolved, LensChange::Shared),
    ] {
        assert_eq!(
            changes(before, after, &map, &[], ComparisonMode::Commits)
                .iter()
                .find(|d| d.kind == "hop")
                .unwrap()
                .status,
            expected
        );
    }
    let mut empty = resolved.clone();
    empty.paths.clear();
    empty.areas.clear();
    for (report, expected) in [
        (&resolved, LensChange::Added),
        (&unresolved, LensChange::Broken),
    ] {
        assert_eq!(
            changes(&empty, report, &map, &[], ComparisonMode::Commits)
                .iter()
                .find(|d| d.kind == "hop")
                .unwrap()
                .status,
            expected
        );
    }
    assert!(
        changes(&resolved, &empty, &map, &[], ComparisonMode::Commits)
            .iter()
            .all(|d| d.status == LensChange::Removed)
    );
}
