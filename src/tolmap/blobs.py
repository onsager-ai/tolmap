"""Tier-1 geometry: derive organic district regions from the real file positions.

Points are placed by a per-district spring layout, rasterised into a density
field, assigned winner-take-all, and contoured. The silhouette of each district
is therefore a property of the code in it, not a drawn shape.
"""
import json, math, sys
from collections import defaultdict, Counter

import networkx as nx
import numpy as np
from scipy.ndimage import gaussian_filter
from skimage import measure

SEED = 7
G = 420          # grid resolution
SIG = 9.0        # gaussian sigma, in grid cells
THRESH = 0.16    # contour level, relative to each district's own peak

NAMES = {
    "scrapy": {0:"crawl control",1:"engine & pipelines",2:"http & extraction",3:"command line",
               4:"utilities & config",5:"transport & tls",6:"spider middleware",7:"scheme handlers"},
    "django": {0:"mail & core utils",1:"forms & templating",2:"management & checks",
               3:"views & middleware",4:"orm",5:"gis · geos",6:"db backends",7:"auth & admin",
               8:"gis · gdal",9:"postgres",10:"files & cache",11:"messages"},
    "celery": {0:"worker",1:"tasks & app",2:"result backends",3:"canvas & scheduling",
               4:"command line",5:"events & signals"},
    "sqlalchemy": {0:"dialects",1:"sql core",2:"testing suite",3:"orm",4:"engine",
                   5:"extensions",6:"util",7:"event system",8:"connection pool"},
    "rich": {0:"renderables",1:"console & live",2:"unicode tables"},
    "flask": {0:"app & cli",1:"app core & blueprints",2:"json"},
    "httpx": {0:"client & models",1:"transports"},
    "prometheus": {0:"service discovery",1:"tsdb storage",2:"labels & parsing",
                   3:"promql & rules",4:"remote write",5:"web api",6:"chunk encoding",
                   7:"kubernetes discovery",8:"runtime util",9:"runtime probes",
                   10:"internal tools"},
    "vue": {0:"runtime core",1:"compiler transforms",2:"sfc compiler",
            3:"shared & dom runtime",4:"ssr compiler",5:"v2 compat",6:"reactivity",
            7:"server renderer",8:"test runtime"},
}


def ordered_subgraph(G, nodes):
    """An induced subgraph with iteration order fixed by the caller.

    `G.subgraph(nodes)` returns a *view* whose node iteration follows a set of
    the node names, so on a str-keyed graph its order varies with
    PYTHONHASHSEED. Nothing downstream reads that order deliberately, but
    `subdivide` builds igraph vertex indices from it and `spring_layout` seeds
    its position array from it, so the hash seed leaked all the way into the
    coordinates: two runs on the same commit moved the median file 0.28 of the
    map (finding 9). Membership and modularity never moved -- only geometry.

    Build a real graph instead: nodes in the order given, edges sorted. The
    Rust port must do the same, because its default HashMap iteration is
    randomised too and a faithful port reproduces this exactly.
    """
    H = nx.Graph()
    H.add_nodes_from(nodes)
    keep = set(nodes)
    seen = set()
    for a, b, d in G.edges(data=True):
        if a in keep and b in keep:
            seen.add((a, b) if a <= b else (b, a))
    for a, b in sorted(seen):
        H.add_edge(a, b, **G[a][b])
    return H


def subdivide(sub, min_n=40, target=14):
    """Leiden again inside a district. Large districts have internal structure;
    laying their files out as one cloud produces a featureless disc, which is
    the one shape a reader cannot remember."""
    import igraph as ig, leidenalg as la
    fs = list(sub.nodes())
    if len(fs) < min_n:
        return {f: 0 for f in fs}
    idx = {f: i for i, f in enumerate(fs)}
    g = ig.Graph(n=len(fs))
    g.add_edges([(idx[a], idx[b]) for a, b in sub.edges()])
    g.es["weight"] = [sub[a][b].get("weight", 1.0) for a, b in sub.edges()]
    res = max(0.6, len(fs) / target / 3.0)
    p = la.find_partition(g, la.RBConfigurationVertexPartition, weights="weight",
                          resolution_parameter=res, n_iterations=-1, seed=SEED)
    return {fs[i]: c for i, c in enumerate(p.membership)}


