#!/usr/bin/env python3
"""Why do districts move so much under `--refs scip`? (issue #110 P2a, finding 46)

    scip_churn.py variants --hand H.graph.json --scip S.graph.json --out DIR [--seeds N]
    scip_churn.py pairs    --hand H.graph.json --scip S.graph.json --repo CLONE --out JSON
    scip_churn.py merges   --hand H.graph.json --scip S.graph.json
                           --hand-map H.json --scip-map S.json --out JSON
    scip_churn.py score    --work DIR --out JSON --markdown MD
    scip_churn.py side-by-side --hand-map H.json --scip-map S.json --name NAME
    scip_churn.py missed   --hand H.graph.json --ingest P0.json --repo CLONE --out JSON

Runs on a GitHub-hosted runner (ci.yml `scip-churn`); the laptop does not
run corpus-scale analysis (CLAUDE.md, finding 38's machine rule).

Finding 45 measured SCIP maps placing 47.8-85.8% of files where the hand
maps do. Placement against a fixture measures agreement, not correctness,
so the question is what that number is made of. This script answers it by
rebuilding the partition from graphs that differ from the hand graph in one
controlled way each, all through the product's own `tolmap build --graph`:

- `perm-K`: the hand graph with its node order shuffled. The graph is the
  same; only vertex ids change, so Leiden (seeded with the fixed SEED = 7)
  walks a different random trajectory. `SEED` is a compile-time constant
  and there is no flag to change it, so this is the seed proxy: the spread
  it gives is the partitioner's own run-to-run sensitivity on that graph.
- `uniform-K` / `candidate-K`: the hand graph with exactly as many static
  pairs removed and added as SCIP removes and adds, chosen at random
  (`random.Random(SEED + K)`). The removed pairs are uniform over the hand
  static pairs. The added pairs are uniform over every file pair
  (`uniform`), or over pairs the graph already links by co-change,
  proximity or semantics (`candidate`, which is local the way real
  references are). Added pairs take SCIP's own added weights, shuffled.
  This is the noise baseline: a graph change of SCIP's size and mass that
  carries no information.
- `pairs`: the hand graph with SCIP's pair set (its removed and added
  pairs), hand weights on the shared pairs. `weights`: the hand pair set
  with SCIP's weights on the shared pairs. Together they split SCIP's
  change into "which pairs" and "how heavy".
- `rebuild-hand` / `rebuild-scip`: the two graphs reassembled by the same
  code that assembles the variants. They must partition exactly as the
  dumped graphs do; `score` reports it, so the construction is checked on
  every run rather than trusted.

How a variant is assembled mirrors `finish_graph` in src/extract.rs: an
edge is a candidate if it has a static pair, co-change, or (for up to 600
files, or within one directory above that) semantic similarity over 0.28;
the static signal is renormalised to a maximum of 1 per graph; an edge
whose ALPHA..DELTA weight is under 0.02 is dropped; every signal is
rounded to four places. Hand and SCIP static values live on different
scales (import counts against distinct referenced symbols), so SCIP values
are brought to hand units with one factor `k`, the ratio of the two
graphs' static mass on their shared pairs. Blend normalises each signal on
its mass (finding 1), so `k` only matters where a variant mixes the two.

A pair added at random that no signal links has no co-change by
construction and gets its proximity from the same formula as the product.
Its semantic value is unknown without the identifier vectors and is set to
0: below the 0.28 candidate floor it can only have been small, so this
makes random additions slightly lighter than real ones, never heavier.
"""

from __future__ import annotations

import argparse
import ast
import json
import random
import sys
from collections import Counter, defaultdict
from pathlib import Path

from remote_build_result import placement

SEED = 7  # src/lib.rs SEED
ALPHA, BETA, GAMMA, DELTA = 0.45, 0.35, 0.08, 0.12  # src/extract.rs
SHARE = {"static_signal": 0.45, "cochange": 0.35, "proximity": 0.08, "semantic": 0.12}  # src/pipeline.rs
SEMANTIC_CANDIDATE = 0.28  # src/extract.rs add_semantic_candidate
FULL_SEMANTIC_SWEEP_MAX_FILES = 600  # src/extract.rs finish_graph
FINISH_FLOOR = 0.02  # src/extract.rs finish_graph
PRUNE_KEEP_PER_NODE = 14  # src/pipeline.rs
NODE_RELATIVE_FRACTION = 0.02  # src/pipeline.rs
SIGNALS = ("static_signal", "cochange", "proximity", "semantic")


