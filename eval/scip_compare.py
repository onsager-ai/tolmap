#!/usr/bin/env python3
"""Compare SCIP-derived references with today's pipeline (issue #110, P0).

Runs on a GitHub-hosted runner only (.github/workflows/scip-spike.yml).

    scip_compare.py repo --stem S --base DIR --idx DIR --tolmap BIN --out DIR
    scip_compare.py summary --results DIR --out FILE --markdown FILE

`repo` reads one repository's baseline (the map, symbols document,
`dump-graph` graph and control map that `tolmap` on main built from the same
checkout) and every SCIP index job's output for it, then reports:

- index cost (wall time, peak RSS, index size) and exit codes;
- file -> file edges, hand-written (the map's `E`, i.e. the resolved
  imports) against SCIP: both, SCIP-only, hand-only, with ten seeded samples
  of each; also at target-directory granularity, which is the fair unit for
  Go (the hand-written resolver spreads each Go import over every file of the
  package, `extract::resolve_multi`);
- TypeScript cross-workspace-package edges, and dify's #101 pair
  (`cli/` -> `packages/contracts/`);
- symbol -> symbol pairs against the symbols document's edges, per kind,
  and callable references against finding 30's call-resolution share;
- relationship (implementation) pairs against the typed edges of finding 37;
- a re-partition: SCIP edges replace the `static` signal in the dump-graph
  graph, `tolmap build --graph` runs the unchanged blend (finding 1's mass
  normalisation) / prune / partition, and `tolmap parity` scores district
  placement against the control map built from the unmodified graph.

How the SCIP graph is built, and what it cannot reproduce exactly: a
`dump-graph` edge keeps its cochange/proximity/semantic values; its
`static_signal` is recomputed exactly as `extract::finish_graph` does
(undirected sum, divided by the per-language maximum floored at 1.0) from
the SCIP pairs, and the raw-weight floor (0.02) is re-applied. A SCIP pair
that was not a candidate edge before gets proximity computed exactly and
cochange = semantic = 0: finish_graph dropped such a pair only when
0.35*cochange + 0.08*proximity + 0.12*semantic < 0.02, so both are small
(cochange < 0.058, semantic < 0.167) but not provably zero. The
`hand-rebuilt` variant runs this same construction on the hand-written
imports, which checks the construction against the control map.

`summary` merges every repository's result into one JSON and a markdown
report for the step summary.
"""

from __future__ import annotations

import argparse
import json
import random
import re
import subprocess
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path

import scip_ingest

SEED = 7
SAMPLES = 10
ALPHA, BETA, GAMMA, DELTA = 0.45, 0.35, 0.08, 0.12

# Which default-mode indexes make up each re-partition, per repository. A
# language the repository maps but no listed index covers (or whose index
# failed) keeps its hand-written static edges -- issue #110's per-language
# fallback -- and the result says so.
SETS = {
    "langgenius__dify": {
        "scip": ["py-default", "ts-default"],
        "scip-api-root": ["py-api-default", "ts-default"],
        "scip-installs": ["py-api-install", "ts-install"],
    },
    "prometheus__prometheus": {
        "scip": ["go-default", "ts-default"],
        "scip-installs": ["go-install", "ts-default"],
    },
}
SYMBOL_KINDS = [
    "unknown",
    "call",
    "extends",
    "implements",
    "overrides",
    "annotation",
    "decorator",
    "value",
    "possible_implementation",
]


def sample(items) -> list:
    items = sorted(items)
    if len(items) <= SAMPLES:
        return items
    return sorted(random.Random(SEED).sample(items, SAMPLES))


def directory(path: str) -> str:
    return path.rsplit("/", 1)[0] if "/" in path else ""


def proximity(a: str, b: str) -> float:
    """`extract::proximity`: shared leading directory parts over the longer."""
    left = [part for part in directory(a).split("/") if part]
    right = [part for part in directory(b).split("/") if part]
    shared = 0
    for x, y in zip(left, right):
        if x != y:
            break
        shared += 1
    return shared / max(len(left), len(right), 1)


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
        result["peak_rss_kb"] = int(match.group(1))
    match = re.search(r"Exit status: (\d+)", text)
    if match:
        result["exit_status"] = int(match.group(1))
    return result


