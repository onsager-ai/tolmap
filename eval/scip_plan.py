#!/usr/bin/env python3
"""Matrices for issue #110's P0 SCIP spike (.github/workflows/scip-spike.yml).

Five repositories at their `eval/corpus.toml` pins, and one index job per
(repository, language, variant). Commits are read from the manifest rather
than copied here, so the spike measures exactly the checkout the corpus runs
and `docs/FINDINGS.md` tables were built from.

Variants:
  default  -- the indexer runs on the checkout as is. Nothing from the
              repository's own dependency graph is installed (no npm/pnpm
              install, no pip/uv install, no module download: Go runs with
              GOPROXY=off and an empty module cache). This is the only mode
              the hosted worker could run without a sandbox (issue #110,
              risk 1), so it is the one the re-partition is built from.
  install  -- the repository's own lockfile install first, on this throwaway
              runner only, to measure what skipping installs costs.

`root` is where the indexer runs, relative to the checkout; the ingest step
prefixes every SCIP document path with it so paths line up with the map's
repository-relative `F`. dify's Python is run both at the repository root
(what `--all-sources` maps) and at `api/` (the nested project root finding 25
found; Pyright resolves `core.x` only from there).

`runs: 2` repeats the index on the same runner so byte stability can be
checked (issue #110, risk 5): one repository per indexer.

Printed as JSON: {"baseline": {"include": [...]}, "index": {"include": [...]}}.
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "eval" / "corpus.toml"

REPOS = (
    "django/django",
    "langgenius/dify",
    "prometheus/prometheus",
    "vuejs/core",
    "n8n-io/n8n",
)

# (slug, id, lang, variant, root, runs)
INDEXES = (
    ("django/django", "py-default", "py", "default", ".", 2),
    ("langgenius/dify", "py-default", "py", "default", ".", 1),
    ("langgenius/dify", "py-api-default", "py", "default", "api", 1),
    ("langgenius/dify", "py-api-install", "py", "install", "api", 1),
    ("langgenius/dify", "ts-default", "ts", "default", ".", 1),
    ("langgenius/dify", "ts-install", "ts", "install", ".", 1),
    ("prometheus/prometheus", "go-default", "go", "default", ".", 2),
    ("prometheus/prometheus", "go-install", "go", "install", ".", 1),
    ("prometheus/prometheus", "ts-default", "ts", "default", "web/ui", 1),
    ("vuejs/core", "ts-default", "ts", "default", ".", 2),
    ("n8n-io/n8n", "ts-default", "ts", "default", ".", 1),
)


def stem(slug: str) -> str:
    return slug.replace("/", "__")


def main() -> int:
    with MANIFEST.open("rb") as handle:
        entries = {entry["slug"]: entry for entry in tomllib.load(handle)["repo"]}
    missing = [slug for slug in REPOS if slug not in entries]
    if missing:
        print(f"not in {MANIFEST}: {missing}", file=sys.stderr)
        return 1
    baseline = [
        {
            "slug": slug,
            "stem": stem(slug),
            "commit": entries[slug]["commit"],
            "args": entries[slug].get("args", ["--all-sources"]),
        }
        for slug in REPOS
    ]
    index = [
        {
            "slug": slug,
            "stem": stem(slug),
            "commit": entries[slug]["commit"],
            "id": index_id,
            "lang": lang,
            "variant": variant,
            "root": root,
            "runs": runs,
        }
        for slug, index_id, lang, variant, root, runs in INDEXES
    ]
    print(json.dumps({"baseline": {"include": baseline}, "index": {"include": index}}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
