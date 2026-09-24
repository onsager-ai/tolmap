#!/usr/bin/env python3
"""Summarise symbol cards and audit exported rings on a remote-build runner.

This is corpus-scale JSON work. Run it in GitHub Actions, not on the
development laptop with the broken cooling system.
"""

import argparse
import collections
import gzip
import json
import math
import statistics
from pathlib import Path


def measure(map_file: Path, symbols_file: Path, compare_file: Path | None = None) -> dict:
    map_doc = json.loads(map_file.read_text())
    document = json.loads(symbols_file.read_text())
    decoded = decode_geometry(document)
    symbols = document["symbols"]
    edges = document["edges"]
    by_kind = dict(sorted(collections.Counter(row[2] for row in symbols).items()))
    largest = 0
    largest_id = None
    largest_gzip = 0
    static_districts = 0
    district_dir = symbols_file.with_suffix("")
    for district in map_doc["districts"]:
        files = {i for i, node in enumerate(map_doc["N"]) if str(node[0]) == district}
        local_ids = {i for i, row in enumerate(symbols) if row[0] in files}
        touching = [edge for edge in edges if edge[0] in local_ids or edge[1] in local_ids]
        all_ids = sorted(local_ids | {end for edge in touching for end in edge[:2]})
        slice_doc = {
            "district": int(district),
            "files": sorted(files),
            "symbol_indices": all_ids,
            "symbols": [symbols[i] for i in all_ids],
            "edges": touching,
            "kinds": document.get("kinds", ["unknown"]),
            "module_code_lines": {key: value for key, value in document["module_code_lines"].items() if int(key) in files},
        }
        if "symbol_rings" in document:
            slice_doc["symbol_rings"] = [document["symbol_rings"][i] for i in all_ids]
            slice_doc["module_rings"] = {key: value for key, value in document["module_rings"].items() if int(key) in files}
            slice_doc["header_rings"] = {key: value for key, value in document["header_rings"].items() if int(key) in all_ids}
        size = len(json.dumps(slice_doc, separators=(",", ":"), ensure_ascii=False).encode())
        district_file = district_dir / f"{district}.json"
        if district_file.exists():
            raw = district_file.read_bytes()
            assert json.loads(raw) == slice_doc, f"static district {district} differs from API projection"
            size = len(raw)
            gzip_size = len(gzip.compress(raw, mtime=0))
            static_districts += 1
        else:
            gzip_size = 0
        if size > largest:
            largest = size
            largest_id = int(district)
            largest_gzip = gzip_size
    coverage = document["coverage"]
    assert static_districts == len(map_doc["districts"]), (
        static_districts, len(map_doc["districts"])
    )
    total = coverage["calls_total"]
    geometry = decoded.get("symbol_rings", [])
    with_ring = sum(ring is not None for ring in geometry)
    parcels = map_doc.get("P") or {}
    eligible = [i for i, row in enumerate(symbols) if row[6] >= 1 and str(row[0]) in parcels]
    missing = sum(i >= len(geometry) or geometry[i] is None for i in eligible)
    by_file = collections.defaultdict(list)
    for i, row in enumerate(symbols):
        if row[5] == -1 and i < len(geometry) and geometry[i]:
            by_file[row[0]].append((area(geometry[i]), row[6]))
    correlations = [pearson(pairs) for pairs in by_file.values() if len(pairs) >= 3]
    correlations = [r for r in correlations if r is not None]
    before = {key: value for key, value in document.items() if key not in ("symbol_rings", "module_rings", "header_rings")}
    result = {
        "geometry_encoding": "delta_1e11",
        "symbols": len(symbols),
        "by_kind": by_kind,
        "edges": len(edges),
        "edge_counts_by_kind": dict(sorted(collections.Counter(document.get("kinds", ["unknown"])[edge[3] if len(edge) > 3 else 0] for edge in edges).items())),
        "abstract_symbols": sum(bool(row[7]) for row in symbols if len(row) > 7),
        "inherited_calls_resolved": coverage.get("inherited_calls_resolved", 0),
        "possible_implementations": coverage.get("possible_implementations", 0),
        "calls_total": total,
        "calls_resolved": coverage["calls_resolved"],
        "resolution_rate": coverage["calls_resolved"] / total if total else 0,
        "unresolved": coverage["unresolved"],
        "document_bytes": symbols_file.stat().st_size,
        "document_gzip_bytes": len(gzip.compress(symbols_file.read_bytes(), mtime=0)),
        "largest_district": largest_id,
        "largest_district_bytes": largest,
        "largest_district_gzip_bytes": largest_gzip,
        "static_districts": static_districts,
        "symbols_with_ring": with_ring,
        "eligible_symbols_without_ring": missing,
        "eligible_symbols": len(eligible),
        "files_with_pearson": len(correlations),
        "median_within_file_pearson": statistics.median(correlations) if correlations else None,
        "document_bytes_before_geometry": compact_bytes(before),
    }
    result.update(contour_metrics(decoded))
    result.update(audit_rings(decoded))
    result.update(sample_sibling_overlaps(decoded))
    if compare_file:
        raw = compare_file.read_bytes()
        result["before_precision_bytes"] = len(raw)
        result["before_precision_gzip_bytes"] = len(gzip.compress(raw, mtime=0))
        # compare_ref can now be another packed-card revision, not only the
        # pre-precision float revision this metric first compared against.
        old = decode_geometry(json.loads(raw))
        result["main_contours"] = contour_metrics(old)
        result["before_precision_collapsed_by_decimals"] = {
            str(places): collapsed_at_precision(old, places)
            for places in range(6, 12)
        }
    return result