def set_compare(hand: set, scip: set, with_samples: bool = True) -> dict:
    both = hand & scip
    result = {
        "hand": len(hand),
        "scip": len(scip),
        "both": len(both),
        "scip_only": len(scip - hand),
        "hand_only": len(hand - scip),
        "hand_recall": round(len(both) / len(hand), 4) if hand else None,
        "scip_new_share": round(len(scip - hand) / len(scip), 4) if scip else None,
    }
    if with_samples:
        result["samples_scip_only"] = [list(pair) for pair in sample(scip - hand)]
        result["samples_hand_only"] = [list(pair) for pair in sample(hand - scip)]
    return result


def package_root_of(roots: list[str]):
    ordered = sorted(roots, key=len, reverse=True)

    def find(path: str) -> str:
        for root in ordered:
            if root == "" or path.startswith(root + "/"):
                return root
        return "<none>"

    return find


def parity(tolmap: Path, reference: Path, candidate: Path) -> dict:
    run = subprocess.run(
        [str(tolmap), "parity", str(reference), str(candidate)],
        capture_output=True,
        text=True,
    )
    text = run.stdout + run.stderr
    result = {"report": text.strip().splitlines()[:3]}
    match = re.search(
        r"district placement: ([\d.]+)% \((\d+)/(\d+) common files matched via (\d+)/(\d+) districts\)",
        text,
    )
    if match:
        result.update(
            placement_pct=float(match.group(1)),
            placed=int(match.group(2)),
            common=int(match.group(3)),
            matched_districts=int(match.group(4)),
        )
    match = re.search(r"modularity: reference ([-\d.]+), candidate ([-\d.]+)", text)
    if match:
        result.update(reference_q=float(match.group(1)), candidate_q=float(match.group(2)))
    return result


def build_graph(graph: dict, directed: dict[tuple[str, str], float]) -> tuple[dict, dict]:
    """A dump-graph document with `static_signal` recomputed from `directed`."""
    nodes = graph["nodes"]
    position = {node["file"]: i for i, node in enumerate(nodes)}
    lang_of = {node["file"]: node.get("lang") or graph["lang"] for node in nodes}
    static = defaultdict(float)
    for (a, b), value in sorted(directed.items()):
        key = (a, b) if position[a] < position[b] else (b, a)
        static[key] += value
    static_max = defaultdict(float)
    for (a, _), value in static.items():
        static_max[lang_of[a]] = max(static_max[lang_of[a]], value)
    existing = {(edge["a"], edge["b"]): edge for edge in graph["edges"]}
    keys = sorted(set(existing) | set(static), key=lambda k: (position[k[0]], position[k[1]]))
    edges = []
    new_pairs = 0
    for a, b in keys:
        edge = existing.get((a, b))
        s = static.get((a, b), 0.0) / max(static_max[lang_of[a]], 1.0)
        if edge is None:
            new_pairs += 1
            c, p, sem = 0.0, proximity(a, b), 0.0
        else:
            c, p, sem = edge["cochange"], edge["proximity"], edge["semantic"]
        weight = ALPHA * s + BETA * c + GAMMA * p + DELTA * sem
        if weight < 0.02:
            continue
        edges.append(
            {
                "a": a,
                "b": b,
                "weight": round(weight, 5),
                "static_signal": round(s, 4),
                "cochange": c,
                "proximity": round(p, 4),
                "semantic": sem,
            }
        )
    out = dict(graph)
    out["edges"] = edges
    out["imports"] = [[a, b, round(value, 3)] for (a, b), value in sorted(directed.items())]
    stats = {
        "static_pairs_undirected": len(static),
        "candidate_edges_kept": len(edges),
        "pairs_new_to_candidate_set": new_pairs,
    }
    return out, stats


