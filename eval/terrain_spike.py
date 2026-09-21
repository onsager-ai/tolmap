"""Measurements behind docs/TERRAIN.md: how big a district may be, and what an
oversized one is made of.

Measurement only. Nothing here feeds the map; it exists so every number in the
proposal can be re-derived from a map and the graph that produced it.

Inputs, per repository `<name>`, in one directory:

    tolmap build <repo> --all-sources --name <name> --out DIR --no-parcels
    tolmap dump-blend <repo> --all-sources --out DIR/<name>.blend.json

(or `--pkg`/`--lang` for a single-source fixture, copying `data/<name>.json`
into DIR instead of rebuilding it). The map gives files `F`, per-file rows `N`
(district, x, y, ...) and the directed import list `E`. The blend dump gives
the weighted, pruned graph Leiden actually partitions. They are different
graphs: `E` is what eval/mapstats.py calls "kept edges"; the blend graph is
what subdivision would re-cluster, so terrain is measured on it.

    python eval/terrain_spike.py scale
    python eval/terrain_spike.py terrain --maps DIR django prometheus n8n ...
    python eval/terrain_spike.py sensitivity --maps DIR

`scale` reads data/*.json plus any `--maps` repositories and prints the
Radical Law table and the size band under two reference scales. `terrain`
decomposes every district above the band into arterials, parcels and organic
sub-districts and scores each piece. `sensitivity` re-runs the decomposition
under the neighbouring band ratios.

The mechanism, in the proposal's terms:

  band      T(N) = sqrt(N) / C_REF. Split above SPLIT*T, merge below MERGE*T.
            C_REF is the median districts/sqrt(files) of the acceptance
            fixtures at or above finding 5's 50-file floor. SPLIT and MERGE
            are the US Census tract ratios (target 4,000, split above 8,000,
            merge below 1,200); they are borrowed, not tuned, and they are the
            only two constants here.
  arterial  a file whose removal strands at least MERGE*T files -- a whole
            sub-district's worth -- out of the district's largest connected
            component. Repeated until no file qualifies.
  parcels   connected components, once arterials leave, smaller than MERGE*T.
  organic   components of at least MERGE*T files. Any above SPLIT*T is split
            by Leiden (resolution 1.1, seed 7, the map's own settings); a
            Leiden community below MERGE*T is folded into the neighbouring
            community it shares most weight with, which always exists because
            the component is connected. Recurse on anything still above
            SPLIT*T.

Determinism: every iteration below is over sorted indices or sorted names; no
set is iterated upstream of Leiden (finding 9).
"""
import argparse
import json
import math
import os
import statistics
import sys
from collections import Counter, defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "data")
FIXTURES = ["django", "prometheus", "sqlalchemy", "vue", "scrapy", "celery", "rich", "flask", "httpx"]
FLOOR_FILES = 50         # finding 5: below ~50 files a repository does not cluster
SPLIT, MERGE = 2.0, 0.3  # US Census tract hysteresis, borrowed
GRID, SIGMA = 420, 9.0   # src/blobs.rs contour raster, mirrored for Polsby-Popper
SEED, RESOLUTION = 7, 1.1


def load_sizes(path):
    with open(path) as handle:
        doc = json.load(handle)
    return len(doc["F"]), Counter(row[0] for row in doc["N"])


def fixture_c_ref():
    cs = []
    for name in FIXTURES:
        n, sizes = load_sizes(os.path.join(DATA, f"{name}.json"))
        if n >= FLOOR_FILES:
            cs.append(len(sizes) / math.sqrt(n))
    return statistics.median(cs)


def band(n, c_ref):
    t = math.sqrt(n) / c_ref
    return t, SPLIT * t, MERGE * t


