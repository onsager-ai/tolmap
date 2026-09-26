//! Conservative stage estimates seeded from finding 36's dify worker timeline.
//! Completed jobs adjust each stage by a clipped ratio, so one pathological
//! run cannot replace the seed or make another repository's ETA collapse.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::progress::{ProgressValue, StageId};
use crate::worker::RepoFeatures;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimingRow {
    pub features: RepoFeatures,
    pub elapsed_s: f64,
    /// StageId::ALL order; None means the stage did not finish successfully.
    pub stage_s: Vec<Option<f64>>,
    /// The job child's own peak RSS, from `wait4` (`jobs::wait_with_peak`),
    /// in bytes. `None` on a platform `wait4` does not cover, or for a row
    /// written before #97 phase 0. `#[serde(default)]` so those old rows
    /// still deserialize.
    #[serde(default)]
    pub peak_rss_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum EtaBasis {
    Model,
    Rate,
    Blend,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS)]
pub struct Eta {
    pub low_s: f64,
    pub high_s: f64,
    pub basis: EtaBasis,
}

impl Eta {
    pub fn midpoint(self) -> f64 {
        (self.low_s + self.high_s) / 2.0
    }
}

/// Number of stages, for the fixed-size per-stage arrays below.
const STAGE_COUNT: usize = StageId::ALL.len();

/// Stored timing rows are in `StageId::ALL` order. Rows written before the
/// three `--refs scip` indexing stages existed (issue #110) have 18 entries,
/// and rows written before the install stage (#110 P1c) have 21; this
/// inserts the missing stages where they now sit, so every later stage
/// keeps its own observations instead of shifting onto a neighbour's.
pub fn upgrade_stage_layout(mut stage_s: Vec<Option<f64>>) -> Vec<Option<f64>> {
    const BEFORE_INDEXING: usize = 18;
    const BEFORE_INSTALL: usize = 21;
    if stage_s.len() == BEFORE_INDEXING {
        // IndexGo, IndexPy and IndexTs, right after Resolve.
        let at = StageId::Resolve.index();
        for _ in 0..3 {
            stage_s.insert(at, None);
        }
    }
    if stage_s.len() == BEFORE_INSTALL {
        stage_s.insert(StageId::Install.index() - 1, None);
    }
    stage_s
}

/// Seconds per mapped file (and a fixed start-up cost) for each indexer,
/// from finding 41's no-install runs on standard runners: scip-python took
/// 79 s on django's 851 files and 246 s on dify's 1,989; scip-go 3 s on
/// prometheus's 409; scip-typescript 315 s on n8n's 11,991, 20 s on vue's
/// 239 and 7 s on prometheus's 187-file UI. A seed only: completed jobs
/// refit it like every other stage.
fn index_seed(language: &str) -> (f64, f64) {
    match language {
        "py" => (2.0, 0.11),
        "go" => (1.0, 0.008),
        _ => (3.0, 0.03),
    }
}

/// Start-up seconds and seconds per TypeScript file for the sandboxed
/// dependency install (#110 P1c). Finding 41's only measured install is
/// dify's pnpm `--frozen-lockfile --ignore-scripts`, +23.0 s over 4,358
/// TypeScript files on a standard runner; the start-up share is the jail
/// and its self-test. A seed only, refit from completed jobs like the rest.
const INSTALL_SEED: (f64, f64) = (5.0, 0.004);

pub fn expected_passes(stage: StageId, features: &RepoFeatures) -> usize {
    if matches!(stage, StageId::Parse | StageId::Resolve) {
        features.languages.len().max(1)
    } else {
        1
    }
}

pub fn progress_total(value: &ProgressValue, features: &RepoFeatures) -> Option<u64> {
    let source_files: u64 = features.languages.values().map(|lang| lang.files).sum();
    if source_files > 0
        && matches!(
            value.stage,
            StageId::Parse | StageId::Resolve | StageId::Symbols | StageId::SymbolCards
        )
    {
        Some(value.total.unwrap_or(0).max(source_files))
    } else {
        value.total
    }
}

#[derive(Clone, Debug, Default)]
pub struct EtaModel {
    rows: Vec<TimingRow>,
}

// The 18 non-indexing values are finding 36's stage_finished durations in
// StageId::ALL order. Parse and resolve aggregate both selected source passes. Corpus
// build totals (django 6.14 s, dify 41.925 s, n8n 101.163 s) set the
// prior's broad range; they are not fabricated per-stage observations.
const DIFY_FILES: f64 = 6_347.0;
// The four zeros after resolve's 0.223 are the `--refs scip` indexing
// stages and the install stage between the Python and TypeScript indexers,
// which finding 36's hand-written timeline never ran; their seeds are
// `index_seed` and `INSTALL_SEED`, and only for a job that asked for SCIP.
const DIFY_STAGE_S: [f64; STAGE_COUNT] = [
    0.032, 0.0, 0.0, 0.0, 0.103, 26.913, 0.223, 0.0, 0.0, 0.0, 0.0, 0.382, 0.044, 0.219, 0.131,
    0.010, 1.613, 5.109, 0.021, 0.390, 5.549, 0.182,
];

impl EtaModel {
    pub fn from_rows(rows: Vec<TimingRow>) -> Self {
        Self {
            rows: rows.into_iter().rev().take(256).collect(),
        }
    }

    pub fn record(&mut self, row: TimingRow) {
        self.rows.push(row);
        if self.rows.len() > 256 {
            self.rows.remove(0);
        }
    }

