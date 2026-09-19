//! Milestone-1 acceptance gate: does a candidate map reproduce a reference
//! map closely enough to ship?
//!
//! Two numbers are gated, per `docs/ARCHITECTURE.md`'s "one real risk" and
//! `CLAUDE.md`'s "checks before a change lands":
//!
//! - **District placement**: the fraction of files the candidate puts in the
//!   district the reference assigns. Must be >= [`PLACEMENT_THRESHOLD`].
//! - **Modularity delta**: `|candidate.q - reference.q|`. Must be <=
//!   [`MODULARITY_THRESHOLD`].
//!
//! District ids are an artefact of one clustering run (finding 4), not a
//! stable label -- the candidate's district 3 has no reason to mean the same
//! thing as the reference's district 3. So placement is measured by first
//! matching candidate districts onto reference districts by best Jaccard
//! overlap, greedily, above a 0.35 threshold. This is the exact algorithm
//! `eval/stability.py`'s `match_districts` and `eval/batch_stability.py` use
//! to measure warm-start retention (finding 4's 46% -> 88% table) -- reusing
//! it here means "did the port reproduce the reference" and "did warm-starting
//! hold the map still" are the same question asked of two different pairs of
//! runs, not two different metrics that happen to sound alike.
//!
//! Alongside the two gated numbers, the structural fields that should be
//! byte-identical (not just close) are compared and reported: `F` (the file
//! list), `E` (the edge list), `L` (landmarks) and the symbol tables `S`/`U`.
//! These aren't gated with a threshold because they aren't allowed any slack
//! -- `docs/ARCHITECTURE.md` states the port reproduces `F` and `E` exactly
//! on all nine fixtures, so any difference here is a regression, not noise.
//!
//! Coordinates (`District.c`/`.blob`, `NodeRow`'s point/rect fields, `roads`,
//! `P`) are float geometry produced by a spring layout and a squarified
//! treemap seeded from the (possibly differently-numbered) districts -- they
//! are not expected to match even when everything else does, and this report
//! does not gate them. Says so explicitly rather than silently omitting them,
//! per "an unverified claim is worse than a stated gap."

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::schema::MapDocument;

/// District placement must meet this fraction to pass (`docs/ARCHITECTURE.md`,
/// `CLAUDE.md`: "the port must still place >= 95% of files in the district
/// the reference assigns").
pub const PLACEMENT_THRESHOLD: f64 = 0.95;

/// Modularity must land within this absolute distance of the reference's
/// (`CLAUDE.md`: "with modularity within 0.02").
pub const MODULARITY_THRESHOLD: f64 = 0.02;

/// Below this Jaccard overlap, a candidate district is not considered a
/// match for any reference district -- mirrors `eval/stability.py`'s
/// `match_districts`, which exists to answer exactly this question for
/// warm-start retention.
const JACCARD_THRESHOLD: f64 = 0.35;

#[derive(Debug, Clone, PartialEq)]
pub struct FieldCheck {
    pub name: &'static str,
    pub matches: bool,
    pub detail: String,
}

pub struct ParityReport {
    pub repo: String,
    pub reference_files: usize,
    pub candidate_files: usize,
    pub common_files: usize,
    pub placement_fraction: f64,
    pub matched_districts: usize,
    pub reference_districts: usize,
    pub candidate_districts: usize,
    pub reference_modularity: f64,
    pub candidate_modularity: f64,
    pub fields: Vec<FieldCheck>,
}

impl ParityReport {
    /// Whether every gated check passed. Coordinate/geometry fields are
    /// intentionally excluded -- see the module doc.
    pub fn passed(&self) -> bool {
        self.placement_fraction >= PLACEMENT_THRESHOLD
            && self.modularity_delta() <= MODULARITY_THRESHOLD
            && self.fields.iter().all(|field| field.matches)
    }

    pub fn modularity_delta(&self) -> f64 {
        (self.candidate_modularity - self.reference_modularity).abs()
    }
}

impl fmt::Display for ParityReport {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(out, "parity: {}", self.repo)?;

        let placement_ok = self.placement_fraction >= PLACEMENT_THRESHOLD;
        writeln!(
            out,
            "  {} district placement: {:.1}% ({}/{} common files matched via {}/{} districts) [gate >= {:.0}%]",
            if placement_ok { "PASS" } else { "FAIL" },
            self.placement_fraction * 100.0,
            (self.placement_fraction * self.common_files as f64).round() as usize,
            self.common_files,
            self.matched_districts,
            self.reference_districts.min(self.candidate_districts),
            PLACEMENT_THRESHOLD * 100.0,
        )?;
        if self.reference_files != self.candidate_files || self.common_files != self.reference_files
        {
            writeln!(
                out,
                "       reference has {} files, candidate has {}, {} in common",
                self.reference_files, self.candidate_files, self.common_files
            )?;
        }

