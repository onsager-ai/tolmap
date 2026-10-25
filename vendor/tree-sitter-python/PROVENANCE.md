# Pinned Python grammar scanner repair

Upstream: https://github.com/tree-sitter/tree-sitter-python, version 0.25.0,
crate VCS commit `293fdc02038ee2bf0e2e206711b69c90ac0d413f`.
Original crates.io archive SHA-256 (previous Cargo.lock checksum):
`6bf85fd39652e740bf60f46f4cda9492c3a9ad75880575bf14960f775cb74a1c`.
MIT license and copyright are preserved in LICENSE.

UPSTREAM-SHA256SUMS records every copied file's original SHA-256. All files
except src/scanner.c are byte-identical to the registry crate, including
parser.c, grammar.json, node-types.json, C headers, Rust bindings, and build.rs.
No grammar regeneration, parser-table edit, tree-sitter core change, or query
change is involved. Cargo's existing cc build compiles the same pinned parser
and the patched scanner; the path patch removes only the registry source and
checksum from this one Cargo.lock package. CI validates --locked resolution.

## Measured failure

Original profile run 40233256723 / job 121289322773 / artifact 12614152595
(archive SHA-256 `8653eb1fa9afa666392656814d39321b58cd3f6a46591e16757b71c09707c12c`)
sampled the external scanner's comment skip loop in all five stacks, before
tolmap's code-line collection. The original production and symbol-enabled
release builds both exceeded the unchanged 30s normal-fixture probe.

Mechanism run 40235342996 / job 121295596575 / artifact 12615160818
(archive SHA-256 `402db4544fe32a420368a439c8a6c6aeb7e01567081a6b5cc860fa10c23b3378`)
measured 1k/2k/4k/8k/16k comments at 0.10038/0.38979/1.56327/6.26980/24.85490s.
External state 10 enables STRING_START, COMMENT and EXCEPT (mask 2184), with
error recovery false. It returned false on all 1002/2002/4002/8002/16002 calls
while skipping 15534520/62069020/248138020/992276020/3968552020 characters.
mark_end precedes lookahead, so each following comment repeats that lookahead.

Upstream [PR 242](https://github.com/tree-sitter/tree-sitter-python/pull/242)
introduced a guard against unrelated comment scans. Its guard, plus EXCEPT,
already exists in 0.25.0. Reapplying it or upgrading to master is not this fix.

## Equivalence boundary

The added early return applies only after a comment is found, with neither
INDENT nor NEWLINE valid and that first comment's indentation at least the
current indentation. The unchanged subsequent code cannot emit INDENT or
NEWLINE; its explicit and implicit DEDENT branches require a strictly smaller
first-comment indentation; STRING_START requires no comment. No scanner state
has been mutated and a false scan's lookahead is discarded. Error recovery
requires INDENT and cannot enter this case. All other paths are unchanged.

CI scripts/scanner-probe.py builds original and patched scanners against the
same pinned parser/core. It compares full S-expressions and every node's kind,
byte range, named/extra flags over scaled fixtures, edge cases, all Python
fixture packages pinned by data/fixtures.toml and the upstream test corpus at
the commit above. The full original normal fixture cannot finish its 30s
probe; the full repaired fixture and unchanged long fixture remain mandatory.
CI also checks deterministic skipped-character scaling and Rust reader metrics.
These checks must pass at the final head; this document does not preclaim them.