    fn file_count(features: &RepoFeatures) -> Option<f64> {
        let count: u64 = features.languages.values().map(|v| v.files).sum();
        (count > 0).then_some(count as f64)
    }

    fn work_units(features: &RepoFeatures) -> Option<f64> {
        if features.languages.is_empty() {
            return None;
        }
        // Linear in files and source bytes. The byte coefficient is small
        // enough that one generated giant file cannot dominate file count.
        Some(
            features
                .languages
                .values()
                .map(|lang| 0.75 * lang.files as f64 + 0.25 * lang.bytes as f64 / 8_000.0)
                .sum(),
        )
    }

    fn seed_stage(stage: StageId, features: &RepoFeatures) -> f64 {
        if stage == StageId::Install {
            // The worker names a package manager only when the job may
            // install (a SCIP job with installs on) and the policy wants to:
            // a TypeScript workspace with a pnpm or npm lockfile.
            if features.refs.as_deref() != Some("scip") || features.install.is_none() {
                return 0.0;
            }
            let files = features.languages.get("ts").map_or(0, |lang| lang.files);
            return INSTALL_SEED.0 + INSTALL_SEED.1 * files as f64;
        }
        if let Some(language) = stage.indexed_language() {
            if features.refs.as_deref() != Some("scip") {
                return 0.0; // a hand-written build never starts this stage
            }
            let Some(files) = features.languages.get(language).map(|lang| lang.files) else {
                return 0.0; // no source of this language, nothing to index
            };
            let (start_up, per_file) = index_seed(language);
            return start_up + per_file * files as f64;
        }
        let seed = DIFY_STAGE_S[stage.index() - 1];
        if matches!(
            stage,
            StageId::CloneObjects | StageId::CloneDeltas | StageId::CloneCheckout
        ) {
            return 0.0; // nested in Clone; counting them again doubles clone time
        }
        let files = Self::work_units(features).unwrap_or_else(|| {
            // Clone bytes are an early, weak size proxy. The wide interval
            // below reflects how much it can differ from source-file count.
            features
                .clone_bytes
                .map(|v| (v as f64 / 40_000.0).clamp(100.0, 100_000.0))
                .unwrap_or(DIFY_FILES)
        });
        let scale = (files / DIFY_FILES).max(0.0);
        match stage {
            StageId::Clone => {
                // A cache hit/local clone cost ~0.03 s in finding 36. The
                // byte term is a deliberately rough remote-transfer proxy.
                seed + features.clone_bytes.unwrap_or(0) as f64 / 25_000_000.0
            }
            StageId::History => {
                seed * (0.3
                    + 0.7
                        * features
                            .commits
                            .map(|n| n as f64 / 4_000.0)
                            .unwrap_or(1.0)
                            .clamp(0.1, 8.0))
            }
            StageId::Detect | StageId::Naming | StageId::WriteMap | StageId::Write => {
                seed.max(0.01)
            }
            _ => seed * (0.15 + 0.85 * scale),
        }
    }

    pub fn stage(&self, stage: StageId, features: &RepoFeatures) -> f64 {
        let seed = Self::seed_stage(stage, features);
        if seed == 0.0 {
            return 0.0;
        }
        let mut sum = 3.0; // three seed observations keep early fits stable
        let mut weight = 3.0;
        for row in &self.rows {
            let Some(Some(observed)) = row.stage_s.get(stage.index() - 1) else {
                continue;
            };
            let predicted = Self::seed_stage(stage, &row.features);
            if predicted <= 0.0 || !observed.is_finite() || *observed < 0.0 {
                continue;
            }
            sum += (observed / predicted).clamp(0.5, 2.0);
            weight += 1.0;
        }
        seed * sum / weight
    }

    pub fn predict(
        &self,
        features: &RepoFeatures,
        done: &[bool; STAGE_COUNT],
        running: Option<(StageId, f64, Option<f64>, Option<f64>)>,
    ) -> Eta {
        let mut seconds = 0.0;
        let mut basis = EtaBasis::Model;
        for stage in StageId::ALL {
            if done[stage.index() - 1] {
                continue;
            }
            let predicted = self.stage(stage, features);
            if let Some((active, elapsed, rate_remaining, fraction)) = running {
                if stage == active {
                    let model_remaining = (predicted - elapsed).max(0.0);
                    if let Some(rate) =
                        rate_remaining.filter(|rate| rate.is_finite() && *rate >= 0.0)
                    {
                        let progress = fraction
                            .unwrap_or(elapsed / predicted.max(0.001))
                            .clamp(0.0, 1.0);
                        let weight = (0.15 + 0.85 * progress).min(0.95);
                        seconds += model_remaining * (1.0 - weight) + rate * weight;
                        basis = if weight >= 0.9 {
                            EtaBasis::Rate
                        } else {
                            EtaBasis::Blend
                        };
                    } else {
                        seconds += model_remaining;
                    }
                    continue;
                }
            }
            seconds += predicted;
        }
        let seen_files = self
            .rows
            .iter()
            .filter(|row| row.stage_s.iter().any(Option::is_some))
            .filter_map(|r| Self::file_count(&r.features))
            .fold(DIFY_FILES, f64::max);
        let file_extrapolation = Self::file_count(features)
            .map(|files| (files / seen_files).max(1.0).ln_1p())
            .unwrap_or(1.0);
        let seen_bytes = self
            .rows
            .iter()
            .filter(|row| row.stage_s.iter().any(Option::is_some))
            .filter_map(|r| {
                let bytes: u64 = r.features.languages.values().map(|lang| lang.bytes).sum();
                (bytes > 0).then_some(bytes as f64)
            })
            .fold(DIFY_FILES * 8_000.0, f64::max);
        let source_bytes: u64 = features.languages.values().map(|lang| lang.bytes).sum();
        let byte_extrapolation = if source_bytes > 0 {
            (source_bytes as f64 / seen_bytes).max(1.0).ln_1p()
        } else {
            0.0
        };
        let width = 0.45 + 0.35 * file_extrapolation.max(byte_extrapolation);
        let mut low_s = (seconds * (1.0 - width).max(0.15)).max(0.0);
        let mut high_s = seconds * (1.0 + width);
        if features.languages.is_empty() && features.clone_bytes.is_none() {
            // Nothing is known about the repository yet (a queued job). The
            // indexing stages seed per language, so with no languages they
            // cost nothing, and a `--refs scip` job (`TOLMAP_REFS=scip`;
            // hand is the default) would be quoted a hand build's range. Use
            // the whole-build range each path was measured at instead:
            // finding 36's hand corpus (django 6.14 s .. n8n 101.163 s), or
            // finding 44's `--refs scip` builds (prometheus 17.3 s .. n8n
            // 371.4 s, indexing included).
            let (fastest, slowest) = if features.refs.as_deref() == Some("scip") {
                (17.3, 371.4)
            } else {
                (6.14, 101.163)
            };
            low_s = low_s.min(fastest);
            high_s = high_s.max(slowest);
        }
        Eta {
            low_s,
            high_s,
            basis,
        }
    }
}

