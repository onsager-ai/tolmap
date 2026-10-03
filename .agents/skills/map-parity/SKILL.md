---
name: map-parity
description: Verify Tolmap extraction, blending, clustering or layout against provenance-backed map fixtures, deterministic output and repository-owned district/modularity acceptance gates.
---

# Tolmap map parity

## Scope

Owns fixture provenance and parity procedure. AGENTS.md owns acceptance thresholds,
determinism and naming continuity. data/fixtures.toml and the corresponding lane
in .github/workflows/ci.yml own generator pins, flags and native-library setup.

## Prerequisites

Identify affected languages/resolvers and the fixture's source commit, generator,
seed, naming cache and history requirements. The frozen Python reference is not
the universal current fixture generator. Preserve frozen reference ownership.

## Procedure

1. Reproduce the unchanged baseline with the recorded generator/configuration.
   Keep each external clone and source commit explicit; blocked history/network
   setup is not evidence of parity.
2. Re-derive an affected fixture only for a demonstrated correctness/model change.
   Record why the prior graph is wrong; never refresh merely to hide a failure.
3. Run the owning parity lanes against the justified baseline, using AGENTS.md's
   placement/modularity thresholds. Check naming continuity and deterministic
   ordering/output under the same inputs.
4. Run affected Rust/SCIP lanes with their native setup and exact workflow flags.
   Separate expected measured gaps from newly unexpected failures. For blend,
   clustering or layout changes, hand measurements to benchmark and update the
   findings in the same change as required by the contract.

## Completion

Report input/generator pins, baseline rationale, observed parity and determinism,
actual lane results, known gaps and blocked prerequisites. Do not report a known
red lane as green or substitute a product check command for development gates.
