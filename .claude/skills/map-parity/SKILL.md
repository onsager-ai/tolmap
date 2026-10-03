---
name: map-parity
description: Verify Tolmap extraction, blending, clustering or layout changes against provenance-backed map fixtures and declared parity criteria.
---

# Map parity

1. Read data/fixtures.toml and the relevant ci.yml fixture lane. Record each source
   commit, generator, naming cache and affected language/resolver path.
2. Reproduce the existing baseline with that generator and configuration. The frozen
   Python reference remains useful but is not the universal current oracle.
3. Re-derive an affected fixture only for a justified correctness/model change,
   recording why the prior graph is wrong; never update solely to erase failure.
4. Run applicable parity checks: at least 95% district placement and modularity
   within 0.02. Preserve naming continuity and deterministic ordering.
5. Run the owning Rust/SCIP lanes and report the scope tested, known measured gaps
   and unexpected failures separately. Do not call a known red lane green.

Use the actual workflow's native-library setup and flags. Some lanes need full
repository history and pinned external clones; offline inability is a blocked
check, not evidence of parity.
