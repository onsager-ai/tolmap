#!/usr/bin/env python3
"""Derive tolmap-shaped reference data from a `rust-analyzer scip` index
(issue #126, the P0-style Rust spike -- CI only, no product change).

Rust has no hand resolver yet (#126 is design-only so far), so unlike
`eval/scip_ingest.py`'s callers this script never builds a tolmap map or
symbols document. `ingest()` itself does not need one: `mapped` is just the
set of `.rs` files tracked by the checkout, `lang_of` maps all of them to
"rust", and `spans=None` skips the symbol-span crediting that needs
tolmap's own symbols document. What is left -- documents, occurrences,
definitions, file->file pairs -- is exactly what issue #126's "measure
first" step asked for, without writing a line of extraction or resolver
code.

Runs on a GitHub-hosted runner only (.github/workflows/rust-scip-spike.yml):
indexing a real repository is corpus-scale work the maintainer's laptop
cannot take (CLAUDE.md), and this script's own regex sampling below is a
few hundred small files, not the tree-sitter-scale parse CLAUDE.md warns
about.

Beyond the generic ingest, this script samples what a no-build index loses
on real code (#126's "measure what it loses"):

- **proc-macro-generated members.** A `#[derive(Serialize)]` on `struct S`
  never writes `fn serialize` into the source; the impl exists only after
  macro expansion. If the index has no symbol for `S`'s generated method,
  the macro was not expanded for that impl. This is a proxy, not a proof:
  a macro can expand into a free function or a trait impl with no method
  matching our marker list, which would show up here as a false "lost".
- **cfg-gated files.** A file gated by `#[cfg(windows)]`/`#[cfg(target_os
  = "...")"]` at module level compiles out entirely on a Linux runner's
  default target; if rust-analyzer never emits a Document for it, it is
  gone from the index by construction, not by a bug in this script.
- **build.rs.** Reports whether a crate has a build script and whether any
  tracked source reads its OUT_DIR output (`env!("OUT_DIR")` /
  `include!(concat!(env!("OUT_DIR"...`); the index either has occurrences
  in the generated file's synthetic path or it does not.

Both proc-macro and cfg samples are heuristic pattern matches over source
text, not a second parser. A no-match is reported as "not observed", never
asserted as "impossible", per CLAUDE.md's lower-bound rule.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path

from scip_ingest import ingest, iter_index
import scip_pb2

# Macros known to generate a member whose name is a reliable marker of
# expansion having happened. Not exhaustive -- #126's fixture repos decide
# which of these ever fire. A macro absent from this table is still listed
# in `derive_samples`, just without an expansion verdict.
MARKERS: dict[str, tuple[str, ...]] = {
    "Serialize": ("serialize(",),
    "Deserialize": ("deserialize(",),
    "TS": ("decl(", "inline(", "name(", "dependencies("),
    "Parser": ("augment_args(", "from_arg_matches(", "update_from_arg_matches("),
    "Subcommand": ("augment_subcommands(", "has_subcommand("),
    "ValueEnum": ("value_variants(", "to_possible_value("),
    "Args": ("augment_args(",),
    "EnumIter": ("iter(",),
    "FromPrimitive": ("from_i64(", "from_u64("),
    "IntoStaticStr": ("into(",),
    "AsRefStr": ("as_ref(",),
    "Zeroize": ("zeroize(",),
    "Arbitrary": ("arbitrary(",),
    "PinnedDrop": ("drop(",),
}
# Rustc's own built-in derives: expansion never needs a proc-macro process,
# so they are excluded from the "did it need a proc macro" sample even
# when they are the only derive present on a sampled type.
BUILTIN_DERIVES = {
    "Debug", "Clone", "Copy", "PartialEq", "Eq", "PartialOrd", "Ord",
    "Default", "Hash",
}

DERIVE_RE = re.compile(r"#\[derive\(([^)]*)\)\]")
ITEM_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum)\s+(\w+)")
CFG_RE = re.compile(
    r"#!?\[cfg\([^)]*\b(windows|unix|target_os\s*=\s*\"[^\"]+\"|target_family\s*=\s*\"[^\"]+\")"
)
OUT_DIR_RE = re.compile(r'env!\(\s*"OUT_DIR"\s*\)')


def tracked_rs_files(clone: Path) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(clone), "ls-files", "-z", "--", "*.rs"],
        check=True, capture_output=True,
    ).stdout
    return sorted(p for p in out.decode().split("\0") if p)


def derive_samples(clone: Path, files: list[str], limit: int = 12) -> list[dict]:
    """Up to `limit` (type, macros, file, line) sampled from files carrying
    at least one non-builtin derive, spread across the file list rather
    than clustered in whichever file sorts first."""
    hits: list[dict] = []
    stride = max(1, len(files) // (limit * 8)) if files else 1
    for path in files[::stride]:
        text = (clone / path).read_text(errors="replace")
        lines = text.splitlines()
        for i, line in enumerate(lines):
            m = DERIVE_RE.search(line)
            if not m:
                continue
            macros = [name.strip() for name in m.group(1).split(",") if name.strip()]
            macros = [name.rsplit("::", 1)[-1] for name in macros]
            if all(name in BUILTIN_DERIVES for name in macros):
                continue
            # The derived item is the next non-attribute, non-blank line.
            j = i + 1
            while j < len(lines) and (not lines[j].strip() or lines[j].lstrip().startswith("#[")):
                j += 1
            if j >= len(lines):
                continue
            item = ITEM_RE.match(lines[j])
            if not item:
                continue
            hits.append({"file": path, "line": i + 1, "type": item.group(1), "macros": macros})
            if len(hits) >= limit:
                return hits
    return hits


def cfg_gated_files(clone: Path, files: list[str]) -> list[str]:
    gated = []
    for path in files:
        text = (clone / path).read_text(errors="replace")
        # Only a module-level (`#![cfg(...)]`, first non-comment lines) or
        # whole-file gate matters for "is this file dropped entirely"; a
        # `#[cfg(test)]` block inside an otherwise-indexed file is not the
        # same failure mode and would need per-item spans to detect, which
        # this sample does not attempt.
        head = "\n".join(text.splitlines()[:15])
        if CFG_RE.search(head):
            gated.append(path)
    return gated


def build_rs_report(clone: Path, files: list[str]) -> dict:
    build_scripts = sorted(
        subprocess.run(
            ["git", "-C", str(clone), "ls-files", "-z", "--", "*/build.rs", "build.rs"],
            check=True, capture_output=True,
        ).stdout.decode().split("\0")
    )
    build_scripts = [p for p in build_scripts if p]
    out_dir_readers = [p for p in files if OUT_DIR_RE.search((clone / p).read_text(errors="replace"))]
    return {"build_scripts": build_scripts, "out_dir_readers": out_dir_readers}


def symbol_strings_by_file(index_path: Path, prefix: str) -> dict[str, set[str]]:
    """Every symbol string that appears (defined or referenced) in each
    document, keyed by the same repo-relative path `ingest()` uses. Kept
    separate from `ingest()`'s own pass so this sampler never has to widen
    that function's contract just to expose raw symbol text."""
    prefix = prefix.strip("/")
    prefix = f"{prefix}/" if prefix and prefix != "." else ""
    by_file: dict[str, set[str]] = {}
    for field, raw in iter_index(index_path):
        if field != 2:
            continue
        document = scip_pb2.Document.FromString(raw)
        relative = document.relative_path
        relative = relative[2:] if relative.startswith("./") else relative
        path = prefix + relative
        bucket = by_file.setdefault(path, set())
        for occurrence in document.occurrences:
            if occurrence.symbol:
                bucket.add(occurrence.symbol)
        for info in document.symbols:
            if info.symbol:
                bucket.add(info.symbol)
    return by_file


