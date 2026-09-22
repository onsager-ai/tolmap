//! Step 2 of the polyglot union-extraction work: measures what step 1's
//! merge (`extract::build_multi_source`) actually produces on a real,
//! multi-source repository, so `docs/ARCHITECTURE.md`'s polyglot paragraph
//! can be corrected to what was measured instead of what was argued before
//! any of this existed. See `docs/FINDINGS.md` finding 13 for the numbers
//! this produced on the corpus, and `CLAUDE.md`: measurement changes go with
//! their numbers, and this module does not retune the pipeline from what it
//! finds -- it reports, and leaves the coefficients/floors/thresholds alone.
//!
//! `tolmap polyglot-report <repo> --all-sources --out report.json` is the
//! CLI entry point (`src/main.rs`); [`report`] is the library one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};

use crate::extract::{self, LanguageKind};
use crate::parity::match_districts;
use crate::partition::LeidenFfi;
use crate::pipeline::{self, PruneStats, PruneVariant};
use crate::schema::GraphData;

/// Runs the full measurement and writes it to `out` as JSON, printing a
/// human summary to stdout. `sources` should already be sorted the way
/// `extract::build_multi_source` sorts internally; this function re-sorts
/// defensively so caller order never matters.
pub fn run(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    resolution: f64,
    prune_variant: PruneVariant,
    out: &Path,
) -> Result<()> {
    let report = report(repo, sources, resolution, prune_variant)?;
    std::fs::write(out, serde_json::to_string_pretty(&report)?)?;
    println!("{}", summarize(&report));
    Ok(())
}