// ---------------------------------------------------------------------
// Memory: docs/WORKER_TIER.md #2.1 needs a predicted peak RSS next to the
// time model above, so class selection has something to compare against an
// agent's advertised memory. Phase 0 (this file) only builds and tests the
// model; nothing here reaches `JobSnapshot`, the API or a worker class --
// there is exactly one class today (docs/WORKER_TIER.md #10.1), so nothing
// consumes a prediction yet. It exists so evidence accrues from day one
// (#2.1's own words: "worth building in phase 0 ... the evidence for any
// later class decision").
//
// Shape: log(peak) against log(file count), fit separately per (reference
// mode, primary language) group, exactly as #2.1 asks. "Primary language" is
// the language with the most reported files; a tie is broken by picking the
// lexicographically first name, which is what falls out of scanning
// `RepoFeatures::languages` (a `BTreeMap`, so already name-ordered) and only
// replacing the running best on a strict `>` -- deterministic without a
// second sort.
//
// The seed curves below are fit from *per-band or per-repository medians*
// findings 18 and 44-46 report, not from a residual distribution over many
// repositories in one band (no such per-repository dataset is checked into
// this repository, only the aggregates in FINDINGS.md), so the "upper
// quantile" asked for is approximated rather than computed exactly: finding
// 18 is the one place with a p10-p90 spread alongside the median, so its
// spread is what stands in for a residual, applied to every seed as
// `UPPER_QUANTILE_MULTIPLIER` (see its own comment). This is a documented
// approximation, not a precise 90th percentile; it can be replaced once
// finished jobs give the model real per-repository observations, which is
// exactly what the refit loop below is for.

/// A finished job's own group (reference mode is `hand` when `refs` is
/// absent, matching every other place in this file and #2.1's own text) and
/// its primary language, or `None` when the worker has not reported
/// languages yet (a queued job).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MemoryGroup<'a> {
    scip: bool,
    language: Option<&'a str>,
}

impl<'a> MemoryGroup<'a> {
    fn of(features: &'a RepoFeatures) -> Self {
        let mut best: Option<(&str, u64)> = None;
        for (language, info) in &features.languages {
            if best.is_none_or(|(_, files)| info.files > files) {
                best = Some((language.as_str(), info.files));
            }
        }
        MemoryGroup {
            scip: features.refs.as_deref() == Some("scip"),
            language: best.map(|(language, _)| language),
        }
    }
}

/// finding 18's per-band medians, `(files, peak MiB)`: small, medium, large,
/// ultra. Reference-mode `hand`, no language split -- finding 18 measured
/// across the whole 132-repository corpus without separating by language,
/// so this is the only seed available for `hand`, applied regardless of
/// which language dominates a repository.
const HAND_ANCHORS: [(f64, f64); 4] = [
    (51.0, 20.8),
    (704.0, 114.2),
    (4_050.0, 519.8),
    (11_991.0, 1_786.7),
];

/// The geometric mean of finding 18's four per-band p90/median ratios:
/// 34.1/20.8 = 1.639, 254.5/114.2 = 2.228, 1266.9/519.8 = 2.438, and
/// 5305.8/1786.7 = 2.969 -> geometric mean 2.27. Applied once, after
/// blending in real observations against the *median* curve (see
/// `MemoryModel::predict_peak`), as the stand-in "upper quantile" described
/// in the module comment above: findings 44-46 give a single peak per
/// repository under `scip`, with no p10-p90 spread to draw its own
/// multiplier from, so reusing finding 18's is the closest measured
/// evidence rather than an invented number. Once finished `scip` rows
/// accumulate, the refit loop below moves the median from measurement; this
/// constant still sets how far above that median the prediction sits.
const UPPER_QUANTILE_MULTIPLIER: f64 = 2.27;

