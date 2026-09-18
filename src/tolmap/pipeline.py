"""Stages 03-06: mass-normalised blend, seeded Leiden, two-tier layout, landmarks."""
import json, math, random, sys
from collections import defaultdict, Counter

import igraph as ig
import leidenalg as la
import networkx as nx
import numpy as np

SEED = 7
SHARE = {"static": 0.45, "cochange": 0.35, "prox": 0.08, "sem": 0.12}


def blend(data, share=SHARE):
    """Scale each signal so its TOTAL MASS matches the intended share.

    The per-edge coefficient is not the influence share: a dense signal
    (proximity) accumulates mass across many edges, a sparse one (imports)
    does not. Normalise on mass, then apply the share.
    """
    E = data["edges"]
    raw = {k: sum(e[k] for e in E) or 1.0 for k in share}
    scale = {k: share[k] / raw[k] for k in share}
    for e in E:
        e["w"] = sum(scale[k] * e[k] for k in share)
    # rescale to a friendly range
    mx = max(e["w"] for e in E)
    for e in E:
        e["w"] = e["w"] / mx
    return data


def prune(data, keep_per_node=14, floor=0.02):
    """Keep each node's strongest edges; a fully dense graph clusters into mush."""
    E = data["edges"]
    by = defaultdict(list)
    for e in E:
        by[e["a"]].append(e)
        by[e["b"]].append(e)
    keep = set()
    for n, es in by.items():
        es.sort(key=lambda x: -x["w"])
        for e in es[:keep_per_node]:
            if e["w"] >= floor:
                keep.add((e["a"], e["b"]))
    data["edges"] = [e for e in E if (e["a"], e["b"]) in keep]
    return data


def partition(data, resolution=1.0, seed=SEED):
    files = [n["f"] for n in data["nodes"]]
    idx = {f: i for i, f in enumerate(files)}
    g = ig.Graph(n=len(files))
    g.vs["name"] = files
    es, ws = [], []
    for e in data["edges"]:
        es.append((idx[e["a"]], idx[e["b"]]))
        ws.append(e["w"])
    g.add_edges(es)
    g.es["weight"] = ws
    part = la.find_partition(
        g, la.RBConfigurationVertexPartition,
        weights="weight", resolution_parameter=resolution,
        n_iterations=-1, seed=seed)
    return {files[i]: c for i, c in enumerate(part.membership)}, g, part


def merge_tiny(memb, data, min_size=4):
    """Fold undersized communities into the neighbour they're most attached to.

    Nodes with no cross-district edges at all (empty __init__.py and friends)
    fall back to the district that dominates their directory.
    """
    sizes = Counter(memb.values())
    w = defaultdict(float)
    for e in data["edges"]:
        ca, cb = memb[e["a"]], memb[e["b"]]
        if ca != cb:
            w[(ca, cb)] += e["w"]
            w[(cb, ca)] += e["w"]
    for c, n in sorted(sizes.items(), key=lambda x: x[1]):
        if n >= min_size or sizes[c] == 0:
            continue
        opts = [(wt, o) for (a, o), wt in w.items()
                if a == c and sizes[o] >= min_size]
        if opts:
            tgt = max(opts)[1]
        else:
            # no edges: use the district owning the most files in this directory
            orphans = [f for f, cc in memb.items() if cc == c]
            d = "/".join(orphans[0].split("/")[:-1])
            sib = Counter(memb[f] for f in memb
                          if "/".join(f.split("/")[:-1]) == d and memb[f] != c)
            if not sib:                       # try the parent directory
                d = "/".join(d.split("/")[:-1])
                sib = Counter(memb[f] for f in memb
                              if "/".join(f.split("/")[:-1]).startswith(d)
                              and memb[f] != c and sizes[memb[f]] >= min_size)
            if not sib:
                continue
            tgt = sib.most_common(1)[0][0]
        for f, cc in list(memb.items()):
            if cc == c:
                memb[f] = tgt
        sizes[tgt] += n
        sizes[c] = 0
    # renumber
    order = [c for c, n in Counter(memb.values()).most_common()]
    remap = {c: i for i, c in enumerate(order)}
    return {f: remap[c] for f, c in memb.items()}