def load(path: Path) -> dict:
    return json.loads(Path(path).read_text())


def directory(path: str) -> str:
    return path.rsplit("/", 1)[0] if "/" in path else ""


def proximity(a: str, b: str) -> float:
    """src/extract.rs `proximity`."""
    left = [part for part in directory(a).split("/") if part]
    right = [part for part in directory(b).split("/") if part]
    shared = 0
    for x, y in zip(left, right):
        if x != y:
            break
        shared += 1
    return shared / max(len(left), len(right), 1)


class Graph:
    """A dumped GraphData, indexed by file and by canonical pair."""

    def __init__(self, raw: dict):
        self.raw = raw
        self.files = [node["file"] for node in raw["nodes"]]
        self.index = {file: i for i, file in enumerate(self.files)}
        self.edges = {(e["a"], e["b"]): e for e in raw["edges"]}

    def key(self, a: str, b: str) -> tuple[str, str]:
        return (a, b) if self.index[a] < self.index[b] else (b, a)

    def static_pairs(self) -> dict[tuple[str, str], float]:
        return {pair: e["static_signal"] for pair, e in self.edges.items() if e["static_signal"] > 0}

    def import_pairs(self) -> set[tuple[str, str]]:
        """Undirected pairs of the emitted import list (the map's `E`)."""
        return {self.key(a, b) for a, b, _ in self.raw["imports"] if a != b}


def assemble(hand: Graph, scip: Graph, static: dict[tuple[str, str], float], imports: list) -> dict:
    """A GraphData whose static signal is `static`, finished as finish_graph does."""
    files = hand.files
    many = len(files) > FULL_SEMANTIC_SWEEP_MAX_FILES
    top = max(static.values(), default=0.0) or 1.0
    pairs = set(hand.edges) | set(scip.edges) | set(static)
    edges = []
    for a, b in sorted(pairs, key=lambda p: (hand.index[p[0]], hand.index[p[1]])):
        base = hand.edges.get((a, b)) or scip.edges.get((a, b))
        cochange = base["cochange"] if base else 0.0
        semantic = base["semantic"] if base else 0.0
        near = proximity(a, b)
        value = round(static.get((a, b), 0.0) / top, 4)
        semantic_candidate = semantic > SEMANTIC_CANDIDATE and (not many or directory(a) == directory(b))
        if not (value > 0 or cochange > 0 or semantic_candidate):
            continue
        weight = ALPHA * value + BETA * cochange + GAMMA * near + DELTA * semantic
        if weight < FINISH_FLOOR:
            continue
        edges.append({
            "a": a, "b": b, "weight": round(weight, 5), "static_signal": value,
            "cochange": round(cochange, 4), "proximity": round(near, 4), "semantic": round(semantic, 4),
        })
    out = dict(hand.raw)
    out["edges"] = edges
    out["imports"] = imports
    out.pop("references", None)
    return out


def split(hand: Graph, scip: Graph) -> dict:
    h, s = hand.static_pairs(), scip.static_pairs()
    shared = sorted(set(h) & set(s))
    hand_mass = sum(h[p] for p in shared)
    scip_mass = sum(s[p] for p in shared)
    k = hand_mass / scip_mass if scip_mass else 1.0
    return {
        "hand": h, "scip": s, "k": k,
        "shared": shared,
        "hand_only": sorted(set(h) - set(s)),
        "scip_only": sorted(set(s) - set(h)),
    }


