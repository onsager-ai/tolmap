---
name: benchmark
description: Measure Tolmap blend, clustering, layout or renderer changes using pinned corpus inputs and comparable controls, and update the relevant findings.
---

# Benchmark

1. Find the relevant findings by heading and read the owning evaluation script.
   Record corpus commits, seed, generator, resolution and naming cache state.
2. Establish an unchanged control using the same data, path and measurement setup.
3. Run eval/batch_stability.py for blend/clustering/layout changes as required by
   AGENTS. For renderer work use the existing workflow's browser/performance lane.
4. Compare only like-for-like inputs; repeat deterministic output checks. Preserve
   parity criteria and distinguish informational budgets from blocking assertions.
5. Update docs/FINDINGS.md with method, numbers, limits and justified interpretation
   in the same change. Include referenced evidence in the reviewable change.

Do not infer performance improvements from changed sample sizes or warm caches.
