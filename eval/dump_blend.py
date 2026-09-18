"""Dump the weighted graph Leiden actually partitions, for porting diffs.

`F` and `E` in a map are the file list and the *emitted directed import list*.
Neither is the graph the partitioner sees. That graph is `data["edges"]` after
`blend()` has mass-normalised the four signals and `prune()` has cut each node
to its strongest edges, and it appears nowhere in the map -- so a port can
reproduce `F` and `E` byte-exactly and still hand Leiden a different graph,
whose best partition is equally modular and differently shaped.

Reported modularity does not catch it either: `pipeline.run()` reads
`part.modularity` from the pre-merge partition object, so the number is
structurally blind to `merge_tiny`.

    python eval/dump_blend.py <graph.json> <out.json>

Emits node order, the pruned weighted edge set, the weight total both before
and after pruning, and the raw per-signal masses before normalisation. Diff a
port's equivalent against it: whichever checkpoint first diverges names the
stage at fault. Blend and prune are separately falsifiable only because both
weight totals are present -- with just the post-prune figure a divergence
cannot be attributed to either stage, which is the one question this artifact
exists to answer.

Compare weights with an absolute tolerance of TOLERANCE, not exact equality.
Summing the same terms in a different order can differ in the last ulp, and a
value sitting near a rounding boundary then differs in the emitted digit;
compared exactly that flags every edge and buries the real divergence.
"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src"))

from tolmap import pipeline                                        # noqa: E402

SIGNALS = ("static", "cochange", "prox", "sem")

#: Absolute tolerance for comparing weights against a port's output. Below
#: this is float summation order, above it is a real difference.
TOLERANCE = 1e-9


def dump(graph_path):
    raw = json.load(open(graph_path))
    masses = {k: round(sum(e[k] for e in raw["edges"]), 10) for k in SIGNALS}
    candidates = len(raw["edges"])

    # blend() normalises each signal to its intended share of TOTAL MASS, then
    # divides through by the largest resulting edge. Those five numbers -- the
    # four per-signal scales and the max -- fix the whole weight distribution,
    # and they are checkpoints in their own right rather than only inputs to
    # the edge list. On a repo with a sparse static signal against a dense
    # co-change one, each static edge gets a large scale, one edge dominates,
    # and dividing by it can push most of the graph under prune()'s 0.02 floor:
    # on sqlalchemy 96.1% of edges land below it against 0.1% on scrapy. A port
    # whose raw masses differ slightly then gets a different max, a different
    # floor cut and a different graph -- while every edge it does keep looks
    # correct. Recomputed here rather than read back, since blend() keeps only
    # the blended weight.
    pre = json.load(open(graph_path))
    raw_totals = {k: sum(e[k] for e in pre["edges"]) or 1.0 for k in pipeline.SHARE}
    scale = {k: pipeline.SHARE[k] / raw_totals[k] for k in pipeline.SHARE}
    mx = max(sum(scale[k] * e[k] for k in pipeline.SHARE) for e in pre["edges"])

    blended = pipeline.blend(json.load(open(graph_path)))
    weight_sum_pre_prune = round(sum(e["w"] for e in blended["edges"]), 10)
    blended_weights = sorted(e["w"] for e in blended["edges"])
    below_floor = sum(1 for w in blended_weights if w < 0.02) / len(blended_weights)

    data = pipeline.prune(blended)
    # Totals sum the RAW weights; only the emitted per-edge list is rounded.
    # Summing the rounded values instead makes the total a function of the
    # rounding -- 1561 edges at 10dp shifted scrapy's post-prune figure by
    # 1.24e-9, which is above the tolerance below, so the two checkpoints in
    # one artifact would have disagreed for a reason that is not the port's.
    weight_sum = round(sum(e["w"] for e in data["edges"]), 10)
    edges = sorted((e["a"], e["b"], round(e["w"], 10)) for e in data["edges"])
    return {
        "repo": raw.get("repo"),
        "tolerance": TOLERANCE,
        "candidate_edges": candidates,
        "raw_signal_mass": masses,
        "signal_scale": {k: round(v, 12) for k, v in scale.items()},
        "max_blended_edge": round(mx, 12),
        "below_prune_floor": round(below_floor, 6),
        "n_nodes": len(data["nodes"]),
        "n_blended_edges": len(blended["edges"]),
        "weight_sum_pre_prune": weight_sum_pre_prune,
        "n_pruned_edges": len(edges),
        "weight_sum": weight_sum,
        "node_order": [n["f"] for n in data["nodes"]],
        "edges": [[a, b, w] for a, b, w in edges],
    }


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: dump_blend.py <graph.json> <out.json>")
    out = dump(sys.argv[1])
    json.dump(out, open(sys.argv[2], "w"), indent=0)
    print(f"{out['repo']}: {out['n_nodes']} nodes, {out['candidate_edges']} candidate edges, "
          f"weight {out['weight_sum_pre_prune']} pre-prune -> "
          f"{out['n_pruned_edges']} edges, weight {out['weight_sum']} post-prune "
          f"(compare within {out['tolerance']}); "
          f"{out['below_prune_floor']*100:.1f}% of blended edges below the 0.02 floor")
