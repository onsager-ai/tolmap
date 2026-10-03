---
name: viewer-verification
description: Verify Tolmap map renderer or viewer changes on desktop and phone with existing fixtures, visibility gates and browser checks.
---

# Viewer verification

1. Read relevant docs/UX.md and .github/workflows/viewer-check.yml. Install the
   locked web dependencies and browser; prepare maps and API fixtures as documented.
2. Exercise overview, district, file and symbol views on desktop and phone. Require
   named districts, landmark lists and a card for each applicable tap.
3. Check symbol/class detail gates, touch behavior and the imperative-renderer boundary.
4. Run the workflow's view-stability checks and review screenshots. Use workflow
   dispatch when local resources cannot support the check; report its actual result.
5. Preserve existing thresholds and record evidence and any blocked cases.

Machine-specific hardware limits are local preferences; they do not waive checks.