/// The measurement itself, as a `serde_json::Value` (this is a one-shot
/// measurement tool, not a schema anything else in the codebase deserialises
/// -- a typed struct would buy nothing a `json!` literal doesn't already
/// give the two call sites, `run` above and the CI ceiling check).
pub fn report(
    repo: &Path,
    sources: &[(String, LanguageKind)],
    resolution: f64,
    prune_variant: PruneVariant,
) -> Result<Value> {
    let mut sorted_sources = sources.to_vec();
    sorted_sources.sort_by(|a, b| a.1.as_str().cmp(b.1.as_str()).then_with(|| a.0.cmp(&b.0)));

    let merged = extract::build_multi_source(repo, &sorted_sources)?;
    let file_language = file_language_map(&merged)?;
    let languages = sorted_sources
        .iter()
        .map(|(_, language)| *language)
        .collect::<BTreeSet<_>>();

    // Raw (pre-blend) per-signal masses, and the raw candidate edge set's
    // intra/cross-language split -- `merged.edges` here is exactly
    // `extract::build`'s output: every candidate pair whose raw
    // ALPHA*static + BETA*cochange + GAMMA*proximity + DELTA*semantic
    // cleared 0.02, none of it yet mass-normalised.
    let candidate_edges = merged.edges.len();
    let signal_cross_mass = signal_cross_language_mass(&merged, &file_language);
    let (candidate_intra, candidate_cross) = split_intra_cross(
        merged.edges.iter().map(|e| (e.a.as_str(), e.b.as_str())),
        &file_language,
    );

    // Run the same selected route as the product, retaining its pre-prune
    // floor classification for the per-language report.
    let mut pruned = merged.clone();
    let merged_prune_stats = pipeline::apply_prune_variant(&mut pruned, prune_variant)?;
    let below_floor_merged =
        below_floor_share_per_language(&merged, &merged_prune_stats, &file_language, &languages);
    let (kept_intra, kept_cross) = split_intra_cross(
        pruned.edges.iter().map(|e| (e.a.as_str(), e.b.as_str())),
        &file_language,
    );

    // The actual partition the product would draw districts from -- reused
    // (not re-derived) so "district" here means the same thing `tolmap
    // build` means by it.
    let partitioner = LeidenFfi;
    let output = pipeline::run_with_variant(
        merged.clone(),
        resolution,
        &partitioner,
        None,
        prune_variant,
    )?;
    let files_in_order = output
        .weighted
        .nodes
        .iter()
        .map(|node| node.file.clone())
        .collect::<Vec<_>>();
    let merged_membership = files_in_order
        .iter()
        .cloned()
        .zip(output.membership.iter().copied())
        .collect::<BTreeMap<_, _>>();

    let clustering_vs_language = clustering_vs_language_stats(
        &output.membership,
        &files_in_order,
        &file_language,
        &languages,
    );

    // Per-language: file counts, below-floor share in this language's own
    // single-source graph, and projection drift against the merged map.
    let mut per_language = serde_json::Map::new();
    for (pkg, language) in &sorted_sources {
        let lang_key = language.as_str();
        let single = extract::build(repo, pkg, *language)?;
        let single_candidate_edges = single.edges.len();
        let mut single_pruned = single.clone();
        let single_prune_stats = pipeline::apply_prune_variant(&mut single_pruned, prune_variant)?;
        let single_below_floor = single_prune_stats.below_floor_share();

        let single_output = pipeline::run_with_variant(
            single.clone(),
            resolution,
            &partitioner,
            None,
            prune_variant,
        )?;
        let single_membership = single_output
            .weighted
            .nodes
            .iter()
            .map(|node| node.file.clone())
            .zip(single_output.membership.iter().copied())
            .collect::<BTreeMap<_, _>>();

        let lang_files: BTreeSet<&str> = file_language
            .iter()
            .filter(|(_, l)| **l == *language)
            .map(|(f, _)| f.as_str())
            .collect();
        let retention = projection_drift(&merged_membership, &single_membership, &lang_files);

        per_language.insert(
            lang_key.to_owned(),
            json!({
                "pkg": pkg,
                "files": lang_files.len(),
                "candidate_edges_intra": candidate_intra.get(lang_key).copied().unwrap_or(0),
                "kept_edges_intra": kept_intra.get(lang_key).copied().unwrap_or(0),
                "below_prune_floor_merged": below_floor_merged.get(lang_key).copied().unwrap_or(0.0),
                "below_prune_floor_single_source": single_below_floor,
                "single_source_candidate_edges": single_candidate_edges,
                "projection_drift_retention": retention.0,
                "projection_drift_files_compared": retention.1,
            }),
        );
    }

    let mut per_language_pair = serde_json::Map::new();
    for (pair, count) in &candidate_cross {
        let kept = kept_cross.get(pair).copied().unwrap_or(0);
        per_language_pair.insert(
            pair.clone(),
            json!({ "candidate_edges": count, "kept_edges": kept }),
        );
    }

    Ok(json!({
        "repo": merged.repo,
        "sources": sorted_sources.iter().map(|(pkg, lang)| json!({"pkg": pkg, "lang": lang.as_str()})).collect::<Vec<_>>(),
        "files_total": merged.nodes.len(),
        "candidate_edges_total": candidate_edges,
        "kept_edges_total": pruned.edges.len(),
        "signal_cross_language_mass": signal_cross_mass,
        "per_language": per_language,
        "per_language_pair": per_language_pair,
        "clustering_vs_language": clustering_vs_language,
        "resolution": resolution,
        "prune_variant": prune_variant.to_string(),
        "prune_keep_per_node": pipeline::PRUNE_KEEP_PER_NODE,
        "prune_floor": merged_prune_stats.floor,
        "prune_floor_basis": merged_prune_stats.floor_basis,
    }))
}

fn file_language_map(data: &GraphData) -> Result<BTreeMap<String, LanguageKind>> {
    data.nodes
        .iter()
        .map(|node| Ok((node.file.clone(), LanguageKind::parse(&node.lang)?)))
        .collect()
}

