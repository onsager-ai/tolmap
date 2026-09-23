#!/usr/bin/env python3
"""Ten displayed-footprint metrics for a product map. Only two are gates.

Usage: python3 eval/footprint_metrics.py map.json [--gate] [--out result.json]
No repository checkout, third-party package, or source text is required.
"""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import json
import heapq
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
        radii[key] = distances[min(len(distances) - 1, int(0.92 * (len(distances) - 1)))]
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


def grid_multiple(groups, points, edges):
    """Six-nearest proxy for the prototype's Delaunay edge enrichment.

    SciPy is unavailable by contract. The proxy is kept out of the gate and
    named in the result so it is never mistaken for the prototype's exact
    Delaunay statistic. Its sampling and tie breaks are deterministic.
    """
    near = set()
    by_distance = {}
    for members in groups.values():
        if len(members) < 4:
            continue
        def tree(indices, axis=0):
            if not indices:
                return None
            indices.sort(key=lambda i: (points[i][axis], points[i][1 - axis], i))
            mid = len(indices) // 2
            return indices[mid], axis, tree(indices[:mid], 1 - axis), tree(indices[mid + 1:], 1 - axis)

        root = tree(list(members))

        def nearest(node, query, heap):
            if node is None:
                return
            index, axis, left, right = node
            delta = points[query][axis] - points[index][axis]
            first, second = (left, right) if delta <= 0 else (right, left)
            nearest(first, query, heap)
            if index != query:
                d2 = sum((points[query][a] - points[index][a]) ** 2 for a in (0, 1))
                candidate = (-d2, -index)
                if len(heap) < 6:
                    heapq.heappush(heap, candidate)
                elif candidate > heap[0]:
                    heapq.heapreplace(heap, candidate)
            if len(heap) < 6 or delta * delta <= -heap[0][0]:
                nearest(second, query, heap)

        for i in members:
            heap = []
            nearest(root, i, heap)
            for _, neg_j in heap:
                j = -neg_j
                near.add(tuple(sorted((i, j))))
        by_distance[tuple(members)] = members
    if not near or not by_distance:
        return None
    rng = random.Random(3)
    groups_list = list(by_distance.values())
    random_pairs = set()
    attempts = 0
    while len(random_pairs) < len(near) and attempts < len(near) * 30:
        group = rng.choice(groups_list)
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
                               'median': median(sizes), 'max': sizes[-1] if sizes else None},
        'file_count': len(files),
        'code_lines_source': 'code_lines' if code else 'loc_fallback',
        'grid_multiple_method': 'six_nearest_proxy',
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
