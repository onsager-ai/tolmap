#!/usr/bin/env python3
"""Ten displayed-footprint metrics for a product map. Only two are gates.

Usage: python3 eval/footprint_metrics.py map.json [--gate] [--out result.json]
No repository checkout, third-party package, or source text is required.
"""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
import math
from pathlib import Path
import random
import statistics


def area(ring):
    return abs(sum(x * ring[(i + 1) % len(ring)][1] - y * ring[(i + 1) % len(ring)][0]
                   for i, (x, y) in enumerate(ring))) / 2 if len(ring) >= 3 else 0.0


def perimeter(ring):
    return sum(math.dist(point, ring[(i + 1) % len(ring)]) for i, point in enumerate(ring))


def inside(point, ring):
    result = False
    x, y = point
    for i, a in enumerate(ring):
        b = ring[i - 1]
        if (a[1] > y) != (b[1] > y) and x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]:
            result = not result
    return result


def components(rings):
    """Count exterior rings; a hole nested inside an exterior is not a component."""
    valid = [ring for ring in rings if area(ring) > 0]
    return sum(sum(inside(ring[0], other) for j, other in enumerate(valid) if j != i) % 2 == 0
               for i, ring in enumerate(valid))


def pearson(a, b):
    if len(a) < 3 or len(a) != len(b):
        return None
    am, bm = statistics.mean(a), statistics.mean(b)
    numerator = sum((x - am) * (y - bm) for x, y in zip(a, b))
    denominator = math.sqrt(sum((x - am) ** 2 for x in a) * sum((y - bm) ** 2 for y in b))
    return numerator / denominator if denominator else None


def ranks(values):
    order = sorted(range(len(values)), key=lambda i: (values[i], i))
    result = [0.0] * len(values)
    j = 0
    while j < len(order):
        k = j + 1
        while k < len(order) and values[order[k]] == values[order[j]]:
            k += 1
        for pos in order[j:k]:
            result[pos] = (j + k - 1) / 2
        j = k
    return result


def spearman(a, b):
    return pearson(ranks(a), ranks(b))


def median(values):
    return statistics.median(values) if values else None


def percentile(values, fraction):
    position = fraction * (len(values) - 1)
    lower = math.floor(position)
    upper = math.ceil(position)
    return values[lower] + (values[upper] - values[lower]) * (position - lower)


def level(groups, points, edges):
    keys = sorted(groups)
    centers = {}
    radii = {}
    sizes = {}
    for key in keys:
        members = groups[key]
        sizes[key] = len(members)
        centers[key] = [statistics.mean(points[i][axis] for i in members) for axis in (0, 1)]
        distances = sorted(math.dist(points[i], centers[key]) for i in members)
        radii[key] = percentile(distances, 0.92)
    owner = {i: key for key, members in groups.items() for i in members}
    weights = Counter()
    for (a, b), weight in edges.items():
        if a in owner and b in owner and owner[a] != owner[b]:
            weights[tuple(sorted((owner[a], owner[b])))] += weight
    gap = lambda a, b: math.dist(centers[a], centers[b]) - radii[a] - radii[b]
    density = lambda a, b: weights[tuple(sorted((a, b)))] / (sizes[a] * sizes[b])
    pairs = [(a, b) for j, a in enumerate(keys) for b in keys[j + 1:]]
    rho = spearman([density(a, b) for a, b in pairs], [gap(a, b) for a, b in pairs]) if len(pairs) >= 3 else None
    hits = []
    for a in keys:
        others = [b for b in keys if b != a]
        if not others or not any(weights[tuple(sorted((a, b)))] for b in others):
            continue
        strongest = max(others, key=lambda b: (density(a, b), str(b)))
        nearest = min(others, key=lambda b: (gap(a, b), str(b)))
        hits.append(strongest == nearest)
    return (statistics.mean(hits) if hits else None, rho,
            sum(gap(a, b) < 0 for a, b in pairs))