/// Splits an edge iterator into per-language intra-language counts (keyed by
/// language string) and per-unordered-pair cross-language counts (keyed by
/// `"{lower} + {higher}"`, sorted so `"go+ts"` and `"ts+go"` never both
/// appear).
fn split_intra_cross<'a>(
    edges: impl Iterator<Item = (&'a str, &'a str)>,
    file_language: &BTreeMap<String, LanguageKind>,
) -> (BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let mut intra = BTreeMap::new();
    let mut cross = BTreeMap::new();
    for (a, b) in edges {
        let la = file_language[a];
        let lb = file_language[b];
        if la == lb {
            *intra.entry(la.as_str().to_owned()).or_insert(0) += 1;
        } else {
            let (low, high) = if la.as_str() <= lb.as_str() {
                (la.as_str(), lb.as_str())
            } else {
                (lb.as_str(), la.as_str())
            };
            *cross.entry(format!("{low}+{high}")).or_insert(0) += 1;
        }
    }
    (intra, cross)
}

/// Per-signal raw mass restricted to cross-language edges, both as a raw sum
/// and as a share of that signal's total raw mass across every candidate
/// edge (intra- and cross-language). This is the number that settles
/// whether "co-change is the only signal that bridges languages"
/// (`docs/ARCHITECTURE.md`) is true: static is architecturally 0
/// cross-language by construction (see `extract::union_sources`), but
/// nothing stops proximity (a path-prefix ratio) or semantic (IDF over a
/// vocabulary pooled across every source) from crossing too.
fn signal_cross_language_mass(
    data: &GraphData,
    file_language: &BTreeMap<String, LanguageKind>,
) -> Value {
    type SignalSelector = fn(&crate::schema::SignalEdge) -> f64;
    let signals: [(&str, SignalSelector); 4] = [
        ("static", |e| e.static_signal),
        ("cochange", |e| e.cochange),
        ("proximity", |e| e.proximity),
        ("semantic", |e| e.semantic),
    ];
    let mut result = serde_json::Map::new();
    for (name, select) in signals {
        let mut total = 0.0_f64;
        let mut cross = 0.0_f64;
        for edge in &data.edges {
            let value = select(edge);
            total += value;
            if file_language[&edge.a] != file_language[&edge.b] {
                cross += value;
            }
        }
        let share = if total > 0.0 { cross / total } else { 0.0 };
        result.insert(
            name.to_owned(),
            json!({ "raw_cross_language_mass": cross, "share_of_signal_total": share }),
        );
    }
    Value::Object(result)
}

/// Share of a language's own intra-language candidate edges classified below
/// the selected route's floor before the top-N cap is applied.
fn below_floor_share_per_language(
    candidates: &GraphData,
    stats: &PruneStats,
    file_language: &BTreeMap<String, LanguageKind>,
    languages: &BTreeSet<LanguageKind>,
) -> BTreeMap<String, f64> {
    let mut result = BTreeMap::new();
    for &language in languages {
        let intra = candidates
            .edges
            .iter()
            .enumerate()
            .filter(|(_, edge)| {
                file_language[&edge.a] == language && file_language[&edge.b] == language
            })
            .collect::<Vec<_>>();
        let share = if intra.is_empty() {
            0.0
        } else {
            intra
                .iter()
                .filter(|(index, _edge)| stats.below_floor_edges[*index])
                .count() as f64
                / intra.len() as f64
        };
        result.insert(language.as_str().to_owned(), share);
    }
    result
}

/// The fraction of `lang_files` whose merged-map district matches (by
/// best-Jaccard overlap, `src/parity.rs`'s own matching) the district that
/// language's own single-source map assigns -- finding 4's retention
/// measure, applied to a (merged graph, single-source graph) pair instead of
/// a (warm start, cold start) pair. Returns `(retention, files_compared)`.
fn projection_drift(
    merged_membership: &BTreeMap<String, usize>,
    single_membership: &BTreeMap<String, usize>,
    lang_files: &BTreeSet<&str>,
) -> (f64, usize) {
    if lang_files.is_empty() {
        return (0.0, 0);
    }
    let matches = match_districts(merged_membership, single_membership, lang_files);
    let kept = lang_files
        .iter()
        .filter(|file| {
            let merged_district = merged_membership[**file];
            let single_district = single_membership[**file];
            matches
                .get(&merged_district)
                .is_some_and(|(target, _jaccard)| *target == single_district)
        })
        .count();
    (kept as f64 / lang_files.len() as f64, lang_files.len())
}

