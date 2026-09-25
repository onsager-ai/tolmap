#!/usr/bin/env python3
"""Check a `tolmap build --refs scip` map's reference paths (issue #110 P1a).

    check_scip_references.py MAP --expect py=scip --expect go=scip ...
    check_scip_references.py MAP --expect ts=scip --expect-install ts=installed
    check_scip_references.py MAP --expect ts=hand \
        --expect-install ts=fell_back:sandbox_unavailable
    check_scip_references.py MAP --expect-install ts=none

CI's `scip` job builds the synthetic fixtures with the pinned indexers on the
runner. Three byte-identical builds prove determinism only if SCIP was
actually used: a build where every language fell back to the hand-written
graph is deterministic for free. This asserts each named language took the
expected path, and prints the whole `coverage.references` block so a failed
gate shows why.

`--expect-install LANG=STATUS[:REASON]` (issue #110 P1c) checks the row's
`install` record: `installed`, `skipped` or `fell_back`, optionally with its
reason code, or `none` for a row without one (a build with installs off).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("map", type=Path)
    parser.add_argument("--expect", action="append", default=[], metavar="LANG=PATH")
    parser.add_argument(
        "--expect-install", action="append", default=[], metavar="LANG=STATUS[:REASON]"
    )
    parser.add_argument("--symbols", type=Path, help="symbols document to check for SCIP rows")
    args = parser.parse_args()
    document = json.loads(args.map.read_text())
    references = (document.get("coverage") or {}).get("references")
    print(json.dumps(references, indent=1, sort_keys=True))
    if not references:
        print("no coverage.references in the map: was it built with --refs scip?")
        return 1
    failed = False
    for expectation in args.expect:
        language, path = expectation.split("=", 1)
        row = references.get(language)
        actual = row and row["path"]
        if actual != path:
            print(f"{language}: expected {path}, got {actual} ({row and row['reason']})")
            failed = True
    for expectation in args.expect_install:
        language, expected = expectation.split("=", 1)
        install = (references.get(language) or {}).get("install")
        if expected == "none":
            if install is not None:
                print(f"{language}: expected no install record, got {install}")
                failed = True
            continue
        status, _, reason = expected.partition(":")
        actual = install and (install["status"], install["reason"])
        if not install or install["status"] != status or (reason and install["reason"] != reason):
            print(f"{language}: expected install {expected}, got {actual}")
            failed = True
    if args.symbols:
        symbols = json.loads(args.symbols.read_text())
        kinds = symbols.get("kinds", [])
        if "reference" not in kinds:
            print("symbols document has no 'reference' kind although a language took SCIP")
            failed = True
        counts: dict[str, int] = {}
        for _, _, _, kind in symbols["edges"]:
            counts[kinds[kind]] = counts.get(kinds[kind], 0) + 1
        print("symbol edge rows by kind:", json.dumps(counts, sort_keys=True))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
