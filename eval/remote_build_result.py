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
REPO_ARGS_JSON, TOLMAP_REF, COMPARE_REF, COMMAND, STEM, PRIMARY_EXIT,
COMPARE_EXIT, HAS_COMPARE) rather than argv, matching the workflow step's
`env:` block -- this script has exactly one caller and is not meant to grow
a CLI.

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


def sha256_and_stats(json_path: Path) -> tuple[str | None, int | None, float | None]:
    """SHA-256 of the file, plus a file/node count and (when present) the
    dump-blend `below_prune_floor` share. `F` (a map's file list) and
    `n_nodes` (dump-blend's node count) are mutually exclusive keys, so
    trying `F` first and falling back to `n_nodes` picks the right one for
    either JSON shape without needing to know which command produced it.
    """
    if not json_path.is_file():
        return None, None, None
    digest = hashlib.sha256(json_path.read_bytes()).hexdigest()
    files = None
    below_prune_floor = None
    try:
        doc = json.loads(json_path.read_text())
        if "F" in doc:
            files = len(doc["F"])
        elif "n_nodes" in doc:
            files = doc["n_nodes"]
        below_prune_floor = doc.get("below_prune_floor")
    except (json.JSONDecodeError, OSError):
        pass
    return digest, files, below_prune_floor


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
    primary_sha, primary_files, primary_floor = sha256_and_stats(Path("out-primary") / f"{stem}.json")
    primary_exit_raw = env("PRIMARY_EXIT")
    primary_exit = int(primary_exit_raw) if primary_exit_raw.strip() else None
    primary_status = "built" if primary_exit == 0 and primary_sha else "failed"
    primary_reason = None if primary_status == "built" else failure_tail(Path("primary.log"))

    result = {
        "slug": slug,
        "commit": env("COMMIT"),
        "band": env("BAND"),
        "args": json.loads(env("REPO_ARGS_JSON") or "[]"),
        "tolmap_ref": tolmap_ref,
        "compare_ref": compare_ref,
        "command": command,
        "status": primary_status,
        "exit_code": primary_exit,
        "reason": primary_reason,
        "peak_rss_kb": primary_peak,
        "wall_s": primary_wall,
        "map_sha256": primary_sha,
        "files": primary_files,
        "below_prune_floor": primary_floor,
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

    if has_compare:
        compare_wall, compare_peak = time_fields(Path("compare.time.txt"))
        compare_sha, compare_files, compare_floor = sha256_and_stats(Path("out-compare") / f"{stem}.json")
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
            compare_files=compare_files,
            compare_below_prune_floor=compare_floor,
        )
        if primary_status == "built" and compare_status == "built":
            result["identical"] = primary_sha == compare_sha

    Path("result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(f"{slug}: {primary_status}" + (f", compare {result['compare_status']}" if has_compare else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