def district_graph(memb, data):
    G = nx.Graph()
    for c in set(memb.values()):
        G.add_node(c)
    w = defaultdict(float)
    for e in data["edges"]:
        ca, cb = memb[e["a"]], memb[e["b"]]
        if ca != cb:
            w[tuple(sorted((ca, cb)))] += e["w"]
    for (a, b), wt in w.items():
        G.add_edge(a, b, weight=wt)
    return G


def layout(memb, data, seed=SEED):
    """Tier 1: spring layout on the district graph. Tier 2: squarified treemap."""
    G = district_graph(memb, data)
    pos = nx.spring_layout(G, weight="weight", seed=seed, iterations=400, k=1.1)
    # normalise tier-1 to [0,1]
    xs = [p[0] for p in pos.values()]; ys = [p[1] for p in pos.values()]
    x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
    sx = (x1 - x0) or 1; sy = (y1 - y0) or 1
    cen = {c: ((p[0] - x0) / sx, (p[1] - y0) / sy) for c, p in pos.items()}

    sizes = Counter(memb.values())
    total = sum(sizes.values())
    node_by = defaultdict(list)
    loc = {n["f"]: n["loc"] for n in data["nodes"]}
    for f, c in memb.items():
        node_by[c].append(f)

    districts, coords = {}, {}
    for c, members in node_by.items():
        area = sizes[c] / total
        side = math.sqrt(area) * 0.62          # districts occupy ~62% linear of the field
        cx, cy = cen[c]
        rect = (cx - side / 2, cy - side / 2, side, side)
        districts[c] = {"centroid": [round(cx, 4), round(cy, 4)],
                        "area": round(area, 4), "rect": [round(v, 4) for v in rect],
                        "size": sizes[c]}
        members.sort(key=lambda f: (-loc[f], f))   # stable order
        for f, r in zip(members, squarify([loc[f] for f in members], *rect)):
            coords[f] = {"rect": [round(v, 5) for v in r],
                         "xy": [round(r[0] + r[2] / 2, 5), round(r[1] + r[3] / 2, 5)]}
    return districts, coords, G


def squarify(values, x, y, w, h):
    """Standard squarified treemap; deterministic given the input order."""
    total = sum(values) or 1
    vals = [v * w * h / total for v in values]
    out = [None] * len(vals)
    order = list(range(len(vals)))
    def worst(row, length):
        if not row or length == 0: return float("inf")
        s = sum(r[1] for r in row); mx = max(r[1] for r in row); mn = min(r[1] for r in row)
        if s == 0: return float("inf")
        return max(length * length * mx / (s * s), (s * s) / (length * length * mn))
    i = 0
    while i < len(order):
        row = []
        length = min(w, h)
        while i < len(order):
            cand = row + [(order[i], vals[order[i]])]
            if row and worst(cand, length) > worst(row, length):
                break
            row = cand; i += 1
        s = sum(r[1] for r in row)
        if w >= h:
            rw = s / h if h else 0
            oy = y
            for idx, v in row:
                rh = (v / s * h) if s else 0
                out[idx] = (x, oy, rw, rh); oy += rh
            x += rw; w -= rw
        else:
            rh = s / w if w else 0
            ox = x
            for idx, v in row:
                rw2 = (v / s * w) if s else 0
                out[idx] = (ox, y, rw2, rh); ox += rw2
            y += rh; h -= rh
    return [o if o else (x, y, 0, 0) for o in out]


ENTRY_HINTS = ("__init__.py", "cmdline.py", "app.py", "main.py", "cli.py",
               "crawler.py", "__main__.py")


