#!/usr/bin/env python3
"""Render eval/corpus.toml from a completed eval/build_corpus.py run.

Reads $TOLMAP_CORPUS_DIR/builds.json (default ~/.cache/tolmap-corpus) and
writes one [[repo]] entry per repository, ordered deterministically by band
then slug, so the manifest is reproducible from the same builds.json without
rebuilding anything.

Band correction: build_corpus.py's --candidates screening assigns a band
before a repo is built, from an estimate of repository size, not from what
--all-sources actually indexes.  Issue #50 defines a band by *measured*
mapped source files (small < 300, medium 300-2k, large 2k-8k, ultra >= 8k).
For a built repo those disagreed on 81 of 128 repositories in this corpus
run (see docs/FINDINGS.md finding 17) -- almost two-thirds -- so this script
recomputes band from the measured `files` count for every built repo.  A
failed repo has no measured file count, so its pre-build screening band is
kept as the best available estimate.

Non-default build arguments (anything other than exactly ["--all-sources"])
carry a short, human-checked reason, inferred from inspecting each cached
clone's top-level layout (never from rebuilding): a src/ layout, a
root-level package directory, a dual-package repo, or a monorepo where
auto-detection would be ambiguous.  This is not recorded anywhere in
builds.json, so it lives in ARG_REASONS below rather than being invented
per-run.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CORPUS_DIR = Path(
    os.environ.get("TOLMAP_CORPUS_DIR", "~/.cache/tolmap-corpus")
).expanduser()
DEFAULT_OUT = ROOT / "eval" / "corpus.toml"
BAND_ORDER = ("small", "medium", "large", "ultra")

# Checked by listing each cached clone's top-level directory (read-only, no
# build): src/-layout Python packages, a root-level package dir sharing the
# repo's own name, a dual-package repo, Go libraries flat at repo root, and
# one large monorepo where --all-sources' single-source auto-detect is
# ambiguous across many sibling package trees.
ARG_REASONS = {
    "encode/httpx": "package lives at repo root as httpx/, alongside docs/ and tests/; pinned so auto-detect can't waver",
    "immerjs/immer": "TypeScript source under src/; --all-sources would also see docs/, website/ and test dirs at root",
    "odoo/odoo": "monorepo: hundreds of sibling addon packages under addons/ plus the core odoo/ package; --all-sources' single-source auto-detect is ambiguous, so the whole repo is pinned as one Python source",
    "pallets/click": "src/ layout (src/click); repo root also has docs/, examples/, tests/",
    "pallets/flask": "src/ layout (src/flask); repo root also has docs/, examples/, tests/",
    "pallets/itsdangerous": "src/ layout (src/itsdangerous); repo root also has docs/, tests/",
    "pmndrs/zustand": "TypeScript source under src/; repo root also has docs/, examples/, tests/",
    "psf/requests": "src/ layout (src/requests); repo root also has docs/, tests/, ext/",
    "pytest-dev/pytest": "source is src/_pytest (src/pytest is a thin re-export shim); repo root also has doc/, testing/, bench/",
    "python-attrs/attrs": "two sibling packages under src/ (src/attr, the original, and src/attrs, its re-export); needs two --pkg flags to cover both",
    "spf13/cobra": "flat single Go package at repo root, no nested module ambiguity",
    "spf13/viper": "flat single Go package at repo root, no nested module ambiguity",
    "tiangolo/sqlmodel": "package lives at repo root as sqlmodel/, alongside docs/, docs_src/, tests/",
}


def band_for_files(files: int) -> str:
    if files < 300:
        return "small"
    if files < 2000:
        return "medium"
    if files < 8000:
        return "large"
    return "ultra"


def toml_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    escaped = escaped.replace("\n", "\\n").replace("\t", "\\t")
    return f'"{escaped}"'


def toml_array(values: list[str]) -> str:
    return "[" + ", ".join(toml_string(v) for v in values) + "]"


def load_builds(path: Path) -> dict[str, dict]:
    with path.open() as handle:
        raw = json.load(handle)
    return raw.get("repos", raw)


def render_entry(slug: str, row: dict) -> str:
    args = list(row.get("args", ["--all-sources"]))
    status = row["status"]
    built = status == "built"
    band = band_for_files(row["files"]) if built else row["band"]

    lines = ["[[repo]]"]
    lines.append(f"slug = {toml_string(slug)}")
    lines.append(f"commit = {toml_string(row['commit'])}")
    lines.append(f"band = {toml_string(band)}")
    lines.append(f"status = {toml_string(status)}")
    lines.append(f"args = {toml_array(args)}")
    if args != ["--all-sources"]:
        reason = ARG_REASONS.get(slug)
        if reason is None:
            raise SystemExit(
                f"{slug}: non-default args {args!r} have no recorded reason in "
                "ARG_REASONS; inspect the cached clone and add one"
            )
        lines.append(f"reason = {toml_string(reason)}")
    if built:
        lines.append(f"files = {row['files']}")
    else:
        lines.append(f"failure_reason = {toml_string(row.get('reason', ''))}")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus-dir", type=Path, default=DEFAULT_CORPUS_DIR)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args()

    builds_path = args.corpus_dir.expanduser() / "builds.json"
    repos = load_builds(builds_path)

    def sort_key(item: tuple[str, dict]) -> tuple[int, str]:
        slug, row = item
        band = band_for_files(row["files"]) if row["status"] == "built" else row["band"]
        return (BAND_ORDER.index(band), slug)

    ordered = sorted(repos.items(), key=sort_key)

    header = (
        "# Generated by eval/manifest_from_builds.py from "
        f"{builds_path}. Do not hand-edit; regenerate instead.\n"
        "# See issue #50 and docs/FINDINGS.md finding 17.\n"
    )
    body = "\n\n".join(render_entry(slug, row) for slug, row in ordered)
    args.out.write_text(header + "\n" + body + "\n")

    built = sum(1 for _, row in ordered if row["status"] == "built")
    failed = len(ordered) - built
    print(f"wrote {len(ordered)} entries ({built} built, {failed} failed) to {args.out}")


if __name__ == "__main__":
    main()
