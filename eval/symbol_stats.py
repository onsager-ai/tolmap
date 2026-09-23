#!/usr/bin/env python3
"""Summarise the separate symbol document and district response sizes.

This reads JSON only. It is safe to run on the development laptop; the
repository build that produced the documents runs in GitHub Actions.
"""

import argparse
import collections
import json
from pathlib import Path


def measure(map_file: Path, symbols_file: Path) -> dict:
    map_doc = json.loads(map_file.read_text())
    document = json.loads(symbols_file.read_text())
    symbols = document["symbols"]
    edges = document["edges"]
    by_kind = dict(sorted(collections.Counter(row[2] for row in symbols).items()))
    largest = 0
    largest_id = None
    for district in map_doc["districts"]:
        files = {i for i, node in enumerate(map_doc["N"]) if str(node[0]) == district}
        local_ids = {i for i, row in enumerate(symbols) if row[0] in files}
        touching = [edge for edge in edges if edge[0] in local_ids or edge[1] in local_ids]
        all_ids = sorted(local_ids | {end for edge in touching for end in edge[:2]})
        slice_doc = {
            "district": int(district),
            "files": sorted(files),
            "symbol_indices": all_ids,
            "symbols": [symbols[i] for i in all_ids],
            "edges": touching,
            "module_code_lines": {key: value for key, value in document["module_code_lines"].items() if int(key) in files},
        }
        size = len(json.dumps(slice_doc, separators=(",", ":"), ensure_ascii=False).encode())
        if size > largest:
            largest = size
            largest_id = int(district)
    coverage = document["coverage"]
    total = coverage["calls_total"]
    return {
        "symbols": len(symbols),
        "by_kind": by_kind,
        "edges": len(edges),
        "calls_total": total,
        "calls_resolved": coverage["calls_resolved"],
        "resolution_rate": coverage["calls_resolved"] / total if total else 0,
        "unresolved": coverage["unresolved"],
        "document_bytes": symbols_file.stat().st_size,
        "largest_district": largest_id,
        "largest_district_bytes": largest,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("map", type=Path)
    parser.add_argument("symbols", type=Path)
    args = parser.parse_args()
    print(json.dumps(measure(args.map, args.symbols), indent=2, sort_keys=True))
