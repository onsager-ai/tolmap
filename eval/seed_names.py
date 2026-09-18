"""Rebuild the naming cache for a fixture from the fixture itself.

Regenerating a map in `data/` is only reproducible if the district names come
back unchanged. They do not by default: the model hook in `naming.py` is
defined and unwired, so a bare rerun falls back to the deterministic IDF namer
and renames every district in the corpus -- scrapy's `crawl control` becomes
`downloadermiddlewares & extensio`, truncated at 32 characters. Name drift
invalidates every spatial memory a team has built, which is worse than a
mediocre name, so a rerun must never be the thing that causes it.

`name_districts()` keys its cache on `fingerprint(members)` -- a sha1 of the
sorted member paths -- and consults nothing else, so the cache survives
district renumbering and depends only on membership being identical. The
committed fixture carries both halves: `names` maps district id to name, and
`F`/`N` give each district its members. That is enough to reconstruct the
cache exactly, which is why this is derived here rather than committed as a
second copy of the names that could drift from the map's own.

    python eval/seed_names.py out/ scrapy django ...

Then build into that same `--out` directory and the previous names return
verbatim. If membership has genuinely changed, the fingerprint misses and the
namer runs -- which is the correct signal, not a failure.
"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src"))

from tolmap import naming                                          # noqa: E402

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data")


def seed(name, out_dir, data_dir=DATA):
    doc = json.load(open(os.path.join(data_dir, f"{name}.json")))
    by = {}
    for f, row in zip(doc["F"], doc["N"]):
        by.setdefault(row[0], []).append(f)
    cache = {}
    for d, members in by.items():
        nm = doc["names"].get(str(d))
        if nm is None:
            raise SystemExit(f"{name}: district {d} has no name in the fixture")
        cache[naming.fingerprint(members)] = {
            "name": nm, "district": d, "size": len(members)}
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, f"{name}.names.json")
    json.dump(cache, open(path, "w"), indent=1, sort_keys=True)
    return path, len(cache)


def main(argv):
    if len(argv) < 3:
        raise SystemExit("usage: seed_names.py <out_dir> <map name> [<map name> ...]")
    out_dir = argv[1]
    for name in argv[2:]:
        path, n = seed(name, out_dir)
        print(f"{name:12} {n:2d} district names -> {path}")


if __name__ == "__main__":
    main(sys.argv)