def variants(args) -> int:
    hand, scip = Graph(load(args.hand)), Graph(load(args.scip))
    if hand.files != scip.files:
        raise SystemExit("hand and SCIP graphs have different file lists")
    parts = split(hand, scip)
    h, s, k = parts["hand"], parts["scip"], parts["k"]
    args.out.mkdir(parents=True, exist_ok=True)

    def write(name: str, graph: dict) -> None:
        (args.out / f"{name}.graph.json").write_text(json.dumps(graph, separators=(",", ":")))

    write("rebuild-hand", assemble(hand, scip, dict(h), hand.raw["imports"]))
    write("rebuild-scip", assemble(hand, scip, {p: v * k for p, v in s.items()}, scip.raw["imports"]))
    pairs_only = {p: h[p] for p in parts["shared"]} | {p: s[p] * k for p in parts["scip_only"]}
    write("pairs", assemble(hand, scip, pairs_only, hand.raw["imports"]))
    weights_only = {p: s[p] * k for p in parts["shared"]} | {p: h[p] for p in parts["hand_only"]}
    write("weights", assemble(hand, scip, weights_only, hand.raw["imports"]))

    removed, added = len(parts["hand_only"]), len(parts["scip_only"])
    added_values = sorted(s[p] * k for p in parts["scip_only"])
    hand_pairs = sorted(h)
    n = len(hand.files)
    languages = [node.get("lang") or hand.raw["lang"] for node in hand.raw["nodes"]]
    non_static_candidates = sorted(p for p, e in hand.edges.items() if e["static_signal"] <= 0)
    shortfall = {}
    for counter in range(1, args.seeds + 1):
        for mode in ("uniform", "candidate"):
            rng = random.Random(SEED + counter + (0 if mode == "uniform" else 1000))
            drop = set(rng.sample(hand_pairs, removed)) if removed else set()
            chosen: list[tuple[str, str]] = []
            taken = set(h)
            if mode == "candidate":
                pool = [p for p in non_static_candidates if p not in taken]
                chosen = rng.sample(pool, min(added, len(pool)))
                taken |= set(chosen)
                if len(chosen) < added:
                    shortfall[f"candidate-{counter}"] = added - len(chosen)
            while len(chosen) < added:
                i, j = rng.randrange(n), rng.randrange(n)
                if i == j or languages[i] != languages[j]:
                    continue
                pair = hand.key(hand.files[i], hand.files[j])
                if pair in taken:
                    continue
                taken.add(pair)
                chosen.append(pair)
            values = list(added_values)
            rng.shuffle(values)
            static = {p: v for p, v in h.items() if p not in drop}
            static.update(zip(sorted(chosen, key=lambda p: (hand.index[p[0]], hand.index[p[1]])), values))
            write(f"{mode}-{counter}", assemble(hand, scip, static, hand.raw["imports"]))
        rng = random.Random(SEED + counter + 2000)
        permuted = dict(hand.raw)
        nodes = list(hand.raw["nodes"])
        rng.shuffle(nodes)
        permuted["nodes"] = nodes
        write(f"perm-{counter}", permuted)

    def concentration(values) -> dict:
        """How heavy-tailed a static signal is: SCIP weighs a pair by the
        distinct symbols it references, hand by its import statements."""
        ordered = sorted(values, reverse=True)
        total = sum(ordered) or 1.0
        top = ordered[: max(1, len(ordered) // 10)]
        median = ordered[len(ordered) // 2] if ordered else 0.0
        return {"top_decile_mass_share": sum(top) / total,
                "max_over_median": (ordered[0] / median) if median else None}

    summary = {
        "static_concentration_hand": concentration(h.values()),
        "static_concentration_scip": concentration(s.values()),
        "files": n,
        "static_pairs_hand": len(h),
        "static_pairs_scip": len(s),
        "static_pairs_shared": len(parts["shared"]),
        "static_pairs_hand_only": removed,
        "static_pairs_scip_only": added,
        "k": k,
        "static_mass_share_scip_only": (sum(added_values) / (sum(h[p] for p in parts["shared"]) + sum(added_values)))
        if added_values else 0.0,
        "candidate_shortfall": shortfall,
    }
    (args.out / "summary.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    print(json.dumps(summary, sort_keys=True))
    return 0


# --- blended, pruned graph (src/pipeline.rs NodeRelative), for `merges` ---

def blended(raw: dict) -> dict[tuple[str, str], float]:
    edges = raw["edges"]
    mass = {signal: (sum(e[signal] for e in edges) or 1.0) for signal in SIGNALS}
    weights = [sum(SHARE[sig] / mass[sig] * e[sig] for sig in SIGNALS) for e in edges]
    top = max(weights)
    weights = [w / top for w in weights]
    strongest: dict[str, float] = {}
    incident: dict[str, list[int]] = defaultdict(list)
    for i, e in enumerate(edges):
        for f in (e["a"], e["b"]):
            strongest[f] = max(strongest.get(f, weights[i]), weights[i])
            incident[f].append(i)
    keep = set()
    for f, idx in incident.items():
        idx = sorted(idx, key=lambda i: -weights[i])  # stable, like Rust's sort_by
        floor = NODE_RELATIVE_FRACTION * strongest[f]
        for i in idx[:PRUNE_KEEP_PER_NODE]:
            if weights[i] >= floor:
                keep.add(i)
    return {(edges[i]["a"], edges[i]["b"]): weights[i] for i in sorted(keep)}


def membership(document: dict) -> dict[str, int]:
    return {f: int(row[0]) for f, row in zip(document["F"], document["N"])}


def modularity(weights: dict[tuple[str, str], float], member: dict[str, int], files: list[str]) -> float:
    """Weighted Newman modularity (resolution 1) of `member` on `weights`."""
    total = sum(weights.values())
    if total == 0:
        return 0.0
    strength: Counter = Counter()
    inside: Counter = Counter()
    for (a, b), w in weights.items():
        strength[a] += w
        strength[b] += w
        if member[a] == member[b]:
            inside[member[a]] += w
    by_district: Counter = Counter()
    for f in files:
        by_district[member[f]] += strength[f]
    return sum(inside[d] / total - (by_district[d] / (2 * total)) ** 2 for d in by_district)


def names(document: dict) -> dict[int, str]:
    return {int(k): v for k, v in (document.get("names") or {}).items()}


def merges(args) -> int:
    hand, scip = Graph(load(args.hand)), Graph(load(args.scip))
    hand_map, scip_map = load(args.hand_map), load(args.scip_map)
    wh, ws = blended(hand.raw), blended(scip.raw)
    mh, ms = membership(hand_map), membership(scip_map)
    files = hand.files
    hn, sn = names(hand_map), names(scip_map)
    parts = split(hand, scip)
    status = {p: "shared" for p in parts["shared"]}
    status.update({p: "hand-only" for p in parts["hand_only"]})
    status.update({p: "scip-only" for p in parts["scip_only"]})
    total_h, total_s = sum(wh.values()), sum(ws.values())
    composition = defaultdict(Counter)
    for f in files:
        composition[ms[f]][mh[f]] += 1
    districts = []
    for d, parts_of in sorted(composition.items()):
        districts.append({
            "scip_district": d, "scip_name": sn.get(d, str(d)), "files": sum(parts_of.values()),
            "from_hand": [[hn.get(x, str(x)), c] for x, c in sorted(parts_of.items(), key=lambda xc: (-xc[1], xc[0]))],
        })
    # Every pair of hand districts that share a SCIP district: how much
    # blended weight joins them in each graph, and which edges carry it.
    joins = []
    hand_ids = sorted(set(mh.values()))
    for i, x in enumerate(hand_ids):
        for y in hand_ids[i + 1:]:
            together = sum(min(c[x], c[y]) for c in composition.values())
            if not together:
                continue
            cross = lambda w: [(p, v) for p, v in w.items() if {mh[p[0]], mh[p[1]]} == {x, y}]
            ch, cs = cross(wh), cross(ws)
            delta = defaultdict(float)
            for p, v in cs:
                delta[p] += v / total_s
            for p, v in ch:
                delta[p] -= v / total_h
            top = sorted(delta.items(), key=lambda kv: -kv[1])[: args.top]
            joins.append({
                "hand": [hn.get(x, str(x)), hn.get(y, str(y))],
                "files_sharing_a_scip_district": together,
                "cross_share_hand": sum(v for _, v in ch) / total_h,
                "cross_share_scip": sum(v for _, v in cs) / total_s,
                "top_gains": [
                    {"pair": list(p), "status": status.get(p, "no static"),
                     "share_hand": wh.get(p, 0.0) / total_h, "share_scip": ws.get(p, 0.0) / total_s}
                    for p, _ in top
                ],
            })
    # Weight inside each hand district, before and after: a district whose
    # internal weight SCIP removes has nothing left holding it together.
    inside = []
    for x in hand_ids:
        ih = sum(v for p, v in wh.items() if mh[p[0]] == x == mh[p[1]]) / total_h
        is_ = sum(v for p, v in ws.items() if mh[p[0]] == x == mh[p[1]]) / total_s
        lost = sorted(
            ((p, wh[p] / total_h - ws.get(p, 0.0) / total_s) for p in wh if mh[p[0]] == x == mh[p[1]]),
            key=lambda kv: -kv[1])[: args.top]
        inside.append({
            "hand": hn.get(x, str(x)), "files": sum(1 for f in files if mh[f] == x),
            "inside_share_hand": ih, "inside_share_scip": is_,
            "top_losses": [{"pair": list(p), "status": status.get(p, "no static"), "lost_share": v} for p, v in lost],
        })
    result = {
        "q_hand_partition_on_hand_graph": modularity(wh, mh, files),
        "q_hand_partition_on_scip_graph": modularity(ws, mh, files),
        "q_scip_partition_on_scip_graph": modularity(ws, ms, files),
        "q_scip_partition_on_hand_graph": modularity(wh, ms, files),
        "pruned_edges_hand": len(wh), "pruned_edges_scip": len(ws),
        "scip_districts": districts, "joins": joins, "inside": inside,
    }
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True) + "\n")
    return 0


# --- import classification (Python), for `pairs` and `missed` ---

class Imports(ast.NodeVisitor):
    """Every import statement in one file, with its context."""

    def __init__(self, module: str, is_pkg: bool):
        self.module, self.is_pkg = module, is_pkg
        self.context: list[str] = []
        self.found: list[tuple[list[str], set[str], str, list[str]]] = []

    def base(self, node) -> str:
        if not node.level:
            return node.module or ""
        package = self.module if self.is_pkg else self.module.rsplit(".", 1)[0]
        parts = package.split(".")
        base = ".".join(parts[: len(parts) - (node.level - 1)]) if node.level > 1 else package
        return f"{base}.{node.module}" if node.module else base

    def targets(self, node) -> list[str]:
        if isinstance(node, ast.Import):
            out = []
            for alias in node.names:
                parts = alias.name.split(".")
                out += [".".join(parts[: i + 1]) for i in range(len(parts))]
            return out
        base = self.base(node)
        return [base] + [f"{base}.{alias.name}" for alias in node.names if alias.name != "*"]

    def record(self, node) -> None:
        tags = set(self.context)
        tags.add("relative" if isinstance(node, ast.ImportFrom) and node.level else "absolute")
        if isinstance(node, ast.ImportFrom) and any(a.name == "*" for a in node.names):
            tags.add("star")
        if isinstance(node, ast.ImportFrom):
            base, names = self.base(node), [alias.name for alias in node.names]
        else:
            base, names = "", []
        self.found.append((self.targets(node), tags, base, names))

    visit_Import = record
    visit_ImportFrom = record

    def nested(self, tag, node):
        self.context.append(tag)
        self.generic_visit(node)
        self.context.pop()

    def visit_FunctionDef(self, node):
        self.nested("in-function", node)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_Try(self, node):
        self.nested("in-try", node)

    def visit_If(self, node):
        test = ast.unparse(node.test)
        self.nested("type-checking" if "TYPE_CHECKING" in test else "in-if", node)


def import_index(repo: Path, graph: Graph) -> dict[str, list[tuple[list[str], set[str]]]]:
    out = {}
    for node in graph.raw["nodes"]:
        path = repo / node["file"]
        if not node["file"].endswith(".py") or not path.is_file():
            continue
        try:
            tree = ast.parse(path.read_text(errors="replace"))
        except SyntaxError:
            out[node["file"]] = None
            continue
        visitor = Imports(node["module"], node["file"].endswith("__init__.py"))
        visitor.visit(tree)
        out[node["file"]] = visitor.found
    return out


def classify(a: str, b: str, graph: Graph, imports: dict) -> list[str]:
    """Why `a` links to `b` in the hand graph, from `a`'s import statements."""
    found = imports.get(a)
    if found is None:
        return ["not parsed"]
    module_b = graph.raw["nodes"][graph.index[b]]["module"]
    modules = {node["module"] for node in graph.raw["nodes"]}
    tags = set()
    for targets, context, base, names in found:
        if module_b in targets:
            tags |= context
            # `from pkg import x` names the package itself as its base. When
            # every x is a submodule, the package object is never used; when
            # an x is a name, it is the package's re-export of it.
            if base == module_b and b.endswith("__init__.py") and "*" not in names:
                if all(f"{base}.{name}" in modules for name in names):
                    tags.add("submodule via package")
                else:
                    tags.add("name from package")
    if not tags:
        return ["no import of target"]
    if b.endswith("__init__.py"):
        tags.add("target is __init__")
    if a.endswith("__init__.py"):
        tags.add("source is __init__")
    return sorted(tags)


def directed_imports(graph: Graph) -> set[tuple[str, str]]:
    return {(a, b) for a, b, _ in graph.raw["imports"] if a != b}


def pairs(args) -> int:
    hand, scip = Graph(load(args.hand)), Graph(load(args.scip))
    hi, si = hand.import_pairs(), scip.import_pairs()
    hs, ss = hand.static_pairs(), scip.static_pairs()
    result = {
        "import_pairs": {"hand": len(hi), "scip": len(si), "shared": len(hi & si),
                         "hand_only": len(hi - si), "scip_only": len(si - hi)},
        "static_edges": {"hand": len(hs), "scip": len(ss), "shared": len(set(hs) & set(ss)),
                         "hand_only": len(set(hs) - set(ss)), "scip_only": len(set(ss) - set(hs))},
        "references": scip.raw.get("references"),
    }
    lang = hand.raw["lang"]
    if lang == "py" and args.repo and (scip.raw.get("references") or {}).get("py", {}).get("path") == "scip":
        imports = import_index(args.repo, hand)
        hd, sd = directed_imports(hand), directed_imports(scip)
        s_undirected = {scip.key(a, b) for a, b in sd}
        h_undirected = {hand.key(a, b) for a, b in hd}
        hand_only = sorted(p for p in hd if hand.key(*p) not in s_undirected)
        scip_only = sorted(p for p in sd if scip.key(*p) not in h_undirected)
        uses = defaultdict(set)
        for a, b, name in hand.raw["uses"]:
            uses[(a, b)].add(name)
        rows = []
        for a, b in hand_only:
            rows.append({"a": a, "b": b, "why": classify(a, b, hand, imports), "uses": sorted(uses[(a, b)])[:8]})
        result["hand_only_directed"] = rows
        result["hand_only_by_class"] = dict(Counter(" + ".join(r["why"]) for r in rows).most_common())
        scip_rows = []
        for a, b in scip_only:
            why = classify(a, b, hand, imports)
            scip_rows.append({"a": a, "b": b, "why": why})
        result["scip_only_directed"] = scip_rows
        result["scip_only_by_class"] = dict(Counter(
            "imports target" if r["why"] not in (["no import of target"], ["not parsed"]) else r["why"][0]
            for r in scip_rows).most_common())
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True) + "\n")
    print(json.dumps({k: v for k, v in result.items() if not k.endswith("_directed")}, sort_keys=True))
    return 0