def score_derive_samples(samples: list[dict], symbols_by_file: dict[str, set[str]]) -> list[dict]:
    scored = []
    for hit in samples:
        symbols = symbols_by_file.get(hit["file"], set())
        type_symbols = [s for s in symbols if f"{hit['type']}#" in s or f"{hit['type']}." in s]
        verdicts = {}
        for macro in hit["macros"]:
            markers = MARKERS.get(macro)
            if markers is None:
                verdicts[macro] = "no marker known"
                continue
            found = any(marker in s for s in type_symbols for marker in markers)
            verdicts[macro] = "expanded (marker seen)" if found else "not observed"
        scored.append({**hit, "indexed_symbols_for_type": len(type_symbols), "verdicts": verdicts})
    return scored


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--index", type=Path, required=True)
    parser.add_argument("--clone", type=Path, required=True)
    parser.add_argument("--prefix", default=".")
    parser.add_argument("--slug", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    files = tracked_rs_files(args.clone)
    lang_of = {f: "rust" for f in files}
    result = ingest(args.index, args.prefix, set(files), lang_of, "rust", None)
    result["slug"] = args.slug
    result["rs_files_tracked"] = len(files)
    result["index_size_bytes"] = args.index.stat().st_size if args.index.exists() else 0

    samples = derive_samples(args.clone, files)
    symbols_by_file = symbol_strings_by_file(args.index, args.prefix) if args.index.exists() else {}
    result["derive_samples"] = score_derive_samples(samples, symbols_by_file)
    gated = cfg_gated_files(args.clone, files)
    result["cfg_gated_files_sampled"] = gated[:25]
    result["cfg_gated_files_sampled_count"] = len(gated)
    result["cfg_gated_and_unindexed"] = sorted(set(gated) & set(result["unindexed_lang_files"]))
    result["build_rs"] = build_rs_report(args.clone, files)

    args.out.write_text(json.dumps(result, indent=2, sort_keys=True))
    print(json.dumps({k: v for k, v in result.items() if k not in (
        "file_edges", "use_pair_symbols", "member_only_use_pairs", "symbol_edges",
        "relationship_pairs", "relationship_file_pairs", "unmapped_targets",
    )}, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