/// NMI, adjusted Rand, and the share of files in a district that is >= 90%
/// one language, between the merged map's district membership and the
/// per-file language label. Near 1.0 (NMI/ARI) means the map redrew the
/// file extension rather than finding real cross-language structure --
/// `docs/ARCHITECTURE.md`'s polyglot paragraph argued districts *should*
/// cross languages; this is whether they actually do, on top of modularity
/// (which looks good in exactly the redrew-the-extension case too, so it
/// cannot be the acceptance gate for polyglot on its own -- see finding 13).
fn clustering_vs_language_stats(
    membership: &[usize],
    files_in_order: &[String],
    file_language: &BTreeMap<String, LanguageKind>,
    languages: &BTreeSet<LanguageKind>,
) -> Value {
    let language_index = languages
        .iter()
        .enumerate()
        .map(|(index, &language)| (language, index))
        .collect::<BTreeMap<_, _>>();
    let language_labels = files_in_order
        .iter()
        .map(|file| language_index[&file_language[file]])
        .collect::<Vec<_>>();

    let nmi = normalized_mutual_information(membership, &language_labels);
    let ari = adjusted_rand_index(membership, &language_labels);

    let mut by_district = BTreeMap::<usize, BTreeMap<LanguageKind, usize>>::new();
    for (file, &district) in files_in_order.iter().zip(membership) {
        *by_district
            .entry(district)
            .or_default()
            .entry(file_language[file])
            .or_insert(0) += 1;
    }
    let total_files = files_in_order.len().max(1);
    let mut files_in_dominant_districts = 0usize;
    for counts in by_district.values() {
        let district_total: usize = counts.values().sum();
        let dominant = counts.values().copied().max().unwrap_or(0);
        if district_total > 0 && dominant as f64 / district_total as f64 >= 0.9 {
            files_in_dominant_districts += district_total;
        }
    }
    let share_dominant = files_in_dominant_districts as f64 / total_files as f64;

    json!({
        "nmi": nmi,
        "adjusted_rand": ari,
        "districts": by_district.len(),
        "share_files_in_districts_over_90pct_one_language": share_dominant,
    })
}

/// Normalized mutual information between two label vectors of equal length,
/// arithmetic-mean normalisation (`2*MI / (H(a) + H(b))`, natural log
/// throughout -- the base cancels in the ratio). Matches scikit-learn's
/// `normalized_mutual_info_score(..., average_method="arithmetic")`
/// convention, including its degenerate case: both label vectors constant
/// (H(a) == H(b) == 0) is defined as perfect agreement, 1.0, rather than a
/// 0/0 division.
fn normalized_mutual_information(a: &[usize], b: &[usize]) -> f64 {
    let n = a.len();
    if n == 0 {
        return 0.0;
    }
    let (row_sums, col_sums, joint) = contingency(a, b);
    let n_f = n as f64;
    let mut mutual_information = 0.0_f64;
    for (&(i, j), &n_ij) in &joint {
        if n_ij == 0 {
            continue;
        }
        let n_ij = n_ij as f64;
        let n_i = row_sums[&i] as f64;
        let n_j = col_sums[&j] as f64;
        mutual_information += (n_ij / n_f) * ((n_ij * n_f) / (n_i * n_j)).ln();
    }
    let h_a = entropy(&row_sums, n_f);
    let h_b = entropy(&col_sums, n_f);
    let normalizer = (h_a + h_b) / 2.0;
    if normalizer <= 1e-12 {
        return 1.0; // both label vectors constant: trivially "matched"
    }
    (mutual_information / normalizer).clamp(0.0, 1.0)
}

