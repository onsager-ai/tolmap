#!/usr/bin/env python3
"""Pull a .github/workflows/remote-build.yml run's artifacts onto this
machine, so the viewer and eval/corpus_stats.py can use maps that were
built on a GitHub-hosted runner instead of here.

This machine hard-resets under sustained load (CLAUDE.md), which is the
whole reason the heavy work moved to Actions; this script is deliberately
light -- one `gh run download` and some file copying, no parsing of the
maps themselves.

A run's `command` (recorded in each result.json by
eval/remote_build_result.py) decides where its JSON lands, because `build`
and `dump-blend` (issue #57's below-prune-floor measurement) produce
different document shapes (src/schema.rs's map vs. src/blenddump.rs's
weighted graph) -- putting a dump-blend document under maps/ would corrupt
eval/corpus_stats.py, which expects every maps/*.json to be an actual map.

Writes:
  - $TOLMAP_CORPUS_DIR/maps/<stem>.json for every `build` repo the run
    built successfully with its primary tolmap ref (same naming as
    eval/build_corpus.py, so eval/corpus_stats.py and the viewer need no
    changes to read them).
  - $TOLMAP_CORPUS_DIR/maps/<stem>.compare.json for every `build` repo the
    run also built with a `compare_ref` binary (present only on comparison
    runs; not read by corpus_stats.py, kept for manual inspection and
    diffing against the primary map).
  - $TOLMAP_CORPUS_DIR/blends/<stem>.json (and .compare.json) for every
    `dump-blend` repo instead of maps/, for the same reason.
  - $TOLMAP_CORPUS_DIR/builds.json, merged: this run's results are set for
    the slugs it covers; every other slug already in the file (e.g. from a
    local eval/build_corpus.py run, or an earlier remote run) is left
    alone. Same {"version": 1, "repos": {...}} shape build_corpus.py
    writes, so corpus_stats.py needs no changes either. dump-blend rows
    land in this same file (their `files`/`below_prune_floor` fields are
    still useful to see alongside build rows); corpus_stats.py reads
    `entry["slug"]` against eval/corpus.toml's manifest, which dump-blend
    slugs are not necessarily part of, so it never looks at them.

Requires the `gh` CLI, authenticated for onsager-ai/tolmap (public repo,
so read access is all `gh run download` needs).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

DEFAULT_CORPUS_DIR = Path(
    os.environ.get("TOLMAP_CORPUS_DIR", "~/.cache/tolmap-corpus")
).expanduser()
DEFAULT_REPO = "onsager-ai/tolmap"
RUN_URL_RE = re.compile(r"/actions/runs/(\d+)")


def resolve_run_id(value: str) -> str:
    match = RUN_URL_RE.search(value)
    return match.group(1) if match else value


def load_results(path: Path) -> dict[str, dict]:
    if not path.exists():
        return {}
    with path.open() as handle:
        raw = json.load(handle)
    return raw.get("repos", raw)


def save_results(path: Path, results: dict[str, dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as handle:
        json.dump({"version": 1, "repos": results}, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temp = Path(handle.name)
    temp.replace(path)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--run",
        required=True,
        help="workflow run id, or its https://github.com/.../actions/runs/<id> URL",
    )
    parser.add_argument("--repo", default=DEFAULT_REPO, help="owner/repo the run belongs to")
    parser.add_argument("--corpus-dir", type=Path, default=DEFAULT_CORPUS_DIR)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    run_id = resolve_run_id(args.run)
    corpus = args.corpus_dir.expanduser()
    maps_dir = corpus / "maps"
    blends_dir = corpus / "blends"
    results_path = corpus / "builds.json"
    results = load_results(results_path)

    with tempfile.TemporaryDirectory(prefix="tolmap-remote-build-") as tmp:
        tmp_path = Path(tmp)
        print(f"downloading artifacts for run {run_id} ({args.repo})...")
        subprocess.run(
            ["gh", "run", "download", run_id, "--repo", args.repo, "--dir", str(tmp_path)],
            check=True,
        )

        written_maps = 0
        written_blends = 0
        written_results = 0
        for entry_dir in sorted(tmp_path.glob("result-*")):
            result_file = entry_dir / "result.json"
            if not result_file.is_file():
                continue
            result = json.loads(result_file.read_text())
            slug = result["slug"]
            stem = slug.replace("/", "__")
            # Older result.json (before the dump-blend addition) has no
            # "command" key -- everything before that was a map build.
            is_blend = result.get("command") == "dump-blend"
            out_dir = blends_dir if is_blend else maps_dir
            out_dir.mkdir(parents=True, exist_ok=True)

            primary_json = entry_dir / f"{stem}.json"
            if primary_json.is_file():
                shutil.copy2(primary_json, out_dir / f"{stem}.json")
                written_blends += is_blend
                written_maps += not is_blend

            compare_json = entry_dir / f"{stem}.compare.json"
            if compare_json.is_file():
                shutil.copy2(compare_json, out_dir / f"{stem}.compare.json")

            results[slug] = result
            written_results += 1

        if written_results == 0:
            print(
                f"no result-* artifacts with a result.json found in run {run_id}; "
                "nothing written",
                file=sys.stderr,
            )
            return 1

    save_results(results_path, results)
    print(
        f"wrote {written_maps} map(s) to {maps_dir}, {written_blends} blend dump(s) to "
        f"{blends_dir}, and merged {written_results} result(s) into {results_path}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
