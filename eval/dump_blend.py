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

Emits node order, the pruned weighted edge set, the total weight, and the raw
per-signal masses before normalisation. Diff a port's equivalent against it:
whichever checkpoint first diverges names the stage at fault.
"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src"))

from tolmap import pipeline                                        # noqa: E402

SIGNALS = ("static", "cochange", "prox", "sem")


def dump(graph_path):
    raw = json.load(open(graph_path))
    masses = {k: round(sum(e[k] for e in raw["edges"]), 10) for k in SIGNALS}
    candidates = len(raw["edges"])

    data = pipeline.prune(pipeline.blend(json.load(open(graph_path))))
    edges = sorted((e["a"], e["b"], round(e["w"], 10)) for e in data["edges"])
    return {
        "repo": raw.get("repo"),
        "candidate_edges": candidates,
        "raw_signal_mass": masses,
        "n_nodes": len(data["nodes"]),
        "n_pruned_edges": len(edges),
        "weight_sum": round(sum(w for _, _, w in edges), 10),
        "node_order": [n["f"] for n in data["nodes"]],
        "edges": [[a, b, w] for a, b, w in edges],
    }


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: dump_blend.py <graph.json> <out.json>")
    out = dump(sys.argv[1])
    json.dump(out, open(sys.argv[2], "w"), indent=0)
    print(f"{out['repo']}: {out['n_nodes']} nodes, {out['candidate_edges']} candidate edges, "
          f"{out['n_pruned_edges']} pruned, weight_sum {out['weight_sum']}")