        let modularity_ok = self.modularity_delta() <= MODULARITY_THRESHOLD;
        writeln!(
            out,
            "  {} modularity: reference {:.4}, candidate {:.4}, delta {:.4} [gate <= {:.2}]",
            if modularity_ok { "PASS" } else { "FAIL" },
            self.reference_modularity,
            self.candidate_modularity,
            self.modularity_delta(),
            MODULARITY_THRESHOLD,
        )?;

        for field in &self.fields {
            writeln!(
                out,
                "  {} {}: {}",
                if field.matches { "PASS" } else { "FAIL" },
                field.name,
                field.detail,
            )?;
        }

        writeln!(
            out,
            "  coordinates (District.c/.blob, node xy/rect, roads, parcels): not gated -- float geometry, compared with tolerance if at all",
        )?;

        write!(
            out,
            "-> {}",
            if self.passed() {
                "parity passed"
            } else {
                "parity FAILED"
            }
        )
    }
}

pub fn compare_files(
    reference: &Path,
    candidate: &Path,
    // Reserved for a future naming-cache comparison (`naming.rs` scores
    // directory segments by idf against a shared corpus); no check in this
    // gate set needs it, so it is accepted and unused rather than dropped --
    // dropping it would change the CLI surface `main.rs` already commits to.
    _idf_names: Option<&Path>,
) -> Result<ParityReport> {
    let reference = load_map(reference)?;
    let candidate = load_map(candidate)?;

    let reference_membership = membership(&reference);
    let candidate_membership = membership(&candidate);

    let reference_files: BTreeSet<&str> = reference_membership.keys().map(String::as_str).collect();
    let candidate_files: BTreeSet<&str> = candidate_membership.keys().map(String::as_str).collect();
    let common_files: BTreeSet<&str> = reference_files
        .intersection(&candidate_files)
        .copied()
        .collect();

    let matches = match_districts(&candidate_membership, &reference_membership, &common_files);
    let matched_districts = matches.len();

    let kept = common_files
        .iter()
        .filter(|file| {
            let candidate_district = candidate_membership[**file];
            let reference_district = reference_membership[**file];
            matches
                .get(&candidate_district)
                .is_some_and(|(target, _jaccard)| *target == reference_district)
        })
        .count();
    let placement_fraction = if common_files.is_empty() {
        0.0
    } else {
        kept as f64 / common_files.len() as f64
    };

    let reference_districts = reference_membership.values().collect::<BTreeSet<_>>().len();
    let candidate_districts = candidate_membership.values().collect::<BTreeSet<_>>().len();

    let fields = vec![
        check_field(
            "F (file list)",
            &reference.files,
            &candidate.files,
            |value| format!("{} files", value.len()),
        ),
        check_field(
            "E (edge list)",
            &reference.edges,
            &candidate.edges,
            |value| format!("{} edges", value.len()),
        ),
        check_field(
            "L (landmarks)",
            &reference.landmarks,
            &candidate.landmarks,
            |value| format!("{} landmarks", value.len()),
        ),
        check_field(
            "S (symbol table)",
            &reference.symbols,
            &candidate.symbols,
            |value| format!("{} files with symbols", value.len()),
        ),
        check_field(
            "U (uses table)",
            &reference.uses,
            &candidate.uses,
            |value| format!("{} files with uses", value.len()),
        ),
    ];

    Ok(ParityReport {
        repo: reference.repo.clone(),
        reference_files: reference_files.len(),
        candidate_files: candidate_files.len(),
        common_files: common_files.len(),
        placement_fraction,
        matched_districts,
        reference_districts,
        candidate_districts,
        reference_modularity: reference.q,
        candidate_modularity: candidate.q,
        fields,
    })
}

fn load_map(path: &Path) -> Result<MapDocument> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse map document {}", path.display()))
}

/// file -> district id, in that document's own (arbitrary) numbering.
fn membership(document: &MapDocument) -> BTreeMap<String, usize> {
    document
        .files
        .iter()
        .zip(document.nodes.iter())
        .map(|(file, node)| (file.clone(), node.district()))
        .collect()
}