def contour_metrics(document):
    cards = [card for card in document.get("symbol_rings", []) if card]
    vertices = [sum(len(ring) for ring in card) for card in cards]
    axis = total = 0
    for card in cards:
        for ring in card:
            for a, b in zip(ring, ring[1:] + ring[:1]):
                total += 1
                axis += a[0] == b[0] or a[1] == b[1]
    return {
        "median_vertices_per_card": statistics.median(vertices) if vertices else None,
        "mean_vertices_per_card": sum(vertices) / len(vertices) if vertices else None,
        "axis_aligned_edge_share": axis / total if total else None,
        "axis_aligned_edges": axis,
        "contour_edges": total,
    }


def sample_sibling_overlaps(document):
    """5×5 diagnostic per intersecting sibling box; counts are lower bounds."""
    symbols = document["symbols"]
    groups = collections.defaultdict(list)
    for i, row in enumerate(symbols):
        card = document["symbol_rings"][i]
        if not card:
            continue
        parent = row[5]
        if parent >= 0 and symbols[parent][0] != row[0]:
            parent = -1
        groups[(row[0], parent)].append(card)
    for file, card in document.get("module_rings", {}).items():
        groups[(int(file), -1)].append(card)
    for parent, card in document.get("header_rings", {}).items():
        row = symbols[int(parent)]
        groups[(row[0], int(parent))].append(card)

    overlaps = 0
    max_estimated_area = 0.0
    for cards in groups.values():
        boxes = []
        for card in cards:
            points = [point for ring in card for point in ring]
            boxes.append((min(p[0] for p in points), max(p[0] for p in points),
                          min(p[1] for p in points), max(p[1] for p in points), card))
        boxes.sort(key=lambda box: box[0])
        active = []
        for box in boxes:
            active = [other for other in active if other[1] > box[0]]
            for other in active:
                x0, x1 = box[0], min(box[1], other[1])
                y0, y1 = max(box[2], other[2]), min(box[3], other[3])
                if x1 <= x0 or y1 <= y0:
                    continue
                hits = sum(
                    contains([x0 + (ix + 0.5) * (x1 - x0) / 5,
                              y0 + (iy + 0.5) * (y1 - y0) / 5], box[4])
                    and contains([x0 + (ix + 0.5) * (x1 - x0) / 5,
                                  y0 + (iy + 0.5) * (y1 - y0) / 5], other[4])
                    for ix in range(5) for iy in range(5)
                )
                if hits:
                    overlaps += 1
                    max_estimated_area = max(max_estimated_area,
                                             (x1 - x0) * (y1 - y0) * hits / 25)
            active.append(box)
    return {"sampled_sibling_overlap_pairs_5x5": overlaps,
            "max_sampled_sibling_overlap_area": max_estimated_area}


def decode_ring(stream):
    if stream and isinstance(stream[0], list):
        return stream  # older float-coordinate sibling
    assert len(stream) >= 6 and len(stream) % 2 == 0
    x = y = 0
    ring = []
    for dx, dy in zip(stream[::2], stream[1::2]):
        x += dx
        y += dy
        ring.append([x / 10**11, y / 10**11])
    return ring