class Map:
    def __init__(self, directory, name):
        with open(os.path.join(directory, f"{name}.json")) as handle:
            doc = json.load(handle)
        with open(os.path.join(directory, f"{name}.blend.json")) as handle:
            blend = json.load(handle)
        self.name = name
        self.files = doc["F"]
        self.index = {f: i for i, f in enumerate(self.files)}
        self.district = [row[0] for row in doc["N"]]
        self.xy = [(row[1], row[2]) for row in doc["N"]]
        self.names = doc.get("names", {})
        self.members = defaultdict(list)
        for i, d in enumerate(self.district):
            self.members[d].append(i)
        self.adj = defaultdict(dict)
        for a, b, w in blend["edges"]:
            ia, ib = self.index[a], self.index[b]
            if ia != ib:
                self.adj[ia][ib] = self.adj[ia].get(ib, 0.0) + w
                self.adj[ib][ia] = self.adj[ib].get(ia, 0.0) + w

    def oversized(self, c_ref, split=SPLIT):
        n = len(self.files)
        t = math.sqrt(n) / c_ref
        return sorted((d for d, m in self.members.items() if len(m) > split * t and len(m) >= FLOOR_FILES),
                      key=lambda d: (-len(self.members[d]), d))

    def induced(self, nodes):
        return {u: {v: w for v, w in self.adj[u].items() if v in nodes} for u in sorted(nodes)}


def restrict(sub, nodes):
    return {a: {b: w for b, w in sub[a].items() if b in nodes} for a in sorted(nodes)}


def components(sub):
    seen, comps = set(), []
    for start in sorted(sub):
        if start in seen:
            continue
        stack, comp = [start], []
        seen.add(start)
        while stack:
            u = stack.pop()
            comp.append(u)
            for v in sorted(sub[u]):
                if v not in seen:
                    seen.add(v)
                    stack.append(v)
        comps.append(sorted(comp))
    return comps


def largest(sub, nodes):
    return max((len(c) for c in components(restrict(sub, nodes))), default=0)


def arterials(sub, nodes, floor):
    """Files whose removal strands >= floor files from the largest component.

    Stranding is measured against the district's own state before the
    removal, so a district that is already disconnected does not make every
    file look load-bearing. Candidates are articulation points of the current
    graph; the implementation should compute every candidate's stranded count
    exactly from the block-cut tree in O(V+E) rather than capping the search
    as this script does for speed (64 highest-degree articulation points).
    """
    import igraph as ig
    found, nodes = [], set(nodes)
    while True:
        order = sorted(nodes)
        pos = {u: i for i, u in enumerate(order)}
        g = ig.Graph(n=len(order))
        g.add_edges(sorted({tuple(sorted((pos[a], pos[b]))) for a in order for b in sub[a] if b in nodes}))
        cands = [order[i] for i in g.articulation_points()]
        before = largest(sub, nodes)
        best, best_u = 0, None
        for u in sorted(cands, key=lambda x: (-len(sub[x]), x))[:64]:
            stranded = before - 1 - largest(sub, nodes - {u})
            if stranded > best:
                best, best_u = stranded, u
        if best_u is None or best < floor:
            return found, nodes
        found.append((best_u, best))
        nodes.discard(best_u)


def leiden(sub, nodes):
    import igraph as ig
    import leidenalg as la
    order = sorted(nodes)
    pos = {u: i for i, u in enumerate(order)}
    edges, weights = [], []
    for a in order:
        for b in sorted(sub[a]):
            if b in nodes and a < b:
                edges.append((pos[a], pos[b]))
                weights.append(sub[a][b])
    g = ig.Graph(n=len(order), edges=edges)
    g.es["weight"] = weights
    # n_iterations=-1: iterate until no improvement, as src/tolmap/pipeline.py
    # and native/leiden_bridge.cpp both do. leidenalg's default is 2 passes,
    # which is a different search (finding 11's lesson, one parameter over).
    part = la.find_partition(g, la.RBConfigurationVertexPartition, weights="weight",
                             resolution_parameter=RESOLUTION, n_iterations=-1, seed=SEED)
    groups = defaultdict(list)
    for i, c in enumerate(part.membership):
        groups[c].append(order[i])
    return [groups[c] for c in sorted(groups)]