fn entropy(sums: &BTreeMap<usize, usize>, n: f64) -> f64 {
    -sums
        .values()
        .map(|&count| {
            let p = count as f64 / n;
            if p > 0.0 {
                p * p.ln()
            } else {
                0.0
            }
        })
        .sum::<f64>()
}

/// Contingency table: (row sums keyed by `a`'s label, column sums keyed by
/// `b`'s label, joint counts keyed by `(a_label, b_label)`).
type Contingency = (
    BTreeMap<usize, usize>,
    BTreeMap<usize, usize>,
    BTreeMap<(usize, usize), usize>,
);

fn contingency(a: &[usize], b: &[usize]) -> Contingency {
    let mut row_sums = BTreeMap::new();
    let mut col_sums = BTreeMap::new();
    let mut joint = BTreeMap::new();
    for (&x, &y) in a.iter().zip(b) {
        *row_sums.entry(x).or_insert(0) += 1;
        *col_sums.entry(y).or_insert(0) += 1;
        *joint.entry((x, y)).or_insert(0) += 1;
    }
    (row_sums, col_sums, joint)
}

fn choose2(n: usize) -> f64 {
    if n < 2 {
        0.0
    } else {
        (n * (n - 1)) as f64 / 2.0
    }
}

/// Adjusted Rand index between two label vectors of equal length -- the
/// standard Hubert-Arabie formula. Degenerate case (the denominator is ~0,
/// which happens when at least one of the two partitions has every item in
/// its own singleton cluster, or both are one single cluster): defined as
/// 1.0, matching scikit-learn's `adjusted_rand_score` special case, rather
/// than a division by ~0.
fn adjusted_rand_index(a: &[usize], b: &[usize]) -> f64 {
    let n = a.len();
    if n == 0 {
        return 1.0;
    }
    let (row_sums, col_sums, joint) = contingency(a, b);
    let sum_joint_choose2 = joint.values().map(|&n_ij| choose2(n_ij)).sum::<f64>();
    let sum_row_choose2 = row_sums.values().map(|&n_i| choose2(n_i)).sum::<f64>();
    let sum_col_choose2 = col_sums.values().map(|&n_j| choose2(n_j)).sum::<f64>();
    let total_choose2 = choose2(n);
    if total_choose2 == 0.0 {
        return 1.0;
    }
    let expected_index = sum_row_choose2 * sum_col_choose2 / total_choose2;
    let max_index = 0.5 * (sum_row_choose2 + sum_col_choose2);
    let denominator = max_index - expected_index;
    if denominator.abs() <= 1e-12 {
        return 1.0;
    }
    (sum_joint_choose2 - expected_index) / denominator
}

