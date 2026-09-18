"""Weighted Voronoi parcels: tile each district so every file gets a plot whose
AREA is proportional to its line count.

A plain Voronoi tiles the plane but its cell areas encode local point density,
which is an artefact of the layout rather than a fact about the code. A power
diagram — cell(i) = argmin |x-p_i|^2 - w_i — tiles the plane too, and the
weights w_i are free parameters we can solve for until area matches target.
"""
import json, math, sys
from collections import defaultdict

import numpy as np
from matplotlib.path import Path
from skimage import measure

GRID = 168          # per-district raster
ITERS = 26
DECIM = 18          # polygon points kept per parcel


def district_mask(blob, lo, span, g=GRID):
    """Rasterise the district outline (possibly several islands)."""
    ys, xs = np.mgrid[0:g, 0:g]
    pts = np.column_stack([xs.ravel(), ys.ravel()]) / (g - 1) * span + lo
    m = np.zeros(g * g, dtype=bool)
    for poly in blob:
        m |= Path(np.asarray(poly)).contains_points(pts)
    return m.reshape(g, g)


def power_cells(points, targets, mask, lo, span, g=GRID, iters=ITERS):
    """Iterate the weights until each cell holds its share of the district."""
    ys, xs = np.nonzero(mask)
    P = np.column_stack([xs, ys]) / (g - 1) * span + lo          # (M,2) world
    n = len(points)
    total = mask.sum()
    if n == 0 or total == 0:
        return None, None
    tgt = np.asarray(targets, dtype=float)
    tgt = tgt / tgt.sum() * total

    d2 = ((P[:, None, :] - points[None, :, :]) ** 2).sum(-1)      # (M,n)
    w = np.zeros(n)
    # a sensible step: weights live in squared-distance units
    step = (span * span) / max(n, 1) * 0.18

    owner = None
    for t in range(iters):
        owner = np.argmin(d2 - w[None, :], axis=1)
        area = np.bincount(owner, minlength=n).astype(float)
        err = (tgt - area) / np.maximum(tgt, 1.0)
        if np.abs(err).max() < 0.08:
            break
        w += step * np.clip(err, -1.5, 1.5) * (1.0 - 0.55 * t / iters)
        w -= w.mean()
    return owner, (ys, xs)


def cell_polys(owner, idx, n, lo, span, g=GRID):
    ys, xs = idx
    out = {}
    for i in range(n):
        sel = owner == i
        if not sel.any():
            continue
        m = np.zeros((g + 2, g + 2), dtype=float)
        m[ys[sel] + 1, xs[sel] + 1] = 1.0
        cs = measure.find_contours(m, 0.5)
        if not cs:
            continue
        cs.sort(key=len, reverse=True)
        poly = cs[0][:, ::-1] - 1.0
        if len(poly) > DECIM:
            poly = poly[:: max(1, len(poly) // DECIM)]
        if len(poly) < 3:
            continue
        out[i] = (poly / (g - 1) * span + lo)
    return out


def build(repo, path=None):
    path = path or f"blob_{repo}.json"
    D = json.load(open(path))
    F, N = D["F"], D["N"]
    by = defaultdict(list)
    for i, row in enumerate(N):
        by[row[0]].append(i)

    parcels = {}
    for dk, members in by.items():
        blob = D["districts"][str(dk)]["blob"]
        if not blob:
            continue
        pts = np.array([[N[i][1], N[i][2]] for i in members], dtype=float)
        lo = pts.min(axis=0)
        hi = pts.max(axis=0)
        allp = np.array([q for poly in blob for q in poly], dtype=float)
        lo = np.minimum(lo, allp.min(axis=0))
        hi = np.maximum(hi, allp.max(axis=0))
        pad = (hi - lo).max() * 0.06
        lo -= pad
        span = float((hi - lo + pad).max())

        # a crowded district needs a finer raster or quantisation, not the
        # weights, ends up deciding the areas
        g = int(max(GRID, min(320, math.sqrt(len(members)) * 26)))
        mask = district_mask(blob, lo, span, g)
        if mask.sum() < len(members) * 4:      # too thin to subdivide honestly
            continue
        loc = [max(N[i][3], 1) for i in members]
        owner, idx = power_cells(pts, loc, mask, lo, span, g)
        if owner is None:
            continue
        polys = cell_polys(owner, idx, len(members), lo, span, g)
        for k, i in enumerate(members):
            if k in polys:
                parcels[str(i)] = [[round(float(x), 4), round(float(y), 4)]
                                   for x, y in polys[k]]

    D["P"] = parcels
    json.dump(D, open(path, "w"), separators=(",", ":"))

    # how faithful did the area encoding stay?
    got, want = [], []
    for k, poly in parcels.items():
        p = np.asarray(poly)
        a = 0.5 * abs(np.dot(p[:, 0], np.roll(p[:, 1], 1)) -
                      np.dot(p[:, 1], np.roll(p[:, 0], 1)))
        got.append(a)
        want.append(N[int(k)][3])
    got, want = np.array(got), np.array(want)
    if len(got) > 3:
        gn = got / got.sum(); wn = want / want.sum()
        corr = np.corrcoef(gn, wn)[0, 1]
    else:
        corr = float("nan")
    import os
    print(f"        parcels={len(parcels)}/{len(F)}  area~loc r={corr:.3f}")


if __name__ == "__main__":
    for r in sys.argv[1:]:
        build(r)
