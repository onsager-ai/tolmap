#!/usr/bin/env python3
"""Resolve a `repos` selector against eval/corpus.toml into a GitHub Actions
matrix, for .github/workflows/remote-build.yml's `setup` job.

`repos` is one of:
  - "all" (case-insensitive): every [[repo]] entry in the manifest.
  - a band name ("small", "medium", "large", "ultra", case-insensitive):
    every entry with that band.
  - "fixtures": the nine exact pins from data/fixtures.toml.
  - "stability": scrapy, django and celery 300 commits back, plus httpx 200.
  - a comma-separated list of slugs ("owner/repo,owner/repo"): exactly
    those entries, in manifest order (duplicates collapsed). This is the
    form issue #59's owed comparison uses (msgraph-sdk-go, aws-sdk-go-v2,
    prometheus/prometheus).

Prints a JSON object `{"include": [...]}` to stdout, one dict per matched
repo, with a GitHub Actions matrix `include` list directly usable as
`strategy.matrix: ${{ fromJSON(...) }}`. Each entry carries the fields the
build job needs: slug, stem (slug with "/" replaced by "__", matching
build_corpus.py's clone/map naming), commit, band, and args (the
manifest's own args list -- extra_args from the workflow input is appended
at build time, not here, since it applies uniformly to every job).

Exits 1 with a message on stderr, and prints nothing to stdout, when the
selector matches zero entries -- a matrix job strategy with an empty
`include` list is a GitHub Actions error, and a silent empty matrix (the
workflow reporting "0 jobs ran, all green") would be a worse failure mode
than a loud one here.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "eval" / "corpus.toml"
FIXTURE_MANIFEST = ROOT / "data" / "fixtures.toml"
BANDS = ("small", "medium", "large", "ultra")
STABILITY_BACK = {
    "scrapy/scrapy": 300,
    "django/django": 300,
    "celery/celery": 300,
    "encode/httpx": 200,
}


def load_manifest(path: Path) -> list[dict]:
    with path.open("rb") as handle:
        raw = tomllib.load(handle)
    entries = raw.get("repo", [])
    if not isinstance(entries, list):
        raise SystemExit(f"{path}: expected [[repo]] entries")
    return entries


def resolve(entries: list[dict], selector: str) -> list[dict]:
    selector = selector.strip()
    lowered = selector.lower()
    if lowered in ("", "all"):
        return list(entries)
    if lowered in BANDS:
        return [entry for entry in entries if entry["band"] == lowered]
    by_slug = {entry["slug"]: entry for entry in entries}
    slugs = [part.strip() for part in selector.split(",") if part.strip()]
    if not slugs:
        raise SystemExit(f"repos selector {selector!r} named no slugs, band, or 'all'")
    unknown = [slug for slug in slugs if slug not in by_slug]
    if unknown:
        raise SystemExit(
            f"repos selector named slug(s) not in {DEFAULT_MANIFEST.name}: {', '.join(unknown)}"
        )
    seen: set[str] = set()
    matched = []
    for slug in slugs:
        if slug in seen:
            continue
        seen.add(slug)
        matched.append(by_slug[slug])
    return matched


def load_fixtures(path: Path) -> list[dict]:
    """Translate data/fixtures.toml into the remote-build matrix shape.

    Several corpus entries use newer commits than the acceptance fixtures.
    A dedicated selector keeps the acceptance measurement pinned to the
    actual oracle without changing eval/corpus.toml's independent corpus.
    """
    with path.open("rb") as handle:
        raw = tomllib.load(handle)
    entries = []
    for name, fixture in raw.items():
        url = fixture["url"]
        slug = url.removeprefix("https://github.com/").removesuffix(".git")
        entries.append(
            {
                "slug": slug,
                "stem": name,
                "commit": fixture["commit"],
                "band": "fixture",
                "args": ["--pkg", fixture["pkg"], "--lang", fixture["lang"]],
            }
        )
    return entries


def to_matrix(entries: list[dict]) -> dict:
    include = []
    for entry in entries:
        include.append(
            {
                "slug": entry["slug"],
                "stem": entry.get("stem", entry["slug"].replace("/", "__")),
                "commit": entry["commit"],
                "band": entry["band"],
                "args": list(entry.get("args", ["--all-sources"])),
                **({"back": entry["back"]} if "back" in entry else {}),
            }
        )
    return {"include": include}


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--repos",
        required=True,
        help="comma-separated slugs, a band name, or 'all'",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.repos.strip().lower() == "fixtures":
        matched = load_fixtures(FIXTURE_MANIFEST)
        print(json.dumps(to_matrix(matched), sort_keys=True))
        print(
            f"matched {len(matched)} acceptance fixtures from {FIXTURE_MANIFEST}",
            file=sys.stderr,
        )
        return 0
    entries = load_manifest(args.manifest)
    if args.repos.strip().lower() == "stability":
        by_slug = {entry["slug"]: entry for entry in entries}
        matched = []
        for slug, back in STABILITY_BACK.items():
            entry = dict(by_slug[slug])
            entry["back"] = back
            matched.append(entry)
        print(json.dumps(to_matrix(matched), sort_keys=True))
        print(
            f"matched {len(matched)} warm-start repositories from {args.manifest}",
            file=sys.stderr,
        )
        return 0
    matched = resolve(entries, args.repos)
    if not matched:
        print(
            f"repos selector {args.repos!r} matched zero entries in {args.manifest}",
            file=sys.stderr,
        )
        return 1
    print(json.dumps(to_matrix(matched), sort_keys=True))
    print(
        f"matched {len(matched)} of {len(entries)} manifest entries for selector {args.repos!r}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
