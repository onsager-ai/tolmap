//! Worker classes, one FIFO queue per class, and the queue-ETA simulation
//! (docs/WORKER_TIER.md §7, phase 0 of #97).
//!
//! Plain data in, plain data out: no locks, no tokio, no snapshots. The job
//! registry (`jobs.rs`) owns the queues and calls [`next_for`] when a worker
//! frees and [`simulate`] after every change, so the rule that decides which
//! job a worker takes and the rule that predicts when each queued job starts
//! live side by side here, where the tests can hold them against each other.
//! The spec's point is that the estimate and the scheduler cannot disagree;
//! `simulation_agrees_with_dispatch_on_every_generated_shape` is the test
//! that makes that a checked property rather than an intention.

use std::collections::{BTreeMap, VecDeque};

/// A worker class: what a worker can hold, not what it is called. Classes
/// are numbers (§Terms); a class is addressed by its index in a slice
/// ordered by [`order_classes`], smallest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Class {
    /// Usable memory in bytes: total minus the agent and OS reserve. `None`
    /// is unknown or unbounded -- local mode, where nothing has measured the
    /// machine -- and sorts as the largest, because a class nobody has
    /// bounded is the one a job that fits nowhere else is sent to anyway.
    pub usable_memory: Option<u64>,
    /// Workers (local mode: concurrent job slots) in this class.
    pub slots: usize,
}

impl Class {
    // `Option`'s own order puts `None` first, which is the opposite of what
    // "unbounded" means here; the leading flag moves it last.
    fn sort_key(&self) -> (bool, Option<u64>, usize) {
        (self.usable_memory.is_none(), self.usable_memory, self.slots)
    }
}

/// Orders classes smallest to largest. Every function below takes classes
/// (or class indices) in this order: "spill down" and "largest first" are
/// both statements about it.
pub fn order_classes(classes: &mut [Class]) {
    classes.sort_by_key(Class::sort_key);
}

/// The class of each worker, indexed by worker id. Ids run smallest class
/// first. That numbering is load-bearing: [`simulate`] breaks equal
/// `free_in_s` by worker id, and the registry hands a new job to the idle
/// eligible worker with the lowest id, so a job that finds an idle worker
/// of its own class and an idle larger one both free takes its own class's
/// in the simulation and in dispatch alike, leaving the larger one for work
/// only it can do.
pub fn worker_classes(classes: &[Class]) -> Vec<usize> {
    classes
        .iter()
        .enumerate()
        .flat_map(|(class, spec)| std::iter::repeat_n(class, spec.slots))
        .collect()
}

/// The class a job is queued in: the smallest whose usable memory holds the
/// predicted peak (§2.1). With no prediction, or one that no class holds,
/// the largest -- no caps means best effort, never a rejection (#97).
/// `classes` must be ordered by [`order_classes`] and non-empty.
pub fn bind(predicted_peak: Option<u64>, classes: &[Class]) -> usize {
    let largest = classes.len().saturating_sub(1);
    let Some(peak) = predicted_peak else {
        return largest;
    };
    classes
        .iter()
        .position(|class| class.usable_memory.is_none_or(|usable| usable >= peak))
        .unwrap_or(largest)
}

/// Which queue a worker of `worker_class` takes from when it frees (§7.1):
/// its own class's, else the largest smaller class's that is not empty
/// ("spill down"). Never a larger class's, and nothing is preempted, so a
/// large job that arrives while a large worker runs a spilled small job
/// waits for exactly that one job. `None` means the worker goes idle.
pub fn next_for<T>(worker_class: usize, queues: &[VecDeque<T>]) -> Option<usize> {
    let eligible = worker_class.saturating_add(1).min(queues.len());
    queues[..eligible]
        .iter()
        .rposition(|queue| !queue.is_empty())
}

/// One worker as the simulation sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Worker {
    pub id: usize,
    pub class: usize,
    /// Seconds from now until this worker can start another job: the
    /// remaining midpoint of its current job, 0 when idle.
    ///
    /// §7.2 writes this as an absolute `free_at` and subtracts `now` at the
    /// end. Times here are relative to now instead, so `now` is 0 and never
    /// subtracted: `(now + a + b) - now` is not `a + b` in floating point,
    /// and the one-slot result has to equal today's plain running sum
    /// exactly, not to within a rounding error.
    pub free_in_s: f64,
}

