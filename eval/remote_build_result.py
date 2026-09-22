#!/usr/bin/env python3
"""Write result.json for one .github/workflows/remote-build.yml build job.

Reads the primary (and, when set, compare) `tolmap` invocation's
`/usr/bin/time -v` output and produced JSON, from the working directory
conventions that workflow's "Build with primary binary" / "Build with
compare binary" steps establish:
  primary.time.txt, primary.log, out-primary/<stem>.json
  compare.time.txt, compare.log, out-compare/<stem>.json  (only if compared)

The produced JSON is one of two shapes depending on `inputs.command`:
`build` writes a map (`F`/`E`/`L`/... -- src/schema.rs), `dump-blend`
writes the weighted graph the partitioner receives (`n_nodes`,
`below_prune_floor`, ... -- src/blenddump.rs, issue #57's below-prune-floor
measurement). `files`/`below_prune_floor` below are read from whichever
keys are present rather than branching on COMMAND, so a result.json's
shape always matches what its JSON actually contains.

All inputs come from environment variables (SLUG, COMMIT, BAND,
REPO_ARGS_JSON, EXTRA_ARGS, TOLMAP_REF, COMPARE_REF, COMMAND, STEM,
PRIMARY_EXIT, COMPARE_EXIT, HAS_COMPARE) rather than argv, matching the
workflow step's `env:` block -- this script has exactly one caller and is
not meant to grow a CLI.

Deliberately tolerant of a missing map (a failed or OOM-killed build still
gets a result.json recording that, per CLAUDE.md's "numbers must be a
lower bound" spirit applied to this corpus run itself: a repository that
failed is a recorded fact, not a silently dropped row). `reason`/
`compare_reason` carry the failing log's tail (eval/build_corpus.py's own
`failure_tail`, reproduced here) whenever status is not "built".
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
from urllib.error import URLError
from urllib.request import urlopen


def time_fields(path: Path) -> tuple[float | None, int | None]:
    """Parse `/usr/bin/time -v` output for wall-clock seconds and peak RSS
    in kilobytes (the unit `time -v` itself reports; result.json's
    peak_rss_kb is that value unconverted).
    """
    wall_s = None
    peak_kb = None
    if not path.exists():
        return wall_s, peak_kb
    for line in path.read_text(errors="replace").splitlines():
        key, _, value = line.partition(": ")
        value = value.strip()
        if "Maximum resident set size" in key:
            try:
                peak_kb = int(value)
            except ValueError:
                pass
        elif "Elapsed (wall clock) time" in key:
            parts = value.split(":")
            try:
                wall_s = sum(float(part) * 60**index for index, part in enumerate(reversed(parts)))
            except ValueError:
                pass
    return wall_s, peak_kb


def failure_tail(log_path: Path, lines: int = 8) -> str:
    """Same idea as eval/build_corpus.py's `failure_tail`: the last few
    non-blank lines of the build log, so a failed row's result.json carries
    enough to triage without downloading the full artifact. "no build
    output" covers the case where the log was never written at all (e.g.
    the clone step itself failed before this build step ever ran).
    """
    if not log_path.exists():
        return "no build output"
    tail = log_path.read_text(errors="replace").splitlines()[-lines:]
    return " | ".join(part.strip() for part in tail if part.strip())[-1200:]


def map_membership(doc: dict) -> dict[str, int]:
    return {file: int(node[0]) for file, node in zip(doc.get("F", []), doc.get("N", []))}


def placement(candidate: dict, reference: dict) -> tuple[float, float]:
    """The parity gate's greedy best-Jaccard placement and q delta.

    Mirrors src/parity.rs, including the 0.35 overlap floor and descending
    candidate/reference-id tie break. Kept here so acceptance numbers are
    computed on the runner that built the map rather than on the laptop.
    """
    cand = map_membership(candidate)
    ref = map_membership(reference)
    common = sorted(set(cand) & set(ref))
    cand_groups: dict[int, set[str]] = {}
    ref_groups: dict[int, set[str]] = {}
    for file in common:
        cand_groups.setdefault(cand[file], set()).add(file)
        ref_groups.setdefault(ref[file], set()).add(file)
    pairs = []
    for cand_id, cand_files in cand_groups.items():
        for ref_id, ref_files in ref_groups.items():
            overlap = len(cand_files & ref_files)
            if overlap:
                pairs.append((overlap / len(cand_files | ref_files), cand_id, ref_id))
    pairs.sort(reverse=True)
    used_cand: set[int] = set()
    used_ref: set[int] = set()
    matches: dict[int, int] = {}
    for jaccard, cand_id, ref_id in pairs:
        if cand_id in used_cand or ref_id in used_ref or jaccard < 0.35:
            continue
        matches[cand_id] = ref_id
        used_cand.add(cand_id)
        used_ref.add(ref_id)
    kept = sum(1 for file in common if matches.get(cand[file]) == ref[file])
    fraction = kept / len(common) if common else 0.0
    return fraction, abs(float(candidate.get("q", 0.0)) - float(reference.get("q", 0.0)))


def fixture_reference(stem: str) -> dict | None:
    url = f"https://raw.githubusercontent.com/onsager-ai/tolmap/main/data/{stem}.json"
    try:
        with urlopen(url, timeout=30) as response:
            return json.load(response)
    except (OSError, URLError, json.JSONDecodeError):
        return None


def sha256_and_stats(json_path: Path, fixture_stem: str | None = None) -> dict:
    """SHA-256 and the measurement fields present in a map or blend dump."""
    empty = {
        "sha256": None,
        "files": None,
        "below_prune_floor": None,
        "kept_edges": None,
        "modularity_q": None,
        "districts": None,
        "single_file_district_share": None,
        "landmarks": None,
        "placement": None,
        "modularity_delta": None,
    }
    if not json_path.is_file():
        return empty
    stats = dict(empty)
    stats["sha256"] = hashlib.sha256(json_path.read_bytes()).hexdigest()
    try:
        doc = json.loads(json_path.read_text())
        if "F" in doc:
            stats["files"] = len(doc["F"])
            membership = map_membership(doc)
            sizes: dict[int, int] = {}
            for district in membership.values():
                sizes[district] = sizes.get(district, 0) + 1
            stats["modularity_q"] = doc.get("q")
            stats["districts"] = len(sizes)
            stats["single_file_district_share"] = (
                sum(size for size in sizes.values() if size == 1) / len(membership)
                if membership
                else 0.0
            )
            stats["landmarks"] = len(doc.get("L", []))
            if fixture_stem:
                reference = fixture_reference(fixture_stem)
                if reference is not None:
                    stats["placement"], stats["modularity_delta"] = placement(doc, reference)
        elif "n_nodes" in doc:
            stats["files"] = doc["n_nodes"]
            stats["below_prune_floor"] = doc.get("below_prune_floor")
            stats["kept_edges"] = doc.get("n_pruned_edges")
    except (json.JSONDecodeError, OSError):
        pass
    return stats


def env(name: str, default: str = "") -> str:
    return os.environ.get(name, default)


def main() -> int:
    slug = env("SLUG")
    stem = env("STEM")
    tolmap_ref = env("TOLMAP_REF")
    compare_ref = env("COMPARE_REF") or None
    command = env("COMMAND") or "build"
    has_compare = env("HAS_COMPARE") == "true"

    primary_wall, primary_peak = time_fields(Path("primary.time.txt"))
    fixture_stem = stem if env("BAND") == "fixture" and command == "build" else None
    primary_stats = sha256_and_stats(
        Path("out-primary") / f"{stem}.json", fixture_stem=fixture_stem
    )
    primary_sha = primary_stats["sha256"]
    primary_exit_raw = env("PRIMARY_EXIT")
    primary_exit = int(primary_exit_raw) if primary_exit_raw.strip() else None
    primary_status = "built" if primary_exit == 0 and primary_sha else "failed"
    primary_reason = None if primary_status == "built" else failure_tail(Path("primary.log"))

    result = {
        "slug": slug,
        "commit": env("COMMIT"),
        "band": env("BAND"),
        "args": json.loads(env("REPO_ARGS_JSON") or "[]"),
        "extra_args": env("EXTRA_ARGS"),
        "tolmap_ref": tolmap_ref,
        "compare_ref": compare_ref,
        "command": command,
        "status": primary_status,
        "exit_code": primary_exit,
        "reason": primary_reason,
        "peak_rss_kb": primary_peak,
        "wall_s": primary_wall,
        "map_sha256": primary_sha,
        "files": primary_stats["files"],
        "below_prune_floor": primary_stats["below_prune_floor"],
        "kept_edges": primary_stats["kept_edges"],
        "modularity_q": primary_stats["modularity_q"],
        "districts": primary_stats["districts"],
        "single_file_district_share": primary_stats["single_file_district_share"],
        "landmarks": primary_stats["landmarks"],
        "placement": primary_stats["placement"],
        "modularity_delta": primary_stats["modularity_delta"],
        "stability_back": int(env("BACK")) if env("BACK").strip() else None,
        "stability_retention": None,
        "compare_status": None,
        "compare_exit_code": None,
        "compare_reason": None,
        "compare_peak_rss_kb": None,
        "compare_wall_s": None,
        "compare_map_sha256": None,
        "compare_files": None,
        "compare_below_prune_floor": None,
        "identical": None,
    }

    if command == "stability" and primary_status == "built":
        previous_path = Path("out-previous") / f"{stem}.json"
        current_path = Path("out-primary") / f"{stem}.json"
        try:
            previous_doc = json.loads(previous_path.read_text())
            current_doc = json.loads(current_path.read_text())
            result["stability_retention"], _ = placement(current_doc, previous_doc)
        except (json.JSONDecodeError, OSError):
            pass

    if has_compare:
        compare_wall, compare_peak = time_fields(Path("compare.time.txt"))
        compare_stats = sha256_and_stats(Path("out-compare") / f"{stem}.json")
        compare_sha = compare_stats["sha256"]
        compare_exit_raw = env("COMPARE_EXIT")
        compare_exit = int(compare_exit_raw) if compare_exit_raw.strip() else None
        compare_status = "built" if compare_exit == 0 and compare_sha else "failed"
        compare_reason = None if compare_status == "built" else failure_tail(Path("compare.log"))
        result.update(
            compare_status=compare_status,
            compare_exit_code=compare_exit,
            compare_reason=compare_reason,
            compare_peak_rss_kb=compare_peak,
            compare_wall_s=compare_wall,
            compare_map_sha256=compare_sha,
            compare_files=compare_stats["files"],
            compare_below_prune_floor=compare_stats["below_prune_floor"],
        )
        if primary_status == "built" and compare_status == "built":
            result["identical"] = primary_sha == compare_sha

    Path("result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(f"{slug}: {primary_status}" + (f", compare {result['compare_status']}" if has_compare else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
