---
name: viewer-verification
description: Verify Tolmap renderer/viewer changes on desktop and phone with its pinned maps/API fixtures, visibility gates, view-stability checks and reviewed screenshots.
---

# Tolmap viewer verification

## Scope

Owns Tolmap's view and interaction evidence. AGENTS.md owns symbol/class visibility
thresholds and the imperative renderer boundary; docs/UX.md and
.github/workflows/viewer-check.yml own current behavior and browser setup.

## Prerequisites

Identify affected overview/district/file/symbol paths. Follow the workflow's locked
web dependencies, browser, map fixtures and API fixture service. That synthetic
API lane verifies viewer behavior; do not claim it proves the real product API.

## Procedure

1. Prepare maps and the documented API fixture service, then start the viewer with
   the exact workflow setup. Keep source/map versions and endpoint configuration
   in the evidence; do not treat another repo's fixture server as interchangeable.
2. On desktop and phone verify named districts, landmark lists and cards from each
   applicable district/file/symbol tap. Check touch behavior and visibility gates
   from AGENTS.md without copying or relaxing the contract's thresholds.
3. Run the owning view-stability check and review screenshots, including the Rust
   map fixture where required. Preserve React chrome/state versus imperative-map
   ownership and avoid extending the frozen reference viewer.
4. When local prerequisites are unavailable, use a supported workflow dispatch
   under existing authority and report its actual SHA/result. A request to run CI
   and an observed successful run are different evidence.

## Completion

Report maps/API fixtures, viewport/interaction coverage, commands/run URL,
reviewed screenshots, results and blocked cases. Hardware preferences do not
waive gates, and fixture success is not evidence of native harness discovery.
