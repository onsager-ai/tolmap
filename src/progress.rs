//! Progress is an observation of the deterministic pipeline. Counters never
//! enter a map or a graph, and only the throttled sink performs I/O.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StageId {
    Clone,
    CloneObjects,
    CloneDeltas,
    CloneCheckout,
    Detect,
    Parse,
    Resolve,
    // `--refs scip` only (issue #110): one stage per indexed language, in
    // the order extraction runs them. Hand-written builds never start them.
    IndexGo,
    IndexPy,
    // Issue #110 P1c: the sandboxed dependency install that runs just
    // before scip-typescript, when the install policy wants one.
    Install,
    IndexTs,
    History,
    BlendPrune,
    Partition,
    Neighbourhoods,
    Naming,
    Regions,
    Footprints,
    WriteMap,
    Symbols,
    SymbolCards,
    Write,
}

impl StageId {
    pub const ALL: [Self; 22] = [
        Self::Clone,
        Self::CloneObjects,
        Self::CloneDeltas,
        Self::CloneCheckout,
        Self::Detect,
        Self::Parse,
        Self::Resolve,
        Self::IndexGo,
        Self::IndexPy,
        Self::Install,
        Self::IndexTs,
        Self::History,
        Self::BlendPrune,
        Self::Partition,
        Self::Neighbourhoods,
        Self::Naming,
        Self::Regions,
        Self::Footprints,
        Self::WriteMap,
        Self::Symbols,
        Self::SymbolCards,
        Self::Write,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|stage| *stage == self).unwrap() + 1
    }

    /// The indexing stage for `language` under `--refs scip`, or `None` for
    /// a language the product never indexes (Rust, finding 55).
    pub fn index_for(language: crate::extract::LanguageKind) -> Option<Self> {
        match language {
            crate::extract::LanguageKind::Go => Some(Self::IndexGo),
            crate::extract::LanguageKind::Python => Some(Self::IndexPy),
            crate::extract::LanguageKind::TypeScript => Some(Self::IndexTs),
            crate::extract::LanguageKind::Rust => None,
        }
    }

    /// The language an indexing stage indexes, as a `LanguageFeatures` key.
    pub fn indexed_language(self) -> Option<&'static str> {
        match self {
            Self::IndexGo => Some("go"),
            Self::IndexPy => Some("py"),
            Self::IndexTs => Some("ts"),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Clone => "Clone or fetch",
            Self::CloneObjects => "Receiving objects",
            Self::CloneDeltas => "Resolving deltas",
            Self::CloneCheckout => "Updating files",
            Self::Detect => "Detecting source",
            Self::History => "Reading history",
            Self::Parse => "Parsing files",
            Self::Resolve => "Resolving imports",
            Self::IndexGo => "Indexing Go",
            Self::IndexPy => "Indexing Python",
            Self::Install => "Installing dependencies",
            Self::IndexTs => "Indexing TypeScript",
            Self::BlendPrune => "Blending and pruning",
            Self::Partition => "Partitioning districts",
            Self::Neighbourhoods => "Partitioning neighbourhoods",
            Self::Naming => "Naming districts",
            Self::Regions => "Drawing regions",
            Self::Footprints => "Drawing footprints",
            Self::WriteMap => "Writing map",
            Self::Symbols => "Extracting symbols",
            Self::SymbolCards => "Drawing symbol cards",
            Self::Write => "Writing symbols",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Self::Clone | Self::CloneObjects | Self::CloneDeltas => "objects",
            Self::Write | Self::WriteMap => "bytes",
            Self::CloneCheckout
            | Self::Parse
            | Self::Resolve
            | Self::IndexGo
            | Self::IndexPy
            | Self::IndexTs
            | Self::Symbols
            | Self::SymbolCards => "files",
            Self::History => "commits",
            Self::Regions | Self::Footprints | Self::Neighbourhoods | Self::Naming => "districts",
            Self::Detect | Self::BlendPrune | Self::Partition | Self::Install => "steps",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ProgressValue {
    pub stage: StageId,
    pub stage_index: usize,
    pub stage_count: usize,
    pub label: String,
    pub unit: String,
    pub done: u64,
    pub total: Option<u64>,
    pub rate_per_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_rate_bytes_per_s: Option<f64>,
}

#[derive(Clone)]
pub struct Progress {
    emit: Arc<dyn Fn(crate::worker::WorkerEvent) + Send + Sync>,
}

impl Progress {
    pub fn new(emit: impl Fn(crate::worker::WorkerEvent) + Send + Sync + 'static) -> Self {
        Self {
            emit: Arc::new(emit),
        }
    }

    pub fn silent() -> Self {
        Self::new(|_| {})
    }

    pub fn stage(&self, id: StageId, total: Option<u64>) -> StageCounter {
        (self.emit)(crate::worker::WorkerEvent::StageStarted { v: 1, stage: id });
        let counter = StageCounter {
            id,
            total,
            done: AtomicU64::new(0),
            last_ms: AtomicU64::new(0),
            started: Instant::now(),
            progress: self.clone(),
            finished: false,
        };
        counter.emit_progress();
        counter
    }

    pub fn log(&self, message: impl Into<String>) {
        (self.emit)(crate::worker::WorkerEvent::Log {
            v: 1,
            message: message.into(),
        });
    }

    pub fn emit_event(&self, event: crate::worker::WorkerEvent) {
        (self.emit)(event);
    }
}

pub struct StageCounter {
    id: StageId,
    total: Option<u64>,
    done: AtomicU64,
    last_ms: AtomicU64,
    started: Instant,
    progress: Progress,
    finished: bool,
}

impl StageCounter {
    pub fn advance(&self, amount: u64) {
        self.done.fetch_add(amount, Ordering::Relaxed);
        self.maybe_emit();
    }

    pub fn set(&self, done: u64) {
        self.done.fetch_max(done, Ordering::Relaxed);
        self.maybe_emit();
    }

    fn maybe_emit(&self) {
        let elapsed = self.started.elapsed().as_millis() as u64;
        let previous = self.last_ms.load(Ordering::Relaxed);
        if elapsed < previous.saturating_add(250)
            || self
                .last_ms
                .compare_exchange(previous, elapsed, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        self.emit_progress();
    }

    fn emit_progress(&self) {
        let done = self.done.load(Ordering::Relaxed);
        let elapsed = self.started.elapsed().as_secs_f64();
        (self.progress.emit)(crate::worker::WorkerEvent::Progress {
            v: 1,
            value: ProgressValue {
                stage: self.id,
                stage_index: self.id.index(),
                stage_count: StageId::ALL.len(),
                label: self.id.label().to_owned(),
                unit: self.id.unit().to_owned(),
                done,
                total: self.total,
                rate_per_s: (done > 0 && elapsed > 0.0).then_some(done as f64 / elapsed),
                transfer_bytes: None,
                transfer_rate_bytes_per_s: None,
            },
        });
    }

    pub fn finish(mut self) {
        self.emit_progress();
        self.finished = true;
        (self.progress.emit)(crate::worker::WorkerEvent::StageFinished {
            v: 1,
            stage: self.id,
            duration_s: self.started.elapsed().as_secs_f64(),
            success: true,
        });
    }
}

impl Drop for StageCounter {
    fn drop(&mut self) {
        if !self.finished {
            (self.progress.emit)(crate::worker::WorkerEvent::StageFinished {
                v: 1,
                stage: self.id,
                duration_s: self.started.elapsed().as_secs_f64(),
                success: false,
            });
        }
    }
}