fn summarize(report: &Value) -> String {
    let repo = report["repo"].as_str().unwrap_or("?");
    let files = report["files_total"].as_u64().unwrap_or(0);
    let candidates = report["candidate_edges_total"].as_u64().unwrap_or(0);
    let kept = report["kept_edges_total"].as_u64().unwrap_or(0);
    let nmi = report["clustering_vs_language"]["nmi"]
        .as_f64()
        .unwrap_or(0.0);
    let ari = report["clustering_vs_language"]["adjusted_rand"]
        .as_f64()
        .unwrap_or(0.0);
    let dominant_share = report["clustering_vs_language"]
        ["share_files_in_districts_over_90pct_one_language"]
        .as_f64()
        .unwrap_or(0.0);

    let mut lines = vec![format!(
        "{repo}: {files} files, {candidates} candidate edges -> {kept} kept after prune"
    )];
    lines.push(format!(
        "  membership vs language: NMI {nmi:.3}, adjusted Rand {ari:.3}, {:.1}% of files in a >=90%-one-language district",
        dominant_share * 100.0
    ));
    if let Some(signals) = report["signal_cross_language_mass"].as_object() {
        let mut signal_line = String::from("  cross-language share of each signal's own mass: ");
        let mut parts = Vec::new();
        for name in ["static", "cochange", "proximity", "semantic"] {
            if let Some(entry) = signals.get(name) {
                let share = entry["share_of_signal_total"].as_f64().unwrap_or(0.0);
                parts.push(format!("{name} {:.1}%", share * 100.0));
            }
        }
        signal_line.push_str(&parts.join(", "));
        lines.push(signal_line);
    }
    if let Some(per_language) = report["per_language"].as_object() {
        for (lang, stats) in per_language {
            let files = stats["files"].as_u64().unwrap_or(0);
            let below_merged = stats["below_prune_floor_merged"].as_f64().unwrap_or(0.0);
            let below_single = stats["below_prune_floor_single_source"]
                .as_f64()
                .unwrap_or(0.0);
            let retention = stats["projection_drift_retention"].as_f64().unwrap_or(0.0);
            lines.push(format!(
                "  {lang}: {files} files, below-floor {:.1}% merged vs {:.1}% single-source, projection-drift retention {:.1}%",
                below_merged * 100.0,
                below_single * 100.0,
                retention * 100.0
            ));
        }
    }
    if let Some(pairs) = report["per_language_pair"].as_object() {
        for (pair, stats) in pairs {
            let candidate = stats["candidate_edges"].as_u64().unwrap_or(0);
            let kept = stats["kept_edges"].as_u64().unwrap_or(0);
            lines.push(format!(
                "  {pair}: {candidate} candidate cross-language edges, {kept} kept"
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nmi_and_ari_are_one_for_identical_partitions() {
        let a = vec![0, 0, 1, 1, 2, 2];
        assert!((normalized_mutual_information(&a, &a) - 1.0).abs() < 1e-9);
        assert!((adjusted_rand_index(&a, &a) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn nmi_and_ari_are_near_zero_for_a_constant_label_against_a_real_partition() {
        // One vector is all-one-cluster (no information at all); the other
        // is a real 3-way split. Mutual information must be exactly 0
        // (a constant label carries no information about anything), so NMI
        // and ARI both come out 0, not the degenerate 1.0 the
        // both-constant case gets.
        let language = vec![0, 0, 0, 0, 0, 0]; // everything one language
        let membership = vec![0, 0, 1, 1, 2, 2]; // three real districts
        assert!(normalized_mutual_information(&membership, &language).abs() < 1e-9);
        assert!(adjusted_rand_index(&membership, &language).abs() < 1e-9);
    }

    #[test]
    fn nmi_and_ari_are_one_when_both_labels_are_constant() {
        let a = vec![0, 0, 0, 0];
        let b = vec![0, 0, 0, 0];
        assert!((normalized_mutual_information(&a, &b) - 1.0).abs() < 1e-9);
        assert!((adjusted_rand_index(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn nmi_and_ari_report_high_agreement_when_membership_exactly_tracks_language() {
        // Districts happen to line up one-to-one with language -- the "the
        // map redrew the file extension" case finding 13 is checking for.
        let language = vec![0, 0, 0, 1, 1, 1];
        let membership = vec![5, 5, 5, 9, 9, 9]; // different ids, same grouping
        assert!((normalized_mutual_information(&membership, &language) - 1.0).abs() < 1e-9);
        assert!((adjusted_rand_index(&membership, &language) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn choose2_of_zero_or_one_is_zero() {
        assert_eq!(choose2(0), 0.0);
        assert_eq!(choose2(1), 0.0);
        assert_eq!(choose2(4), 6.0);
    }

    #[test]
    fn split_intra_cross_buckets_edges_by_language_pair() {
        let mut file_language = BTreeMap::new();
        file_language.insert("a.go".to_owned(), LanguageKind::Go);
        file_language.insert("b.go".to_owned(), LanguageKind::Go);
        file_language.insert("c.ts".to_owned(), LanguageKind::TypeScript);
        let edges = vec![("a.go", "b.go"), ("a.go", "c.ts"), ("b.go", "c.ts")];
        let (intra, cross) = split_intra_cross(edges.into_iter(), &file_language);
        assert_eq!(intra.get("go"), Some(&1));
        assert_eq!(cross.get("go+ts"), Some(&2));
    }
}