def pack(sizes):
    """Greedy circle packing, largest first, each new circle set at the free
    spot nearest the centre. Deterministic, and it produces clumps rather than
    the ring a force layout falls into on a disconnected graph."""
    items = sorted(sizes.items(), key=lambda kv: (-kv[1], kv[0]))
    placed = []          # (x, y, r)
    out = {}
    for k, n in items:
        r = math.sqrt(max(n, 1)) * 0.30
        if not placed:
            out[k] = np.zeros(2); placed.append((0.0, 0.0, r)); continue
        best, bestd = None, float("inf")
        for (px, py, pr) in placed:                 # tangent candidates
            for t in range(48):
                a = 2 * math.pi * t / 48
                x = px + (pr + r) * math.cos(a)
                y = py + (pr + r) * math.sin(a)
                if any((x - qx) ** 2 + (y - qy) ** 2 < (qr + r) ** 2 - 1e-9
                       for (qx, qy, qr) in placed):
                    continue
                d = x * x + y * y
                if d < bestd:
                    bestd, best = d, (x, y)
        if best is None:
            ang = 2.399963 * len(placed)
            rad = 0.4 * math.sqrt(len(placed) + 1)
            best = (rad * math.cos(ang), rad * math.sin(ang))
        out[k] = np.array(best, dtype=float)
        placed.append((best[0], best[1], r))
    return out


def place(layout, graph):
    """Two levels of spring layout: sub-clusters within a district, then files
    within each sub-cluster. The lobes this produces are what give a district a
    silhouette instead of a disc."""
    memb = {f: v["d"] for f, v in layout["nodes"].items()}
    by = defaultdict(list)
    for f, c in memb.items():
        by[c].append(f)

    Gf = nx.Graph()
    Gf.add_nodes_from(memb)
    for e in graph["edges"]:
        if e["a"] in memb and e["b"] in memb:
            Gf.add_edge(e["a"], e["b"], weight=e["w"])

    total = len(memb)
    pts, subs = {}, {}
    for c, fs in by.items():
        sub = ordered_subgraph(Gf, fs)
        smemb = subdivide(sub)
        subs.update({f: (c, s) for f, s in smemb.items()})
        sby = defaultdict(list)
        for f, s in smemb.items():
            sby[s].append(f)

        # tier A: arrange the sub-clusters
        SG = nx.Graph()
        SG.add_nodes_from(sby)
        w = Counter()
        for a, b, d in sub.edges(data=True):
            sa, sb = smemb[a], smemb[b]
            if sa != sb:
                w[tuple(sorted((sa, sb)))] += d.get("weight", 1.0)
        for (a, b), v in w.items():
            SG.add_edge(a, b, weight=v)
        # A spring layout of a disconnected sub-cluster graph degenerates into a
        # ring — the components get pushed onto a circle. Pack instead: it is
        # deterministic, and it clumps, which is what makes a coastline.
        if len(sby) > 1 and nx.is_connected(SG) and SG.number_of_edges() >= len(sby):
            sp = nx.spring_layout(SG, weight="weight", seed=SEED, iterations=300)
        elif len(sby) > 1:
            sp = pack({s: len(m) for s, m in sby.items()})
        else:
            sp = {list(sby)[0]: np.zeros(2)}

        # Normalise the sub-cluster spacing so typical neighbours sit 1 apart,
        # then size each sub-cluster's point cloud against that spacing. If the
        # clouds are much smaller than the gaps between them, the district
        # shatters into scattered islands instead of forming one lobed coast.
        keys = list(sby)
        C = np.array([np.asarray(sp[s], dtype=float) for s in keys])
        if len(keys) > 1:
            dm = np.linalg.norm(C[:, None, :] - C[None, :, :], axis=-1)
            np.fill_diagonal(dm, np.inf)
            nn = np.median(dm.min(axis=1)) or 1.0
            C = C / nn
        nmean = max(np.mean([len(m) for m in sby.values()]), 1.0)

        arr = np.zeros((len(fs), 2))
        order = []
        for s, base in zip(keys, C):
            members = sby[s]
            g2 = ordered_subgraph(sub, members)
            p2 = (nx.spring_layout(g2, weight="weight", seed=SEED, iterations=200)
                  if len(members) > 1 else {members[0]: np.zeros(2)})
            a2 = np.array([p2[f] for f in members], dtype=float)
            a2 -= a2.mean(axis=0)
            r2 = np.percentile(np.linalg.norm(a2, axis=1), 88) or 1.0
            spread = 0.62 * math.sqrt(len(members) / nmean)
            a2 = a2 / r2 * spread
            for f, xy in zip(members, a2 + base):
                order.append(f)
                arr[len(order) - 1] = xy

        arr -= arr.mean(axis=0)
        r = np.percentile(np.linalg.norm(arr, axis=1), 90) or 1.0
        # pull extreme outliers in: one stray file must not drag the coastline
        n = np.linalg.norm(arr, axis=1)
        cap = r * 1.5
        far = n > cap
        arr[far] *= (cap / n[far])[:, None]
        arr /= r
        scale = 1.30 * math.sqrt(len(fs) / total)
        cx, cy = layout["districts"][str(c)]["centroid"]
        arr = arr * scale + np.array([cx, cy])
        for f, xy in zip(order, arr):
            pts[f] = xy
    return memb, pts, by, subs