def fold_small(sub, groups, floor, ceiling=math.inf):
    """Fold communities below `floor` into the neighbour they share most weight with.

    A merge that would push the target above `ceiling` is refused while any
    other neighbour can take it. Without that cap the folding snowballs: on
    n8n's `cli` district Leiden splits a 614-file component into 15 communities
    (121, 63, 61, 58, ...), 13 of them below the 63-file floor, and folding each
    into its strongest neighbour rebuilt the original 614 -- undoing the split
    the band had asked for.
    """
    groups = [set(g) for g in groups]
    while True:
        small = [i for i, g in enumerate(groups) if len(g) < floor]
        if not small or len(groups) == 1:
            return [sorted(g) for g in groups]
        i = min(small, key=lambda k: (len(groups[k]), min(groups[k])))
        weight = Counter()
        for u in sorted(groups[i]):
            for v, w in sub[u].items():
                for j, g in enumerate(groups):
                    if j != i and v in g:
                        weight[j] += w
        if not weight:
            return [sorted(g) for g in groups]
        fits = [k for k in weight if len(groups[k]) + len(groups[i]) <= ceiling]
        pool = fits or list(weight)
        j = max(pool, key=lambda k: (weight[k], -k))
        groups[j] |= groups[i]
        del groups[i]


def organic_split(sub, comp, lo, hi):
    if len(comp) <= hi:
        return [sorted(comp)]
    parts = fold_small(sub, leiden(sub, set(comp)), lo, hi)
    if len(parts) == 1:
        return parts
    out = []
    for p in parts:
        out.extend(organic_split(sub, p, lo, hi))
    return out


def polsby_popper(xy, groups, context=()):
    """4*pi*A/P^2 per group, mirroring src/blobs.rs's contour construction.

    `context` groups compete for raster ownership as the rest of the map does
    in blobs.rs and span the same raster; only `groups` are scored. Scored on
    whatever coordinates are passed -- see docs/TERRAIN.md on why current
    coordinates cannot predict the shape of a sub-district.
    """
    import numpy as np
    from scipy.ndimage import gaussian_filter
    from skimage import measure
    everything = list(groups) + list(context)
    pts = np.array([xy[u] for g in everything for u in g])
    low, high = pts.min(0), pts.max(0)
    pad = (high - low).max() * 0.10
    low, high = low - pad, high + pad
    span = max((high - low).max(), 1e-9)
    low = low - (span - (high - low)) / 2

    def cell(p):
        q = (np.array(p) - low) / span * (GRID - 1)
        return int(round(q[1])), int(round(q[0]))

    fields = []
    for g in everything:
        f = np.zeros((GRID, GRID))
        for u in g:
            y, x = cell(xy[u])
            f[y, x] += 1
        fields.append(gaussian_filter(f, max(4.5, SIGMA * (len(g) / 120.0) ** 0.22), mode="constant"))
    fields = np.array(fields)
    owner = fields.argmax(0)
    out = []
    for k, g in enumerate(groups):
        peak = fields[k].max()
        cells = [cell(xy[u]) for u in g]
        for th in [0.16, 0.12, 0.09, 0.07, 0.05, 0.035, 0.02, 0.012, 0.006]:
            mask = (owner == k) & (fields[k] > peak * th)
            if sum(mask[y, x] for y, x in cells) >= 0.95 * len(cells):
                break
        area = mask.sum()
        contours = measure.find_contours(np.pad(mask.astype(float), 1), 0.5)
        perim = sum(np.linalg.norm(np.diff(c, axis=0), axis=1).sum() for c in contours)
        out.append(4 * math.pi * area / perim ** 2 if perim else 0.0)
    return out


def coherence(m, group):
    dirs = Counter(m.files[u].rsplit("/", 1)[0] for u in group)
    return dirs.most_common(1)[0][1] / len(group)


def decompose(m, d, c_ref, merge=MERGE):
    n = len(m.files)
    t = math.sqrt(n) / c_ref
    hi, lo = SPLIT * t, merge * t
    mem = set(m.members[d])
    sub = m.induced(mem)
    arts, rest = arterials(sub, mem, lo)
    rsub = restrict(sub, rest)
    comps = components(rsub)
    parcels = [c for c in comps if len(c) < lo]
    organic = [c for c in comps if len(c) >= lo]
    subs = []
    for c in organic:
        subs.extend(organic_split(rsub, c, lo, hi))
    return dict(arterials=arts, parcels=parcels, subs=subs, rsub=rsub, lo=lo, hi=hi, t=t)


