"""Check a `tolmap polyglot-report` JSON output against the ceilings
docs/FINDINGS.md finding 13 records for the synthetic polyglot fixture
(`eval/gen_synthetic_polyglot_fixture.py`). CI's gate job runs this after
`tolmap polyglot-report --all-sources` on a freshly generated copy of that
fixture.

These are *ceilings*, not a target to approach: the fixture and pipeline are
both fully deterministic (finding 9), so the underlying numbers are exact
and reproducible, not a distribution a ceiling needs to buffer against
sampling noise. The headroom above the measured values (recorded in finding
13) exists so an unrelated, legitimate change to the naming/layout/geometry
stages doesn't fail this check; what it must still catch is a real
regression in the union-extraction or blend/prune machinery -- a language
collapsing entirely below the prune floor, or the merge silently stopping
producing any cross-language structure at all.

Per CLAUDE.md, these numbers are not retuned from what step 2 measured --
finding 13 records the measurement, this script enforces it, and changing a
ceiling is the same kind of decision changing `ALL_SOURCES_MIN_FILES` is
(the user's, not an automated one).

    python eval/check_polyglot_ceilings.py <report.json>
"""
import json
import sys

# See docs/FINDINGS.md finding 13 for the measured values these ceilings
# leave headroom above (NMI 0.878, adjusted Rand 0.920, go below-floor-
# merged 25.0%, ts below-floor-merged 0.0%, on the 50-file synthetic
# fixture committed via eval/gen_synthetic_polyglot_fixture.py).
NMI_CEILING = 0.95
BELOW_PRUNE_FLOOR_CEILING = 0.5


def main(argv=None):
    argv = argv if argv is not None else sys.argv[1:]
    if len(argv) != 1:
        sys.exit("usage: check_polyglot_ceilings.py <report.json>")
    with open(argv[0]) as f:
        report = json.load(f)

    failures = []

    nmi = report["clustering_vs_language"]["nmi"]
    if nmi > NMI_CEILING:
        failures.append(
            f"NMI {nmi:.4f} exceeds ceiling {NMI_CEILING} -- district membership is "
            "tracking the language label almost exactly (the map redrew the file "
            "extension); see finding 13"
        )
    else:
        print(f"NMI {nmi:.4f} <= ceiling {NMI_CEILING}: ok")

    for lang, stats in sorted(report["per_language"].items()):
        share = stats["below_prune_floor_merged"]
        if share > BELOW_PRUNE_FLOOR_CEILING:
            failures.append(
                f"{lang}: below-prune-floor share in the merged graph is {share:.4f}, "
                f"exceeds ceiling {BELOW_PRUNE_FLOOR_CEILING} -- finding 10's hazard "
                f"(one language's dominant edge dragging another's distribution under "
                f"the floor) one level up; see finding 13"
            )
        else:
            print(f"{lang}: below-prune-floor share {share:.4f} <= ceiling {BELOW_PRUNE_FLOOR_CEILING}: ok")

    if failures:
        sys.exit("\n".join(["polyglot ceilings FAILED:"] + failures))
    print("all polyglot ceilings ok")


if __name__ == "__main__":
    main()
