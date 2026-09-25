#!/usr/bin/env python3
"""Measure sandboxed dependency installs for `--refs scip` (issue #110 P1c).

Runs on a GitHub-hosted runner only (.github/workflows/scip-install.yml,
behind remote-build.yml `command=scip-install`); it reads real-repository
maps, which is corpus-scale work the maintainer's laptop must not do
(CLAUDE.md).

    scip_install_measure.py repo --stem S --slug SLUG --commit SHA --work DIR --tolmap BIN --out FILE
    scip_install_measure.py summary --results DIR --out FILE --markdown FILE

`repo` expects, under --work, three builds of one checkout by the branch
binary, in this order (the last one writes node_modules into the checkout):

    hand/<stem>.json          --refs hand
    scip/<stem>.json          --refs scip, installs off (P1a's behaviour)
    install/<stem>.json       --refs scip --install sandbox, as root
    <name>.time.txt, .log     /usr/bin/time -v and each build's log

and reports, per language, each SCIP build's reference path, reason,
recall and pair counts from `coverage.references`, the install record, the
install's own time, bytes and egress from the build log, and each build's
wall time, peak RSS, districts and modularity, with placement of each SCIP
map against the hand map (`tolmap parity`).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

BUILDS = ("hand", "scip", "install")


def parse_time(path: Path) -> dict:
    if not path.exists():
        return {}
    text = path.read_text(errors="replace")
    result = {}
    match = re.search(r"Elapsed \(wall clock\) time \(h:mm:ss or m:ss\): ([\d:.]+)", text)
    if match:
        seconds = 0.0
        for part in match.group(1).split(":"):
            seconds = seconds * 60 + float(part)
        result["wall_s"] = round(seconds, 1)
    match = re.search(r"Maximum resident set size \(kbytes\): (\d+)", text)
    if match:
        result["peak_rss_mb"] = round(int(match.group(1)) / 1024, 1)
    match = re.search(r"Exit status: (\d+)", text)
    if match:
        result["exit_status"] = int(match.group(1))
    return result


def install_log(path: Path) -> dict:
    """The sandbox's own lines from a `--install sandbox` build log."""
    if not path.exists():
        return {}
    text = path.read_text(errors="replace")
    result = {}
    match = re.search(r"install (pnpm|npm): (\w+) after ([\d.]+)s, ([\d.]+) MB written", text)
    if match:
        result.update(
            manager=match.group(1),
            outcome=match.group(2),
            install_s=float(match.group(3)),
            written_mb=float(match.group(4)),
        )
    match = re.search(
        r"egress: (\d+) tunnel\(s\) to (\S+), ([\d.]+) MB in, ([\d.]+) MB out; refused: (.*)", text
    )
    if match:
        result.update(
            tunnels=int(match.group(1)),
            egress_in_mb=float(match.group(3)),
            egress_out_mb=float(match.group(4)),
            refused=match.group(5).strip(),
        )
    match = re.search(r"install sandbox unavailable: (.*)", text)
    if match:
        result["unavailable"] = match.group(1).strip()
    match = re.search(r"Installing dependencies: done in ([\d.]+)s", text)
    if match:
        result["stage_s"] = float(match.group(1))
    indexing = re.search(r"Indexing TypeScript: done in ([\d.]+)s", text)
    if indexing:
        result["index_ts_s"] = float(indexing.group(1))
    return result


def indexing_seconds(path: Path) -> dict:
    if not path.exists():
        return {}
    result = {}
    for match in re.finditer(r"Indexing (Go|Python|TypeScript): done in ([\d.]+)s", path.read_text(errors="replace")):
        result[match.group(1)] = float(match.group(2))
    return result


def parity(tolmap: Path, reference: Path, candidate: Path) -> dict:
    if not reference.exists() or not candidate.exists():
        return {}
    run = subprocess.run(
        [str(tolmap), "parity", str(reference), str(candidate)], capture_output=True, text=True
    )
    text = run.stdout + run.stderr
    match = re.search(r"district placement: ([\d.]+)%", text)
    return {"placement_pct": float(match.group(1))} if match else {}


def map_summary(path: Path) -> dict:
    if not path.exists():
        return {}
    document = json.loads(path.read_text())
    coverage = document.get("coverage") or {}
    return {
        "files": len(document["F"]),
        "districts": len(document["districts"]),
        "q": round(document["q"], 4),
        "edges": len(document["E"]),
        "zero_edge_files": coverage.get("zero_edge_files"),
        "references": coverage.get("references"),
    }