def delaunay_edges(members, points):
    """Bowyer-Watson with a triangle walk and cavity flood, using stdlib only.

    A full scan of all triangles for each inserted point is quadratic at
    12k files. Adjacent triangles around each new point form one cavity, so
    edge adjacency limits the work to that cavity after a local walk.
    """
    if len(members) < 3:
        return set()
    xs = [points[i][0] for i in members]
    ys = [points[i][1] for i in members]
    lo = min(xs), min(ys)
    scale = max(max(xs) - lo[0], max(ys) - lo[1], 1e-9)
    coordinates = [(-10.0, -10.0), (11.0, -10.0), (0.5, 11.0)]
    order = sorted(members, key=lambda i: (points[i][0], points[i][1], i))
    for i in order:
        # Distinct, deterministic perturbations resolve duplicate displayed
        # centroids and cocircular ties without depending on Qhull's options.
        delta = ((i * 73856093) % 1009 + 1) * 1e-12
        delta_y = ((i * 19349663) % 1013 + 1) * 1e-12
        coordinates.append(((points[i][0] - lo[0]) / scale + delta,
                            (points[i][1] - lo[1]) / scale + delta_y))
    triangles = {}
    edge_to_triangles = defaultdict(set)
    incident = defaultdict(set)
    next_id = 0

    def triangle_edges(a, b, c):
        return tuple(tuple(sorted(edge)) for edge in ((a, b), (b, c), (c, a)))

    def orient(a, b, c):
        ax, ay = coordinates[a]
        bx, by = coordinates[b]
        cx, cy = coordinates[c]
        return (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)

    def add(a, b, c):
        nonlocal next_id
        if orient(a, b, c) < 0:
            b, c = c, b
        ax, ay = coordinates[a]
        bx, by = coordinates[b]
        cx, cy = coordinates[c]
        denominator = 2 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by))
        if abs(denominator) < 1e-18:
            return
        aa, bb, cc = ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy
        ux = (aa * (by - cy) + bb * (cy - ay) + cc * (ay - by)) / denominator
        uy = (aa * (cx - bx) + bb * (ax - cx) + cc * (bx - ax)) / denominator
        radius2 = (ax - ux) ** 2 + (ay - uy) ** 2
        key = next_id
        next_id += 1
        triangles[key] = (a, b, c, ux, uy, radius2)
        for vertex in (a, b, c):
            incident[vertex].add(key)
        for edge in triangle_edges(a, b, c):
            edge_to_triangles[edge].add(key)

    add(0, 1, 2)
    previous = 0
    for current in range(3, len(coordinates)):
        x, y = coordinates[current]
        start = min(incident[previous]) if incident[previous] else min(triangles)
        chosen = None
        visited = set()
        while start not in visited:
            visited.add(start)
            a, b, c, *_ = triangles[start]
            outside = [edge for edge in ((a, b), (b, c), (c, a))
                       if orient(edge[0], edge[1], current) < -1e-12]
            if not outside:
                chosen = start
                break
            neighbours = edge_to_triangles[tuple(sorted(outside[0]))] - {start}
            if not neighbours:
                break
            start = min(neighbours)
        if chosen is None:
            chosen = next((key for key, (a, b, c, *_) in triangles.items()
                           if min(orient(a, b, current), orient(b, c, current),
                                  orient(c, a, current)) >= -1e-12), None)
        if chosen is None:
            raise ValueError('could not locate a point in Delaunay supertriangle')
        bad = set()
        pending = [chosen]
        while pending:
            key = pending.pop()
            if key in bad:
                continue
            a, b, c, ux, uy, radius2 = triangles[key]
            if (x - ux) ** 2 + (y - uy) ** 2 > radius2 * (1 + 1e-10):
                continue
            bad.add(key)
            for edge in triangle_edges(a, b, c):
                pending.extend(edge_to_triangles[edge] - bad)
        if not bad:
            bad.add(chosen)
        boundary = Counter(edge for key in bad
                           for edge in triangle_edges(*triangles[key][:3]))
        for key in sorted(bad):
            a, b, c, *_ = triangles.pop(key)
            for vertex in (a, b, c):
                incident[vertex].remove(key)
            for edge in triangle_edges(a, b, c):
                edge_to_triangles[edge].remove(key)
                if not edge_to_triangles[edge]:
                    del edge_to_triangles[edge]
        for a, b in sorted(edge for edge, count in boundary.items() if count == 1):
            add(a, b, current)
        previous = current
    return {tuple(sorted((order[a - 3], order[b - 3])))
            for a, b, c, *_ in triangles.values()
            for a, b in triangle_edges(a, b, c) if a >= 3 and b >= 3}


