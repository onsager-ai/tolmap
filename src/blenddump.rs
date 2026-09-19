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

use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::extract::{self, LanguageKind};
use crate::pipeline;

/// Below this, a weight difference is float summation order; above it, real.
pub const TOLERANCE: f64 = 1e-9;

const FLOOR: f64 = 0.02;

fn round_to(value: f64, places: i32) -> f64 {
    let factor = 10f64.powi(places);
    (value * factor).round() / factor
}

pub fn dump(repo: &Path, pkg: &str, lang: &str, out: &Path) -> Result<()> {
    let language = LanguageKind::parse(lang)?;
    let mut data = extract::build(repo, pkg, language)?;

    let candidate_edges = data.edges.len();
    let mass =
        |select: fn(&crate::schema::SignalEdge) -> f64| data.edges.iter().map(select).sum::<f64>();
    let raw = json!({
        "static": round_to(mass(|e| e.static_signal), 10),
        "cochange": round_to(mass(|e| e.cochange), 10),
        "prox": round_to(mass(|e| e.proximity), 10),
        "sem": round_to(mass(|e| e.semantic), 10),
    });

    pipeline::blend(&mut data)?;
    let blended: Vec<f64> = data.edges.iter().map(|e| e.weight).collect();
    let weight_sum_pre_prune = round_to(blended.iter().sum::<f64>(), 10);
    let n_blended_edges = blended.len();
    let below_prune_floor =
        blended.iter().filter(|w| **w < FLOOR).count() as f64 / n_blended_edges.max(1) as f64;

    pipeline::prune(&mut data, 14, FLOOR);
    let weight_sum = round_to(data.edges.iter().map(|e| e.weight).sum::<f64>(), 10);
    let mut edges: Vec<(String, String, f64)> = data
        .edges
        .iter()
        .map(|e| (e.a.clone(), e.b.clone(), round_to(e.weight, 10)))
        .collect();
    edges.sort_by(|left, right| left.partial_cmp(right).expect("weights are finite"));

    let document = json!({
        "repo": data.repo,
        "tolerance": TOLERANCE,
        "candidate_edges": candidate_edges,
        "raw_signal_mass": raw,
        "n_nodes": data.nodes.len(),
        "n_blended_edges": n_blended_edges,
        "weight_sum_pre_prune": weight_sum_pre_prune,
        "n_pruned_edges": edges.len(),
        "weight_sum": weight_sum,
        "below_prune_floor": round_to(below_prune_floor, 6),
        "node_order": data.nodes.iter().map(|n| n.file.clone()).collect::<Vec<_>>(),
        "edges": edges,
    });
    std::fs::write(out, serde_json::to_string_pretty(&document)?)?;
    println!(
        "{}: {} nodes, {} candidate edges, weight {} pre-prune -> {} edges, weight {} post-prune \
         (compare within {}); {:.1}% of blended edges below the {} floor",
        data.repo,
        data.nodes.len(),
        candidate_edges,
        weight_sum_pre_prune,
        edges.len(),
        weight_sum,
        TOLERANCE,
        below_prune_floor * 100.0,
        FLOOR
    );
    Ok(())
}