/// `--refs scip` Python peaks, `(files, peak MiB)`. Fixture-scale points are
/// finding 45's table (httpx, flask, rich, celery, scrapy, sqlalchemy --
/// sqlalchemy's indexer ran its full peak even though the map fell back to
/// hand on recall); the two largest points are finding 44's corpus-scale
/// figures for django and dify.
const SCIP_PY_ANCHORS: [(f64, f64); 8] = [
    (23.0, 443.0),
    (24.0, 397.0),
    (100.0, 724.0),
    (161.0, 1_401.0),
    (188.0, 1_155.0),
    (258.0, 3_901.0),
    (851.0, 4_904.0),
    (6_347.0, 6_914.0),
];

/// `--refs scip` Go peaks. Only prometheus is measured (finding 45, `409`/
/// `444` files indexed by directory recall 0.9969), so there is only one
/// point; `seed_bytes` anchors `HAND_ANCHORS`' own log-log slope through it
/// rather than inventing a second Go point to fit independently.
const SCIP_GO_ANCHOR: (f64, f64) = (444.0, 783.0);

/// `--refs scip` TypeScript peaks, `(files, peak MiB)`: vue (finding 45) and
/// n8n (finding 46, the `scip`-only peak, before its install attempt that
/// the registry-only egress policy refuses). dify's TypeScript peak
/// (finding 46) is not used here: dify's primary language is Python, so its
/// row belongs to the Python group above, and using its number here as well
/// would double-count one repository's cost across two language groups.
const SCIP_TS_ANCHORS: [(f64, f64); 2] = [(239.0, 684.0), (11_991.0, 8_082.0)];

/// A queued `scip` job with no languages yet: the median of finding 45's
/// nine `--refs scip` fixture peaks (397, 443, 684, 724, 1155, 1401, 3900,
/// 3901, 4904 MiB -> median 1155), times the same upper-quantile multiplier
/// every other seed gets. This is #2.1's "reference mode ... known at
/// admission" case: nothing about file count or language is known yet, so
/// the estimate cannot be file-count-derived the way every other group's is.
const SCIP_UNKNOWN_PEAK_MIB: f64 = 1_155.0;

/// `ru_maxrss` (`jobs::wait_with_peak`) converts to bytes exactly. The
/// anchor tables above read off finding 18's and findings 44-46's own
/// tables, both labeled "MB"; this model treats that unit as MiB (2^20
/// bytes) for this conversion, which is about 4.9% higher than a strict
/// decimal MB (10^6) reading would give -- a small, deliberate addition to
/// the same side as `UPPER_QUANTILE_MULTIPLIER`, not a rounding bug.
const MIB: f64 = 1_048_576.0;

/// Least-squares slope and intercept of `ln(peak)` on `ln(files)`, in natural
/// log. Mirrors finding 18's own method (Pearson r on log-log). With a
/// single anchor point there is no unique slope, so the caller supplies one
/// (`HAND_ANCHORS`' own slope, the only multi-point fit available) and this
/// just re-centers the intercept through that one point.
fn fit_loglog(points: &[(f64, f64)], fallback_slope: Option<f64>) -> (f64, f64) {
    if points.len() < 2 {
        let (x, y) = points[0];
        let slope = fallback_slope.unwrap_or(0.0);
        return (slope, y.ln() - slope * x.ln());
    }
    let n = points.len() as f64;
    let (sum_x, sum_y) = points
        .iter()
        .fold((0.0, 0.0), |(sx, sy), &(x, y)| (sx + x.ln(), sy + y.ln()));
    let (mean_x, mean_y) = (sum_x / n, sum_y / n);
    let (mut cov, mut var) = (0.0, 0.0);
    for &(x, y) in points {
        let (lx, ly) = (x.ln() - mean_x, y.ln() - mean_y);
        cov += lx * ly;
        var += lx * lx;
    }
    let slope = if var > 0.0 { cov / var } else { 0.0 };
    (slope, mean_y - slope * mean_x)
}

fn loglog_predict(points: &[(f64, f64)], fallback_slope: Option<f64>, files: f64) -> f64 {
    let (slope, intercept) = fit_loglog(points, fallback_slope);
    (slope * files.max(1.0).ln() + intercept).exp()
}

#[derive(Clone, Debug, Default)]
pub struct MemoryModel {
    rows: Vec<TimingRow>,
}

/// Real, matching observations needed before they collectively outweigh the
/// seed curve. Larger than `EtaModel::stage`'s three seed pseudo-observations
/// on purpose (#2.1: "used until a group has enough finished rows (pick a
/// threshold, e.g. 8, and say why)") -- an underestimated *time* costs a
/// stale progress bar; an underestimated *memory* prediction costs an OOM
/// and a rerun, so a memory group should need more corroborating evidence
/// before it can move the seed as far.
const MEMORY_SEED_WEIGHT: f64 = 8.0;

impl MemoryModel {
    pub fn from_rows(rows: Vec<TimingRow>) -> Self {
        Self {
            rows: rows.into_iter().rev().take(256).collect(),
        }
    }

    pub fn record(&mut self, row: TimingRow) {
        self.rows.push(row);
        if self.rows.len() > 256 {
            self.rows.remove(0);
        }
    }

    fn file_count(features: &RepoFeatures) -> Option<f64> {
        let count: u64 = features.languages.values().map(|v| v.files).sum();
        (count > 0).then_some(count as f64)
    }

