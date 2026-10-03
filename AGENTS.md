# tolmap

Rust indexer/API and React + TypeScript product. Python in src/tolmap/ and the
vanilla-JS viewer/ are frozen reference implementations; do not extend them.
Use lowercase tolmap for package, binary, module and domain; Tolman is the person.

## Invariants

- Determinism: SEED = 7 and deterministic ordering throughout. The same repository
  at the same commit produces a byte-identical map.
- Blend signals by mass, not independent normalization per edge.
- Never rename a district without its previous name in hand.
- Report lower bounds; do not inflate uncertain extraction counts.
- Terms follow docs/GLOSSARY.md. Code objects retain their code names.
- Rust defines the map JSON schema once; TypeScript bindings are generated.
- Preserve comments explaining measured failures of apparently simpler approaches.
- React owns chrome/state; an imperative renderer owns the map surface.
- Overview shows district outlines and roads only. File symbols require at least
  40 px on screen; class members require a short side of at least 110 px.

## Workspace isolation

The primary checkout stays on main. Code changes use an adjacent task worktree:
`git worktree add -b <branch> ../tolmap-wt-<slug> origin/main`.
Preserve unrelated changes and give delegated work an exact ref/worktree path.
Enable the existing checkout guard according to .githooks/README.md; it is a
post-checkout guard, not a Git pre-checkout hook.

## Verification and completion

Rust 2021 and strict TypeScript apply; frozen Python remains 3.11/std-lib first.
Use exact flags and prerequisites from .github/workflows/ci.yml. Core Rust lanes
include cargo fmt --check, cargo clippy --locked --release --all-targets and
cargo test --locked --release; native Leiden setup is described by that workflow.

Extraction/clustering/layout changes require affected fixture regeneration and
parity: at least 95% district placement, modularity within 0.02. Use source pins
and generators in data/fixtures.toml; the Python reference is not every current
fixture's generator. Never refresh a baseline merely to hide a regression.
Blend/clustering/layout changes also require eval/batch_stability.py and findings
updated with measurements in the same change.

Viewer changes require named districts, listed landmarks and cards from district,
file and symbol taps on both phone and desktop. Use viewer-verification and the
existing browser workflow.

## Conditional reading

For architecture-changing work, read relevant HANDOFF and docs/ARCHITECTURE.md sections. Pipeline changes load
relevant docs/PIPELINE.md and findings; use the map-parity/benchmark skills. Search
relevant headings in docs/FINDINGS.md instead of loading its entire history.
Viewer work loads docs/UX.md and relevant renderer findings. SCIP work loads
its sandbox/worker documentation. Product check semantics live in docs/CHECK.md;
that command is not a substitute for the development test gates.

For native capability mapping, use harness-operations; Codex-specific reference
routing is in .agents/adapters/codex.md. These adapters grant no extra scope.

<!-- agent-config:begin -->
## Shared agent conventions (generated)

- **authority:** Opening, updating or merging a pull request requires authority from the task or declared repository policy. Shared procedures grant no authority themselves; opening or updating authority does not authorize merging.
- **checks:** Run checks appropriate to the affected behavior. Report commands, actual results, blocked prerequisites and remaining scope. A quick check does not replace a declared merge gate.
- **discovery:** Before editing a module, locate applicable ancestor/module instruction files and load only relevant references. Shared workflows and their dependencies are checked in under .agents/skills; Claude discovery copies are generated under .claude/skills.
- **ownership:** Edit repo-owned contracts and local skills at their canonical paths. Shared skills, Claude projections, this managed section and synchronization tooling are generated: change the upstream source or manifest selection and regenerate; do not hand-edit generated copies.
- **workflow-policy:** Repository policy owns contribution process, scope and gates; shared methods do not impose an SDD spec requirement where the repo has none. Native tool names in shared workflows illustrate operations: use equivalent available connected tools, and report unavailable capabilities. No global skill install or personal MCP setup is required for discovery.
<!-- agent-config:end -->
