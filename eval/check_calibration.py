#!/usr/bin/env python3
"""Summarise a `tolmap check` history replay (issue #170 PR 2): the
distributions of `districts_crossed` and `delta_q` per repository, how
often each candidate threshold would have fired, and the commits with the
most negative Δq, so a person can judge whether they are real coupling
regressions.

The replay itself runs on GitHub Actions (remote-build.yml,
`command=check-calibrate`, which writes `check-cal-<stem>-<shard>`
artifacts through eval/check_replay.py). This script only reads JSON, so it
is safe to run anywhere:

    python3 eval/check_calibration.py --run <run id or URL>   # gh run download
    python3 eval/check_calibration.py --dir <downloaded artifacts>
    python3 eval/check_calibration.py self-test

Percentiles are nearest-rank (the smallest value with at least p% of the
sample at or below it), so every number printed is a value some commit
actually had. Every rate is over all replayed commits with a report,
including the many that change no file on the map: that is the population
a CI gate sees.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path

DEFAULT_REPO = "onsager-ai/tolmap"
RUN_URL_RE = re.compile(r"/actions/runs/(\d+)")
PERCENTILES = (50, 90, 95, 99)
# The lower tail is where a Δq threshold fires, so Δq also gets these.
LOW_PERCENTILES = (1, 5, 10)
DISTRICT_CANDIDATES = tuple(range(1, 13))
# Finding 61: with co-change held at the base, a single new cross-district
# import moves Δq by about 1e-4, so the scan starts well below that.
DQ_CANDIDATES = (0.00005, 0.0001, 0.00015, 0.0002, 0.0003, 0.0005, 0.001, 0.005, 0.01)
TOP_DROPS = 10


def percentile(values: list[float], p: float) -> float:
    """Nearest-rank percentile of a non-empty sample."""
    ordered = sorted(values)
    rank = max(1, math.ceil(p / 100 * len(ordered)))
    return ordered[rank - 1]


def fire_rate(rows: list[dict], max_districts: int | None, max_dq: float | None) -> tuple[int, int]:
    """(fired, total) under the check's own rule: districts_crossed >
    max_districts, or delta_q < -max_dq."""
    fired = sum(
        1
        for row in rows
        if (max_districts is not None and row["districts_crossed"] > max_districts)
        or (max_dq is not None and row["delta_q"] < -max_dq)
    )
    return fired, len(rows)


def load(directory: Path) -> dict[str, dict]:
    """Every shard under `directory`, grouped by repository slug."""
    repos: dict[str, dict] = defaultdict(lambda: {"rows": [], "attempted": [], "pin": None})
    shard_files = sorted(directory.rglob("shard.json"))
    if not shard_files:
        raise SystemExit(f"no shard.json under {directory}: not a check-calibrate download")
    for shard_file in shard_files:
        shard = json.loads(shard_file.read_text())
        repo = repos[shard["slug"]]
        repo["pin"] = shard["pin"]
        commits = shard_file.parent / "commits.jsonl"
        if not commits.exists():
            continue
        for line in commits.read_text().splitlines():
            row = json.loads(line)
            repo["attempted"].append(row)
            if row["report"] is None:
                continue
            report = json.loads((shard_file.parent / row["report"]).read_text())
            repo["rows"].append(
                {
                    "index": row["index"],
                    "commit": row["commit"],
                    "subject": row["subject"],
                    "merge": row["merge"],
                    "wall_s": row["wall_s"],
                    "districts_crossed": report["districts_crossed"],
                    "delta_q": report["delta_q"],
                    "files": len(report["files"]),
                    "unplaced": len(report["unplaced_files"]),
                    "edges_added": len(report["edges_added"]),
                    "edges_removed": len(report["edges_removed"]),
                    "import_edges_added": sum(
                        1 for edge in report["edges_added"] if edge["static_import"]
                    ),
                    "base_files": report.get("base_files"),
                    "base_districts": report.get("base_districts"),
                    "top_edge": report["edges_added"][0] if report["edges_added"] else None,
                }
            )
    for repo in repos.values():
        repo["rows"].sort(key=lambda row: row["index"])
        repo["attempted"].sort(key=lambda row: row["index"])
    return dict(sorted(repos.items()))


def summarise(repos: dict[str, dict]) -> dict:
    out: dict = {"repos": {}}
    for slug, repo in repos.items():
        rows = repo["rows"]
        attempted = repo["attempted"]
        codes: dict[str, int] = defaultdict(int)
        for row in attempted:
            codes[str(row["exit_code"])] += 1
        entry: dict = {
            "pin": repo["pin"],
            "attempted": len(attempted),
            "reports": len(rows),
            "exit_codes": dict(sorted(codes.items())),
            "merges": sum(1 for row in attempted if row["merge"]),
        }
        if rows:
            districts = [row["districts_crossed"] for row in rows]
            dq = [row["delta_q"] for row in rows]
            walls = [row["wall_s"] for row in rows]
            base_files = [row["base_files"] for row in rows if row["base_files"] is not None]
            base_districts = [
                row["base_districts"] for row in rows if row["base_districts"] is not None
            ]
            entry.update(
                {
                    "touching_map": sum(1 for row in rows if row["files"]),
                    "with_unplaced_files": sum(1 for row in rows if row["unplaced"]),
                    "districts_crossed": {
                        "min": min(districts),
                        **{f"p{p}": percentile(districts, p) for p in PERCENTILES},
                        "max": max(districts),
                    },
                    "delta_q": {
                        "min": min(dq),
                        **{f"p{p}": percentile(dq, p) for p in LOW_PERCENTILES},
                        **{f"p{p}": percentile(dq, p) for p in PERCENTILES},
                        "max": max(dq),
                    },
                    "negative_dq": sum(1 for value in dq if value < 0),
                    "wall_s": {"p50": percentile(walls, 50), "max": max(walls)},
                    "base_files": {
                        "min": min(base_files),
                        "max": max(base_files),
                    }
                    if base_files
                    else None,
                    "base_districts": {
                        "min": min(base_districts),
                        "p50": percentile(base_districts, 50),
                        "max": max(base_districts),
                    }
                    if base_districts
                    else None,
                    "fire_rate_max_districts": {
                        str(n): fire_rate(rows, n, None)[0] for n in DISTRICT_CANDIDATES
                    },
                    "fire_rate_max_dq": {
                        f"{x:g}": fire_rate(rows, None, x)[0] for x in DQ_CANDIDATES
                    },
                    "most_negative_dq": [
                        {
                            key: row[key]
                            for key in (
                                "commit",
                                "subject",
                                "delta_q",
                                "districts_crossed",
                                "files",
                                "edges_added",
                                "import_edges_added",
                                "edges_removed",
                                "top_edge",
                            )
                        }
                        for row in sorted(rows, key=lambda row: (row["delta_q"], row["index"]))[
                            :TOP_DROPS
                        ]
                        if row["delta_q"] < 0
                    ],
                }
            )
        out["repos"][slug] = entry
    return out


def markdown(summary: dict, chosen: tuple[int | None, float | None], repos: dict[str, dict]) -> str:
    lines = ["## `tolmap check` history replay (issue #170)", ""]
    lines.append(
        "| repo | commits | reports | exit codes | touch the map | with unplaced files | base files | base districts |"
    )
    lines.append("|---|---|---|---|---|---|---|---|")
    for slug, entry in summary["repos"].items():
        codes = ", ".join(f"{code}: {count}" for code, count in entry["exit_codes"].items())
        files = entry.get("base_files") or {}
        districts = entry.get("base_districts") or {}
        lines.append(
            f"| {slug} | {entry['attempted']} | {entry['reports']} | {codes} | "
            f"{entry.get('touching_map', 0)} | {entry.get('with_unplaced_files', 0)} | "
            f"{files.get('min', '?')}-{files.get('max', '?')} | "
            f"{districts.get('min', '?')}-{districts.get('max', '?')} |"
        )
    lines += ["", "### districts_crossed", ""]
    lines.append("| repo | min | p50 | p90 | p95 | p99 | max |")
    lines.append("|---|---|---|---|---|---|---|")
    for slug, entry in summary["repos"].items():
        stats = entry.get("districts_crossed")
        if stats:
            lines.append(
                f"| {slug} | "
                + " | ".join(str(stats[key]) for key in ("min", "p50", "p90", "p95", "p99", "max"))
                + " |"
            )
    lines += ["", "### delta_q", ""]
    keys = ("min", "p1", "p5", "p10", "p50", "p90", "p95", "p99", "max")
    lines.append("| repo | " + " | ".join(keys) + " | commits with Δq < 0 |")
    lines.append("|---|" + "---|" * (len(keys) + 1))
    for slug, entry in summary["repos"].items():
        stats = entry.get("delta_q")
        if stats:
            lines.append(
                f"| {slug} | "
                + " | ".join(f"{stats[key]:+.6f}" for key in keys)
                + f" | {entry['negative_dq']} |"
            )
    lines += ["", "### Fire rate by candidate threshold (commits that would exit 1)", ""]
    slugs = list(summary["repos"])
    lines.append("| threshold | " + " | ".join(slugs) + " |")
    lines.append("|---|" + "---|" * len(slugs))
    for n in DISTRICT_CANDIDATES:
        cells = []
        for slug in slugs:
            entry = summary["repos"][slug]
            if "fire_rate_max_districts" in entry:
                cells.append(f"{entry['fire_rate_max_districts'][str(n)]}/{entry['reports']}")
            else:
                cells.append("-")
        lines.append(f"| --max-districts {n} | " + " | ".join(cells) + " |")
    for x in DQ_CANDIDATES:
        cells = []
        for slug in slugs:
            entry = summary["repos"][slug]
            if "fire_rate_max_dq" in entry:
                cells.append(f"{entry['fire_rate_max_dq'][f'{x:g}']}/{entry['reports']}")
            else:
                cells.append("-")
        lines.append(f"| --max-dq {x:g} | " + " | ".join(cells) + " |")
    max_districts, max_dq = chosen
    if max_districts is not None or max_dq is not None:
        lines += ["", f"### Chosen: --max-districts {max_districts} --max-dq {max_dq}", ""]
        lines.append("| repo | districts fired | Δq fired | either fired |")
        lines.append("|---|---|---|---|")
        for slug, repo in repos.items():
            rows = repo["rows"]
            if not rows:
                continue
            d = fire_rate(rows, max_districts, None)
            q = fire_rate(rows, None, max_dq)
            e = fire_rate(rows, max_districts, max_dq)
            lines.append(
                f"| {slug} | {d[0]}/{d[1]} | {q[0]}/{q[1]} | {e[0]}/{e[1]} ({100 * e[0] / e[1]:.1f}%) |"
            )
    for slug, entry in summary["repos"].items():
        drops = entry.get("most_negative_dq") or []
        if not drops:
            continue
        lines += ["", f"### {slug}: the {len(drops)} most negative Δq", ""]
        lines.append("| commit | Δq | districts | files | + edges (imports) | − edges | heaviest added edge | subject |")
        lines.append("|---|---|---|---|---|---|---|---|")
        for row in drops:
            edge = row["top_edge"]
            edge_text = (
                f"`{edge['source']}` → `{edge['target']}` ({edge['source_district']}→{edge['target_district']})"
                if edge
                else ""
            )
            subject = row["subject"].replace("|", "\\|")
            lines.append(
                f"| {row['commit'][:10]} | {row['delta_q']:+.6f} | {row['districts_crossed']} | "
                f"{row['files']} | {row['edges_added']} ({row['import_edges_added']}) | "
                f"{row['edges_removed']} | {edge_text} | {subject} |"
            )
    return "\n".join(lines) + "\n"


def download(run: str, into: Path, repo: str) -> None:
    match = RUN_URL_RE.search(run)
    run_id = match.group(1) if match else run
    subprocess.run(
        ["gh", "run", "download", run_id, "-R", repo, "-p", "check-cal-*", "-D", str(into)],
        check=True,
    )


def self_test() -> int:
    assert percentile([1, 2, 3, 4], 50) == 2
    assert percentile([1, 2, 3, 4], 99) == 4
    assert percentile([5], 1) == 5
    assert percentile(list(range(1, 101)), 95) == 95
    rows = [
        {"districts_crossed": 1, "delta_q": 0.0},
        {"districts_crossed": 4, "delta_q": -0.001},
        {"districts_crossed": 2, "delta_q": -0.01},
    ]
    assert fire_rate(rows, 3, None) == (1, 3)
    assert fire_rate(rows, None, 0.005) == (1, 3)
    assert fire_rate(rows, None, 0.001) == (1, 3), "delta_q == -max_dq does not fire"
    assert fire_rate(rows, 3, 0.005) == (2, 3)
    assert fire_rate(rows, None, None) == (0, 3)
    with tempfile.TemporaryDirectory() as temporary:
        shard = Path(temporary) / "check-cal-x__y-0"
        shard.mkdir()
        (shard / "shard.json").write_text(
            json.dumps({"slug": "x/y", "pin": "a" * 40, "shard": 0, "shards": 1, "commits": 2})
        )
        report = {
            "districts_crossed": 2,
            "delta_q": -0.002,
            "files": [{}, {}],
            "unplaced_files": [],
            "edges_added": [
                {"source": "a", "target": "b", "source_district": 0, "target_district": 1, "static_import": True}
            ],
            "edges_removed": [],
            "base_files": 10,
            "base_districts": 3,
        }
        (shard / "000-aaaa.json").write_text(json.dumps(report))
        (shard / "commits.jsonl").write_text(
            json.dumps({"index": 0, "commit": "a" * 40, "base": "b" * 40, "subject": "s", "merge": False, "exit_code": 0, "wall_s": 1.0, "report": "000-aaaa.json"})
            + "\n"
            + json.dumps({"index": 1, "commit": "b" * 40, "base": "c" * 40, "subject": "t", "merge": True, "exit_code": 2, "wall_s": 0.5, "report": None})
            + "\n"
        )
        repos = load(Path(temporary))
        summary = summarise(repos)["repos"]["x/y"]
        assert summary["attempted"] == 2 and summary["reports"] == 1
        assert summary["exit_codes"] == {"0": 1, "2": 1}
        assert summary["districts_crossed"]["p99"] == 2
        assert summary["most_negative_dq"][0]["import_edges_added"] == 1
        assert "x/y" in markdown(summarise(repos), (1, 0.001), repos)
    print("check_calibration self-test: ok")
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if argv[:1] == ["self-test"]:
        return self_test()
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--run", help="a remote-build run id or URL; its check-cal-* artifacts are downloaded with gh")
    source.add_argument("--dir", type=Path, help="a directory of already-downloaded check-cal-* artifacts")
    parser.add_argument("--repo", default=DEFAULT_REPO)
    parser.add_argument("--max-districts", type=int, help="the candidate default to report fire rates for")
    parser.add_argument("--max-dq", type=float, help="the candidate default to report fire rates for")
    parser.add_argument("--json", type=Path, help="write the full summary here")
    parser.add_argument("--summary", type=Path, help="append the markdown here (e.g. $GITHUB_STEP_SUMMARY)")
    args = parser.parse_args(argv)
    if args.run:
        temporary = tempfile.TemporaryDirectory()
        directory = Path(temporary.name)
        download(args.run, directory, args.repo)
    else:
        directory = args.dir
    repos = load(directory)
    summary = summarise(repos)
    text = markdown(summary, (args.max_districts, args.max_dq), repos)
    sys.stdout.write(text)
    if args.summary:
        with args.summary.open("a") as handle:
            handle.write(text)
    if args.json:
        args.json.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