/// Best-Jaccard greedy match from candidate district id -> (reference
/// district id, jaccard), restricted to the common file set. Mirrors
/// `eval/stability.py::match_districts` field for field, including its tie
/// break (`pairs.sort(reverse=True)` over `(jaccard, candidate_id,
/// reference_id)`, so ties favour the larger id on both sides) -- this is
/// the established definition of "which district is this, now", already
/// relied on for the warm-start retention numbers in `docs/FINDINGS.md`
/// finding 4, and reused verbatim rather than re-derived.
fn match_districts(
    candidate: &BTreeMap<String, usize>,
    reference: &BTreeMap<String, usize>,
    common_files: &BTreeSet<&str>,
) -> BTreeMap<usize, (usize, f64)> {
    let mut candidate_groups: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    let mut reference_groups: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    for file in common_files {
        candidate_groups
            .entry(candidate[*file])
            .or_default()
            .insert(file);
        reference_groups
            .entry(reference[*file])
            .or_default()
            .insert(file);
    }

    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (&candidate_id, candidate_set) in &candidate_groups {
        for (&reference_id, reference_set) in &reference_groups {
            let intersection = candidate_set.intersection(reference_set).count();
            if intersection == 0 {
                continue;
            }
            let union = candidate_set.union(reference_set).count();
            let jaccard = intersection as f64 / union as f64;
            if jaccard > 0.0 {
                pairs.push((jaccard, candidate_id, reference_id));
            }
        }
    }
    pairs.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then(right.2.cmp(&left.2))
    });

    let mut used_candidate = BTreeSet::new();
    let mut used_reference = BTreeSet::new();
    let mut matched = BTreeMap::new();
    for (jaccard, candidate_id, reference_id) in pairs {
        if used_candidate.contains(&candidate_id) || used_reference.contains(&reference_id) {
            continue;
        }
        if jaccard < JACCARD_THRESHOLD {
            continue;
        }
        matched.insert(candidate_id, (reference_id, jaccard));
        used_candidate.insert(candidate_id);
        used_reference.insert(reference_id);
    }
    matched
}

fn check_field<T: PartialEq>(
    name: &'static str,
    reference: &T,
    candidate: &T,
    describe: impl Fn(&T) -> String,
) -> FieldCheck {
    let matches = reference == candidate;
    let detail = if matches {
        format!("identical ({})", describe(reference))
    } else {
        format!(
            "differs (reference: {}, candidate: {})",
            describe(reference),
            describe(candidate)
        )
    };
    FieldCheck {
        name,
        matches,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;

    #[test]
    fn identical_membership_matches_every_district_and_keeps_everything() {
        let reference: Map<String, usize> = [("a", 0), ("b", 0), ("c", 1)]
            .into_iter()
            .map(|(f, d)| (f.to_owned(), d))
            .collect();
        // Candidate uses different district ids for the same grouping.
        let candidate: Map<String, usize> = [("a", 9), ("b", 9), ("c", 2)]
            .into_iter()
            .map(|(f, d)| (f.to_owned(), d))
            .collect();
        let common: BTreeSet<&str> = reference.keys().map(String::as_str).collect();
        let matches = match_districts(&candidate, &reference, &common);
        assert_eq!(matches.get(&9), Some(&(0, 1.0)));
        assert_eq!(matches.get(&2), Some(&(1, 1.0)));
    }

    #[test]
    fn a_district_below_the_jaccard_floor_is_left_unmatched() {
        // Reference splits ten files into ten singleton districts; candidate
        // lumps all ten into one. Every candidate/reference pair overlaps at
        // exactly one file out of a ten-file union (jaccard 0.1), below the
        // 0.35 floor, so the candidate's one big district matches nothing.
        let files: Vec<String> = ('a'..='j').map(|c| c.to_string()).collect();
        let reference: Map<String, usize> = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.clone(), index))
            .collect();
        let candidate: Map<String, usize> = files.iter().map(|file| (file.clone(), 99)).collect();
        let common: BTreeSet<&str> = files.iter().map(String::as_str).collect();
        let matches = match_districts(&candidate, &reference, &common);
        assert!(
            matches.is_empty(),
            "expected no match below the jaccard floor, got {matches:?}"
        );
    }

    #[test]
    fn placement_counts_only_files_that_land_in_the_matched_reference_district() {
        // Candidate district 9 matches reference district 0 (perfect overlap
        // on {a, b}); file c sits in a different candidate district that
        // maps to nothing, so it should not count as kept even though c
        // happens to be in reference district 0 too.
        let reference: Map<String, usize> = [("a", 0), ("b", 0), ("c", 0), ("d", 1), ("e", 1)]
            .into_iter()
            .map(|(f, d)| (f.to_owned(), d))
            .collect();
        let candidate: Map<String, usize> = [("a", 9), ("b", 9), ("c", 7), ("d", 1), ("e", 1)]
            .into_iter()
            .map(|(f, d)| (f.to_owned(), d))
            .collect();
        let common: BTreeSet<&str> = reference.keys().map(String::as_str).collect();
        let matches = match_districts(&candidate, &reference, &common);
        let kept = common
            .iter()
            .filter(|file| {
                matches
                    .get(&candidate[**file])
                    .is_some_and(|(target, _)| *target == reference[**file])
            })
            .count();
        // a, b, d, e kept; c's district (7) has no match.
        assert_eq!(kept, 4);
    }
}