    /// The *median* curve, in bytes, for a group and a file count -- no
    /// upper-quantile margin. Shared by `predict_peak`'s seed and its refit
    /// loop (both call sites the module comment above and #2.1 ask for), so
    /// a real observation's ratio is always taken against the same baseline
    /// the seed itself is built from. See the anchor tables above for what
    /// each branch is fit from.
    fn median_bytes(group: MemoryGroup<'_>, files: f64) -> f64 {
        let hand_slope = fit_loglog(&HAND_ANCHORS, None).0;
        let mib = if group.scip {
            match group.language {
                Some("py") => loglog_predict(&SCIP_PY_ANCHORS, None, files),
                Some("go") => loglog_predict(&[SCIP_GO_ANCHOR], Some(hand_slope), files),
                Some("ts") => loglog_predict(&SCIP_TS_ANCHORS, None, files),
                // No finding measures this language's SCIP indexer peak (or
                // none is known yet within a `scip` job): the hand curve is
                // the only measurement in hand, so it is used as a floor
                // rather than inventing a number for an unmeasured indexer.
                _ => loglog_predict(&HAND_ANCHORS, None, files),
            }
        } else {
            loglog_predict(&HAND_ANCHORS, None, files)
        };
        mib * MIB
    }

    /// The reference-mode median prior alone, for a queued job whose worker
    /// has not reported `features` yet (#2.1 point 4). `hand`'s prior
    /// reuses `median_bytes` at finding 18's own medium-band median file
    /// count -- the corpus's own middle, not a magic number -- so it never
    /// drifts from the same curve a known-file-count `hand` job gets.
    /// `scip` has no per-file-count curve to evaluate at an arbitrary point
    /// without also knowing a language, so it is `SCIP_UNKNOWN_PEAK_MIB`
    /// directly. Neither branch applies `UPPER_QUANTILE_MULTIPLIER`;
    /// `predict_peak` does, once, on whatever this returns.
    fn unknown_median(scip: bool) -> f64 {
        if scip {
            SCIP_UNKNOWN_PEAK_MIB * MIB
        } else {
            Self::median_bytes(
                MemoryGroup {
                    scip: false,
                    language: None,
                },
                HAND_ANCHORS[1].0, // the medium-band median file count
            )
        }
    }

    /// Predicted peak RSS in bytes: an upper bound meant to overestimate
    /// (#2.1: "it errs high on purpose"), never CLAUDE.md's lower-bound rule,
    /// which governs what a *map* reports, not this internal scheduling
    /// estimate (#2.1 says so explicitly).
    pub fn predict_peak(&self, features: &RepoFeatures) -> u64 {
        let group = MemoryGroup::of(features);
        let median = match Self::file_count(features) {
            Some(files) => Self::median_bytes(group, files),
            None => Self::unknown_median(group.scip),
        };
        let mut sum = MEMORY_SEED_WEIGHT;
        let mut weight = MEMORY_SEED_WEIGHT;
        for row in &self.rows {
            // Only a successfully finished job's peak is evidence: a failed
            // or cancelled row's `stage_s` is all `None` (see `worker_loop`'s
            // `TimingRow` construction), and a killed job's peak is a floor
            // on what it needed, not a measurement of what finishing would
            // have needed -- it must never *narrow* the prediction the way a
            // real completion can (mirrors `EtaModel`'s
            // `failed_job_does_not_narrow_extrapolation_interval`).
            if row.stage_s.iter().all(Option::is_none) {
                continue;
            }
            let Some(observed) = row.peak_rss_bytes.filter(|bytes| *bytes > 0) else {
                continue;
            };
            let row_group = MemoryGroup::of(&row.features);
            if row_group != group {
                continue;
            }
            let Some(row_files) = Self::file_count(&row.features) else {
                continue;
            };
            // Against the *median*, not `seed` -- a typical finished job's
            // peak sits near the median curve, not the upper-quantile one,
            // so a ratio taken against an already-multiplied seed would be
            // about 1/2.27 for a typical job, clamp to the 0.5 floor below,
            // and pull the blended prediction down to roughly half the
            // seed once enough rows accumulate -- silently discarding the
            // margin #2.1 asks for ("it errs high on purpose").
            let predicted_median = Self::median_bytes(row_group, row_files);
            if predicted_median <= 0.0 {
                continue;
            }
            sum += (observed as f64 / predicted_median).clamp(0.5, 2.0);
            weight += 1.0;
        }
        // The multiplier is applied exactly once, here, after blending real
        // observations against the median curve above -- see the comment
        // in the loop for what goes wrong if it is folded into `median`
        // (or an equivalent per-row value) before that blend instead.
        ((median * sum / weight) * UPPER_QUANTILE_MULTIPLIER)
            .round()
            .max(1.0) as u64
    }
}