def landmarks(memb, data, G_file):
    nodes = {n["f"]: n for n in data["nodes"]}
    bet = nx.betweenness_centrality(G_file, weight=None, seed=SEED, k=min(len(G_file), 120))
    picks = []

    def add(f, why, detail):
        if any(p["node"] == f for p in picks):
            return
        picks.append({"node": f, "why": why, "detail": detail,
                      "district": memb[f]})

    # A package __init__ and a global exceptions module always score high on
    # betweenness and fan-in, because every module touches them. They are
    # namespace artefacts, not architectural landmarks — exclude them from the
    # structural selectors (they may still qualify as an entry point).
    def structural(f):
        base = f.split("/")[-1]
        return base != "__init__.py" and base not in {"exceptions.py", "errors.py"}

    # entry
    ent = [f for f in nodes if f.split("/")[-1] in ENTRY_HINTS and nodes[f]["loc"] > 40]
    ent.sort(key=lambda f: -nodes[f]["fanin"])
    for f in ent[:2]:
        add(f, "entry", f"{nodes[f]['loc']} loc")
    # bridge
    for f, b in sorted(((f, b) for f, b in bet.items() if structural(f)),
                       key=lambda x: -x[1])[:2]:
        add(f, "bridge", f"betweenness {b:.3f}")
    # hub
    for f in sorted((f for f in nodes if structural(f)),
                    key=lambda f: -nodes[f]["fanin"])[:2]:
        add(f, "hub", f"fan-in {nodes[f]['fanin']}")
    # capital of each district — fall through if the top node is already a pick,
    # so a district never silently loses its centre to de-duplication
    deg = dict(G_file.degree(weight="weight"))
    by = defaultdict(list)
    for f, c in memb.items():
        by[c].append(f)
    for c, fs in sorted(by.items(), key=lambda x: -len(x[1])):
        taken = {p["node"] for p in picks}
        ranked = sorted((f for f in fs if structural(f)),
                        key=lambda f: -deg.get(f, 0))
        cap = next((f for f in ranked if f not in taken), None)
        if cap:
            add(cap, "capital", f"district {c} centre")
    # hazard
    haz = sorted(nodes, key=lambda f: -(nodes[f]["churn"] * nodes[f]["cplx"]))[:2]
    for f in haz:
        add(f, "hazard", f"churn {nodes[f]['churn']} x cplx {nodes[f]['cplx']}")
    for i, p in enumerate(picks):
        p["rank"] = i + 1
    return picks


def file_graph(data):
    G = nx.Graph()
    for n in data["nodes"]:
        G.add_node(n["f"])
    for e in data["edges"]:
        G.add_edge(e["a"], e["b"], weight=e["w"])
    return G


def run(path, out, resolution=1.0):
    data = json.load(open(path))
    data = blend(data)
    data = prune(data)
    memb, g, part = partition(data, resolution)
    memb = merge_tiny(memb, data)
    districts, coords, DG = layout(memb, data)
    Gf = file_graph(data)
    lms = landmarks(memb, data, Gf)

    sizes = Counter(memb.values())
    mod = part.modularity
    result = {
        "schema": 1, "repo": data["repo"], "pkg": data["pkg"],
        "params": {"seed": SEED, "resolution": resolution, **SHARE},
        "modularity": round(mod, 4),
        "districts": {str(c): {**districts[c], "members": sizes[c]} for c in sizes},
        "nodes": {f: {"d": memb[f], **coords[f],
                      **{k: data_node[k] for data_node in
                         [next(n for n in data["nodes"] if n["f"] == f)]
                         for k in ("loc", "cplx", "churn", "fanin", "mod")}}
                  for f in memb},
        "landmarks": lms,
        "edges": [{"a": e["a"], "b": e["b"], "w": round(e["w"], 4)} for e in data["edges"]],
    }
    json.dump(result, open(out, "w"), indent=1)
    print(f"{data['repo']:8s} districts={len(sizes):2d} modularity={mod:.3f} "
          f"sizes={sorted(sizes.values(), reverse=True)}")
    return result


if __name__ == "__main__":
    run(sys.argv[1], sys.argv[2], float(sys.argv[3]) if len(sys.argv) > 3 else 1.0)