/// One queued job: an identifier the caller chooses and the midpoint of its
/// own predicted runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued<K> {
    pub job: K,
    pub midpoint_s: f64,
}

/// When one queued job is predicted to start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Start {
    /// The class queue the job waits in.
    pub class: usize,
    /// One-based position within that class queue: the jobs that must
    /// start before this one under strict FIFO.
    pub queue_position: usize,
    /// The worker the simulation expects to take it; `None` when no worker
    /// can (no worker of the job's class or larger).
    pub worker: Option<usize>,
    /// Seconds from now until it starts; `None` exactly when `worker` is.
    pub eta_start_s: Option<f64>,
}

/// §7.2: replays the dispatch rule over the current queues to predict when
/// each queued job starts. `queues[c]` is class `c`'s FIFO, head first, in
/// [`order_classes`] order.
///
/// For each class, largest first, and each job in it in order, the job goes
/// to the worker able to take it (its class, or larger and spilling) with
/// the smallest `free_in_s`, ties by worker id; its start is that worker's
/// `free_in_s`, which then grows by the job's midpoint.
///
/// Largest first is what makes this the same rule as [`next_for`], not an
/// approximation of it. A worker only spills once every queue of its own
/// class and above that it can serve is drained, and by the time a smaller
/// class is simulated every larger job has already been placed, each at a
/// time no later than the `free_in_s` any larger worker is left with. Taking
/// the jobs in admission order across classes instead would let a large
/// worker that frees first take a small job while its own class still has
/// one waiting, which dispatch never does
/// (`largest_class_first_is_what_dispatch_does`).
///
/// The output is keyed by job, so iteration order is the key's `Ord`, never
/// a hash order.
pub fn simulate<K: Ord + Clone>(
    workers: &[Worker],
    queues: &[Vec<Queued<K>>],
) -> BTreeMap<K, Start> {
    let mut free_in_s: Vec<f64> = workers.iter().map(|worker| worker.free_in_s).collect();
    let mut starts = BTreeMap::new();
    for (class, queue) in queues.iter().enumerate().rev() {
        for (index, queued) in queue.iter().enumerate() {
            // `total_cmp`, not `partial_cmp`: a total order means the choice
            // never depends on which operand a NaN happened to be.
            let chosen = (0..workers.len())
                .filter(|&w| workers[w].class >= class)
                .min_by(|&a, &b| {
                    free_in_s[a]
                        .total_cmp(&free_in_s[b])
                        .then(workers[a].id.cmp(&workers[b].id))
                });
            let eta_start_s = chosen.map(|w| {
                let start = free_in_s[w];
                free_in_s[w] += queued.midpoint_s;
                start
            });
            starts.insert(
                queued.job.clone(),
                Start {
                    class,
                    queue_position: index + 1,
                    worker: chosen.map(|w| workers[w].id),
                    eta_start_s,
                },
            );
        }
    }
    starts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::StageId;
    use crate::service::eta::EtaModel;
    use crate::worker::{LanguageFeatures, RepoFeatures};

    const GIB: u64 = 1024 * 1024 * 1024;

    fn class(gib: Option<u64>, slots: usize) -> Class {
        Class {
            usable_memory: gib.map(|gib| gib * GIB),
            slots,
        }
    }

    fn workers(spec: &[(usize, f64)]) -> Vec<Worker> {
        spec.iter()
            .enumerate()
            .map(|(id, &(class, free_in_s))| Worker {
                id,
                class,
                free_in_s,
            })
            .collect()
    }

    fn queue(jobs: &[(&'static str, f64)]) -> Vec<Queued<&'static str>> {
        jobs.iter()
            .map(|&(job, midpoint_s)| Queued { job, midpoint_s })
            .collect()
    }

    /// (worker, eta_start_s, queue_position) per job, for hand-written
    /// expectations.
    fn plan(starts: &BTreeMap<&'static str, Start>) -> Vec<(&'static str, usize, f64, usize)> {
        starts
            .iter()
            .map(|(job, start)| {
                (
                    *job,
                    start.worker.expect("every job here has a worker"),
                    start.eta_start_s.expect("every job here has a start"),
                    start.queue_position,
                )
            })
            .collect()
    }

    // ---- Binding and ordering -------------------------------------------

    #[test]
    fn classes_order_smallest_first_with_unbounded_last() {
        let mut classes = vec![class(None, 1), class(Some(16), 1), class(Some(4), 2)];
        order_classes(&mut classes);
        assert_eq!(
            classes,
            vec![class(Some(4), 2), class(Some(16), 1), class(None, 1)]
        );
    }

    #[test]
    fn worker_ids_run_smallest_class_first() {
        let classes = [class(Some(4), 2), class(Some(16), 1), class(None, 3)];
        assert_eq!(worker_classes(&classes), vec![0, 0, 1, 2, 2, 2]);
    }

    #[test]
    fn bind_takes_the_smallest_class_that_fits() {
        let classes = [class(Some(4), 1), class(Some(8), 1), class(Some(16), 1)];
        assert_eq!(bind(Some(3 * GIB), &classes), 0);
        // Usable memory equal to the prediction fits.
        assert_eq!(bind(Some(4 * GIB), &classes), 0);
        assert_eq!(bind(Some(4 * GIB + 1), &classes), 1);
        assert_eq!(bind(Some(9 * GIB), &classes), 2);
    }

    #[test]
    fn bind_sends_a_job_that_fits_nowhere_to_the_largest_class() {
        let classes = [class(Some(4), 1), class(Some(8), 1), class(Some(16), 1)];
        // No caps: the msgraph-sized outlier is bound, not refused.
        assert_eq!(bind(Some(24 * GIB), &classes), 2);
    }

    #[test]
    fn bind_without_a_prediction_takes_the_largest_class() {
        let classes = [class(Some(4), 1), class(Some(8), 1), class(Some(16), 1)];
        assert_eq!(bind(None, &classes), 2);
        // Local mode's single unbounded class holds every prediction.
        assert_eq!(bind(Some(64 * GIB), &[class(None, 1)]), 0);
        assert_eq!(bind(None, &[class(None, 1)]), 0);
        assert_eq!(
            bind(Some(64 * GIB), &[class(Some(8), 1), class(None, 1)]),
            1
        );
    }

    // ---- next_for --------------------------------------------------------

    #[test]
    fn a_freed_worker_takes_its_own_queue_first() {
        let queues: Vec<VecDeque<u8>> = vec![VecDeque::from([1]), VecDeque::from([2])];
        assert_eq!(next_for(1, &queues), Some(1));
        assert_eq!(next_for(0, &queues), Some(0));
    }

    #[test]
    fn a_freed_worker_spills_down_to_the_largest_smaller_queue() {
        let queues: Vec<VecDeque<u8>> =
            vec![VecDeque::from([1]), VecDeque::from([2]), VecDeque::new()];
        assert_eq!(next_for(2, &queues), Some(1));
        let queues: Vec<VecDeque<u8>> = vec![VecDeque::from([1]), VecDeque::new(), VecDeque::new()];
        assert_eq!(next_for(2, &queues), Some(0));
    }

    #[test]
    fn a_freed_worker_never_takes_a_larger_class() {
        let queues: Vec<VecDeque<u8>> = vec![VecDeque::new(), VecDeque::from([2])];
        assert_eq!(next_for(0, &queues), None);
        let empty: Vec<VecDeque<u8>> = vec![VecDeque::new(), VecDeque::new()];
        assert_eq!(next_for(1, &empty), None);
    }

    // ---- Single slot: today's `refresh_queue_etas`, exactly ----------------

    /// Today's `refresh_queue_etas` arithmetic, lifted verbatim into a pure
    /// function: the remaining midpoints of the running jobs summed in one
    /// `.sum()`, then each queued job starting at the running total and
    /// adding its own midpoint. Kept only as the oracle the one-slot
    /// simulation must reproduce bit for bit.
    fn single_slot_oracle(running: &[f64], queued: &[f64]) -> Vec<(usize, f64)> {
        let mut wait_s: f64 = running.iter().copied().sum();
        let mut out = Vec::new();
        for (index, midpoint) in queued.iter().enumerate() {
            out.push((index + 1, wait_s));
            wait_s += midpoint;
        }
        out
    }

    /// Midpoints the real ETA model quotes for repositories of different
    /// shapes, so the equivalence runs over the awkward floats production
    /// sees rather than round numbers that would add up exactly either way.
    fn model_midpoints() -> Vec<f64> {
        let model = EtaModel::default();
        let never_started = [false; StageId::ALL.len()];
        let shapes: [(&str, u64, u64); 6] = [
            ("py", 17, 40_211),
            ("ts", 4_096, 31_457_280),
            ("go", 250, 1_048_576),
            ("py", 90_000, 734_003_200),
            ("rs", 33, 131_072),
            ("py", 6_347, 50_776_000),
        ];
        let mut midpoints = vec![model
            .predict(&RepoFeatures::default(), &never_started, None)
            .midpoint()];
        for (lang, files, bytes) in shapes {
            let features = RepoFeatures {
                clone_bytes: Some(bytes * 3),
                commits: Some(files / 3 + 1),
                languages: [(lang.to_owned(), LanguageFeatures { files, bytes })]
                    .into_iter()
                    .collect(),
                ..RepoFeatures::default()
            };
            midpoints.push(model.predict(&features, &never_started, None).midpoint());
        }
        midpoints
    }

    #[test]
    fn one_slot_reproduces_todays_queue_eta_exactly() {
        let midpoints = model_midpoints();
        // What the one slot is doing: idle, running a job with a remaining
        // midpoint (the prior's, then an odd mid-stage value), or holding a
        // job whose row is already terminal (a cancel the worker loop has
        // not reaped yet), which today counts as 0.
        let running_shapes: Vec<Vec<f64>> = vec![
            vec![],
            vec![midpoints[0]],
            vec![37.123_456_789_012_34],
            vec![0.0],
            vec![midpoints[4]],
        ];
        let mut compared = 0;
        for running in &running_shapes {
            for depth in 0..=5 {
                // Rotate which features sit where, so the order of the
                // mixed midpoints differs between depths.
                let queued: Vec<f64> = (0..depth)
                    .map(|i| midpoints[(i * 3 + depth) % midpoints.len()])
                    .collect();
                let one_worker = [Worker {
                    id: 0,
                    class: 0,
                    free_in_s: running.first().copied().unwrap_or(0.0),
                }];
                let jobs: Vec<Queued<usize>> = queued
                    .iter()
                    .enumerate()
                    .map(|(job, &midpoint_s)| Queued { job, midpoint_s })
                    .collect();
                let starts = simulate(&one_worker, &[jobs]);
                let oracle = single_slot_oracle(running, &queued);
                assert_eq!(starts.len(), oracle.len());
                for (job, (position, wait_s)) in oracle.into_iter().enumerate() {
                    let start = starts[&job];
                    assert_eq!(start.queue_position, position, "{running:?} {queued:?}");
                    assert_eq!(start.eta_start_s, Some(wait_s), "{running:?} {queued:?}");
                    compared += 1;
                }
            }
        }
        assert_eq!(compared, 5 * (1 + 2 + 3 + 4 + 5));
    }

    // ---- Several slots, one class: hand-computed ----------------------------

    #[test]
    fn two_slots_start_each_job_on_the_first_slot_to_free() {
        // w0 frees in 10 s, w1 in 4 s.
        //   a: w1 at 4   (w1 -> 10)
        //   b: w0 10, w1 10, tie by id -> w0 at 10 (w0 -> 13)
        //   c: w1 at 10  (w1 -> 15)
        //   d: w0 at 13  (w0 -> 15)
        // The one-slot sum would have said 14, 20, 23, 28.
        let starts = simulate(
            &workers(&[(0, 10.0), (0, 4.0)]),
            &[queue(&[("a", 6.0), ("b", 3.0), ("c", 5.0), ("d", 2.0)])],
        );
        assert_eq!(
            plan(&starts),
            vec![
                ("a", 1, 4.0, 1),
                ("b", 0, 10.0, 2),
                ("c", 1, 10.0, 3),
                ("d", 0, 13.0, 4),
            ]
        );
    }

    #[test]
    fn three_slots_break_a_three_way_tie_by_worker_id() {
        // w0 1 s, w1 7 s, w2 2 s.
        //   a: w0 at 1 (w0 -> 6)
        //   b: w2 at 2 (w2 -> 7)
        //   c: w0 at 6 (w0 -> 7)
        //   d: w0 7, w1 7, w2 7 -> w0 at 7 (w0 -> 11)
        let starts = simulate(
            &workers(&[(0, 1.0), (0, 7.0), (0, 2.0)]),
            &[queue(&[("a", 5.0), ("b", 5.0), ("c", 1.0), ("d", 4.0)])],
        );
        assert_eq!(
            plan(&starts),
            vec![
                ("a", 0, 1.0, 1),
                ("b", 2, 2.0, 2),
                ("c", 0, 6.0, 3),
                ("d", 0, 7.0, 4),
            ]
        );
    }

    // ---- Two classes: hand-computed ------------------------------------------

    const SMALL: usize = 0;
    const LARGE: usize = 1;

    #[test]
    fn a_large_job_waits_for_the_large_worker_while_small_jobs_flow() {
        // w0 small frees in 2 s; w1 large is 30 s from done.
        //   L: only w1 -> at 30 (w1 -> 80)
        //   s1: w0 2 vs w1 80 -> w0 at 2 (w0 -> 6)
        //   s2: w0 at 6 (w0 -> 10)
        //   s3: w0 at 10
        let starts = simulate(
            &workers(&[(SMALL, 2.0), (LARGE, 30.0)]),
            &[
                queue(&[("s1", 4.0), ("s2", 4.0), ("s3", 4.0)]),
                queue(&[("L", 50.0)]),
            ],
        );
        assert_eq!(
            plan(&starts),
            vec![
                ("L", 1, 30.0, 1),
                ("s1", 0, 2.0, 1),
                ("s2", 0, 6.0, 2),
                ("s3", 0, 10.0, 3),
            ]
        );
    }

    #[test]
    fn a_large_worker_with_an_empty_queue_spills_down() {
        // w0 small is busy for 20 s; w1 large frees in 3 s with nothing of
        // its own queued, so it takes small jobs until w0 frees.
        //   s1: w0 20 vs w1 3  -> w1 at 3  (w1 -> 8)
        //   s2: w0 20 vs w1 8  -> w1 at 8  (w1 -> 13)
        //   s3: w0 20 vs w1 13 -> w1 at 13 (w1 -> 18)
        //   s4: w0 20 vs w1 18 -> w1 at 18 (w1 -> 23)
        //   s5: w0 20 vs w1 23 -> w0 at 20
        let starts = simulate(
            &workers(&[(SMALL, 20.0), (LARGE, 3.0)]),
            &[
                queue(&[
                    ("s1", 5.0),
                    ("s2", 5.0),
                    ("s3", 5.0),
                    ("s4", 5.0),
                    ("s5", 5.0),
                ]),
                vec![],
            ],
        );
        assert_eq!(
            plan(&starts),
            vec![
                ("s1", 1, 3.0, 1),
                ("s2", 1, 8.0, 2),
                ("s3", 1, 13.0, 3),
                ("s4", 1, 18.0, 4),
                ("s5", 0, 20.0, 5),
            ]
        );
    }

    #[test]
    fn a_large_job_arriving_during_a_spill_waits_for_that_one_job() {
        // w1 large is running a spilled small job with 6 s left; w0 small
        // has 10 s left. Then L arrives behind two queued small jobs.
        //   L: only w1 -> at 6, after the one spilled job (w1 -> 46)
        //   s1: w0 10 vs w1 46 -> w0 at 10 (w0 -> 14)
        //   s2: w0 at 14
        let starts = simulate(
            &workers(&[(SMALL, 10.0), (LARGE, 6.0)]),
            &[queue(&[("s1", 4.0), ("s2", 4.0)]), queue(&[("L", 40.0)])],
        );
        assert_eq!(
            plan(&starts),
            vec![("L", 1, 6.0, 1), ("s1", 0, 10.0, 1), ("s2", 0, 14.0, 2)]
        );
    }

    #[test]
    fn a_job_no_worker_can_take_has_no_start() {
        // A class with no worker of its own and none larger: bind never
        // produces this, but the simulation must not invent a start for it.
        let starts = simulate(&workers(&[(SMALL, 0.0)]), &[vec![], queue(&[("L", 40.0)])]);
        assert_eq!(
            starts["L"],
            Start {
                class: LARGE,
                queue_position: 1,
                worker: None,
                eta_start_s: None,
            }
        );
    }

    // ---- The simulation is the dispatch rule ------------------------------

    /// The dispatcher itself, replayed as events: the worker that frees
    /// next (smallest `free_in_s`, ties by id) takes the head of whatever
    /// queue [`next_for`] names, and a worker [`next_for`] gives nothing to
    /// has nothing more to do (queues only shrink in a replay). This is
    /// §7.1 run forward in time, with no class ordering of its own.
    fn dispatch_replay<K: Ord + Clone>(
        workers: &[Worker],
        queues: &[Vec<Queued<K>>],
    ) -> BTreeMap<K, Start> {
        let mut pending: Vec<VecDeque<(usize, Queued<K>)>> = queues
            .iter()
            .map(|queue| queue.iter().cloned().enumerate().collect())
            .collect();
        let mut free_in_s: Vec<f64> = workers.iter().map(|worker| worker.free_in_s).collect();
        let mut done = vec![false; workers.len()];
        let mut starts = BTreeMap::new();
        while let Some(w) = (0..workers.len()).filter(|&w| !done[w]).min_by(|&a, &b| {
            free_in_s[a]
                .total_cmp(&free_in_s[b])
                .then(workers[a].id.cmp(&workers[b].id))
        }) {
            let Some(class) = next_for(workers[w].class, &pending) else {
                done[w] = true;
                continue;
            };
            let (index, queued) = pending[class].pop_front().expect("next_for names a job");
            starts.insert(
                queued.job,
                Start {
                    class,
                    queue_position: index + 1,
                    worker: Some(workers[w].id),
                    eta_start_s: Some(free_in_s[w]),
                },
            );
            free_in_s[w] += queued.midpoint_s;
        }
        for (class, queue) in pending.into_iter().enumerate() {
            for (index, queued) in queue {
                starts.insert(
                    queued.job,
                    Start {
                        class,
                        queue_position: index + 1,
                        worker: None,
                        eta_start_s: None,
                    },
                );
            }
        }
        starts
    }

    #[test]
    fn largest_class_first_is_what_dispatch_does() {
        // s1 was admitted before L. The large worker frees first (6 s) and
        // its own queue holds L, so dispatch gives it L, not the older s1.
        // Simulating in admission order across classes would have put s1 on
        // w1 at 6 and L at 6 + 4 = 10: an ETA the scheduler never keeps.
        let shape_workers = workers(&[(SMALL, 10.0), (LARGE, 6.0)]);
        let shape_queues = [queue(&[("s1", 4.0)]), queue(&[("L", 40.0)])];
        let simulated = simulate(&shape_workers, &shape_queues);
        assert_eq!(simulated, dispatch_replay(&shape_workers, &shape_queues));
        assert_eq!(simulated["L"].eta_start_s, Some(6.0));
        assert_eq!(simulated["s1"].eta_start_s, Some(10.0));
    }

    /// SplitMix64: a fixed, dependency-free stream, seeded with the
    /// project's `SEED = 7`, so a failure names a shape that reproduces.
    struct SplitMix(u64);

    impl SplitMix {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next_u64() % n
        }
    }

    #[test]
    fn simulation_agrees_with_dispatch_on_every_generated_shape() {
        // Few distinct times and midpoints on purpose: ties between workers
        // (idle ones above all) are where two orderings of the same rule
        // would part, so the generator makes them common. The awkward
        // model midpoints are mixed in so agreement is bitwise, not only on
        // round numbers.
        let awkward = model_midpoints();
        let mut rng = SplitMix(7);
        for shape in 0..5_000 {
            let class_count = 1 + rng.below(3) as usize;
            let worker_count = 1 + rng.below(4) as usize;
            let shape_workers: Vec<Worker> = (0..worker_count)
                .map(|id| Worker {
                    id,
                    class: rng.below(class_count as u64) as usize,
                    free_in_s: [0.0, 0.0, 2.5, 5.0, 10.0, awkward[1]][rng.below(6) as usize],
                })
                .collect();
            let mut job = 0;
            let shape_queues: Vec<Vec<Queued<usize>>> = (0..class_count)
                .map(|_| {
                    (0..rng.below(5))
                        .map(|_| {
                            job += 1;
                            Queued {
                                job,
                                midpoint_s: [1.0, 2.5, 5.0, 7.5, awkward[2], awkward[3]]
                                    [rng.below(6) as usize],
                            }
                        })
                        .collect()
                })
                .collect();
            assert_eq!(
                simulate(&shape_workers, &shape_queues),
                dispatch_replay(&shape_workers, &shape_queues),
                "shape {shape}: {shape_workers:?} {shape_queues:?}"
            );
        }
    }
}
