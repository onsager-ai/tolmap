#!/usr/bin/env python3
"""Regenerate corpus CSV rows and the per-band markdown summary.

Reads eval/corpus.toml, $TOLMAP_CORPUS_DIR/maps, and builds.json written by
eval/build_corpus.py.  The script is stdlib-only and writes deterministic
rows in manifest order.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import statistics
import tomllib
from pathlib import Path

import mapstats


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "eval" / "corpus.toml"
DEFAULT_CORPUS_DIR = Path(
    os.environ.get("TOLMAP_CORPUS_DIR", "~/.cache/tolmap-corpus")
).expanduser()
DEFAULT_CSV = ROOT / "eval" / "corpus_stats.csv"
DEFAULT_MARKDOWN = ROOT / "eval" / "corpus_summary.md"
VIEWPORTS = ((390, 700), (1440, 900))
BANDS = ("small", "medium", "large", "ultra")
# web/src/map/constants.ts's DOT_DENSITY_FLOOR (#48).
DOT_DENSITY_FLOOR = 30.0

FIELDS = (
    "slug",
    "commit",
    "band",
    "status",
    "files",
    "districts",
    "mainland",
    "island",
    "unconnected",
    "landmarks",
    "q",
    "kept_edges",
    "zero_edge_files",
    "px2_file_390x700_min",
    "px2_file_390x700_median",
    "px2_file_1440x900_min",
    "px2_file_1440x900_median",
    "share_under_floor_390x700",
    "share_under_floor_1440x900",
    "zero_blob_included_districts",
    "files_in_zero_blob_districts",
    "build_seconds",
    "peak_rss_mb",
    "failure_reason",
)

SUMMARY_FIELDS = (
    "files",
    "districts",
    "mainland",
    "island",
    "unconnected",
    "landmarks",
    "q",
    "kept_edges",
    "zero_edge_files",
    "px2_file_390x700_min",
    "px2_file_390x700_median",
    "px2_file_1440x900_min",
    "px2_file_1440x900_median",
    "share_under_floor_390x700",
    "share_under_floor_1440x900",
    "build_seconds",
    "peak_rss_mb",
)


def polygon_area(points: list[list[float]]) -> float:
    if len(points) < 3:
        return 0.0
    twice = sum(
        x1 * y2 - x2 * y1
        for (x1, y1), (x2, y2) in zip(points, points[1:] + points[:1])
    )
    return abs(twice) / 2.0


def world_fit_scale(doc: dict, width: int, height: int) -> float:
    """Port of web/src/map/geometry.ts's fitScale(doc, "r", vw, vh).

    PR #49's no-change-proof.ts calls this exact function (unchanged by
    #49) to get the scale at which #48's density floor is checked, and it
    is the scale MapRenderer actually renders at when a map first opens.
    worldBounds in "r" mode unions two things: every file's node position
    (N[i][1], N[i][2]) *and* every district's blob polygon points -- not
    just mainland districts. An earlier version of this function bounded
    only the mainland districts' blobs, which both shrinks the box (nodes
    outside any mainland blob, and island/unconnected blobs, are dropped)
    and so inflates the resulting px²/file figures relative to what the
    viewer actually draws. Fixed for issue #50; see docs/FINDINGS.md
    finding 18.
    """
    xs: list[float] = []
    ys: list[float] = []
    for row in doc["N"]:
        xs.append(row[1])
        ys.append(row[2])
    for district in doc["districts"].values():
        for polygon in district["blob"]:
            for point in polygon:
                xs.append(point[0])
                ys.append(point[1])
    if not xs:
        return math.nan
    pad = 46
    return min(
        (width - 2 * pad) / ((max(xs) - min(xs)) or 1),
        (height - 2 * pad) / ((max(ys) - min(ys)) or 1),
    )


def district_densities(doc: dict, classes: dict, viewport: tuple[int, int]) -> list[tuple[int, float]]:
    """Per-district (size, px²/file) at fit zoom, matching no-change-proof.ts's
    densityByDistrict: every district except "unconnected" ones is included,
    not just mainland -- an island district still has a drawn region and a
    file-dot budget.

    A district can measure exactly 0.0 px²/file here for a reason that has
    nothing to do with viewport size or zoom: `contours()` in src/blobs.rs
    only keeps a marching-squares contour with >=12 points that is also
    >=12% of that district's largest contour's point count, so a district
    with too few member files (empirically, almost always <=12 files; see
    docs/FINDINGS.md finding 18) never clears that filter and gets an empty
    `blob` -- no polygon at any zoom, not a small one. Both `zero_blob_*`
    columns below count these separately from the px2_file_*_min column,
    which a single such district anywhere in a large map can otherwise pin
    at 0.0 regardless of everything else in that map.
    """
    included = classes["mainland_ids"] | classes["island_ids"]
    scale = world_fit_scale(doc, *viewport)
    out = []
    for district_id in sorted(included):
        size = len(classes["members"][district_id])
        if size <= 0:
            continue
        area = sum(
            polygon_area(polygon)
            for polygon in doc["districts"][str(district_id)]["blob"]
        )
        out.append((size, area * scale * scale / size))
    return out


def px_per_file(densities: list[tuple[int, float]]) -> tuple[float, float]:
    if not densities:
        return math.nan, math.nan
    values = [px2 for _, px2 in densities]
    return min(values), statistics.median(values)


def share_under_floor(
    densities: list[tuple[int, float]], total_files: int, floor: float = DOT_DENSITY_FLOOR
) -> float:
    """Share of ALL mapped files (not just mainland/island ones) that sit in
    a district whose px²/file is below the floor at fit zoom -- the file-dot
    budget in web/src/map/MapRenderer.ts's dotFactor engages for these.
    Unconnected files are never thinned (PR #49: "there's no area to budget
    against"), so they never contribute to the numerator, but they do sit in
    the denominator; this makes the share a whole-map figure, not one scoped
    to files the budget could possibly apply to.
    """
    if total_files <= 0:
        return math.nan
    under = sum(size for size, px2 in densities if px2 < floor)
    return under / total_files


def load_builds(path: Path) -> dict[str, dict]:
    if not path.exists():
        return {}
    with path.open() as handle:
        raw = json.load(handle)
    return raw.get("repos", raw)


def map_row(entry: dict, map_path: Path, build: dict) -> dict:
    base = {field: "" for field in FIELDS}
    base.update(
        slug=entry["slug"],
        commit=entry["commit"],
        band=entry["band"],
        status=entry.get("status", build.get("status", "built")),
        failure_reason=entry.get("failure_reason", build.get("reason", "")),
        build_seconds=build.get("build_seconds", ""),
        peak_rss_mb=build.get("peak_rss_mb", ""),
    )
    if not map_path.is_file():
        if base["status"] == "built":
            base["status"] = "missing"
            base["failure_reason"] = f"map not found: {map_path}"
        base["files"] = entry.get("files", "")
        return base
    with map_path.open() as handle:
        doc = json.load(handle)
    classes = mapstats.classify(doc)
    measured_files = len(doc["F"])
    expected_files = entry.get("files")
    if expected_files is not None and measured_files != expected_files:
        raise ValueError(
            f"{entry['slug']}: manifest says {expected_files} files, map has {measured_files}"
        )
    mobile_densities = district_densities(doc, classes, VIEWPORTS[0])
    desktop_densities = district_densities(doc, classes, VIEWPORTS[1])
    mobile_min, mobile_median = px_per_file(mobile_densities)
    desktop_min, desktop_median = px_per_file(desktop_densities)
    # Zero-blob districts are a fixed geometric fact of the map (src/blobs.rs
    # never gives them a polygon at any zoom), so this is the same set for
    # both viewports -- computed once from either density list.
    included = classes["mainland_ids"] | classes["island_ids"]
    zero_blob_ids = {
        district_id
        for district_id in included
        if not doc["districts"][str(district_id)]["blob"]
        or sum(polygon_area(p) for p in doc["districts"][str(district_id)]["blob"]) == 0.0
    }
    base.update(
        status="built",
        files=measured_files,
        districts=len(classes["members"]),
        mainland=len(classes["mainland_ids"]),
        island=len(classes["island_ids"]),
        unconnected=len(classes["unconnected_ids"]),
        landmarks=len(doc["L"]),
        q=doc["q"],
        kept_edges=len(doc["E"]),
        zero_edge_files=measured_files - len(classes["incident_files"]),
        px2_file_390x700_min=mobile_min,
        px2_file_390x700_median=mobile_median,
        px2_file_1440x900_min=desktop_min,
        px2_file_1440x900_median=desktop_median,
        share_under_floor_390x700=share_under_floor(mobile_densities, measured_files),
        share_under_floor_1440x900=share_under_floor(desktop_densities, measured_files),
        zero_blob_included_districts=len(zero_blob_ids),
        files_in_zero_blob_districts=sum(len(classes["members"][d]) for d in zero_blob_ids),
    )
    return base


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return math.nan
    position = fraction * (len(ordered) - 1)
    low = math.floor(position)
    high = math.ceil(position)
    if low == high:
        return ordered[low]
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def format_number(value: float, field: str) -> str:
    if field in {"q"}:
        return f"{value:.3f}"
    if field.startswith("share_under_floor"):
        return f"{value:.1%}"
    if field.startswith("px2_") or field in {"build_seconds", "peak_rss_mb"}:
        return f"{value:.1f}"
    return f"{value:,.0f}"


def triple(rows: list[dict], field: str) -> str:
    values = [float(row[field]) for row in rows if row["status"] == "built" and row[field] != ""]
    if not values:
        return "—"
    median = percentile(values, 0.5)
    p10 = percentile(values, 0.1)
    p90 = percentile(values, 0.9)
    return f"{format_number(median, field)} [{format_number(p10, field)}–{format_number(p90, field)}]"


def write_markdown(path: Path, rows: list[dict]) -> None:
    labels = {
        "files": "files",
        "districts": "districts",
        "mainland": "mainland",
        "island": "islands",
        "unconnected": "unconnected",
        "landmarks": "landmarks",
        "q": "modularity q",
        "kept_edges": "kept edges",
        "zero_edge_files": "zero-edge files",
        "px2_file_390x700_min": "390×700 px²/file min",
        "px2_file_390x700_median": "390×700 px²/file district median",
        "px2_file_1440x900_min": "1440×900 px²/file min",
        "px2_file_1440x900_median": "1440×900 px²/file district median",
        "share_under_floor_390x700": "390×700 share of files under floor",
        "share_under_floor_1440x900": "1440×900 share of files under floor",
        "build_seconds": "build seconds",
        "peak_rss_mb": "peak RSS MB",
    }
    lines = [
        "# Evaluation corpus summary",
        "",
        "Each cell is the per-repository median [p10–p90]. Failed repositories are counted but excluded from metric distributions.",
        "",
        "The px²/file *min* columns are dominated by a geometric artifact, not zoom: a district with too few member files (empirically almost always <=12) never clears the >=12-point marching-squares contour filter in src/blobs.rs and gets no polygon at any zoom, which floors that repo's minimum at 0.0 regardless of everything else in the map (see docs/FINDINGS.md finding 18). The *share of files under floor* rows are the robust statistic: the fraction of all mapped files sitting in a mainland/island district whose px²/file is below the 30 px²/file floor (#48) at fit zoom, including these zero-blob districts (their file-dot budget is genuinely 0, which is what \"under floor\" means for them too).",
        "",
        "| metric | small | medium | large | ultra |",
        "|---|---:|---:|---:|---:|",
    ]
    grouped = {band: [row for row in rows if row["band"] == band] for band in BANDS}
    lines.append(
        "| repositories (built/failed) | "
        + " | ".join(
            f"{sum(row['status'] == 'built' for row in grouped[band])}/{sum(row['status'] != 'built' for row in grouped[band])}"
            for band in BANDS
        )
        + " |"
    )
    for field in SUMMARY_FIELDS:
        lines.append(
            f"| {labels[field]} | "
            + " | ".join(triple(grouped[band], field) for band in BANDS)
            + " |"
        )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--corpus-dir", type=Path, default=DEFAULT_CORPUS_DIR)
    parser.add_argument("--csv", type=Path, default=DEFAULT_CSV)
    parser.add_argument("--markdown", type=Path, default=DEFAULT_MARKDOWN)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    with args.manifest.open("rb") as handle:
        entries = tomllib.load(handle).get("repo", [])
    corpus = args.corpus_dir.expanduser()
    builds = load_builds(corpus / "builds.json")
    rows = []
    for entry in entries:
        stem = entry["slug"].replace("/", "__")
        rows.append(map_row(entry, corpus / "maps" / f"{stem}.json", builds.get(entry["slug"], {})))
    args.csv.parent.mkdir(parents=True, exist_ok=True)
    with args.csv.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=FIELDS)
        writer.writeheader()
        writer.writerows(rows)
    write_markdown(args.markdown, rows)
    print(f"wrote {len(rows)} rows to {args.csv} and {args.markdown}")


if __name__ == "__main__":
    main()
