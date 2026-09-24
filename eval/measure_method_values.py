#!/usr/bin/env python3
"""Compare method-value symbol edges on a remote-build runner.

This reads full corpus artifacts and must not run on the development laptop.
"""

import argparse
import hashlib
import json
from pathlib import Path


def symbol_stats(path: Path) -> tuple[dict, dict]:
    doc = json.loads(path.read_text())
    coverage = doc["coverage"]
    return doc, {
        "edges": len(doc["edges"]),
        "edge_occurrences": sum(edge[2] for edge in doc["edges"]),
        "calls_resolved": coverage["calls_resolved"],
        "calls_total": coverage["calls_total"],
    }


def django_edges(map_path: Path, doc: dict) -> dict[str, int]:
    files = json.loads(map_path.read_text())["F"]
    file_id = files.index("django/core/handlers/base.py")
    def symbol(name: str) -> int:
        matches = [i for i, row in enumerate(doc["symbols"])
                   if row[0] == file_id and row[1] == name]
        assert len(matches) == 1, (name, matches)
        return matches[0]

    source = symbol("load_middleware")
    targets = {name: symbol(name) for name in ("_get_response", "_get_response_async")}
    return {
        name: sum(edge[2] for edge in doc["edges"]
                  if edge[0] == source and edge[1] == target)
        for name, target in targets.items()
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact", type=Path, required=True)
    parser.add_argument("--stem", required=True)
    parser.add_argument("--slug", required=True)
    args = parser.parse_args()
    base = args.artifact / args.stem
    primary_map = base.with_suffix(".json")
    compare_map = args.artifact / f"{args.stem}.compare.json"
    primary_bytes = primary_map.read_bytes()
    compare_bytes = compare_map.read_bytes()
    assert primary_bytes == compare_bytes, "map document changed"
    current, current_stats = symbol_stats(args.artifact / f"{args.stem}.symbols.json")
    previous, previous_stats = symbol_stats(
        args.artifact / f"{args.stem}.compare.symbols.json"
    )
    result = {
        "slug": args.slug,
        "map_sha256": hashlib.sha256(primary_bytes).hexdigest(),
        "map_identical": True,
        "after": current_stats,
        "before": previous_stats,
    }
    if args.slug == "django/django":
        after = django_edges(primary_map, current)
        before = django_edges(compare_map, previous)
        assert all(count > 0 for count in after.values()), after
        # The original measurement compared the fix to a pre-#99 binary.
        # Once #99 is on main, a paired progress build must retain its edges.
        if all(count == 0 for count in before.values()):
            result["baseline"] = "before_method_values"
        else:
            assert after == before, {"after": after, "before": before}
            result["baseline"] = "method_values_present"
        result["django_load_middleware"] = {"after": after, "before": before}
    output = args.artifact / "method_value_measure.json"
    output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(output.read_text())


if __name__ == "__main__":
    main()
