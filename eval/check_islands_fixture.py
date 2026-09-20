"""Check a map built from `eval/gen_synthetic_islands_fixture.py` against the
district classification that fixture is constructed to produce. CI's gate job
runs this after generating the fixture and building it.

This is not a parity check -- there is no reference implementation of issue
#34's classification to compare against, the same way there is none for
polyglot union extraction (docs/ARCHITECTURE.md). It is an exactness check,
and it can be exact precisely because the fixture is synthetic: its three
mainland packages, two islands and two unconnected groups are written by
that generator, so the expected counts below are its construction restated,
not a measurement with tolerance around it. The nine acceptance fixtures in
`data/` cannot stand in for it -- eight have no sub-1% district at all and
the ninth (prometheus) has three, none of which the gate's offline path
builds.

What a failure here means: either `geometry::classify_districts` stopped
separating the three classes, or `pipeline::merge_tiny` started folding the
small groups into their neighbours again (the generator's docstring records
that dead end at length), or the fixture generator drifted from the
classification rule. All three are real regressions; none should be fixed by
editing the numbers below.

    python3 eval/check_islands_fixture.py <map.json>
"""
import collections
import json
import sys

# eval/gen_synthetic_islands_fixture.py: core/web/worker (120 files each),
# islands/reporting + islands/billing (3 files each), unfiled/group_a (2)
# and unfiled/group_b (3).
EXPECTED = {"mainland": 3, "island": 2, "unconnected": 2}


def main(argv=None):
    argv = argv if argv is not None else sys.argv[1:]
    if len(argv) != 1:
        sys.exit("usage: check_islands_fixture.py <map.json>")
    with open(argv[0]) as f:
        doc = json.load(f)

    districts = doc["districts"]
    failures = []

    missing = [d for d, entry in districts.items() if "class" not in entry]
    if missing:
        failures.append(
            f"{len(missing)} district(s) carry no `class` field -- the schema "
            "field did not reach the map JSON at all (src/schema.rs)"
        )

    counts = collections.Counter(entry.get("class") for entry in districts.values())
    for name, expected in sorted(EXPECTED.items()):
        got = counts.get(name, 0)
        if got != expected:
            failures.append(f"{name}: expected {expected} district(s), got {got}")
        else:
            print(f"{name}: {got} district(s): ok")
    unexpected = sorted(set(counts) - set(EXPECTED) - {None})
    if unexpected:
        failures.append(f"unexpected district class(es) in the map: {unexpected}")

    # Unconnected districts are listed, not drawn (finding 17): geometry.rs
    # drops their polygons. Islands keep theirs -- but `blobs::contours` can
    # legitimately fail to close a polygon around a very small district, so
    # only the unconnected side is asserted here.
    drawn = [d for d, e in districts.items() if e.get("class") == "unconnected" and e["blob"]]
    if drawn:
        failures.append(
            f"unconnected district(s) {sorted(drawn)} still carry a region polygon -- "
            "they are meant to be listed, not drawn (finding 17)"
        )
    else:
        print("unconnected districts carry no region polygon: ok")

    # Every file keeps a NodeRow whatever its district's class, as the schema
    # requires -- dropping the polygon must not drop the files with it.
    if len(doc["N"]) != len(doc["F"]):
        failures.append(f"N has {len(doc['N'])} rows for {len(doc['F'])} files")
    else:
        print(f"every one of {len(doc['F'])} files has a node row: ok")

    if failures:
        sys.exit("\n".join(["islands fixture check FAILED:"] + failures))
    print("islands fixture check ok")


if __name__ == "__main__":
    main()
