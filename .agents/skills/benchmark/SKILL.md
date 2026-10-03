---
name: benchmark
description: Measure Tolmap blend, clustering, layout or renderer changes with pinned corpus inputs and comparable controls, and update repository findings with evidence and limits.
---

# Tolmap benchmark

## Scope

Owns comparable measurement and findings updates. AGENTS.md owns seed/parity
requirements; eval/batch_stability.py and the owning renderer workflow own exact
measurement entrypoints. This skill supplies no machine-specific gate waiver.

## Prerequisites

Read relevant findings headings and the owning evaluation script. Record corpus
commits, generator, seed, resolution, naming-cache state and measurement setup.
Use a task checkout and preserve primary-checkout parking.

## Procedure

1. Establish an unchanged control with the same inputs, path and setup. Record
   cold/warm cache conditions, corpus size and repetition method before comparing.
2. For blend/clustering/layout use the contract's required batch stability run;
   renderer changes use the existing browser/performance lane. Do not substitute
   another product's benchmark or resource profile.
3. Compare like-for-like runs, repeat deterministic output checks and apply the
   repository's parity gates through map-parity. Distinguish informational budgets
   from blocking assertions rather than treating every warning as a new gate.
4. Update docs/FINDINGS.md in the same change with method, numbers, interpretation,
   evidence and limitations. Keep the measurement entrypoint linked to its owner.

## Completion

Report control/change inputs, commands, repetitions and numbers. Changed sample
sizes or warmed caches do not prove an improvement. Unavailable local resources
require a supported CI run or a blocked result, not relaxed acceptance criteria.