def compare_repo(args) -> int:
    stem = args.stem
    base = args.base
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    started = time.time()
    map_doc = json.loads((base / f"{stem}.json").read_text())
    symbols_doc = json.loads((base / f"{stem}.symbols.json").read_text())
    graph = json.loads((base / f"{stem}.graph.json").read_text())
    files = map_doc["F"]
    mapped = set(files)
    lang_of = {node["file"]: node.get("lang") or graph["lang"] for node in graph["nodes"]}
    languages = dict(sorted(Counter(lang_of.values()).items()))
    hand = {(files[a], files[b]) for a, b in map_doc["E"]}
    hand_directed = {(a, b): value for a, b, value in graph["imports"]}
    # Both are expected to hold (E is compact()'s copy of the imports, and
    # dump-graph runs the same extraction); recorded rather than asserted so
    # a mismatch is visible in the result instead of losing the whole run.
    consistency = {
        "map_files_equal_graph_nodes": set(lang_of) == mapped,
        "map_E_equals_graph_imports": set(hand_directed) == hand,
    }
    roots = [
        line.strip().rsplit("/", 1)[0] if "/" in line.strip() else ""
        for line in (base / "package_roots.txt").read_text().splitlines()
        if line.strip()
    ]
    root_of = package_root_of(roots)
    spans = scip_ingest.Spans(files, symbols_doc["symbols"])
    symbol_lang = [lang_of[files[row[0]]] for row in symbols_doc["symbols"]]
    kinds = symbols_doc.get("kinds", SYMBOL_KINDS)
    tolmap_pairs_by_kind: dict[str, set] = defaultdict(set)
    tolmap_call_occurrences = Counter()
    for source, target, count, kind in symbols_doc["edges"]:
        tolmap_pairs_by_kind[kinds[kind]].add((source, target))
        if kinds[kind] == "call":
            tolmap_call_occurrences[symbol_lang[source]] += count

    result = {
        "stem": stem,
        "slug": args.slug,
        "commit": args.commit,
        "files": len(files),
        "languages": languages,
        "consistency": consistency,
        "baseline": {
            "districts": len(map_doc["districts"]),
            "q": map_doc["q"],
            "hand_edges": len(hand),
            "symbols": len(symbols_doc["symbols"]),
            "symbol_edges": len(symbols_doc["edges"]),
            "coverage": symbols_doc["coverage"],
            "tolmap_build": parse_time(base / "build.time.txt"),
        },
        "indexes": {},
    }

    ingested: dict[str, dict] = {}
    jobs = sorted(p for p in args.idx.iterdir() if p.is_dir()) if args.idx.is_dir() else []
    for job in jobs:
        meta_path = job / "meta.json"
        if not meta_path.exists():
            continue
        meta = json.loads(meta_path.read_text())
        index_id = job.name.removeprefix(f"scip-idx-{stem}-")
        lang = meta["lang"]
        entry = {
            "lang": lang,
            "variant": meta["variant"],
            "root": meta["root"],
            "exit_codes": meta["exit_codes"],
            "install_exit": meta.get("install_exit"),
            "sha256": meta["sha256"],
            "runs": [parse_time(job / f"run{n}.time.txt") for n in range(1, meta["runs"] + 1)],
            "install": parse_time(job / "install.time.txt"),
            "log_tail": [],
        }
        log = job / "run1.log"
        if log.exists():
            entry["log_tail"] = log.read_text(errors="replace").splitlines()[-15:]
        index_file = job / "index.run1.scip"
        if not index_file.exists() or index_file.stat().st_size == 0:
            entry["status"] = "no index"
            result["indexes"][index_id] = entry
            continue
        entry["index_bytes"] = index_file.stat().st_size
        t0 = time.time()
        data = scip_ingest.ingest(index_file, meta["root"], mapped, lang_of, lang, spans)
        entry["ingest_s"] = round(time.time() - t0, 2)
        entry["status"] = "ok" if meta["exit_codes"][0] == 0 else "index written, nonzero exit"
        if meta["runs"] > 1:
            entry["byte_identical"] = len(set(meta["sha256"])) == 1 and meta["sha256"][0] is not None
            second = job / "index.run2.scip"
            if not entry["byte_identical"] and second.exists():
                again = scip_ingest.ingest(second, meta["root"], mapped, lang_of, lang, spans)
                entry["derived_identical"] = again["fingerprint"] == data["fingerprint"]
            else:
                entry["derived_identical"] = entry["byte_identical"]
        for key in (
            "metadata",
            "documents",
            "duplicate_documents",
            "document_languages",
            "documents_mapped",
            "mapped_files_of_lang",
            "mapped_files_of_lang_indexed",
            "unindexed_lang_files_by_directory",
            "occurrences",
            "definitions",
            "definitions_with_enclosing_range",
            "symbols_defined_in_mapped_files",
            "symbols_defined_in_multiple_mapped_files",
            "references",
            "calls",
            "symbol_stats",
            "relationships",
            "fingerprint",
        ):
            entry[key] = data[key]

        hand_l = {pair for pair in hand if lang_of[pair[0]] == lang}
        scip_all = {(a, b) for a, b, *_ in data["file_edges"] if lang_of[a] == lang}
        scip_uses = {(a, b) for a, b, _, _, uses in data["file_edges"] if uses and lang_of[a] == lang}
        entry["file_edges"] = set_compare(hand_l, scip_all)
        entry["file_edges_uses_only"] = set_compare(hand_l, scip_uses, with_samples=False)
        entry["file_edges_by_target_directory"] = set_compare(
            {(a, directory(b)) for a, b in hand_l},
            {(a, directory(b)) for a, b in scip_all},
            with_samples=False,
        )
        lang_files = [f for f in files if lang_of[f] == lang]
        touched_hand = {x for pair in hand_l for x in pair}
        touched_scip = {x for pair in scip_all for x in pair}
        entry["unmapped_targets_top"] = sorted(data["unmapped_targets"], key=lambda r: (-r[2], r[0], r[1]))[:12]
        entry["files_without_edge"] = {
            "hand": sum(1 for f in lang_files if f not in touched_hand),
            "scip": sum(1 for f in lang_files if f not in touched_scip),
            "of": len(lang_files),
        }
        if lang == "ts":
            cross_hand = {p for p in hand_l if root_of(p[0]) != root_of(p[1])}
            cross_scip = {p for p in scip_all if root_of(p[0]) != root_of(p[1])}
            entry["cross_package"] = set_compare(cross_hand, cross_scip)
            issue101 = lambda p: p[0].startswith("cli/") and p[1].startswith("packages/contracts/")  # noqa: E731
            entry["issue_101_cli_to_contracts"] = {
                "hand": sorted(list(p) for p in hand_l if issue101(p)),
                "scip": sorted(list(p) for p in scip_all if issue101(p)),
                "scip_references_into_unmapped_contracts_files": sum(
                    n for a, b, n in data["unmapped_targets"]
                    if a == "cli" and b.startswith("packages/contracts")
                ),
            }

        scip_pairs = {(a, b): mask for a, b, _, mask in data["symbol_edges"]}
        tolmap_l = {
            kind: {p for p in pairs if symbol_lang[p[0]] == lang}
            for kind, pairs in sorted(tolmap_pairs_by_kind.items())
        }
        all_tolmap = set().union(*tolmap_l.values()) if tolmap_l else set()
        scip_set = set(scip_pairs)
        entry["symbol_pairs"] = {
            "all": set_compare(all_tolmap, scip_set, with_samples=False),
            "tolmap_kind_confirmed_by_scip": {
                kind: {"tolmap": len(pairs), "confirmed": len(pairs & scip_set)}
                for kind, pairs in tolmap_l.items()
            },
            "scip_only_by_target_category": dict(
                sorted(
                    Counter(
                        scip_ingest.CATEGORY_NAMES[bit]
                        for pair, mask in scip_pairs.items()
                        if pair not in all_tolmap
                        for bit in range(8)
                        if mask >> bit & 1
                    ).items()
                )
            ),
            "samples_scip_only": [
                [symbols_doc["symbols"][a][1], files[symbols_doc["symbols"][a][0]],
                 symbols_doc["symbols"][b][1], files[symbols_doc["symbols"][b][0]]]
                for a, b in sample(scip_set - all_tolmap)
            ],
            "samples_tolmap_only": [
                [symbols_doc["symbols"][a][1], files[symbols_doc["symbols"][a][0]],
                 symbols_doc["symbols"][b][1], files[symbols_doc["symbols"][b][0]]]
                for a, b in sample(all_tolmap - scip_set)
            ],
        }
        entry["tolmap_call_edge_occurrences"] = tolmap_call_occurrences[lang]
        impl = {
            (a, b)
            for a, b, flags in data["relationship_pairs"]
            if flags & scip_ingest.REL_IMPLEMENTATION and symbol_lang[a] == lang
        }
        inheritance = set().union(
            *(tolmap_l.get(kind, set()) for kind in ("extends", "implements", "overrides"))
        )
        possible = tolmap_l.get("possible_implementation", set())
        entry["implementation_pairs"] = {
            "scip": len(impl),
            "tolmap_extends_implements_overrides": len(inheritance),
            "both": len(impl & inheritance),
            "tolmap_possible_implementation": len(possible),
            "possible_implementation_confirmed": len(possible & impl),
            "scip_only": len(impl - inheritance - possible),
        }
        result["indexes"][index_id] = entry
        ingested[index_id] = data

    # Re-partition.
    control = base / f"{stem}.control.json"
    result["control_vs_direct_build"] = parity(args.tolmap, base / f"{stem}.json", control)
    sets = SETS.get(stem, {"scip": sorted(i for i, e in result["indexes"].items() if e["variant"] == "default")})
    variants: dict[str, dict] = {"hand-rebuilt": {"directed": hand_directed, "sources": {}}}
    for name, ids in sets.items():
        chosen = {}
        for index_id in ids:
            entry = result["indexes"].get(index_id)
            if entry is None or index_id not in ingested:
                continue
            chosen.setdefault(entry["lang"], index_id)
        for weighting in ("symbols", "binary"):
            directed = {}
            sources = {}
            for lang in languages:
                if lang in chosen:
                    sources[lang] = chosen[lang]
                    for a, b, distinct, _, uses in ingested[chosen[lang]]["file_edges"]:
                        if uses and lang_of[a] == lang:
                            directed[(a, b)] = float(distinct) if weighting == "symbols" else 1.0
                else:
                    sources[lang] = "hand-written (fallback)"
                    for (a, b), value in hand_directed.items():
                        if lang_of[a] == lang:
                            directed[(a, b)] = value
            key = name if weighting == "symbols" else f"{name}/binary"
            variants[key] = {"directed": directed, "sources": sources}

    result["repartition"] = {}
    for key, variant in variants.items():
        safe = key.replace("/", "-")
        document, stats = build_graph(graph, variant["directed"])
        graph_path = out / f"{stem}.{safe}.graph.json"
        graph_path.write_text(json.dumps(document, separators=(",", ":")))
        name = f"{stem}.{safe}"
        t0 = time.time()
        run = subprocess.run(
            [str(args.tolmap), "build", "--graph", str(graph_path), "--name", name,
             "--out", str(out), "--no-parcels"],
            capture_output=True,
            text=True,
        )
        row = {"sources": variant["sources"], **stats, "build_s": round(time.time() - t0, 2)}
        if run.returncode != 0:
            row["error"] = (run.stdout + run.stderr).strip().splitlines()[-5:]
            result["repartition"][key] = row
            continue
        built = json.loads((out / f"{name}.json").read_text())
        row["districts"] = len(built["districts"])
        row["q"] = built["q"]
        row["zero_edge_files"] = (built.get("coverage") or {}).get("zero_edge_files")
        row["vs_control"] = parity(args.tolmap, control, out / f"{name}.json")
        # The product would not switch signals cold: finding 4's warm start
        # seeds Leiden with the previous membership. The same graph,
        # warm-started from the control map, is the retention a user of an
        # existing map would see.
        warm_name = f"{name}.warm"
        run = subprocess.run(
            [str(args.tolmap), "build", "--graph", str(graph_path), "--name", warm_name,
             "--out", str(out), "--no-parcels", "--previous-map", str(control)],
            capture_output=True,
            text=True,
        )
        if run.returncode == 0:
            warm = json.loads((out / f"{warm_name}.json").read_text())
            row["warm"] = {
                "districts": len(warm["districts"]),
                "q": warm["q"],
                "vs_control": parity(args.tolmap, control, out / f"{warm_name}.json"),
            }
        else:
            row["warm"] = {"error": (run.stdout + run.stderr).strip().splitlines()[-5:]}
        result["repartition"][key] = row
        graph_path.unlink()

    result["elapsed_s"] = round(time.time() - started, 1)
    (out / "compare.json").write_text(json.dumps(result, indent=1, sort_keys=True))
    print(json.dumps({k: v for k, v in result.items() if k != "indexes"}, indent=1)[:4000])
    return 0