def measure(args) -> int:
    work = Path(args.work)
    result = {"slug": args.slug, "commit": args.commit, "builds": {}}
    for build in BUILDS:
        row = map_summary(work / build / f"{args.stem}.json")
        row.update(parse_time(work / f"{build}.time.txt"))
        row["indexing_s"] = indexing_seconds(work / f"{build}.log")
        if build != "hand":
            row["placement_vs_hand"] = parity(
                Path(args.tolmap), work / "hand" / f"{args.stem}.json", work / build / f"{args.stem}.json"
            )
        result["builds"][build] = row
    result["install_log"] = install_log(work / "install.log")
    result["placement_install_vs_scip"] = parity(
        Path(args.tolmap), work / "scip" / f"{args.stem}.json", work / "install" / f"{args.stem}.json"
    )
    Path(args.out).write_text(json.dumps(result, indent=1, sort_keys=True))
    print(json.dumps(result, indent=1, sort_keys=True))
    return 0


def fmt(value, digits=1):
    if value is None:
        return "—"
    if isinstance(value, float):
        return f"{value:,.{digits}f}"
    if isinstance(value, int):
        return f"{value:,}"
    return str(value)


def reference_cell(row: dict | None) -> str:
    if not row:
        return "—"
    recall = row.get("recall")
    return f"{row['path']} ({row['reason']}), recall {fmt(recall, 4)}, {fmt(row.get('scip_pairs'))} SCIP pairs"


def summary(args) -> int:
    results = []
    for path in sorted(Path(args.results).glob("*/measure.json")):
        results.append(json.loads(path.read_text()))
    lines = [
        "## SCIP with sandboxed installs (#110 P1c)",
        "",
        "| repo | lang | hand pairs | no install | with install | install record |",
        "|---|---|---:|---|---|---|",
    ]
    for result in results:
        scip = (result["builds"]["scip"].get("references") or {})
        installed = (result["builds"]["install"].get("references") or {})
        for lang in sorted(set(scip) | set(installed)):
            before = scip.get(lang)
            after = installed.get(lang)
            record = (after or {}).get("install")
            lines.append(
                f"| {result['slug']} | {lang} | {fmt((before or after or {}).get('hand_pairs'))} "
                f"| {reference_cell(before)} | {reference_cell(after)} "
                f"| {record['status'] + ' (' + record['reason'] + ')' if record else '—'} |"
            )
    lines += [
        "",
        "| repo | build | wall s | peak RSS MB | districts | q | placement vs hand | TS indexing s |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for result in results:
        for build in BUILDS:
            row = result["builds"][build]
            lines.append(
                f"| {result['slug']} | {build} | {fmt(row.get('wall_s'))} | {fmt(row.get('peak_rss_mb'))} "
                f"| {fmt(row.get('districts'))} | {fmt(row.get('q'), 4)} "
                f"| {fmt((row.get('placement_vs_hand') or {}).get('placement_pct'))} "
                f"| {fmt((row.get('indexing_s') or {}).get('TypeScript'))} |"
            )
    lines += [
        "",
        "| repo | install | stage s | written MB | tunnels | MB in | refused | install vs no-install placement |",
        "|---|---|---:|---:|---:|---:|---|---:|",
    ]
    for result in results:
        log = result.get("install_log") or {}
        lines.append(
            f"| {result['slug']} | {log.get('manager', '—')} {log.get('outcome', log.get('unavailable', '—'))} "
            f"| {fmt(log.get('stage_s'))} | {fmt(log.get('written_mb'))} | {fmt(log.get('tunnels'))} "
            f"| {fmt(log.get('egress_in_mb'))} | {log.get('refused', '—')} "
            f"| {fmt((result.get('placement_install_vs_scip') or {}).get('placement_pct'))} |"
        )
    Path(args.out).write_text(json.dumps(results, indent=1, sort_keys=True))
    Path(args.markdown).write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    repo = commands.add_parser("repo")
    repo.add_argument("--stem", required=True)
    repo.add_argument("--slug", required=True)
    repo.add_argument("--commit", required=True)
    repo.add_argument("--work", required=True)
    repo.add_argument("--tolmap", required=True)
    repo.add_argument("--out", required=True)
    summary_parser = commands.add_parser("summary")
    summary_parser.add_argument("--results", required=True)
    summary_parser.add_argument("--out", required=True)
    summary_parser.add_argument("--markdown", required=True)
    args = parser.parse_args()
    return measure(args) if args.command == "repo" else summary(args)


if __name__ == "__main__":
    sys.exit(main())