def missed(args) -> int:
    """sqlalchemy (finding 45's fallback): which hand pairs the index lacks."""
    hand = Graph(load(args.hand))
    ingest = load(args.ingest)
    scip_pairs = {(a, b) for a, b, *_ in ingest["file_edges"]}
    scip_undirected = {hand.key(a, b) for a, b in scip_pairs if a in hand.index and b in hand.index}
    hd = directed_imports(hand)
    imports = import_index(args.repo, hand)
    lost = sorted(p for p in hd if p not in scip_pairs)
    rows = [{"a": a, "b": b, "why": classify(a, b, hand, imports),
             "reverse_in_scip": (b, a) in scip_pairs, "undirected_in_scip": hand.key(a, b) in scip_undirected}
            for a, b in lost]
    by_tag = Counter(tag for r in rows for tag in r["why"])
    by_dir = Counter(directory(r["a"]) for r in rows)
    kept = [p for p in hd if p in scip_pairs]
    # The gate compares by file, so a hand pair into a package's
    # `__init__.py` counts as lost when SCIP credits the file inside that
    # package that defines the name the `__init__` re-exports. This is the
    # same comparison with that one case counted as kept.
    targets_of = defaultdict(set)
    for a, b in scip_pairs:
        targets_of[a].add(b)
    reexport_kept = sum(
        1 for a, b in hd
        if (a, b) in scip_pairs or (b.endswith("__init__.py") and any(
            t.startswith(directory(b) + "/") for t in targets_of[a])))
    kept_tags = Counter(tag for a, b in kept for tag in classify(a, b, hand, imports))
    result = {
        "hand_directed": len(hd), "scip_directed": len(scip_pairs), "kept": len(kept),
        "recall": len(kept) / len(hd) if hd else None,
        "recall_counting_package_reexports": reexport_kept / len(hd) if hd else None,
        "missed": len(rows),
        "missed_but_reverse_present": sum(r["reverse_in_scip"] for r in rows),
        "missed_by_tag": dict(by_tag.most_common()),
        "kept_by_tag": dict(kept_tags.most_common()),
        "missed_by_source_directory": dict(by_dir.most_common(15)),
        "ingest_summary": {k: ingest.get(k) for k in (
            "documents", "documents_mapped", "mapped_files_of_lang", "mapped_files_of_lang_indexed",
            "occurrences", "definitions", "symbols_defined_in_mapped_files", "references",
            "unindexed_lang_files_by_directory")},
        "sample": rows[:60],
    }
    args.out.write_text(json.dumps(result, indent=1, sort_keys=True) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "sample"}, indent=1, sort_keys=True))
    return 0


