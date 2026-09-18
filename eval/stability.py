"""Stage 05 validation: does anchoring actually keep the map still?"""
import json, math, subprocess, sys
from collections import Counter, defaultdict

import networkx as nx
import numpy as np

import os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))
from tolmap import extract, pipeline


def checkout(repo, ref):
    subprocess.run(["git", "-C", repo, "checkout", "--quiet", ref], check=True)


def build_layout(repo, pkg, tag, resolution=1.1, prev=None):
    gpath = f"/tmp/claude-0/g_{tag}.json"
    extract.build(repo, pkg, gpath)
    data = pipeline.blend(json.load(open(gpath)))
    data = pipeline.prune(data)
    memb, g, part = pipeline.partition(data, resolution)
    memb = pipeline.merge_tiny(memb, data)
    districts, coords, DG, rank = layout_anchored(memb, data, prev)
    return {"memb": memb, "districts": districts, "coords": coords,
            "data": data, "rank": rank}


def match_districts(memb, prev_memb):
    """Map new district ids onto previous ones by membership Jaccard."""
    if not prev_memb:
        return {}
    new_by, old_by = defaultdict(set), defaultdict(set)
    for f, c in memb.items():
        new_by[c].add(f)
    for f, c in prev_memb.items():
        old_by[c].add(f)
    pairs = []
    for nc, ns in new_by.items():
        for oc, os_ in old_by.items():
            j = len(ns & os_) / len(ns | os_)
            if j > 0:
                pairs.append((j, nc, oc))
    pairs.sort(reverse=True)
    used_n, used_o, m = set(), set(), {}
    for j, nc, oc in pairs:
        if nc in used_n or oc in used_o or j < 0.35:
            continue
        m[nc] = (oc, j)
        used_n.add(nc); used_o.add(oc)
    return m


def layout_anchored(memb, data, prev=None):
    """Tier-1 spring layout warm-started from the previous centroids."""
    G = pipeline.district_graph(memb, data)
    init, fixed = None, None
    if prev:
        m = match_districts(memb, prev["memb"])
        init = {}
        for c in G.nodes():
            if c in m:
                oc, j = m[c]
                init[c] = np.array(prev["districts"][oc]["centroid"], dtype=float)
            else:
                init[c] = np.array([0.5 + 0.01 * c, 0.5 - 0.01 * c])
        # hold well-matched districts still; let the rest settle around them
        fixed = [c for c in G.nodes() if c in m and m[c][1] >= 0.6]
        if len(fixed) < 2:
            fixed = None
    pos = nx.spring_layout(G, weight="weight", seed=pipeline.SEED,
                           iterations=400, k=1.1, pos=init, fixed=fixed)
    if not fixed:                       # only renormalise when nothing is pinned
        xs = [p[0] for p in pos.values()]; ys = [p[1] for p in pos.values()]
        x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
        sx = (x1 - x0) or 1; sy = (y1 - y0) or 1
        pos = {c: ((p[0] - x0) / sx, (p[1] - y0) / sy) for c, p in pos.items()}
    else:
        pos = {c: (float(p[0]), float(p[1])) for c, p in pos.items()}

    sizes = Counter(memb.values())
    total = sum(sizes.values())
    node_by = defaultdict(list)
    loc = {n["f"]: n["loc"] for n in data["nodes"]}
    for f, c in memb.items():
        node_by[c].append(f)

    # Tier-2 anchoring. A squarified treemap is globally sensitive to its input:
    # change one file's size and the whole packing reflows. Freezing the ORDER
    # from the previous layout — known files keep their rank, new files append —
    # is what actually holds files still, not pinning the district centroid.
    prev_rank = prev.get("rank", {}) if prev else {}

    districts, coords, order = {}, {}, {}
    for c, members in node_by.items():
        area = sizes[c] / total
        side = math.sqrt(area) * 0.62
        cx, cy = pos[c]
        rect = (cx - side / 2, cy - side / 2, side, side)
        districts[c] = {"centroid": [round(cx, 5), round(cy, 5)],
                        "area": round(area, 5), "rect": [round(v, 5) for v in rect],
                        "size": sizes[c]}
        if prev_rank:
            members.sort(key=lambda f: (prev_rank.get(f, 10**6), -loc[f], f))
        else:
            members.sort(key=lambda f: (-loc[f], f))
        order[c] = list(members)
        # quantise area so ordinary edits don't reflow the packing
        areas = [max(1, round(loc[f] / 25)) for f in members]
        for f, r in zip(members, pipeline.squarify(areas, *rect)):
            coords[f] = {"rect": [round(v, 5) for v in r],
                         "xy": [round(r[0] + r[2] / 2, 5), round(r[1] + r[3] / 2, 5)]}
    rank = {f: i for fs in order.values() for i, f in enumerate(fs)}
    return districts, coords, G, rank


def displacement(A, B):
    common = set(A["coords"]) & set(B["coords"])
    d = []
    for f in common:
        ax, ay = A["coords"][f]["xy"]; bx, by = B["coords"][f]["xy"]
        d.append(math.hypot(ax - bx, ay - by))
    d = np.array(d)
    return {"n": len(d), "mean": float(d.mean()), "median": float(np.median(d)),
            "p90": float(np.percentile(d, 90)),
            "moved_gt_10pct": float((d > 0.10).mean())}


def repo_main(repo, pkg="." , back=300, resolution=1.1):
    head = subprocess.run(["git","-C",repo,"rev-parse","HEAD"],
                          capture_output=True,text=True).stdout.strip()
    old = subprocess.run(["git","-C",repo,"rev-parse",f"HEAD~{back}"],
                         capture_output=True,text=True).stdout.strip()
    if not old:
        print(f"cannot go back {back} commits"); return
    print(f"old={old[:8]}  head={head[:8]}\n")
    checkout(repo, old);  A = build_layout(repo, pkg, "old", resolution)
    checkout(repo, head); Bn = build_layout(repo, pkg, "new", resolution)
    Ba = build_layout(repo, pkg, "anch", resolution, prev=A)
    checkout(repo, head)
    common = set(A["coords"]) & set(Bn["coords"])
    print(f"{len(set(Bn['coords'])-set(A['coords']))} added, "
          f"{len(set(A['coords'])-set(Bn['coords']))} removed, {len(common)} carried over\n")
    for label, B in (("naive recompute", Bn), ("anchored", Ba)):
        s_ = displacement(A, B)
        print(f"{label:18s} median={s_['median']:.4f}  mean={s_['mean']:.4f}  "
              f"moved>10% of field={s_['moved_gt_10pct']*100:.0f}%")


if __name__ == "__main__":
    import sys as _s
    repo = _s.argv[1] if len(_s.argv) > 1 else "scrapy"
    pkg = _s.argv[2] if len(_s.argv) > 2 else "scrapy"
    back = int(_s.argv[3]) if len(_s.argv) > 3 else 300
    repo_main(repo, pkg, back)