def fmt(value, digits=1):
    if value is None:
        return "—"
    if isinstance(value, float):
        return f"{value:,.{digits}f}"
    if isinstance(value, int):
        return f"{value:,}"
    return str(value)


def summary(args) -> int:
    results = []
    for path in sorted(args.results.rglob("compare.json")):
        results.append(json.loads(path.read_text()))
    args.out.write_text(json.dumps(results, indent=1, sort_keys=True))
    lines = ["# SCIP spike (#110 P0)", ""]
    lines += [
        "## Index cost and file edges",
        "",
        "| repo | index | status | wall s | peak RSS MB | install s | index MB | docs mapped / lang files | hand | SCIP | both | SCIP-only | hand-only | hand recall | dir-level recall |",
        "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for r in results:
        for index_id, e in sorted(r["indexes"].items()):
            run = e["runs"][0] if e["runs"] else {}
            fe = e.get("file_edges", {})
            dl = e.get("file_edges_by_target_directory", {})
            lines.append(
                f"| {r['slug']} | {index_id} | {e['status']} (exit {e['exit_codes'][0]}) "
                f"| {fmt(run.get('wall_s'))} | {fmt((run.get('peak_rss_kb') or 0) / 1024)} "
                f"| {fmt(e['install'].get('wall_s'))} | {fmt((e.get('index_bytes') or 0) / 1e6)} "
                f"| {fmt(e.get('mapped_files_of_lang_indexed'))} / {fmt(e.get('mapped_files_of_lang'))} "
                f"| {fmt(fe.get('hand'))} | {fmt(fe.get('scip'))} | {fmt(fe.get('both'))} "
                f"| {fmt(fe.get('scip_only'))} | {fmt(fe.get('hand_only'))} "
                f"| {fmt(fe.get('hand_recall'), 3)} | {fmt(dl.get('hand_recall'), 3)} |"
            )
    lines += ["", "## Determinism", ""]
    for r in results:
        for index_id, e in sorted(r["indexes"].items()):
            if len(e["sha256"]) > 1:
                lines.append(
                    f"- {r['slug']} {index_id}: byte-identical={e.get('byte_identical')}, "
                    f"derived-identical={e.get('derived_identical')}, sha256={e['sha256']}"
                )
    lines += [
        "",
        "## Symbols and calls",
        "",
        "| repo | index | tolmap calls resolved / total (all langs) | tolmap call-edge occurrences (this lang) | SCIP callable refs in symbols | to mapped | to repo (unmapped) | external | tolmap pairs | SCIP pairs | both | tolmap call pairs confirmed | impl: SCIP / tolmap inh. / both / P confirmed |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for r in results:
        cov = r["baseline"]["coverage"]
        for index_id, e in sorted(r["indexes"].items()):
            if "calls" not in e:
                continue
            c = e["calls"]
            sp = e["symbol_pairs"]
            call = sp["tolmap_kind_confirmed_by_scip"].get("call", {})
            ip = e["implementation_pairs"]
            lines.append(
                f"| {r['slug']} | {index_id} | {fmt(cov['calls_resolved'])} / {fmt(cov['calls_total'])} "
                f"| {fmt(e.get('tolmap_call_edge_occurrences'))} "
                f"| {fmt(c.get('callable_refs_in_symbols'))} | {fmt(c.get('to_mapped'))} "
                f"| {fmt(c.get('to_repo_unmapped'))} | {fmt(c.get('external'))} "
                f"| {fmt(sp['all']['hand'])} | {fmt(sp['all']['scip'])} | {fmt(sp['all']['both'])} "
                f"| {fmt(call.get('confirmed'))} / {fmt(call.get('tolmap'))} "
                f"| {fmt(ip['scip'])} / {fmt(ip['tolmap_extends_implements_overrides'])} / {fmt(ip['both'])} / "
                f"{fmt(ip['possible_implementation_confirmed'])} of {fmt(ip['tolmap_possible_implementation'])} |"
            )
    lines += ["", "## Cross-package TypeScript edges and #101", ""]
    for r in results:
        for index_id, e in sorted(r["indexes"].items()):
            if "cross_package" in e:
                cp = e["cross_package"]
                i101 = e["issue_101_cli_to_contracts"]
                lines.append(
                    f"- {r['slug']} {index_id}: cross-package hand {cp['hand']}, SCIP {cp['scip']}, both {cp['both']}; "
                    f"cli/→packages/contracts/ (mapped files) hand {len(i101['hand'])}, SCIP {len(i101['scip'])}; "
                    f"SCIP references from cli/ into unmapped packages/contracts files {i101.get('scip_references_into_unmapped_contracts_files')}"
                )
    lines += [
        "",
        "## Re-partition (SCIP edges as the static signal)",
        "",
        "| repo | variant | sources | districts | q | placement vs control | warm-started: districts / q / placement | zero-edge files |",
        "|---|---|---|---:|---:|---:|---:|---:|",
    ]
    for r in results:
        b = r["baseline"]
        cvd = r.get("control_vs_direct_build", {})
        lines.append(
            f"| {r['slug']} | today (direct build) | hand-written | {b['districts']} | {fmt(b['q'], 4)} "
            f"| control {fmt(cvd.get('placement_pct'))}% | — | — |"
        )
        for key, row in sorted(r.get("repartition", {}).items()):
            vc = row.get("vs_control", {})
            w = row.get("warm", {})
            wv = w.get("vs_control", {})
            sources = ", ".join(f"{k}: {v}" for k, v in sorted(row["sources"].items()))
            lines.append(
                f"| {r['slug']} | {key} | {sources or 'hand'} | {fmt(row.get('districts'))} "
                f"| {fmt(row.get('q'), 4)} | {fmt(vc.get('placement_pct'))}% "
                f"| {fmt(w.get('districts'))} / {fmt(w.get('q'), 4)} / {fmt(wv.get('placement_pct'))}% "
                f"| {fmt(row.get('zero_edge_files'))} |"
            )
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
    repo.add_argument("--base", type=Path, required=True)
    repo.add_argument("--idx", type=Path, required=True)
    repo.add_argument("--tolmap", type=Path, required=True)
    repo.add_argument("--out", type=Path, required=True)
    summ = sub.add_parser("summary")
    summ.add_argument("--results", type=Path, required=True)
    summ.add_argument("--out", type=Path, required=True)
    summ.add_argument("--markdown", type=Path, required=True)
    args = parser.parse_args()
    return compare_repo(args) if args.command == "repo" else summary(args)


if __name__ == "__main__":
    sys.exit(main())