# --- scoring the variant builds ---

def score(args) -> int:
    rows = []
    for fixture in sorted(p for p in args.work.iterdir() if p.is_dir()):
        name = fixture.name
        maps = {}
        for built in sorted(fixture.glob("*/" + name + ".json")):
            maps[built.parent.name] = load(built)
        base = maps.get("hand-graph")
        if base is None:
            continue
        summary_path = args.variants / name / "summary.json"
        row = {"name": name, "summary": load(summary_path) if summary_path.is_file() else None,
               "hand_q": base["q"], "variants": {}}
        for variant, document in sorted(maps.items()):
            fraction, delta = placement(document, base)
            row["variants"][variant] = {
                "placement": fraction, "q": document["q"], "q_delta": delta,
                "districts": len({int(n[0]) for n in document["N"]}),
            }
        # The construction checks: each pair must place 100% with q equal,
        # or the variants are not testing what they claim to.
        checks = {}
        for label, candidate, reference in (
                ("hand-repo = hand-graph", "hand-repo", "hand-graph"),
                ("rebuild-hand = hand-graph", "rebuild-hand", "hand-graph"),
                ("scip-repo = scip-graph", "scip-repo", "scip-graph"),
                ("rebuild-scip = scip-graph", "rebuild-scip", "scip-graph")):
            if candidate in maps and reference in maps:
                fraction, delta = placement(maps[candidate], maps[reference])
                checks[label] = {"placement": fraction, "q_delta": delta}
        row["checks"] = checks
        for label, prefix in (("perm", "perm-"), ("uniform", "uniform-"), ("candidate", "candidate-")):
            values = sorted(v["placement"] for k, v in row["variants"].items() if k.startswith(prefix))
            if values:
                row[label] = {"n": len(values), "min": values[0], "max": values[-1],
                              "mean": sum(values) / len(values), "all": values}
        rows.append(row)
    args.out.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
    lines = ["| fixture | SCIP | pairs only | weights only | random uniform (min–max, mean) | random candidate | node order | construction checks |",
             "|---|---:|---:|---:|---|---|---|---|"]

    def pct(value):
        return "—" if value is None else f"{100 * value:.1f}%"

    for row in rows:
        v = row["variants"]
        get = lambda key: v.get(key, {}).get("placement")
        band = lambda key: (f"{pct(row[key]['min'])}–{pct(row[key]['max'])}, {pct(row[key]['mean'])}"
                            if key in row else "—")
        checks = "; ".join(f"{label} {pct(c['placement'])} Δq {c['q_delta']:.4f}"
                           for label, c in row["checks"].items())
        lines.append(
            f"| {row['name']} | {pct(get('scip-graph'))} | {pct(get('pairs'))} | {pct(get('weights'))} "
            f"| {band('uniform')} | {band('candidate')} | {band('perm')} | {checks} |")
    table = "\n".join(lines) + "\n"
    print(table)
    if args.markdown:
        args.markdown.write_text(table)
    return 0


