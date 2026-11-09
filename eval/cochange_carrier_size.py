"""Measure the two candidate carriers for debt side data (issues #251, #258).

For each built map and its full-history clone at the same commit, replay
`git_history` in src/extract.rs (up to 4,000 non-merge commits, no rename
detection, commits touching 2..=40 mapped files count towards pairs, a pair
kept at count >= 2 with min(solo) >= 3, weight = count / min(solo)) and size:

  * sidecar: the co-change object inserted into the map document;
  * artifact: the same object as a separate JSON file beside the snapshot.

The carrier is one optional object for data `tolmap debt` needs and the map
does not otherwise hold: `cochange` (#251) and `mods` (#258, the Rust `mod`
declaration tree as `[declaring, declared]` index pairs). Each payload is sized
alone and both together.

`mods` is approximated from source here: a `mod name;` item at any
indentation, with an optional `#[path]`, resolved as rustc does to `name.rs` or
`name/mod.rs`. Inline `mod` blocks and macro-generated `mod`s are skipped, so
the count is a lower bound; the product's resolver (src/extract/rust.rs)
follows them.

Both are index-aligned to the map's `F`. Co-change rows are `[a, b, count, weight]`
with a < b, weight rounded to two places. Each threshold is measured holding
every pair above it ("all") and only those with no `E` or `U` edge between
the two files ("hidden").

The replay is checked against the map: a file's solo commit count is its
churn (`N[i][5]`), so any mismatch is reported rather than hidden.

Usage: python3 -I eval/cochange_carrier_size.py <name> <clone> <map.json>
prints one JSON row.
"""

import gzip
import json
import os
import re
import subprocess
import sys

MAX_COMMITS = 4000
THRESHOLDS = [(2, 0.0), (3, 0.5), (5, 0.5), (5, 0.7), (10, 0.5)]


def history(clone, files):
    out = subprocess.run(
        ["git", "-C", clone, "log", f"-n{MAX_COMMITS}", "--no-merges",
         "--pretty=format:@%H", "--name-only", "--no-renames"],
        check=True, capture_output=True,
    ).stdout.decode("utf-8", "replace")
    file_set = set(files)
    commits, current = [], None
    for line in out.splitlines():
        if line.startswith("@"):
            if current:
                commits.append(current)
            current = set()
        elif line.strip() and line in file_set and current is not None:
            current.add(line)
    if current:
        commits.append(current)
    solo, pairs = {}, {}
    for commit in commits:
        for f in commit:
            solo[f] = solo.get(f, 0) + 1
        if not 2 <= len(commit) <= 40:
            continue
        values = sorted(commit)
        for i in range(len(values)):
            for j in range(i + 1, len(values)):
                key = (values[i], values[j])
                pairs[key] = pairs.get(key, 0) + 1
    kept = {}
    for (a, b), count in pairs.items():
        if min(solo[a], solo[b]) >= 3 and count >= 2:
            kept[(a, b)] = (count, count / min(solo[a], solo[b]))
    return len(commits), solo, kept


MOD = re.compile(
    r'((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r#)?(\w+)\s*;')
PATH = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
COMMENT = re.compile(r'//[^\n]*|/\*.*?\*/', re.S)


def mod_pairs(clone, files, index):
    pairs = set()
    for source in files:
        if not source.endswith(".rs"):
            continue
        try:
            text = open(os.path.join(clone, source), encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            continue
        here = os.path.dirname(source)
        base = os.path.basename(source)
        owns_dir = base in ("lib.rs", "main.rs", "mod.rs")
        child_dir = here if owns_dir else source[:-3]
        for match in MOD.finditer(COMMENT.sub("", text)):
            attrs, name = match.groups()
            path = PATH.search(attrs)
            if path:
                candidates = [os.path.normpath(os.path.join(here, path.group(1)))]
            else:
                candidates = [os.path.join(child_dir, name + ".rs"),
                              os.path.join(child_dir, name, "mod.rs")]
            for target in candidates:
                if target in index:
                    pairs.add((index[source], index[target]))
                    break
    return sorted([a, b] for a, b in pairs)


def dumps(value):
    return json.dumps(value, separators=(",", ":"))


def main():
    name, clone, map_path = sys.argv[1:4]
    raw = open(map_path, "rb").read()
    doc = json.loads(raw)
    files = doc["F"]
    index = {f: i for i, f in enumerate(files)}
    commits, solo, kept = history(clone, files)
    churn_mismatch = sum(
        1 for i, f in enumerate(files) if doc["N"][i][5] != solo.get(f, 0)
    )
    linked = set()
    for a, b in doc["E"]:
        linked.add((min(a, b), max(a, b)))
    for key, users in doc["U"].items():
        target = int(key.split(":")[0])
        for user in users:
            linked.add((min(user, target), max(user, target)))
    map_gz = len(gzip.compress(raw, 9, mtime=0))
    row = {
        "name": name, "files": len(files), "commits": commits,
        "churn_mismatch": churn_mismatch, "kept_pairs": len(kept),
        "map_bytes": len(raw), "map_gzip": map_gz, "thresholds": [],
    }
    mods = mod_pairs(clone, files, index)
    row["rust_files"] = sum(1 for f in files if f.endswith(".rs"))
    row["mod_pairs"] = len(mods)

    def size(carrier):
        body = dumps(carrier).encode()
        assert raw.endswith(b"}")
        with_sidecar = raw[:-1] + b',"D":' + body + b"}"
        return {
            "sidecar_bytes": len(with_sidecar) - len(raw),
            "sidecar_gzip": len(gzip.compress(with_sidecar, 9, mtime=0)) - map_gz,
            "artifact_bytes": len(body),
            "artifact_gzip": len(gzip.compress(body, 9, mtime=0)),
        }

    row["mods_only"] = size({"mods": mods})
    for min_count, min_weight in THRESHOLDS:
        rows = {"all": [], "hidden": []}
        for (a, b), (count, weight) in kept.items():
            if count < min_count or weight < min_weight:
                continue
            i, j = sorted((index[a], index[b]))
            entry = [i, j, count, round(weight, 2)]
            rows["all"].append(entry)
            if (i, j) not in linked:
                rows["hidden"].append(entry)
        result = {"min_count": min_count, "min_weight": min_weight}
        for scope, entries in rows.items():
            entries.sort()
            cochange = {"commits": commits, "window": MAX_COMMITS,
                        "min_count": min_count, "min_weight": min_weight,
                        "pairs": entries}
            result[scope] = {"pairs": len(entries),
                             "cochange": size({"cochange": cochange}),
                             "both": size({"cochange": cochange, "mods": mods})}
        row["thresholds"].append(result)
    print(dumps(row))


if __name__ == "__main__":
    main()
