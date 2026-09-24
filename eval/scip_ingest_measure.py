#!/usr/bin/env python3
"""Measure `tolmap build --refs scip` on one repository (issue #110, P1a).

Runs on a GitHub-hosted runner only (.github/workflows/scip-ingest.yml); it
reads real-repository indexes and maps, which is corpus-scale work the
maintainer's laptop must not do (CLAUDE.md).

    scip_ingest_measure.py repo --stem S --slug SLUG --work DIR --tolmap BIN --out FILE
    scip_ingest_measure.py summary --results DIR --out FILE --markdown FILE

`repo` expects, under --work, what scip-ingest.yml's measure job built from
one checkout with the branch binary:

    hand/<stem>.json, .symbols.json   --refs hand (the default)
    hand.graph.json                   dump-graph (hand), for each file's language
    scip1/ scip2/ scip3/              three cold --refs scip builds
    scipwarm/                         --refs scip, warm-started from hand/
    idx/<lang>.scip                   the indexes scip1 read (TOLMAP_SCIP_INDEX_DIR)
    *.time.txt, *.log                 /usr/bin/time -v and build logs

and reports:

- **oracle agreement**: P0's Python ingest (`eval/scip_ingest.py`, the
  oracle) on the very index scip1 read, against what the Rust ingest put in
  the map -- file pairs (the map's `E` for that language), the fallback
  gate's pair count and recall, symbol reference pairs and their occurrence
  counts (crediting on the finished symbols document, as P0 did), and
  implementation relationships;
- **against --refs hand**: districts, modularity, placement of the SCIP map
  against the hand map (cold, and warm-started as the product would switch),
  zero-edge files, coverage paths, symbol edge rows by kind, Go's
  `possible_implementation` rows against exact `implements`;
- **determinism**: sha256 of map + symbols document over the three cold
  builds;
- **cost**: wall time and peak RSS of each build, and each language's
  indexing stage from the build log.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

import scip_ingest  # P0's oracle; needs the generated scip_pb2 on PYTHONPATH

REFERENCE_KINDS = {"call", "annotation", "decorator", "value", "reference"}
INHERITANCE_KINDS = {"extends", "implements", "overrides"}
LANGS = ("go", "py", "ts")


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
        result["wall_s"] = round(seconds, 2)
    match = re.search(r"Maximum resident set size \(kbytes\): (\d+)", text)
    if match:
        result["peak_rss_mb"] = round(int(match.group(1)) / 1024, 1)
    match = re.search(r"Exit status: (\d+)", text)
    if match:
        result["exit_status"] = int(match.group(1))
    return result


def stage_seconds(log: Path) -> dict:
    """`Indexing Python: done in 79.12s` lines from a build log."""
    if not log.exists():
        return {}
    result = {}
    for line in log.read_text(errors="replace").splitlines():
        match = re.match(r"Indexing (Go|Python|TypeScript): done in ([\d.]+)s", line.strip())
        if match:
            result[match.group(1)] = float(match.group(2))
    return result


def digest(*paths: Path) -> str | None:
    if not all(path.exists() for path in paths):
        return None
    h = hashlib.sha256()
    for path in paths:
        h.update(path.read_bytes())
    return h.hexdigest()


def parity(tolmap: Path, reference: Path, candidate: Path) -> dict:
    if not reference.exists() or not candidate.exists():
        return {}
    run = subprocess.run(
        [str(tolmap), "parity", str(reference), str(candidate)], capture_output=True, text=True
    )
    text = run.stdout + run.stderr
    result = {}
    match = re.search(
        r"district placement: ([\d.]+)% \((\d+)/(\d+) common files matched via (\d+)/(\d+) districts\)",
        text,
    )
    if match:
        result.update(placement_pct=float(match.group(1)), placed=int(match.group(2)), common=int(match.group(3)))
    match = re.search(r"modularity: reference ([-\d.]+), candidate ([-\d.]+)", text)
    if match:
        result.update(reference_q=float(match.group(1)), candidate_q=float(match.group(2)))
    return result


def directory(path: str) -> str:
    return path.rsplit("/", 1)[0] if "/" in path else ""


def map_summary(document: dict) -> dict:
    coverage = document.get("coverage") or {}
    return {
        "files": len(document["F"]),
        "districts": len(document["districts"]),
        "q": document["q"],
        "edges": len(document["E"]),
        "zero_edge_files": coverage.get("zero_edge_files"),
    }


def kind_rows(symbols: dict, symbol_lang: list[str]) -> dict:
    kinds = symbols.get("kinds", [])
    rows: dict[str, Counter] = defaultdict(Counter)
    for source, _, _, kind in symbols["edges"]:
        rows[symbol_lang[source]][kinds[kind]] += 1
    return {lang: dict(sorted(counter.items())) for lang, counter in sorted(rows.items())}


def measure(args) -> int:
    work = args.work
    stem = args.stem
    hand_map = json.loads((work / "hand" / f"{stem}.json").read_text())
    hand_symbols = json.loads((work / "hand" / f"{stem}.symbols.json").read_text())
    graph = json.loads((work / "hand.graph.json").read_text())
    scip_path = work / "scip1" / f"{stem}.json"
    result: dict = {"stem": stem, "slug": args.slug, "commit": args.commit}
    result["cost"] = {
        "hand": parse_time(work / "hand.time.txt"),
        **{
            name: {**parse_time(work / f"{name}.time.txt"), "index_s": stage_seconds(work / f"{name}.log")}
            for name in ("scip1", "scip2", "scip3", "scipwarm")
        },
    }
    result["hand"] = map_summary(hand_map)
    if not scip_path.exists():
        result["error"] = "scip1 build produced no map"
        log = work / "scip1.log"
        if log.exists():
            result["log_tail"] = log.read_text(errors="replace").splitlines()[-30:]
        args.out.write_text(json.dumps(result, indent=1, sort_keys=True))
        print(json.dumps(result, indent=1)[:4000])
        return 0
    scip_map = json.loads(scip_path.read_text())
    scip_symbols = json.loads((work / "scip1" / f"{stem}.symbols.json").read_text())
    files = scip_map["F"]
    assert files == hand_map["F"], "the two builds mapped different files"
    lang_of = {node["file"]: node.get("lang") or graph["lang"] for node in graph["nodes"]}
    symbol_lang = [lang_of[files[row[0]]] for row in scip_symbols["symbols"]]
    assert [row[:6] for row in scip_symbols["symbols"]] == [
        row[:6] for row in hand_symbols["symbols"]
    ], "symbol rows differ between --refs hand and --refs scip"
    references = (scip_map.get("coverage") or {}).get("references") or {}
    result["references"] = references
    result["scip"] = map_summary(scip_map)
    result["vs_hand_cold"] = parity(args.tolmap, work / "hand" / f"{stem}.json", scip_path)
    warm_path = work / "scipwarm" / f"{stem}.json"
    if warm_path.exists():
        warm = json.loads(warm_path.read_text())
        result["warm"] = {**map_summary(warm), **parity(args.tolmap, work / "hand" / f"{stem}.json", warm_path)}
    hashes = [digest(work / f"scip{i}" / f"{stem}.json", work / f"scip{i}" / f"{stem}.symbols.json") for i in (1, 2, 3)]
    result["determinism"] = {"sha256": hashes, "identical": len(set(hashes)) == 1 and hashes[0] is not None}
    result["symbol_rows_by_kind"] = {
        "hand": kind_rows(hand_symbols, symbol_lang),
        "scip": kind_rows(scip_symbols, symbol_lang),
    }
    result["symbol_coverage"] = {"hand": hand_symbols.get("coverage"), "scip": scip_symbols.get("coverage")}

    spans = scip_ingest.Spans(files, scip_symbols["symbols"])
    mapped = set(files)
    hand_pairs = {(files[a], files[b]) for a, b in hand_map["E"]}
    scip_pairs = {(files[a], files[b]) for a, b in scip_map["E"]}
    kinds = scip_symbols["kinds"]
    product_refs: dict[str, dict] = defaultdict(dict)
    product_inheritance: dict[str, set] = defaultdict(set)
    for source, target, count, kind in scip_symbols["edges"]:
        lang = symbol_lang[source]
        if kinds[kind] in REFERENCE_KINDS:
            pair = (source, target)
            product_refs[lang][pair] = product_refs[lang].get(pair, 0) + count
        elif kinds[kind] in INHERITANCE_KINDS:
            product_inheritance[lang].add((source, target))
    result["oracle"] = {}
    for lang in LANGS:
        index = work / "idx" / f"{lang}.scip"
        row = references.get(lang)
        if row is None:
            continue
        entry: dict = {"path": row["path"], "reason": row["reason"]}
        if not index.exists() or index.stat().st_size == 0:
            entry["index"] = "missing"
            result["oracle"][lang] = entry
            continue
        oracle = scip_ingest.ingest(index, ".", mapped, lang_of, lang, spans)
        oracle_pairs = {
            (a, b) for a, b, *_ in oracle["file_edges"] if lang_of[a] == lang and lang_of[b] == lang
        }
        hand_l = {pair for pair in hand_pairs if lang_of[pair[0]] == lang}
        if row["granularity"] == "directory":
            hand_unit = {(a, directory(b)) for a, b in hand_l}
            oracle_unit = {(a, directory(b)) for a, b in oracle_pairs}
        else:
            hand_unit, oracle_unit = hand_l, oracle_pairs
        oracle_recall = round(len(hand_unit & oracle_unit) / len(hand_unit), 4) if hand_unit else None
        entry["file_pairs"] = {
            "oracle": len(oracle_pairs),
            "rust_gate_scip_pairs": row.get("scip_pairs"),
            "count_equal": len(oracle_pairs) == row.get("scip_pairs"),
            "oracle_recall": oracle_recall,
            "rust_recall": row.get("recall"),
            "recall_equal": oracle_recall == row.get("recall"),
            "hand_pairs": len(hand_unit),
        }
        if row["path"] == "scip":
            product_l = {pair for pair in scip_pairs if lang_of[pair[0]] == lang}
            entry["file_pairs"].update(
                map_E=len(product_l),
                set_equal=product_l == oracle_pairs,
                only_oracle=len(oracle_pairs - product_l),
                only_map=len(product_l - oracle_pairs),
                samples_only_oracle=sorted(oracle_pairs - product_l)[:5],
                samples_only_map=sorted(product_l - oracle_pairs)[:5],
            )
            oracle_refs = {
                (a, b): count for a, b, count, _ in oracle["symbol_edges"] if symbol_lang[a] == lang
            }
            rust_refs = product_refs.get(lang, {})
            differing = {
                pair
                for pair in set(oracle_refs) | set(rust_refs)
                if oracle_refs.get(pair) != rust_refs.get(pair)
            }
            entry["symbol_refs"] = {
                "oracle_pairs": len(oracle_refs),
                "rust_pairs": len(rust_refs),
                "oracle_occurrences": sum(oracle_refs.values()),
                "rust_occurrences": sum(rust_refs.values()),
                "pairs_with_different_counts": len(differing),
                "equal": not differing,
                "samples_differing": [
                    [
                        scip_symbols["symbols"][a][1],
                        files[scip_symbols["symbols"][a][0]],
                        scip_symbols["symbols"][b][1],
                        oracle_refs.get((a, b)),
                        rust_refs.get((a, b)),
                    ]
                    for a, b in sorted(differing)[:5]
                ],
            }
            implementations = {
                (a, b)
                for a, b, flags in oracle["relationship_pairs"]
                if flags & scip_ingest.REL_IMPLEMENTATION and symbol_lang[a] == lang
            }
            found = implementations & product_inheritance.get(lang, set())
            entry["implementations"] = {
                "oracle": len(implementations),
                "in_map_as_inheritance": len(found),
                "not_typed": len(implementations - found),
            }
        result["oracle"][lang] = entry

    args.out.write_text(json.dumps(result, indent=1, sort_keys=True))
    print(json.dumps(result, indent=1, sort_keys=True)[:6000])
    return 0


def fmt(value, digits=1):
    if value is None:
        return "—"
    if isinstance(value, bool):
        return "yes" if value else "**no**"
    if isinstance(value, float):
        return f"{value:,.{digits}f}"
    if isinstance(value, int):
        return f"{value:,}"
    return str(value)


def summary(args) -> int:
    results = [json.loads(p.read_text()) for p in sorted(args.results.rglob("measure.json"))]
    args.out.write_text(json.dumps(results, indent=1, sort_keys=True))
    lines = ["# SCIP ingest (#110 P1a)", "", "## Reference paths and oracle agreement", ""]
    lines += [
        "| repo | lang | path (reason) | recall (Rust / oracle) | hand pairs | SCIP pairs (Rust / oracle) | map E = oracle pairs | symbol refs equal (pairs, occurrences) | implementations typed / oracle | index s |",
        "|---|---|---|---:|---:|---:|---|---|---:|---:|",
    ]
    for r in results:
        index_s = (r.get("cost", {}).get("scip1", {}) or {}).get("index_s", {})
        names = {"go": "Go", "py": "Python", "ts": "TypeScript"}
        for lang, row in sorted((r.get("references") or {}).items()):
            o = r.get("oracle", {}).get(lang, {})
            fp = o.get("file_pairs", {})
            sr = o.get("symbol_refs", {})
            im = o.get("implementations", {})
            lines.append(
                f"| {r['slug']} | {lang} | {row['path']} ({row['reason']}) "
                f"| {fmt(row.get('recall'), 4)} / {fmt(fp.get('oracle_recall'), 4)} "
                f"| {fmt(row.get('hand_pairs'))} | {fmt(row.get('scip_pairs'))} / {fmt(fp.get('oracle'))} "
                f"| {fmt(fp.get('set_equal'))} "
                f"| {fmt(sr.get('equal'))} ({fmt(sr.get('rust_pairs'))}, {fmt(sr.get('rust_occurrences'))}) "
                f"| {fmt(im.get('in_map_as_inheritance'))} / {fmt(im.get('oracle'))} "
                f"| {fmt(index_s.get(names[lang]))} |"
            )
    lines += [
        "",
        "## Against --refs hand",
        "",
        "| repo | hand districts / q | SCIP districts / q | cold placement | warm districts / q / placement | zero-edge files hand → SCIP | E hand → SCIP | build s hand → SCIP | peak RSS MB hand → SCIP | 3 builds identical |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---|",
    ]
    for r in results:
        h, s, w = r.get("hand", {}), r.get("scip", {}), r.get("warm", {})
        c = r.get("cost", {})
        lines.append(
            f"| {r['slug']} | {fmt(h.get('districts'))} / {fmt(h.get('q'), 4)} "
            f"| {fmt(s.get('districts'))} / {fmt(s.get('q'), 4)} "
            f"| {fmt(r.get('vs_hand_cold', {}).get('placement_pct'))}% "
            f"| {fmt(w.get('districts'))} / {fmt(w.get('q'), 4)} / {fmt(w.get('placement_pct'))}% "
            f"| {fmt(h.get('zero_edge_files'))} → {fmt(s.get('zero_edge_files'))} "
            f"| {fmt(h.get('edges'))} → {fmt(s.get('edges'))} "
            f"| {fmt(c.get('hand', {}).get('wall_s'))} → {fmt(c.get('scip1', {}).get('wall_s'))} "
            f"| {fmt(c.get('hand', {}).get('peak_rss_mb'))} → {fmt(c.get('scip1', {}).get('peak_rss_mb'))} "
            f"| {fmt((r.get('determinism') or {}).get('identical'))} |"
        )
    lines += ["", "## Symbol edge rows by kind (source language)", ""]
    for r in results:
        rows = r.get("symbol_rows_by_kind", {})
        for lang in sorted(set(rows.get("hand", {})) | set(rows.get("scip", {}))):
            lines.append(
                f"- {r['slug']} {lang}: hand {json.dumps(rows.get('hand', {}).get(lang, {}), sort_keys=True)}; "
                f"scip {json.dumps(rows.get('scip', {}).get(lang, {}), sort_keys=True)}"
            )
    lines += ["", "## Errors", ""]
    for r in results:
        if r.get("error"):
            lines.append(f"- {r['slug']}: {r['error']}")
    args.markdown.write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    repo = sub.add_parser("repo")
    repo.add_argument("--stem", required=True)
    repo.add_argument("--slug", required=True)
    repo.add_argument("--commit", required=True)
    repo.add_argument("--work", type=Path, required=True)
    repo.add_argument("--tolmap", type=Path, required=True)
    repo.add_argument("--out", type=Path, required=True)
    summ = sub.add_parser("summary")
    summ.add_argument("--results", type=Path, required=True)
    summ.add_argument("--out", type=Path, required=True)
    summ.add_argument("--markdown", type=Path, required=True)
    args = parser.parse_args()
    return measure(args) if args.command == "repo" else summary(args)


if __name__ == "__main__":
    sys.exit(main())
