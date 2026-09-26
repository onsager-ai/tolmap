#!/usr/bin/env python3
"""SCIP fixtures for the nine acceptance repositories (issue #110 P2a).

    scip_fixtures.py list
    scip_fixtures.py measure --work DIR --tolmap BIN --out JSON [--markdown MD]
    scip_fixtures.py record  --work DIR --out data/scip [--source URL]
    scip_fixtures.py gate    --work DIR --summaries data/scip

SCIP is the oracle the hand-written resolver is tuned against (owner
decision, 2026-09-25T16:56Z: "Tune hand, SCIP as oracle"; `hand` stays
the default). `data/*.json` is the hand path's output (the frozen Python
reference's, or the Rust product's where data/fixtures.toml names a
`generator`) and only knows the hand resolver, so the nine-fixture parity gate (ci.yml
`full-fixtures`) pins `--refs hand`, and the SCIP path gets fixtures of its
own, derived under its extraction as CLAUDE.md's "Checks before a change
lands" requires, so the oracle itself cannot drift unnoticed:
`data/scip/<name>.json`, one per fixture, written by `record` from a
`--refs scip` build with the pinned indexers (ci.yml `scip-fixtures`).

A SCIP fixture is a summary, not a map. The parity gate that matters here
is membership and modularity, and a whole map would re-commit layout
geometry the gate never reads. It holds the file list `F`, each file's
district `D` (map order), `q`, and the build's `coverage.references` block,
which names the indexer versions that produced it -- a fixture is only an
oracle for the indexer versions it records.

`measure` builds the per-fixture hand-versus-SCIP table for the finding.
`gate` fails when a fresh `--refs scip` build places fewer than 95% of files
in the district its SCIP fixture assigns, lands more than 0.02 from its
modularity, changes `F`, or takes a different reference path for a language.

Work directory layout (written by the ci.yml job):
    DIR/hand/<name>.json              tolmap build --refs hand
    DIR/scip/<name>.json              tolmap build --refs scip
    DIR/<name>.<mode>.time.txt        /usr/bin/time -v of that build
    DIR/<name>.<mode>.log             its stdout and stderr

Placement is `remote_build_result.placement`, which mirrors src/parity.rs
(greedy best-Jaccard district matching, 0.35 floor, descending-id tie
break). `measure` also runs `tolmap parity` on every hand/SCIP pair and
fails if the two disagree, so the Python mirror is checked against the
Rust gate on real maps every time it runs.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

from remote_build_result import placement, time_fields

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "data" / "fixtures.toml"
PLACEMENT_THRESHOLD = 0.95  # src/parity.rs PLACEMENT_THRESHOLD
MODULARITY_THRESHOLD = 0.02  # src/parity.rs MODULARITY_THRESHOLD


# Languages the product can take the SCIP path for. Rust has no product
# indexer (rust-analyzer runs a repository's build scripts and proc macros
# natively, finding 55): its map fixtures are hand-only, and ci.yml's
# `hand-score` runs rust-analyzer as their oracle instead (finding 57).
PRODUCT_SCIP_LANGS = ("py", "go", "ts")


def fixtures() -> dict[str, dict]:
    with FIXTURES.open("rb") as handle:
        return {name: row for name, row in tomllib.load(handle).items()
                if row["lang"] in PRODUCT_SCIP_LANGS}


def load(path: Path) -> dict | None:
    return json.loads(path.read_text()) if path.is_file() else None


def references(document: dict) -> dict:
    return (document.get("coverage") or {}).get("references") or {}


def districts(document: dict) -> int:
    return len({int(node[0]) for node in document.get("N", [])})


def edge_pairs(document: dict) -> set[tuple[str, str]]:
    files = document["F"]
    return {(files[edge[0]], files[edge[1]]) for edge in document.get("E", [])}


def summary_as_map(summary: dict) -> dict:
    """A summary in the shape `placement` reads: `F`, `N` rows, `q`."""
    return {"F": summary["F"], "N": [[district] for district in summary["D"]], "q": summary["q"]}


def index_seconds(log: Path) -> dict[str, float]:
    """`Indexing Python: done in 79.12s` lines (src/main.rs's stage log)."""
    if not log.is_file():
        return {}
    seconds = {}
    for line in log.read_text(errors="replace").splitlines():
        match = re.match(r"Indexing (Go|Python|TypeScript): done in ([\d.]+)s", line.strip())
        if match:
            seconds[match.group(1)] = float(match.group(2))
    return seconds


def rust_parity(tolmap: Path, reference: Path, candidate: Path) -> tuple[int, int]:
    """(placed, common) from `tolmap parity`'s placement line.

    Its exit status is ignored: E, S and U legitimately differ between a
    hand and a SCIP map, so the Rust gate as a whole always "fails" here.
    """
    output = subprocess.run(
        [str(tolmap), "parity", str(reference), str(candidate)],
        capture_output=True,
        text=True,
        check=False,
    ).stdout
    match = re.search(r"district placement: [\d.]+% \((\d+)/(\d+) common files", output)
    if not match:
        raise SystemExit(f"no placement line in `tolmap parity` output:\n{output}")
    return int(match.group(1)), int(match.group(2))


def list_fixtures(_args) -> int:
    for name, row in fixtures().items():
        print(name, row["url"], row["commit"], row["pkg"], row["lang"],
              "true" if row["parcels"] else "false")
    return 0


def measure(args) -> int:
    rows = []
    for name in fixtures():
        hand_path = args.work / "hand" / f"{name}.json"
        scip_path = args.work / "scip" / f"{name}.json"
        hand, scip = load(hand_path), load(scip_path)
        row: dict = {"name": name}
        for mode in ("hand", "scip"):
            wall, peak_kb = time_fields(args.work / f"{name}.{mode}.time.txt")
            row[f"{mode}_wall_s"] = wall
            row[f"{mode}_peak_rss_mb"] = round(peak_kb / 1024) if peak_kb else None
        row["index_s"] = index_seconds(args.work / f"{name}.scip.log")
        if hand is None or scip is None:
            row["status"] = "missing " + " and ".join(
                mode for mode, doc in (("hand", hand), ("scip", scip)) if doc is None)
            rows.append(row)
            continue
        fraction, delta = placement(scip, hand)
        placed, common = rust_parity(args.tolmap, hand_path, scip_path)
        if placed != round(fraction * common):
            raise SystemExit(
                f"{name}: Python placement {fraction:.6f} of {common} disagrees with "
                f"`tolmap parity` ({placed}/{common})")
        committed = load(ROOT / "data" / f"{name}.json")
        hand_edges, scip_edges = edge_pairs(hand), edge_pairs(scip)
        row.update(
            status="built",
            files=len(scip["F"]),
            files_identical=hand["F"] == scip["F"],
            hand_districts=districts(hand),
            scip_districts=districts(scip),
            hand_q=hand["q"],
            scip_q=scip["q"],
            placement_vs_hand=fraction,
            placed_vs_hand=placed,
            q_delta_vs_hand=delta,
            passes_parity_thresholds=fraction >= PLACEMENT_THRESHOLD and delta <= MODULARITY_THRESHOLD,
            hand_edges=len(hand_edges),
            scip_edges=len(scip_edges),
            edges_both=len(hand_edges & scip_edges),
            hand_zero_edge_files=(hand.get("coverage") or {}).get("zero_edge_files"),
            scip_zero_edge_files=(scip.get("coverage") or {}).get("zero_edge_files"),
            references=references(scip),
        )
        if committed is not None:
            row["hand_vs_committed"] = dict(zip(("placement", "q_delta"), placement(hand, committed)))
            row["scip_vs_committed"] = dict(zip(("placement", "q_delta"), placement(scip, committed)))
        rows.append(row)
    args.out.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
    table = markdown(rows)
    print(table)
    if args.markdown:
        args.markdown.write_text(table)
    return 0 if all(row["status"] == "built" for row in rows) else 1


def fmt(value, digits: int = 1) -> str:
    if value is None:
        return "—"
    if isinstance(value, float):
        return f"{value:,.{digits}f}"
    return f"{value:,}" if isinstance(value, int) else str(value)


def reference_cell(block: dict) -> str:
    cells = []
    for language, row in sorted(block.items()):
        recall = row.get("recall")
        cells.append(
            f"{language}: {row['path']} ({row['reason']}"
            + (f", recall {recall:.4f}" if recall is not None else "")
            + f", {fmt(row.get('files_indexed'))}/{fmt(row.get('files'))} files)")
    return "; ".join(cells) or "—"


def markdown(rows: list[dict]) -> str:
    lines = [
        "| fixture | files | districts hand → scip | q hand → scip | placement scip vs hand | Δq | ≥95% / ≤0.02 | `E` hand → scip (both) | zero-edge files hand → scip | references | build s hand → scip (indexing) | peak RSS MB hand → scip |",
        "|---|---:|---:|---:|---:|---:|---|---:|---:|---|---:|---:|",
    ]
    for row in rows:
        if row["status"] != "built":
            lines.append(f"| {row['name']} | {row['status']} |" + " |" * 10)
            continue
        indexing = ", ".join(f"{fmt(v)}" for _, v in sorted(row["index_s"].items()))
        lines.append(
            f"| {row['name']} | {fmt(row['files'])} "
            f"| {row['hand_districts']} → {row['scip_districts']} "
            f"| {row['hand_q']:.4f} → {row['scip_q']:.4f} "
            f"| {100 * row['placement_vs_hand']:.1f}% ({row['placed_vs_hand']}/{row['files']}) "
            f"| {row['q_delta_vs_hand']:.4f} "
            f"| {'pass' if row['passes_parity_thresholds'] else '**fail**'} "
            f"| {fmt(row['hand_edges'])} → {fmt(row['scip_edges'])} ({fmt(row['edges_both'])}) "
            f"| {fmt(row['hand_zero_edge_files'])} → {fmt(row['scip_zero_edge_files'])} "
            f"| {reference_cell(row['references'])} "
            f"| {fmt(row['hand_wall_s'])} → {fmt(row['scip_wall_s'])} ({indexing or '—'}) "
            f"| {fmt(row['hand_peak_rss_mb'])} → {fmt(row['scip_peak_rss_mb'])} |")
    return "\n".join(lines) + "\n"


def record(args) -> int:
    args.out.mkdir(parents=True, exist_ok=True)
    table = fixtures()
    for name, row in table.items():
        scip = load(args.work / "scip" / f"{name}.json")
        if scip is None:
            raise SystemExit(f"{name}: no --refs scip map to record")
        summary = {
            "name": name,
            "commit": row["commit"],
            "pkg": row["pkg"],
            "lang": row["lang"],
            "source": args.source,
            "references": references(scip),
            "q": scip["q"],
            "districts": districts(scip),
            "edges": len(scip.get("E", [])),
            "F": scip["F"],
            "D": [int(node[0]) for node in scip["N"]],
        }
        path = args.out / f"{name}.json"
        path.write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
        print(f"wrote {path}: {len(summary['F'])} files, {summary['districts']} districts, q {summary['q']}")
    return 0


def gate(args) -> int:
    failed = []
    for name in fixtures():
        summary = load(args.summaries / f"{name}.json")
        candidate = load(args.work / "scip" / f"{name}.json")
        if summary is None or candidate is None:
            print(f"FAIL {name}: missing "
                  + ("committed SCIP fixture" if summary is None else "--refs scip build"))
            failed.append(name)
            continue
        problems = []
        fraction, delta = placement(candidate, summary_as_map(summary))
        if fraction < PLACEMENT_THRESHOLD:
            problems.append(f"placement {100 * fraction:.1f}% < {100 * PLACEMENT_THRESHOLD:.0f}%")
        if delta > MODULARITY_THRESHOLD:
            problems.append(f"modularity delta {delta:.4f} > {MODULARITY_THRESHOLD}")
        if candidate["F"] != summary["F"]:
            problems.append(f"F differs ({len(summary['F'])} → {len(candidate['F'])} files)")
        paths = {lang: row["path"] for lang, row in references(candidate).items()}
        expected = {lang: row["path"] for lang, row in summary["references"].items()}
        if paths != expected:
            problems.append(f"reference paths {expected} → {paths}")
        verdict = "FAIL" if problems else "PASS"
        print(f"{verdict} {name}: placement {100 * fraction:.1f}%, q {summary['q']:.4f} → "
              f"{candidate['q']:.4f} (delta {delta:.4f}), paths {paths}"
              + (" -- " + "; ".join(problems) if problems else ""))
        if problems:
            failed.append(name)
    if failed:
        print(f"SCIP fixture gate failed: {' '.join(failed)}")
        return 1
    print("SCIP fixture gate passed on all fixtures")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("list").set_defaults(run=list_fixtures)
    run = commands.add_parser("measure")
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--tolmap", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--markdown", type=Path)
    run.set_defaults(run=measure)
    run = commands.add_parser("record")
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--source", default=None, help="the Actions run the maps came from")
    run.set_defaults(run=record)
    run = commands.add_parser("gate")
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--summaries", type=Path, required=True)
    run.set_defaults(run=gate)
    args = parser.parse_args()
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