def decode_geometry(document):
    decoded = dict(document)
    if "symbol_rings" not in document:
        return decoded
    decoded["symbol_rings"] = [
        [decode_ring(ring) for ring in card] if card else None
        for card in document["symbol_rings"]
    ]
    for field in ("module_rings", "header_rings"):
        decoded[field] = {
            key: [decode_ring(ring) for ring in card]
            for key, card in document[field].items()
        }
    return decoded


def iter_contours(document):
    for card in document.get("symbol_rings", []):
        if card:
            yield from card
    for field in ("module_rings", "header_rings"):
        for card in document.get(field, {}).values():
            yield from card


def integer_area(ring, scale=10**11):
    points = [(round(x * scale), round(y * scale)) for x, y in ring]
    return abs(signed_integer_area(points))


def signed_integer_area(points):
    return sum(a[0] * b[1] - a[1] * b[0]
               for a, b in zip(points, points[1:] + points[:1]))


def card_area(card):
    return abs(sum(signed_integer_area([(round(x * 10**11), round(y * 10**11)) for x, y in ring])
                   for ring in card))


def collapsed_at_precision(document, places):
    scale = 10**places
    count = 0
    for card in document.get("symbol_rings", []):
        if card and integer_area(card[0], scale) == 0:
            count += 1
    for field in ("module_rings", "header_rings"):
        for card in document.get(field, {}).values():
            if integer_area(card[0], scale) == 0:
                count += 1
    return count


def contains(point, rings):
    x, y = point
    inside = False
    for ring in rings:
        for (ax, ay), (bx, by) in zip(ring, ring[1:] + ring[:1]):
            if (ay > y) != (by > y) and x < (bx - ax) * (y - ay) / (by - ay) + ax:
                inside = not inside
    return inside


def audit_rings(document):
    rings = document.get("symbol_rings", [])
    duplicate = collinear = collapsed = outside = oversized = 0
    outside_examples = []
    for ring in iter_contours(document):
        duplicate += any(a == b for a, b in zip(ring, ring[1:] + ring[:1]))
        collapsed += len(ring) < 3 or integer_area(ring) == 0
        points = [(round(x * 10**11), round(y * 10**11)) for x, y in ring]
        for a, b, c in zip(points[-1:] + points[:-1], points, points[1:] + points[:1]):
            ab = (b[0] - a[0], b[1] - a[1])
            bc = (c[0] - b[0], c[1] - b[1])
            collinear += ab[0] * bc[1] == ab[1] * bc[0] and ab[0] * bc[0] + ab[1] * bc[1] > 0
    for i, row in enumerate(document["symbols"]):
        parent = row[5]
        if parent < 0 or row[0] != document["symbols"][parent][0]:
            continue
        child = rings[i]
        ancestor = rings[parent]
        if not child or not ancestor:
            continue
        exterior = child[0]
        center = [sum(p[axis] for p in exterior) / len(exterior) for axis in (0, 1)]
        if not contains(center, ancestor):
            outside += 1
            if len(outside_examples) < 2:
                outside_examples.append({
                    "symbol": i, "parent": parent, "center": center,
                    "child": child, "ancestor": ancestor,
                    "child_area": card_area(child), "parent_area": card_area(ancestor),
                })
        oversized += card_area(child) > card_area(ancestor)
    assert duplicate == 0 and collinear == 0 and collapsed == 0 and outside == 0 and oversized == 0, (
        duplicate, collinear, collapsed, outside, oversized, outside_examples)
    return {
        "duplicate_consecutive_points": duplicate,
        "collinear_points": collinear,
        "collapsed_rings": collapsed,
        "child_centroid_outside_parent": outside,
        "child_area_larger_than_parent": oversized,
    }


def compact_bytes(value):
    return len(json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode())


def area(rings):
    return abs(sum(
        sum(a[0] * b[1] - a[1] * b[0] for a, b in zip(ring, ring[1:] + ring[:1]))
        for ring in rings
    )) / 2


def pearson(pairs):
    xs, ys = zip(*pairs)
    xm, ym = statistics.mean(xs), statistics.mean(ys)
    numerator = sum((x - xm) * (y - ym) for x, y in pairs)
    denominator = math.sqrt(sum((x - xm) ** 2 for x in xs) * sum((y - ym) ** 2 for y in ys))
    return numerator / denominator if denominator else None


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("map", type=Path)
    parser.add_argument("symbols", type=Path)
    parser.add_argument("--compare-symbols", type=Path)
    args = parser.parse_args()
    print(json.dumps(measure(args.map, args.symbols, args.compare_symbols), indent=2, sort_keys=True))
