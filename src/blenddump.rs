//! Emit the weighted graph the partitioner actually receives.
//!
//! Instrumentation, not part of the map pipeline. `F` and `E` in a map are the
//! file list and the emitted directed import list; neither is the graph Leiden
//! sees. That graph is the edge set after [`pipeline::blend`] has normalised
//! the four signals on mass and [`pipeline::prune`] has cut each node to its
//! strongest, and it appears nowhere in the output — so this port reproduces
//! `F` and `E` byte-exactly on all nine fixtures while still disagreeing with
//! the reference on membership.
//!
//! The field names and rounding mirror `eval/dump_blend.py` so the two can be
//! diffed directly. Compare weights within `TOLERANCE`: summing the same terms
//! in a different order can differ in the last ulp, and compared exactly that
//! flags every edge and buries the real divergence.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::extract::{self, LanguageKind};
use crate::pipeline::{self, PruneVariant};

/// Below this, a weight difference is float summation order; above it, real.
pub const TOLERANCE: f64 = 1e-9;

fn round_to(value: f64, places: i32) -> f64 {
    let factor = 10f64.powi(places);
    (value * factor).round() / factor
}

pub fn dump(
    repo: &Path,
    pkg: &str,
    lang: &str,
    prune_variant: PruneVariant,
    out: &Path,
) -> Result<()> {
    let language = LanguageKind::parse(lang)?;
    let data = extract::build(repo, pkg, language)?;
    dump_data(
        data,
        prune_variant,
        out,
        Some((repo, &[(pkg.to_owned(), language)])),
    )
}

/// As [`dump`], but unions any number of `(pkg, language)` sources (see
/// `extract::build_multi_source`) instead of parsing one.
pub fn dump_multi(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    prune_variant: PruneVariant,
    out: &Path,
) -> Result<()> {
    let data = extract::build_multi_source(repo, sources)?;
    dump_data(data, prune_variant, out, Some((repo, sources)))
}

fn dump_data(
    mut data: crate::schema::GraphData,
    prune_variant: PruneVariant,
    out: &Path,
    diagnostic_source: Option<(&Path, &[(String, LanguageKind)])>,
) -> Result<()> {
    let mut candidate_files = BTreeSet::new();
    let mut static_files = BTreeSet::new();
    for edge in &data.edges {
        candidate_files.insert(edge.a.clone());
        candidate_files.insert(edge.b.clone());
    }
    for &(a, b, _) in &data.imports {
        static_files.insert(data.nodes[a as usize].file.clone());
        static_files.insert(data.nodes[b as usize].file.clone());
    }
    let candidate_edges = data.edges.len();
    let mass =
        |select: fn(&crate::schema::SignalEdge) -> f64| data.edges.iter().map(select).sum::<f64>();
    let raw = json!({
        "static": round_to(mass(|e| e.static_signal), 10),
        "cochange": round_to(mass(|e| e.cochange), 10),
        "prox": round_to(mass(|e| e.proximity), 10),
        "sem": round_to(mass(|e| e.semantic), 10),
    });

    // Finding 10's two unchosen routes both need the distribution on one
    // side of the max-rescale boundary. Preserve it here as instrumentation:
    // percentile calibration uses `weight / mass_normalized_max`, while the
    // pre-rescale route uses `weight` directly. The vector is sorted so the
    // dump stays deterministic and a consumer can evaluate any percentile
    // without rebuilding the repository.
    let mut mass_normalized_weights = pipeline::mass_normalized_weights(&data)?;
    let mass_normalized_max = mass_normalized_weights
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    mass_normalized_weights.sort_by(f64::total_cmp);

    let stats = pipeline::apply_prune_variant(&mut data, prune_variant)?;
    let weight_sum_pre_prune = round_to(stats.weight_sum_pre_prune, 10);
    let n_blended_edges = stats.candidate_edges;
    let below_prune_floor = stats.below_floor_share();
    let weight_sum = round_to(data.edges.iter().map(|e| e.weight).sum::<f64>(), 10);
    let mut edges: Vec<(String, String, f64)> = data
        .edges
        .iter()
        .map(|e| (e.a.clone(), e.b.clone(), round_to(e.weight, 10)))
        .collect();
    edges.sort_by(|left, right| left.partial_cmp(right).expect("weights are finite"));

    let coverage = diagnostic_source
        .map(|(repo, sources)| {
            extract::coverage_diagnostics(repo, sources, &candidate_files, &static_files, &data)
        })
        .transpose()?;

    // Issue #101: bucket every workspace-package import specifier
    // (resolved / resolved-but-excluded / unresolved / external), across
    // every TypeScript file -- not just the zero-edge ones `coverage`
    // reparses. An importer with other real edges elsewhere would never be
    // revisited by `coverage_diagnostics`, and its unresolved workspace
    // imports would otherwise go uncounted.
    let workspace_imports = diagnostic_source
        .map(|(repo, sources)| extract::workspace_import_coverage(repo, sources))
        .transpose()?;

    let document = json!({
        "repo": data.repo,
        "tolerance": TOLERANCE,
        "prune_variant": prune_variant.to_string(),
        "prune_floor": stats.floor,
        "prune_floor_basis": stats.floor_basis,
        "candidate_edges": candidate_edges,
        "raw_signal_mass": raw,
        "mass_normalized_max": mass_normalized_max,
        "mass_normalized_weights": mass_normalized_weights,
        "n_nodes": data.nodes.len(),
        "n_blended_edges": n_blended_edges,
        "weight_sum_pre_prune": weight_sum_pre_prune,
        "n_pruned_edges": edges.len(),
        "weight_sum": weight_sum,
        "below_prune_floor_count": stats.below_floor_count,
        "below_prune_floor": round_to(below_prune_floor, 6),
        "node_order": data.nodes.iter().map(|n| n.file.clone()).collect::<Vec<_>>(),
        "edges": edges,
        "coverage": coverage,
        "workspace_imports": workspace_imports,
    });
    std::fs::write(out, serde_json::to_string_pretty(&document)?)?;
    println!(
        "{}: {} nodes, {} candidate edges, weight {} pre-prune -> {} edges, weight {} post-prune \
         (compare within {}); {:.1}% of blended edges below the {} {} floor",
        data.repo,
        data.nodes.len(),
        candidate_edges,
        weight_sum_pre_prune,
        edges.len(),
        weight_sum,
        TOLERANCE,
        below_prune_floor * 100.0,
        stats.floor,
        stats.floor_basis,
    );
    if let Some(workspace_imports) = &workspace_imports {
        println!("workspace imports (issue #101): {workspace_imports}");
    }
    Ok(())
}
