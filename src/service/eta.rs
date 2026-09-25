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
/// three `--refs scip` indexing stages existed (issue #110) have 18 entries;
/// this inserts the missing stages after `Resolve` so every later stage
/// keeps its own observations instead of shifting onto a neighbour's.
pub fn upgrade_stage_layout(mut stage_s: Vec<Option<f64>>) -> Vec<Option<f64>> {
    const LEGACY_STAGE_COUNT: usize = 18;
    if stage_s.len() == LEGACY_STAGE_COUNT {
        let at = StageId::Resolve.index();
        for _ in 0..(STAGE_COUNT - LEGACY_STAGE_COUNT) {
            stage_s.insert(at, None);
        }
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
// The three zeros after resolve's 0.223 are the `--refs scip` indexing
// stages, which finding 36's hand-written timeline never ran; their seed is
// `index_seed`, and only for a job that asked for SCIP.
const DIFY_STAGE_S: [f64; STAGE_COUNT] = [
    0.032, 0.0, 0.0, 0.0, 0.103, 26.913, 0.223, 0.0, 0.0, 0.0, 0.382, 0.044, 0.219, 0.131, 0.010,
    1.613, 5.109, 0.021, 0.390, 5.549, 0.182,
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
            // cost nothing, and a `--refs scip` job -- the default since
            // #110 P2a -- would be quoted a hand-written build's range. Use
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
            WorkerEvent::Result { .. } | WorkerEvent::Error { .. } | WorkerEvent::Log { .. } => {}
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
        }
    }

    #[test]
    fn legacy_timing_rows_gain_empty_indexing_stages() {
        let legacy = (0..18).map(|i| Some(i as f64)).collect::<Vec<_>>();
        let upgraded = upgrade_stage_layout(legacy);
        assert_eq!(upgraded.len(), StageId::ALL.len());
        assert_eq!(upgraded[StageId::Resolve.index() - 1], Some(6.0));
        assert_eq!(upgraded[StageId::IndexGo.index() - 1], None);
        assert_eq!(upgraded[StageId::IndexTs.index() - 1], None);
        assert_eq!(upgraded[StageId::History.index() - 1], Some(7.0));
        assert_eq!(upgraded[StageId::Write.index() - 1], Some(17.0));
        let current = vec![Some(1.0); StageId::ALL.len()];
        assert_eq!(upgrade_stage_layout(current.clone()), current);
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

    // #110 P2a: a queued job knows only the service's reference mode. Under
    // the SCIP default its prior must cover finding 44's indexing-inclusive
    // build times, not the hand-written range.
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
        });
        let after = model.predict(&input, &[false; STAGE_COUNT], None);
        assert!((before.low_s - after.low_s).abs() < 0.001);
        assert!((before.high_s - after.high_s).abs() < 0.001);
    }
}