def grid_multiple(groups, points, edges):
    near = set()
    eligible = []
    for members in groups.values():
        if len(members) < 4:
            continue
        near.update(delaunay_edges(members, points))
        eligible.append(members)
    if not near or not eligible:
        return None
    rng = random.Random(3)
    random_pairs = set()
    attempts = 0
    while len(random_pairs) < len(near) and attempts < len(near) * 30:
        group = rng.choice(eligible)
        random_pairs.add(tuple(sorted(rng.sample(group, 2))))
        attempts += 1
    if not random_pairs:
        return None
    near_share = sum(pair in edges for pair in near) / len(near)
    random_share = sum(pair in edges for pair in random_pairs) / len(random_pairs)
    return near_share / random_share if random_share else None


def score(document):
    files = document['F']
    nodes = document['N']
    parcels = document.get('P') or {}
    neighbourhood_ids = document.get('file_neighbourhoods') or []
    neighbourhoods = document.get('neighbourhoods') or {}
    centroids = document.get('footprint_centroids') or []
    points = [centroids[i] if i < len(centroids) else node[1:3]
              for i, node in enumerate(nodes)]
    districts = defaultdict(list)
    local = defaultdict(lambda: defaultdict(list))
    for i, node in enumerate(nodes):
        districts[node[0]].append(i)
        if i < len(neighbourhood_ids):
            local[node[0]][neighbourhood_ids[i]].append(i)
    edges = Counter(tuple(sorted((a, b))) for a, b in document['E'] if a != b)
    district_hit, district_rho, overlap = level(districts, points, edges)
    neighbourhood_hits, neighbourhood_rhos = [], []
    for groups in local.values():
        if len(groups) < 4:
            continue
        hit, rho, _ = level(groups, points, edges)
        if hit is not None:
            neighbourhood_hits.append(hit)
        if rho is not None:
            neighbourhood_rhos.append(rho)
    areas, code_lines = [], []
    missing = 0
    code = document.get('C')
    for i in range(len(files)):
        polygon = parcels.get(str(i), [])
        polygon_area = area(polygon)
        if not polygon_area:
            missing += 1
        else:
            areas.append(polygon_area)
            code_lines.append(code[i] if code and i < len(code) else nodes[i][3])
    connected = [components(entry['blob']) == 1 for entry in neighbourhoods.values()]
    compactness = []
    for entry in neighbourhoods.values():
        if not entry['blob']:
            continue
        ring = max(entry['blob'], key=area)
        p = perimeter(ring)
        if p:
            compactness.append(4 * math.pi * area(ring) / p ** 2)
    sizes = sorted(entry['size'] for entry in neighbourhoods.values())
    return {
        'district_strongest_nearest': district_hit,
        'district_rho': district_rho,
        'neighbourhood_strongest_nearest': median(neighbourhood_hits),
        'neighbourhood_rho': median(neighbourhood_rhos),
        'grid_multiple': grid_multiple(districts, points, edges),
        'overlap': overlap,
        'neighbourhood_connectivity': statistics.mean(connected) if connected else None,
        'area_code_lines_r': pearson(areas, code_lines),
        'files_without_footprint': missing,
        'compactness': median(compactness),
        'neighbourhood_count': len(neighbourhoods),
        'neighbourhood_size': {'min': sizes[0] if sizes else None,
                               'p10': percentile(sizes, 0.10) if sizes else None,
                               'median': median(sizes),
                               'p90': percentile(sizes, 0.90) if sizes else None,
                               'max': sizes[-1] if sizes else None,
                               'bins': {'1-2': sum(s < 3 for s in sizes),
                                        '3-10': sum(3 <= s <= 10 for s in sizes),
                                        '11-40': sum(11 <= s <= 40 for s in sizes),
                                        '41+': sum(s > 40 for s in sizes)}},
        'file_count': len(files),
        'code_lines_source': 'code_lines' if code else 'loc_fallback',
        'grid_multiple_method': 'delaunay_bowyer_watson',
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('map', type=Path)
    parser.add_argument('--gate', action='store_true')
    parser.add_argument('--out', type=Path)
    args = parser.parse_args()
    result = score(json.loads(args.map.read_text()))
    text = json.dumps(result, indent=2, sort_keys=True, allow_nan=False)
    print(text)
    if args.out:
        args.out.write_text(text + '\n')
    if args.gate and (result['files_without_footprint'] != 0 or
                      result['neighbourhood_connectivity'] is None or
                      result['neighbourhood_connectivity'] < 0.95):
        raise SystemExit('footprint hard target failed')


if __name__ == '__main__':
    main()