# --- side-by-side district lists for the owner ---

def side_by_side(args) -> int:
    hand_map, scip_map = load(args.hand_map), load(args.scip_map)
    files = hand_map["F"]
    assert files == scip_map["F"], "file lists differ"
    prefix = ""
    parts = [f.split("/") for f in files]
    common = []
    for column in zip(*parts):
        if len(set(column)) != 1:
            break
        common.append(column[0])
    if common and len(common) < min(len(p) for p in parts):
        prefix = "/".join(common) + "/"
    short = lambda f: f[len(prefix):] if prefix and f.startswith(prefix) else f
    loc = {f: row[3] for f, row in zip(files, hand_map["N"])}
    mh, ms = membership(hand_map), membership(scip_map)
    hn, sn = names(hand_map), names(scip_map)
    fraction, delta = placement(scip_map, hand_map)
    # The parity gate's matching (greedy best Jaccard, 0.35 floor), so
    # "moved" means exactly what the placement number counts.
    groups_h, groups_s = defaultdict(set), defaultdict(set)
    for f in files:
        groups_h[mh[f]].add(f)
        groups_s[ms[f]].add(f)
    candidates = sorted(
        ((len(gs & gh) / len(gs | gh), s, h) for s, gs in groups_s.items() for h, gh in groups_h.items() if gs & gh),
        reverse=True)
    match, used_h, used_s = {}, set(), set()
    for jaccard, s, h in candidates:
        if s in used_s or h in used_h or jaccard < 0.35:
            continue
        match[s] = h
        used_s.add(s)
        used_h.add(h)
    out = [f"## {args.name}", ""]
    out.append(f"Hand: {len(groups_h)} districts, q {hand_map['q']:.4f}.")
    out.append(f"SCIP: {len(groups_s)} districts, q {scip_map['q']:.4f}.")
    out.append(f"Same place: {100 * fraction:.1f}% of {len(files)} files.")
    if prefix:
        out.append(f"Paths are under `{prefix}`.")
    out.append("")

    def listing(groups, label_of, title, extra):
        out.append(f"### {title}")
        out.append("")
        for d in sorted(groups, key=lambda d: (-len(groups[d]), d)):
            members = sorted(groups[d], key=lambda f: (-loc[f], f))
            head = ", ".join(f"`{short(f)}`" for f in members[: args.top])
            more = f", +{len(members) - args.top} more" if len(members) > args.top else ""
            out.append(f"**{label_of(d)}** ({len(members)} files){extra(d)}")
            out.append("")
            out.append(f"{head}{more}")
            out.append("")

    listing(groups_h, lambda d: hn.get(d, str(d)), "Hand districts", lambda d: "")

    def scip_extra(d):
        counts = Counter(mh[f] for f in groups_s[d])
        # Ties sorted by name: `groups_s` holds sets, so most_common()'s
        # insertion order would follow string hashing and differ per run.
        ordered = sorted(counts.items(), key=lambda hc: (-hc[1], hn.get(hc[0], str(hc[0]))))
        src = "; ".join(f"{hn.get(h, str(h))} {c}" for h, c in ordered)
        matched = f", matches **{hn.get(match[d], str(match[d]))}**" if d in match else ", matches none"
        return f"{matched}. From hand: {src}"

    listing(groups_s, lambda d: sn.get(d, str(d)), "SCIP districts", scip_extra)
    out.append("### Files that moved")
    out.append("")
    moves = defaultdict(list)
    for f in files:
        if match.get(ms[f]) != mh[f]:
            moves[(mh[f], ms[f])].append(f)
    if not moves:
        out.append("None.")
    for (h, s), moved in sorted(moves.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        out.append(f"hand **{hn.get(h, str(h))}** → SCIP **{sn.get(s, str(s))}** ({len(moved)})")
        out.append("")
        out.append(", ".join(f"`{short(f)}`" for f in sorted(moved, key=lambda f: (-loc[f], f))))
        out.append("")
    text = "\n".join(out) + "\n"
    if args.out:
        args.out.write_text(text)
    print(text)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("variants")
    run.add_argument("--hand", type=Path, required=True)
    run.add_argument("--scip", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--seeds", type=int, default=8)
    run.set_defaults(run=variants)
    run = commands.add_parser("pairs")
    run.add_argument("--hand", type=Path, required=True)
    run.add_argument("--scip", type=Path, required=True)
    run.add_argument("--repo", type=Path)
    run.add_argument("--out", type=Path, required=True)
    run.set_defaults(run=pairs)
    run = commands.add_parser("merges")
    run.add_argument("--hand", type=Path, required=True)
    run.add_argument("--scip", type=Path, required=True)
    run.add_argument("--hand-map", type=Path, required=True)
    run.add_argument("--scip-map", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--top", type=int, default=8)
    run.set_defaults(run=merges)
    run = commands.add_parser("score")
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--variants", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--markdown", type=Path)
    run.set_defaults(run=score)
    run = commands.add_parser("side-by-side")
    run.add_argument("--hand-map", type=Path, required=True)
    run.add_argument("--scip-map", type=Path, required=True)
    run.add_argument("--name", required=True)
    run.add_argument("--top", type=int, default=5)
    run.add_argument("--out", type=Path)
    run.set_defaults(run=side_by_side)
    run = commands.add_parser("missed")
    run.add_argument("--hand", type=Path, required=True)
    run.add_argument("--ingest", type=Path, required=True)
    run.add_argument("--repo", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.set_defaults(run=missed)
    args = parser.parse_args()
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
