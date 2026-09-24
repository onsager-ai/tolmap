"""Measure issue #101's TypeScript workspace-package import fix on a
remote-build runner, primary (this branch) vs. compare (main).

Run after `tolmap build` with `--compare-ref`, on the four monorepos whose
`package.json`/`pnpm-workspace.yaml` declare workspace packages
(langgenius/dify, n8n-io/n8n, microsoft/vscode, twentyhq/twenty). Reads
`out-primary/<stem>.json` and, when present, `out-compare/<stem>.json`
directly -- both are already on disk in the same job that built them, so
this needs no extra `tolmap` invocation.

Counts, for each map: import edges (`len(doc["E"])`), zero-edge files (a
file index absent from every pair in `E`), district count and modularity
(`len(set of districts)`, `doc["q"]`). "Import edges" here is the map's own
committed `E` list, the same quantity finding 20's table reports as
"edges, main -> fix" -- not `dump-blend`'s post-blend/post-prune weighted
graph, which is a different, smaller count downstream of this one.
"""

import json
import os
import sys
from pathlib import Path


def stats(doc: dict) -> dict:
    files = doc.get("F", [])
    edges = doc.get("E", [])
    incident = set()
    for edge in edges:
        incident.add(edge[0])
        incident.add(edge[1])
    membership = [node[0] for node in doc.get("N", [])]
    return {
        "files": len(files),
        "import_edges": len(edges),
        "zero_edge_files": len(files) - len(incident),
        "districts": len(set(membership)),
        "modularity_q": doc.get("q"),
    }


def main() -> int:
    stem = os.environ["STEM"]
    slug = os.environ["SLUG"]
    primary_path = Path(f"out-primary/{stem}.json")
    compare_path = Path(f"out-compare/{stem}.json")
    output_path = Path(os.environ.get("OUTPUT", "artifact/module_resolution_measure.json"))

    if not primary_path.is_file():
        print(f"{slug}: no primary map at {primary_path}, skipping", file=sys.stderr)
        return 0

    result = {"slug": slug, "primary": stats(json.loads(primary_path.read_text()))}
    if compare_path.is_file():
        result["compare"] = stats(json.loads(compare_path.read_text()))

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
