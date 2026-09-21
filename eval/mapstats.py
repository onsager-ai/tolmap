"""Derive issue #41's district-shape measurements from a compact map.

The mainland/small/zero-edge/stranded classification lives on unmerged PR
#42, so these columns cannot come from the binary on main. They need only the
committed map schema: `F` (files), `N` (per-file rows whose first element is
the district id), and `E` (kept edges as index pairs into `F`).

A mainland district contains at least 1% of all mapped files. That threshold
is issue #41 measurement 1's descriptive cutoff, not a clustering parameter;
it does not feed back into the map. A district is zero-edge when no kept edge
has both endpoints inside it. A small district is stranded when it has no
cross-district kept edge to any mainland district.

    python eval/mapstats.py out/crawlab.json out/n8n.json
"""
import json
import os
import sys
from collections import defaultdict


def stats(path):
    with open(path) as handle:
        doc = json.load(handle)
    files = doc["F"]
    district = [row[0] for row in doc["N"]]
    members = defaultdict(list)
    for index, district_id in enumerate(district):
        members[district_id].append(index)

    internal = defaultdict(int)
    cross = defaultdict(set)
    for a, b in doc["E"]:
        district_a, district_b = district[a], district[b]
        if district_a == district_b:
            internal[district_a] += 1
        else:
            cross[district_a].add(district_b)
            cross[district_b].add(district_a)

    total = len(files)
    mainland = {d for d, values in members.items()
                if len(values) >= 0.01 * total}
    small = set(members) - mainland
    zero_edge = {d for d in members if internal[d] == 0}
    stranded = {d for d in small if not (cross[d] & mainland)}
    return {
        "files": total,
        "districts": len(members),
        "mainland": len(mainland),
        "mainland_file_share": sum(len(members[d]) for d in mainland) / total,
        "small": len(small),
        "zero_edge_districts": len(zero_edge),
        "zero_edge_files": sum(len(members[d]) for d in zero_edge) / total,
        "small_no_mainland_edge": f"{len(stranded)}/{len(small)}",
        "kept_edges": len(doc["E"]),
        "q": doc["q"],
        "largest_district": max(len(values) for values in members.values()),
    }


def main(paths):
    rows = {os.path.basename(path).removesuffix(".json"): stats(path)
            for path in paths}
    if not rows:
        raise SystemExit("usage: python eval/mapstats.py MAP.json [MAP.json ...]")
    keys = list(next(iter(rows.values())))
    print("| repo | " + " | ".join(keys) + " |")
    print("|" + "---|" * (len(keys) + 1))
    for name, row in rows.items():
        cells = []
        for key in keys:
            value = row[key]
            if key in {"mainland_file_share", "zero_edge_files"}:
                cells.append(f"{value:.1%}")
            elif isinstance(value, float):
                cells.append(f"{value:.4f}")
            else:
                cells.append(str(value))
        print(f"| {name} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main(sys.argv[1:])