def relax(pts, memb, by, layout, rounds=90):
    """Push districts apart until their point clouds stop overlapping."""
    cen = {c: np.mean([pts[f] for f in fs], axis=0) for c, fs in by.items()}
    rad = {c: np.percentile([np.linalg.norm(pts[f] - cen[c]) for f in fs], 92)
           for c, fs in by.items()}
    ks = list(by)
    for _ in range(rounds):
        moved = 0.0
        for i in range(len(ks)):
            for j in range(i + 1, len(ks)):
                a, b = ks[i], ks[j]
                d = cen[b] - cen[a]
                dist = np.linalg.norm(d) or 1e-6
                want = (rad[a] + rad[b]) * 1.18
                if dist < want:
                    push = (want - dist) / 2
                    u = d / dist
                    cen[a] -= u * push * 0.5
                    cen[b] += u * push * 0.5
                    moved += push
        if moved < 1e-4:
            break
    for c, fs in by.items():
        old = np.mean([pts[f] for f in fs], axis=0)
        shift = cen[c] - old
        for f in fs:
            pts[f] = pts[f] + shift
    return pts


def contours(pts, memb, by):
    P = np.array(list(pts.values()))
    lo = P.min(axis=0); hi = P.max(axis=0)
    pad = (hi - lo).max() * 0.10
    lo -= pad; hi += pad
    span = (hi - lo).max()
    lo -= (span - (hi - lo)) / 2
    hi = lo + span

    def to_grid(xy):
        return (xy - lo) / span * (G - 1)

    fields = {}
    for c, fs in by.items():
        g = np.zeros((G, G), dtype=float)
        for f in fs:
            gx, gy = to_grid(pts[f])
            xi, yi = int(round(gx)), int(round(gy))
            if 0 <= xi < G and 0 <= yi < G:
                g[yi, xi] += 1.0
        sig = max(4.5, SIG * (len(fs) / 120.0) ** 0.22)
        fields[c] = gaussian_filter(g, sig, mode="constant")

    ks = sorted(by)
    stack = np.stack([fields[c] for c in ks])
    owner = np.argmax(stack, axis=0)
    peak = stack.max(axis=0)

    blobs = {}
    for i, c in enumerate(ks):
        # A region must contain its own files. Rather than a fixed contour level,
        # lower the threshold until at least 95% of the district's points fall
        # inside — orphan points outside their own district are a correctness
        # bug in a map, not a cosmetic one.
        cells = []
        for f in by[c]:
            gx, gy = to_grid(pts[f])
            cells.append((int(round(gy)), int(round(gx))))
        own = None
        for t in (THRESH, 0.12, 0.09, 0.07, 0.05, 0.035, 0.02, 0.012, 0.006):
            cand = (owner == i) & (fields[c] > fields[c].max() * t)
            inside = sum(1 for (yy, xx) in cells
                         if 0 <= yy < G and 0 <= xx < G and cand[yy, xx])
            own = cand
            if inside >= 0.95 * len(cells):
                break
        m = np.zeros((G + 2, G + 2), dtype=float)
        m[1:-1, 1:-1] = own.astype(float)
        cs = measure.find_contours(m, 0.5)
        if not cs:
            continue
        # A district whose members form disconnected clumps is an archipelago,
        # and drawing only its largest island orphans the rest. Keep every
        # island of meaningful size.
        cs.sort(key=len, reverse=True)
        polys = []
        for cc in cs:
            if len(cc) < 0.12 * len(cs[0]) or len(cc) < 12:
                continue
            poly = cc[:, ::-1] - 1.0                      # (row,col) -> (x,y)
            poly = poly[:: max(1, len(poly) // 130)]
            if len(poly) < 4:
                continue
            poly = chaikin(poly, 2)
            polys.append(poly / (G - 1) * span + lo)
        if polys:
            blobs[c] = polys
    return blobs, lo, span, to_grid


def chaikin(p, n=2):
    for _ in range(n):
        out = []
        for i in range(len(p)):
            a, b = p[i], p[(i + 1) % len(p)]
            out.append(a * 0.75 + b * 0.25)
            out.append(a * 0.25 + b * 0.75)
        p = np.array(out)
    return p


def build(repo, res_layout, res_graph, out):
    layout = json.load(open(res_layout))
    graph = json.load(open(res_graph))
    memb, pts, by, subs = place(layout, graph)
    pts = relax(pts, memb, by, layout)
    blobs, lo, span, _ = contours(pts, memb, by)

    w = Counter()
    for e in graph["edges"]:
        if e["a"] not in memb or e["b"] not in memb:
            continue
        a, b = memb[e["a"]], memb[e["b"]]
        if a != b:
            w[tuple(sorted((a, b)))] += e["w"]
    mx = max(w.values()) if w else 1
    cen = {c: np.mean([pts[f] for f in fs], axis=0).tolist() for c, fs in by.items()}
    roads = [[a, b, round(v / mx, 3)] for (a, b), v in w.items() if v / mx > 0.18]

    # Compact, index-addressed payload: seven repositories have to fit in one page.
    F = sorted(memb)
    ix = {f: i for i, f in enumerate(F)}
    N = layout["nodes"]
    lmax = max(N[f]["loc"] for f in F)
    rows = [[memb[f], round(float(pts[f][0]), 4), round(float(pts[f][1]), 4),
             N[f]["loc"], N[f]["cplx"], N[f]["churn"], N[f]["fanin"]] + \
            [round(v, 5) for v in N[f]["rect"]] for f in F]

    # directed import edges: the only signal that supports a real route
    E = [[ix[a], ix[b]] for a, b, n in graph.get("imports", [])
         if a in ix and b in ix]

    L = [[ix[l["node"]], l["why"], l["detail"], l["rank"]]
         for l in layout["landmarks"] if l["node"] in ix]

    # S: the address level below a file — its classes, functions and types,
    # kept in source order so a building's floors read bottom to top.
    S = {}
    for f, lst in (graph.get("symbols") or {}).items():
        if f in ix and lst:
            S[str(ix[f])] = [[n[0][:44], n[1], n[2], n[3]] for n in lst]

    # U: blast radius. For each symbol, which files reference it — the query
    # that makes symbol granularity worth having, since the answer is a sparse
    # SET of files, which is exactly what a map is good at showing.
    symidx = {}
    for fkey, lst in (graph.get("symbols") or {}).items():
        if fkey not in ix:
            continue
        m = {}
        for n, sm in enumerate(lst):
            m.setdefault(sm[0], n)
            base = sm[0].split(".")[-1]
            m.setdefault(base, n)
        symidx[ix[fkey]] = m
    # `from pkg import Thing` usually lands on pkg/__init__.py, which defines
    # nothing and merely re-exports. Follow that chain to the file that actually
    # defines the name, or almost every reference resolves to an empty __init__.
    fwd = defaultdict(list)
    for a, b, nm in (graph.get("uses") or []):
        if a in ix and b in ix:
            fwd[(ix[b], nm)].append(ix[a])
    raw_uses = [(ix[a], ix[b], nm) for a, b, nm in (graph.get("uses") or [])
                if a in ix and b in ix]
    out_edges = defaultdict(set)
    for a, b, nm in raw_uses:
        out_edges[(a, nm)].add(b)

    def define_site(b, nm, hops=4):
        seen = set()
        frontier = [b]
        while frontier and hops >= 0:
            nxt = []
            for cur in frontier:
                if cur in seen:
                    continue
                seen.add(cur)
                j = symidx.get(cur, {}).get(nm)
                if j is not None:
                    return cur, j
                nxt.extend(out_edges.get((cur, nm), ()))
            frontier = nxt
            hops -= 1
        return None, None

    U = defaultdict(set)
    cache = {}
    for a, b, nm in raw_uses:
        key = (b, nm)
        if key not in cache:
            cache[key] = define_site(b, nm)
        d, j = cache[key]
        if d is not None and d != a:
            U[f"{d}:{j}"].add(a)
    U = {k: sorted(v) for k, v in U.items() if v}

    doc = {
        "repo": repo, "q": layout["modularity"],
        "names": {str(k): v for k, v in NAMES.get(repo, {}).items()}
                 or {str(c): f"d{c}" for c in by},
        "districts": {str(c): {"size": len(fs), "c": [round(x, 4) for x in cen[c]],
                               "blob": [[[round(float(x), 4), round(float(y), 4)]
                                         for x, y in poly] for poly in blobs.get(c, [])]}
                      for c, fs in by.items()},
        "F": F, "N": rows, "E": E, "L": L, "S": S, "U": U, "roads": roads,
        "lang": graph.get("lang", "py"),
    }
    json.dump(doc, open(out, "w"), separators=(",", ":"))
    import os
    print(f"{repo:11s} d={len(by):2d} files={len(F):4d} imports={len(E):5d} "
          f"symbols={sum(len(v) for v in S.values()):5d} "
          f"referenced={len(U):5d} bytes={os.path.getsize(out):7d}")


if __name__ == "__main__":
    for repo in sys.argv[1:]:
        build(repo, f"layout_{repo}.json", f"graph_{repo}.json", f"blob_{repo}.json")
