#!/usr/bin/env python3
"""Compare typed symbol edges with main on a remote-build runner only."""

import collections
import gzip
import hashlib
import json
import os
from pathlib import Path


def read(path):
    raw = path.read_bytes()
    return json.loads(raw), {
        "raw_bytes": len(raw),
        "gzip_bytes": len(gzip.compress(raw, mtime=0)),
    }


def pairs(document):
    result = collections.Counter()
    for edge in document["edges"]:
        result[tuple(edge[:2])] += edge[2]
    return result


def stats(document, sizes):
    legend = document.get("kinds", ["unknown"])
    kinds = collections.Counter(legend[edge[3] if len(edge) > 3 else 0]
                                for edge in document["edges"])
    occurrences = collections.Counter()
    for edge in document["edges"]:
        occurrences[legend[edge[3] if len(edge) > 3 else 0]] += edge[2]
    return {
        **sizes,
        "edges_by_kind": dict(sorted(kinds.items())),
        "edge_occurrences_by_kind": dict(sorted(occurrences.items())),
        "abstract_symbols": sum(len(row) > 7 and row[7] for row in document["symbols"]),
        "inherited_calls_resolved": document["coverage"].get("inherited_calls_resolved", 0),
        "possible_implementations": document["coverage"].get("possible_implementations", 0),
        "parent_class_method_unresolved": document["coverage"]["unresolved"].get("parent_class_method", 0),
        "calls_total": document["coverage"]["calls_total"],
        "calls_resolved": document["coverage"]["calls_resolved"],
    }


def main():
    artifact = Path(os.environ["ARTIFACT"])
    stem = os.environ["STEM"]
    primary_map = (artifact / f"{stem}.json").read_bytes()
    compare_map = (artifact / f"{stem}.compare.json").read_bytes()
    assert primary_map == compare_map, "map document differs from main"
    current, current_sizes = read(artifact / f"{stem}.symbols.json")
    previous, previous_sizes = read(artifact / f"{stem}.compare.symbols.json")
    before = pairs(previous)
    after = pairs(current)
    losses = {pair: count - after[pair] for pair, count in before.items()
              if after[pair] < count}
    assert not losses, f"existing edge occurrences lost: {len(losses)} pairs; first {list(losses.items())[:5]}"
    result = {
        "slug": os.environ["SLUG"],
        "map_identical": True,
        "map_sha256": hashlib.sha256(primary_map).hexdigest(),
        "existing_pair_count": len(before),
        "existing_pairs_preserved": True,
        "before": stats(previous, previous_sizes),
        "after": stats(current, current_sizes),
    }
    output = artifact / "typed_edges_measure.json"
    output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(output.read_text())


if __name__ == "__main__":
    main()
