"""Combine per-repository ETA replay errors from remote-build artifacts."""

import json
import statistics
import sys
from pathlib import Path


def main() -> None:
    root, output, summary = map(Path, sys.argv[1:4])
    rows = []
    for path in sorted(root.glob("result-*/eta_replay.json")):
        result = path.parent / "result.json"
        slug = json.loads(result.read_text())["slug"] if result.exists() else path.parent.name
        rows.append({"slug": slug, **json.loads(path.read_text())})
    medians = {
        p: statistics.median(row["checkpoints"][p]["absolute_error_s"] for row in rows)
        for p in ("10", "50", "90")
    } if rows else {}
    output.write_text(json.dumps({"repositories": rows, "median_absolute_error_s": medians}, indent=2) + "\n")
    if rows:
        with summary.open("a") as out:
            out.write("\n## ETA replay\n\n")
            out.write(f"{len(rows)} repositories; median absolute error at 10%, 50%, 90% of wall time: ")
            out.write(", ".join(f"{medians[p]:.2f} s" for p in ("10", "50", "90")) + ".\n")
    print(json.dumps({"count": len(rows), "median_absolute_error_s": medians}))


if __name__ == "__main__":
    main()