def cmd_scale(args):
    c_ref = fixture_c_ref()
    rows = {name: load_sizes(os.path.join(DATA, f"{name}.json")) for name in FIXTURES}
    for name in args.repos:
        rows[name] = load_sizes(os.path.join(args.maps, f"{name}.json"))
    codex = rows.get("codex")
    print(f"C_REF (median over fixtures >= {FLOOR_FILES} files): {c_ref:.3f}")
    refs = [("fixture median", c_ref)]
    if codex:
        refs.insert(0, ("codex", len(codex[1]) / math.sqrt(codex[0])))
    for label, cref in refs:
        print(f"\n### reference = {label}, c = {cref:.3f}")
        print("| map | files | districts | c | predicted | ratio | T | split above | merge below | districts above | files in them |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for name, (n, sizes) in rows.items():
            t, hi, lo = band(n, cref)
            over = [s for s in sizes.values() if s > hi]
            pred = cref * math.sqrt(n)
            print(f"| {name} | {n:,} | {len(sizes)} | {len(sizes) / math.sqrt(n):.3f} | {pred:.0f} | "
                  f"{len(sizes) / pred:.2f} | {t:.0f} | {hi:.0f} | {lo:.0f} | {len(over)} | {sum(over):,} ({sum(over) / n:.0%}) |")


def cmd_terrain(args):
    c_ref = fixture_c_ref()
    rows = []
    for name in args.names:
        m = Map(args.maps, name)
        order = sorted(m.members)
        baseline = dict(zip(order, polsby_popper(m.xy, [m.members[d] for d in order]))) if args.pp else {}
        for d in m.oversized(c_ref):
            r = decompose(m, d, c_ref)
            subs, parcels = r["subs"], r["parcels"]
            plat = [u for c in parcels for u in c]
            pp = []
            if args.pp and subs:
                context = [m.members[k] for k in order if k != d]
                pp = polsby_popper(m.xy, subs + ([plat] if plat else []), context)[:len(subs)]
            contiguous = sum(1 for s in subs if len(components(restrict(r["rsub"], set(s)))) == 1)
            rows.append(dict(
                map=name, d=d, name=m.names.get(str(d)), size=len(m.members[d]),
                share=len(m.members[d]) / len(m.files),
                band=[round(r["lo"]), round(r["t"]), round(r["hi"])],
                arterials=[(m.files[u], s) for u, s in r["arterials"]],
                parcels=len(parcels), parcel_files=len(plat),
                subs=sorted((len(s) for s in subs), reverse=True),
                in_band=sum(1 for s in subs if r["lo"] <= len(s) <= r["hi"]),
                contiguous=contiguous,
                coherence=[coherence(m, s) for s in subs],
                pp=pp, district_pp=baseline.get(d),
                map_pp_median=statistics.median(baseline.values()) if baseline else None,
            ))
    json.dump(rows, sys.stdout, indent=1)
    print()


def cmd_sensitivity(args):
    c_ref = fixture_c_ref()
    maps = {}
    print("| district | size | " + " | ".join(f"merge={f}" for f in (0.2, 0.3, 0.5)) + " |")
    print("|---|---|---|---|---|")
    for spec in args.cases:
        name, d = spec.split(":")
        d = int(d)
        m = maps.setdefault(name, Map(args.maps, name))
        cells = []
        for merge in (0.2, 0.3, 0.5):
            r = decompose(m, d, c_ref, merge)
            plat = sum(len(c) for c in r["parcels"])
            cells.append(f"{len(r['arterials'])} arterials / {plat / len(m.members[d]):.0%} parcels")
        print(f"| {name} d{d} | {len(m.members[d])} | " + " | ".join(cells) + " |")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("scale")
    s.add_argument("--maps", default=".")
    s.add_argument("repos", nargs="*")
    t = sub.add_parser("terrain")
    t.add_argument("--maps", required=True)
    t.add_argument("--pp", action="store_true", help="also score Polsby-Popper on current coordinates")
    t.add_argument("names", nargs="+")
    v = sub.add_parser("sensitivity")
    v.add_argument("--maps", required=True)
    v.add_argument("cases", nargs="+", help="name:district, e.g. n8n:0")
    args = ap.parse_args()
    {"scale": cmd_scale, "terrain": cmd_terrain, "sensitivity": cmd_sensitivity}[args.cmd](args)


if __name__ == "__main__":
    main()
