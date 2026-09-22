#!/usr/bin/env python3
"""Combine .github/workflows/remote-build.yml's per-repo result.json files
into one results.json artifact and a markdown table written to
$GITHUB_STEP_SUMMARY.

Reads `--results-dir` (as `actions/download-artifact` with
`pattern: result-*` lays it out: one `result-<stem>/` subdirectory per
build job, each holding that job's `result.json`, its map(s), and its
logs -- see eval/remote_build_result.py for result.json's fields and
eval/fetch_remote_build.py for the local-side counterpart of this
layout). A build job that failed before writing result.json (e.g. the
runner itself ran out of disk) is skipped with a note on stderr rather
than crashing the summary -- one missing job's record should not blank
out the other 131.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def load_results(results_dir: Path) -> list[dict]:
    results = []
    for entry_dir in sorted(results_dir.glob("result-*")):
        result_file = entry_dir / "result.json"
        if not result_file.is_file():
            print(f"skip {entry_dir.name}: no result.json", file=sys.stderr)
            continue
        try:
            results.append(json.loads(result_file.read_text()))
        except json.JSONDecodeError as exc:
            print(f"skip {entry_dir.name}: malformed result.json ({exc})", file=sys.stderr)
    results.sort(key=lambda row: (row.get("band", ""), row.get("slug", "")))
    return results


def fmt(value, digits: int = 1) -> str:
    if value is None:
        return "—"
    if isinstance(value, float):
        return f"{value:.{digits}f}"
    return str(value)


def short_sha(value: str | None) -> str:
    return value[:12] if value else "—"


def write_markdown(results: list[dict], path: Path) -> None:
    has_compare = any(row.get("compare_ref") for row in results)
    # Set only on `dump-blend` rows (src/blenddump.rs) -- issue #57's
    # below-prune-floor measurement. A `build` run's rows leave this
    # column out entirely rather than showing an all-"—" column.
    has_floor = any(row.get("below_prune_floor") is not None for row in results)
    built = sum(1 for row in results if row.get("status") == "built")
    failed = len(results) - built
    lines = [
        "## Remote build results",
        "",
        f"{len(results)} repositories: {built} built, {failed} failed.",
        "",
    ]
    header_cells = ["slug", "band", "status", "files", "wall (s)", "peak RSS (KB)", "map sha256"]
    sep_cells = ["---", "---", "---", "---:", "---:", "---:", "---"]
    if has_floor:
        header_cells.append("below prune floor")
        sep_cells.append("---:")
    if has_compare:
        header_cells += ["compare status", "identical"]
        sep_cells += ["---", "---"]
    lines += ["| " + " | ".join(header_cells) + " |", "|" + "|".join(sep_cells) + "|"]
    for row in results:
        cells = [
            row.get("slug", ""),
            row.get("band", ""),
            row.get("status", ""),
            fmt(row.get("files"), 0),
            fmt(row.get("wall_s")),
            fmt(row.get("peak_rss_kb"), 0),
            short_sha(row.get("map_sha256")),
        ]
        if has_floor:
            floor = row.get("below_prune_floor")
            cells.append("—" if floor is None else f"{floor:.1%}")
        if has_compare:
            identical = row.get("identical")
            identical_cell = "—" if identical is None else ("yes" if identical else "**no**")
            cells += [row.get("compare_status") or "—", identical_cell]
        lines.append("| " + " | ".join(cells) + " |")
    with path.open("a") as handle:
        handle.write("\n".join(lines) + "\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    results = load_results(args.results_dir)
    args.out.write_text(json.dumps({"version": 1, "results": results}, indent=2, sort_keys=True) + "\n")
    write_markdown(results, args.summary)
    print(f"wrote {len(results)} result(s) to {args.out} and a summary table to {args.summary}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
