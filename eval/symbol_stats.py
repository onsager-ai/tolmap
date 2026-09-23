#!/usr/bin/env python3
"""Summarise the separate symbol document and district response sizes.

This reads JSON only. It is safe to run on the development laptop; the
repository build that produced the documents runs in GitHub Actions.
"""

import argparse
import collections
import json
import math
import statistics
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
        if "symbol_rings" in document:
            slice_doc["symbol_rings"] = [document["symbol_rings"][i] for i in all_ids]
            slice_doc["module_rings"] = {key: value for key, value in document["module_rings"].items() if int(key) in files}
            slice_doc["header_rings"] = {key: value for key, value in document["header_rings"].items() if int(key) in all_ids}
        size = len(json.dumps(slice_doc, separators=(",", ":"), ensure_ascii=False).encode())
        if size > largest:
            largest = size
            largest_id = int(district)
    coverage = document["coverage"]
    total = coverage["calls_total"]
    geometry = document.get("symbol_rings", [])
    with_ring = sum(ring is not None for ring in geometry)
    eligible = [i for i, row in enumerate(symbols) if row[6] >= 1 and str(row[0]) in map_doc.get("P", {})]
    missing = sum(i >= len(geometry) or geometry[i] is None for i in eligible)
    by_file = collections.defaultdict(list)
    for i, row in enumerate(symbols):
        if row[5] == -1 and i < len(geometry) and geometry[i]:
            by_file[row[0]].append((area(geometry[i]), row[6]))
    correlations = [pearson(pairs) for pairs in by_file.values() if len(pairs) >= 3]
    correlations = [r for r in correlations if r is not None]
    before = {key: value for key, value in document.items() if key not in ("symbol_rings", "module_rings", "header_rings")}
    delta = dict(document)
    if geometry:
        delta["symbol_rings"] = [encode(ring) if ring else None for ring in geometry]
        delta["module_rings"] = {key: encode(ring) for key, ring in document["module_rings"].items()}
        delta["header_rings"] = {key: encode(ring) for key, ring in document["header_rings"].items()}
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
        "symbols_with_ring": with_ring,
        "eligible_symbols_without_ring": missing,
        "eligible_symbols": len(eligible),
        "files_with_pearson": len(correlations),
        "median_within_file_pearson": statistics.median(correlations) if correlations else None,
        "document_bytes_before_geometry": compact_bytes(before),
        "document_bytes_delta_encoded": compact_bytes(delta),
    }


def compact_bytes(value):
    return len(json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode())


def area(ring):
    return abs(sum(a[0] * b[1] - a[1] * b[0] for a, b in zip(ring, ring[1:] + ring[:1]))) / 2


def pearson(pairs):
    xs, ys = zip(*pairs)
    xm, ym = statistics.mean(xs), statistics.mean(ys)
    numerator = sum((x - xm) * (y - ym) for x, y in pairs)
    denominator = math.sqrt(sum((x - xm) ** 2 for x in xs) * sum((y - ym) ** 2 for y in ys))
    return numerator / denominator if denominator else None


def encode(ring):
    points = [(round(x * 10000), round(y * 10000)) for x, y in ring]
    output = list(points[0])
    for a, b in zip(points, points[1:]):
        output.extend((b[0] - a[0], b[1] - a[1]))
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("map", type=Path)
    parser.add_argument("symbols", type=Path)
    args = parser.parse_args()
    print(json.dumps(measure(args.map, args.symbols), indent=2, sort_keys=True))