/// Replay the exact worker event stream on a standard runner. Predictions use
/// only events already emitted at each checkpoint, never the final duration.
pub fn replay_timeline(path: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    use crate::worker::WorkerEvent;
    use anyhow::Context;
    let timeline: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let wall = timeline["elapsed_s"]
        .as_f64()
        .context("timeline elapsed_s")?;
    let events = timeline["events"].as_array().context("timeline events")?;
    let model = EtaModel::default();
    let mut features = RepoFeatures::default();
    let mut done = [false; STAGE_COUNT];
    let mut stage_succeeded = [false; STAGE_COUNT];
    let mut completed_passes = [0usize; STAGE_COUNT];
    let mut stage_offsets = [0u64; STAGE_COUNT];
    let mut stage_max = [0u64; STAGE_COUNT];
    let mut starts = [None::<f64>; STAGE_COUNT];
    let mut last_progress = [None::<(u64, f64)>; STAGE_COUNT];
    let mut ewma = [None::<f64>; STAGE_COUNT];
    let mut running = None::<(StageId, f64, Option<f64>, Option<f64>)>;
    let mut predictions = Vec::new();
    for value in events {
        let at = value["at_s"].as_f64().context("event at_s")?;
        let event: WorkerEvent = serde_json::from_value(value.clone())?;
        match event {
            WorkerEvent::Features { features: next, .. } => features = next,
            WorkerEvent::StageStarted { stage, .. } => {
                stage_succeeded[stage.index() - 1] = false;
                stage_offsets[stage.index() - 1] = stage_max[stage.index() - 1];
                starts[stage.index() - 1] = Some(at);
                let active = if matches!(
                    stage,
                    StageId::CloneObjects | StageId::CloneDeltas | StageId::CloneCheckout
                ) {
                    StageId::Clone
                } else {
                    stage
                };
                running = Some((
                    active,
                    starts[active.index() - 1].map_or(0.0, |start| at - start),
                    None,
                    None,
                ));
            }
            WorkerEvent::Progress { mut value, .. } => {
                let index = value.stage.index() - 1;
                value.done = value.done.saturating_add(stage_offsets[index]);
                value.total = value
                    .total
                    .map(|total| total.saturating_add(stage_offsets[index]));
                value.done = value.done.max(stage_max[index]);
                stage_max[index] = value.done;
                if let Some((last_done, last_at)) = last_progress[index] {
                    let dt = at - last_at;
                    if value.done > last_done && dt > 0.0 {
                        let instantaneous = (value.done - last_done) as f64 / dt;
                        ewma[index] = Some(
                            ewma[index]
                                .map_or(instantaneous, |old| 0.35 * instantaneous + 0.65 * old),
                        );
                    }
                }
                last_progress[index] = Some((value.done, at));
                let rate = ewma[index].or(value.rate_per_s).filter(|rate| *rate > 0.0);
                let effective_total = progress_total(&value, &features);
                let remaining = effective_total.and_then(|total| {
                    rate.map(|rate| total.saturating_sub(value.done) as f64 / rate)
                });
                let active = if matches!(
                    value.stage,
                    StageId::CloneObjects | StageId::CloneDeltas | StageId::CloneCheckout
                ) {
                    StageId::Clone
                } else {
                    value.stage
                };
                let fraction = effective_total
                    .filter(|total| *total > 0)
                    .map(|total| value.done as f64 / total as f64);
                running = Some((
                    active,
                    starts[active.index() - 1].map_or(0.0, |start| at - start),
                    remaining,
                    fraction,
                ));
            }
            WorkerEvent::StageFinished { stage, success, .. } => {
                stage_succeeded[stage.index() - 1] = success;
                if success {
                    completed_passes[stage.index() - 1] += 1;
                }
                if running.is_some_and(|(active, _, _, _)| active == stage) {
                    running = None;
                }
            }
            WorkerEvent::Result { .. }
            | WorkerEvent::Error { .. }
            | WorkerEvent::Log { .. }
            | WorkerEvent::InstallRequest { .. } => {}
        }
        if at < wall && !matches!(value["type"].as_str(), Some("result" | "error")) {
            for stage in StageId::ALL {
                done[stage.index() - 1] = stage_succeeded[stage.index() - 1]
                    && completed_passes[stage.index() - 1] >= expected_passes(stage, &features);
            }
            let prediction = model.predict(
                &features,
                &done,
                running.map(|(stage, _, remaining, fraction)| {
                    (
                        stage,
                        starts[stage.index() - 1].map_or(0.0, |start| at - start),
                        remaining,
                        fraction,
                    )
                }),
            );
            predictions.push((at, prediction.midpoint()));
        }
    }
    let mut checkpoints = serde_json::Map::new();
    for (label, fraction) in [("10", 0.1), ("50", 0.5), ("90", 0.9)] {
        let target = wall * fraction;
        let predicted = predictions
            .iter()
            .rev()
            .find(|(at, _)| *at <= target)
            .or_else(|| predictions.first())
            .map(|(_, midpoint)| *midpoint)
            .context("no prediction before terminal event")?;
        let actual = wall - target;
        checkpoints.insert(
            label.to_owned(),
            serde_json::json!({
                "at_s": target, "predicted_remaining_s": predicted,
                "actual_remaining_s": actual, "absolute_error_s": (predicted - actual).abs()
            }),
        );
    }
    Ok(serde_json::json!({"elapsed_s": wall, "checkpoints": checkpoints}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::LanguageFeatures;

    fn features(files: u64) -> RepoFeatures {
        RepoFeatures {
            clone_bytes: Some(files * 40_000),
            commits: Some(4_000),
            languages: [(
                "py".to_owned(),
                LanguageFeatures {
                    files,
                    bytes: files * 8_000,
                },
            )]
            .into(),
            refs: None,
            install: None,
        }
    }

    #[test]
    fn legacy_timing_rows_gain_empty_indexing_stages() {
        let legacy = (0..18).map(|i| Some(i as f64)).collect::<Vec<_>>();
        let upgraded = upgrade_stage_layout(legacy);
        assert_eq!(upgraded.len(), StageId::ALL.len());
        assert_eq!(upgraded[StageId::Resolve.index() - 1], Some(6.0));
        assert_eq!(upgraded[StageId::IndexGo.index() - 1], None);
        assert_eq!(upgraded[StageId::Install.index() - 1], None);
        assert_eq!(upgraded[StageId::IndexTs.index() - 1], None);
        assert_eq!(upgraded[StageId::History.index() - 1], Some(7.0));
        assert_eq!(upgraded[StageId::Write.index() - 1], Some(17.0));
        let current = vec![Some(1.0); StageId::ALL.len()];
        assert_eq!(upgrade_stage_layout(current.clone()), current);
    }

    #[test]
    fn rows_from_before_the_install_stage_keep_their_indexing_times() {
        // A P1a-era row: 21 stages, IndexPy at 8 and IndexTs at 9 (0-based).
        let before = (0..21).map(|i| Some(i as f64)).collect::<Vec<_>>();
        let upgraded = upgrade_stage_layout(before);
        assert_eq!(upgraded.len(), StageId::ALL.len());
        assert_eq!(upgraded[StageId::IndexPy.index() - 1], Some(8.0));
        assert_eq!(upgraded[StageId::Install.index() - 1], None);
        assert_eq!(upgraded[StageId::IndexTs.index() - 1], Some(9.0));
        assert_eq!(upgraded[StageId::Write.index() - 1], Some(20.0));
    }

    #[test]
    fn install_is_estimated_only_when_the_worker_plans_one() {
        let mut input = features(10);
        input.languages.insert(
            "ts".to_owned(),
            LanguageFeatures {
                files: 4_358,
                bytes: 4_358 * 8_000,
            },
        );
        let model = EtaModel::default();
        input.refs = Some("scip".to_owned());
        assert_eq!(model.stage(StageId::Install, &input), 0.0);
        input.install = Some("pnpm".to_owned());
        let install = model.stage(StageId::Install, &input);
        assert!(install > 15.0 && install < 30.0, "{install}");
        input.refs = None;
        assert_eq!(model.stage(StageId::Install, &input), 0.0);
    }

    #[test]
    fn indexing_is_estimated_only_for_scip_jobs() {
        let mut input = features(851);
        let model = EtaModel::default();
        assert_eq!(model.stage(StageId::IndexPy, &input), 0.0);
        input.refs = Some("scip".to_owned());
        let python = model.stage(StageId::IndexPy, &input);
        assert!(python > 60.0 && python < 120.0, "{python}");
        assert_eq!(model.stage(StageId::IndexGo, &input), 0.0);
    }

    // #110 P2a: a queued job knows only the service's reference mode. On the
    // hand default nothing changes; under `TOLMAP_REFS=scip` the prior must
    // cover finding 44's indexing-inclusive build times, not the hand range.
    #[test]
    fn unknown_repository_prior_covers_scip_indexing() {
        let model = EtaModel::default();
        let hand = model.predict(&RepoFeatures::default(), &[false; STAGE_COUNT], None);
        let scip = model.predict(
            &RepoFeatures {
                refs: Some("scip".to_owned()),
                ..RepoFeatures::default()
            },
            &[false; STAGE_COUNT],
            None,
        );
        assert!(hand.high_s >= 101.163 && hand.high_s < 371.4, "{hand:?}");
        assert!(scip.high_s >= 371.4, "{scip:?}");
        assert!(scip.midpoint() > hand.midpoint());
    }

    #[test]
    fn bounded_refit_and_extrapolation() {
        let input = features(6_347);
        let base = EtaModel::default().stage(StageId::Parse, &input);
        let mut model = EtaModel::default();
        let mut stage_s = vec![None; StageId::ALL.len()];
        stage_s[StageId::Parse.index() - 1] = Some(base * 1.5);
        model.record(TimingRow {
            features: input.clone(),
            elapsed_s: 50.0,
            stage_s,
            peak_rss_bytes: None,
        });
        assert!(model.stage(StageId::Parse, &input) > base);
        assert!(model.stage(StageId::Parse, &input) < base * 1.5);
        let near = model.predict(&input, &[false; STAGE_COUNT], None);
        let far = model.predict(&features(100_000), &[false; STAGE_COUNT], None);
        assert!(
            (far.high_s - far.low_s) / far.midpoint()
                > (near.high_s - near.low_s) / near.midpoint()
        );
    }

    #[test]
    fn progress_rate_blends_into_remaining_time() {
        let model = EtaModel::default();
        let input = features(6_347);
        let mut done = [true; STAGE_COUNT];
        done[StageId::Parse.index() - 1] = false;
        let model_only = model.predict(&input, &done, Some((StageId::Parse, 5.0, None, None)));
        let blended = model.predict(
            &input,
            &done,
            Some((StageId::Parse, 5.0, Some(1.0), Some(0.8))),
        );
        assert!(blended.midpoint() < model_only.midpoint());
    }

    #[test]
    fn repeated_parse_pass_keeps_unvisited_language_in_eta() {
        let mut input = features(100);
        input.languages.insert(
            "ts".to_owned(),
            LanguageFeatures {
                files: 200,
                bytes: 1_600_000,
            },
        );
        let progress = ProgressValue {
            stage: StageId::Parse,
            stage_index: StageId::Parse.index(),
            stage_count: 18,
            label: "Parsing files".to_owned(),
            unit: "files".to_owned(),
            done: 100,
            total: Some(100),
            rate_per_s: Some(50.0),
            transfer_bytes: None,
            transfer_rate_bytes_per_s: None,
        };
        assert_eq!(expected_passes(StageId::Parse, &input), 2);
        assert_eq!(progress_total(&progress, &input), Some(300));
    }

    #[test]
    fn failed_job_does_not_narrow_extrapolation_interval() {
        let input = features(100_000);
        let mut model = EtaModel::default();
        let before = model.predict(&input, &[false; STAGE_COUNT], None);
        model.record(TimingRow {
            features: input.clone(),
            elapsed_s: 12.0,
            stage_s: vec![None; StageId::ALL.len()],
            peak_rss_bytes: None,
        });
        let after = model.predict(&input, &[false; STAGE_COUNT], None);
        assert!((before.low_s - after.low_s).abs() < 0.001);
        assert!((before.high_s - after.high_s).abs() < 0.001);
    }

    fn scip_py(files: u64) -> RepoFeatures {
        let mut input = features(files);
        input.refs = Some("scip".to_owned());
        input
    }

    fn finished_row(features: RepoFeatures, peak: u64) -> TimingRow {
        TimingRow {
            features,
            elapsed_s: 10.0,
            stage_s: vec![Some(1.0); StageId::ALL.len()],
            peak_rss_bytes: Some(peak),
        }
    }

    #[test]
    fn memory_seed_with_no_rows_is_positive_and_scip_exceeds_hand() {
        let model = MemoryModel::default();
        let hand = model.predict_peak(&features(851));
        let scip = model.predict_peak(&scip_py(851));
        assert!(hand > 0);
        assert!(scip > hand, "scip {scip} should exceed hand {hand}");

        // #2.1 point 4: a queued job before the worker's first `features`
        // event predicts from the reference-mode prior alone.
        let hand_unknown = model.predict_peak(&RepoFeatures::default());
        let scip_unknown = model.predict_peak(&RepoFeatures {
            refs: Some("scip".to_owned()),
            ..RepoFeatures::default()
        });
        assert!(hand_unknown > 0);
        assert!(
            scip_unknown > hand_unknown,
            "scip {scip_unknown} should exceed hand {hand_unknown} even with no features yet"
        );
    }

    #[test]
    fn memory_refit_moves_the_prediction_but_stays_bounded() {
        let input = scip_py(851);
        let mut model = MemoryModel::default();
        let seed = model.predict_peak(&input);
        // Every observation reports exactly double the seed; the refit loop
        // clamps each observation's ratio to at most 2.0 (mirrors
        // `EtaModel::stage`'s clipped refit), so the blended result should
        // land above the seed but nowhere near an unclamped 2x.
        for _ in 0..(MEMORY_SEED_WEIGHT as usize) {
            model.record(finished_row(input.clone(), seed * 2));
        }
        let refit = model.predict_peak(&input);
        assert!(refit > seed, "refit {refit} should exceed the seed {seed}");
        assert!(
            (refit as f64) <= seed as f64 * 2.01,
            "the clamp must keep the refit from exceeding the observed ratio, got {refit} against seed {seed}"
        );
    }

    // A regression test for exactly the bug the ratio-against-`seed` version
    // of `predict_peak` had: a typical finished job's peak sits on the
    // *median* curve, not the already-multiplied seed, so its ratio against
    // `seed` was about 1/2.27, clamped to the 0.5 floor, and 20 such rows
    // pulled the blended prediction down to about half the seed -- silently
    // discarding `UPPER_QUANTILE_MULTIPLIER` instead of applying it.
    #[test]
    fn memory_refit_at_the_median_preserves_the_upper_quantile() {
        let input = scip_py(851);
        let group = MemoryGroup::of(&input);
        let median = MemoryModel::median_bytes(group, 851.0);

        let mut at_median = MemoryModel::default();
        let seed = at_median.predict_peak(&input);
        for _ in 0..20 {
            at_median.record(finished_row(input.clone(), median.round() as u64));
        }
        let unchanged = at_median.predict_peak(&input);
        assert!(
            (unchanged as i64 - seed as i64).abs() <= 1,
            "20 rows exactly on the median should leave the prediction at the \
             seed (within rounding), not halve it: seed {seed}, got {unchanged}"
        );

        let mut at_double = MemoryModel::default();
        for _ in 0..20 {
            at_double.record(finished_row(input.clone(), (median * 2.0).round() as u64));
        }
        let raised = at_double.predict_peak(&input);
        assert!(
            raised > seed,
            "rows at 2x the median should raise the prediction above the seed, \
             got {raised} against seed {seed}"
        );
    }

    #[test]
    fn memory_failed_rows_do_not_narrow_the_prediction() {
        let input = scip_py(851);
        let mut model = MemoryModel::default();
        let before = model.predict_peak(&input);
        // A killed job's peak is real evidence of what it needed (#97 phase
        // 0's brief), but a job that never finished must never *narrow* the
        // prediction below what a completed job would justify -- a tiny
        // reported peak from a job killed almost immediately is not proof
        // the repository is small.
        model.record(TimingRow {
            features: input.clone(),
            elapsed_s: 1.0,
            stage_s: vec![None; StageId::ALL.len()],
            peak_rss_bytes: Some(1),
        });
        let after = model.predict_peak(&input);
        assert_eq!(before, after);
    }

    #[test]
    fn memory_prediction_is_order_independent() {
        let a = scip_py(200);
        let b = scip_py(900);
        let forward = MemoryModel::from_rows(vec![
            finished_row(a.clone(), 500_000_000),
            finished_row(b.clone(), 900_000_000),
        ]);
        let backward = MemoryModel::from_rows(vec![
            finished_row(b.clone(), 900_000_000),
            finished_row(a.clone(), 500_000_000),
        ]);
        assert_eq!(forward.predict_peak(&a), backward.predict_peak(&a));
        assert_eq!(forward.predict_peak(&b), backward.predict_peak(&b));
    }
}
