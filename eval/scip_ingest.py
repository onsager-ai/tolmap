#!/usr/bin/env python3
"""Derive tolmap-shaped reference data from one SCIP index (issue #110, P0).

Runs on a GitHub-hosted runner only (.github/workflows/scip-spike.yml); an
index of a real repository is corpus-scale work and the maintainer's laptop
cannot take it (CLAUDE.md).

Why Python and not a Rust bin behind a cargo feature: P0 is a measurement,
not the product. A Python script over the `protobuf` wheel keeps Cargo.toml,
Cargo.lock and every product build byte-for-byte untouched, while the
generated `scip_pb2` comes from the pinned upstream `scip.proto` (v0.10.0)
rather than a hand-maintained decoder. P1's ingest belongs in Rust
(issue #110's design: the `scip` crate plus protobuf), and it can be checked
against this script's output on the same index.

What it derives, all restricted to the mapped file set (the map's `F`):

- **file -> file edges**: a non-definition occurrence in file A of a
  non-local symbol whose definition occurrence is in file B, A != B, both
  mapped. Weighted by distinct symbols and by occurrences. A pair supported
  only by namespace/module symbols (descriptor ending in `/` or `:`, e.g. a
  Go package clause or a Python module object) is flagged, because those are
  import-statement edges rather than uses.
- **symbol -> symbol references**: the reference is credited to the
  innermost tolmap symbol span (the symbols document, finding 30) containing
  its line; the target is the innermost span containing the symbol's
  definition line. Using tolmap's own spans on both ends makes the pairs
  directly comparable with the symbols document's edges. References outside
  every span are module-level and not credited (the same rule
  `src/symbols.rs` applies), and a reference to the enclosing symbol or one
  of its ancestors is dropped (also the same rule).
- **relationship-typed edges**: every `SymbolInformation.relationships` row
  (is_implementation / is_type_definition / is_reference / is_definition)
  whose source and target are both defined in mapped files.

Only in-repository definitions count: a symbol with no definition
occurrence anywhere in the index is external. Everything is sorted before
it is written, so identical input gives an identical output.

The top-level `Index` message is walked on the wire and each `Document` is
parsed on its own (two passes: definitions, then references). An index of
n8n's size never has to be materialised as one Python object tree.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import mmap
import sys
import tempfile
from array import array
from collections import Counter, defaultdict
from pathlib import Path

import scip_pb2  # generated on the runner from the pinned scip.proto

DEFINITION = 0x1
IMPORT = 0x2

# Relationship flags, packed into one int per relationship row.
REL_REFERENCE = 1
REL_IMPLEMENTATION = 2
REL_TYPE_DEFINITION = 4
REL_DEFINITION = 8
REL_NAMES = {
    REL_REFERENCE: "reference",
    REL_IMPLEMENTATION: "implementation",
    REL_TYPE_DEFINITION: "type_definition",
    REL_DEFINITION: "definition",
}

# SCIP descriptor suffixes (scip.proto `Descriptor.Suffix`), classified by
# the final character of the symbol string. A backtick-escaped name ends in
# a backtick followed by its suffix, so the last character is still the
# suffix.
CALLABLE, TYPE, TERM, NAMESPACE, META, PARAMETER, TYPE_PARAMETER, OTHER = range(8)
CATEGORY_NAMES = (
    "callable",
    "type",
    "term",
    "namespace",
    "meta",
    "parameter",
    "type_parameter",
    "other",
)


def category(symbol: str) -> int:
    if symbol.endswith(")."):
        return CALLABLE
    last = symbol[-1:]
    if last == "#":
        return TYPE
    if last == ".":
        return TERM
    if last == "/":
        return NAMESPACE
    if last == ":":
        return META
    if last == ")":
        return PARAMETER
    if last == "]":
        return TYPE_PARAMETER
    return OTHER


def _varint(buffer, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        byte = buffer[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return result, offset
        shift += 7


def iter_index(path: Path):
    """Yield (field_number, bytes) for each top-level field of an Index.

    Field 1 is metadata, 2 a Document, 3 an external SymbolInformation. All
    three are length-delimited; anything else is skipped by wire type.
    """
    with path.open("rb") as handle:
        size = path.stat().st_size
        if size == 0:
            return
        with mmap.mmap(handle.fileno(), 0, access=mmap.ACCESS_READ) as buffer:
            offset = 0
            while offset < size:
                key, offset = _varint(buffer, offset)
                field, wire = key >> 3, key & 7
                if wire == 2:
                    length, offset = _varint(buffer, offset)
                    yield field, buffer[offset : offset + length]
                    offset += length
                elif wire == 0:
                    _, offset = _varint(buffer, offset)
                elif wire == 1:
                    offset += 8
                elif wire == 5:
                    offset += 4
                else:
                    raise ValueError(f"{path}: unsupported wire type {wire} at {offset}")


def start_line(occurrence) -> int | None:
    """0-based start line, from the deprecated packed range or the typed one."""
    if len(occurrence.range):
        return occurrence.range[0]
    which = occurrence.WhichOneof("typed_range")
    if which == "single_line_range":
        return occurrence.single_line_range.line
    if which == "multi_line_range":
        return occurrence.multi_line_range.start_line
    return None


def has_enclosing_range(occurrence) -> bool:
    return bool(len(occurrence.enclosing_range)) or (
        occurrence.WhichOneof("typed_enclosing_range") is not None
    )


class Spans:
    """Innermost tolmap symbol span containing a 1-based line, per file.

    `rows` are the symbols document's rows: (file index, name, kind, start,
    end, parent, ...), lines 1-based and inclusive. Built lazily per file as
    a line -> symbol array; larger spans are written first so a nested
    symbol overwrites its container, which makes the lookup O(1).
    """

    def __init__(self, files: list[str], rows: list[list]):
        self.rows = rows
        self.by_file: dict[str, list[int]] = defaultdict(list)
        for index, row in enumerate(rows):
            self.by_file[files[row[0]]].append(index)
        self.depth = [0] * len(rows)
        for index, row in enumerate(rows):
            depth, parent = 0, row[5]
            while parent >= 0:
                depth += 1
                parent = rows[parent][5]
            self.depth[index] = depth
        self.cache: dict[str, array] = {}

    def owner_table(self, path: str) -> array | None:
        table = self.cache.get(path)
        if table is not None or path not in self.by_file:
            return table
        indices = self.by_file[path]
        last = max(self.rows[i][4] for i in indices)
        table = array("i", [-1]) * (last + 2)
        for i in sorted(
            indices,
            key=lambda i: (-(self.rows[i][4] - self.rows[i][3]), self.depth[i], i),
        ):
            start, end = self.rows[i][3], self.rows[i][4]
            table[start : end + 1] = array("i", [i]) * (end - start + 1)
        self.cache[path] = table
        return table

    def innermost(self, path: str, line: int) -> int:
        table = self.owner_table(path)
        if table is None or line >= len(table) or line < 0:
            return -1
        return table[line]

    def is_self_or_ancestor(self, target: int, owner: int) -> bool:
        while owner >= 0:
            if owner == target:
                return True
            owner = self.rows[owner][5]
        return False


def ingest(
    index_path: Path,
    prefix: str,
    mapped: set[str],
    lang_of: dict[str, str],
    lang: str,
    spans: Spans | None,
) -> dict:
    prefix = prefix.strip("/")
    prefix = f"{prefix}/" if prefix and prefix != "." else ""

    def normalise(relative: str) -> str:
        relative = relative[2:] if relative.startswith("./") else relative
        return prefix + relative

    symbol_ids: dict[str, int] = {}
    symbol_names: list[str] = []

    def intern(symbol: str) -> int:
        sid = symbol_ids.get(symbol)
        if sid is None:
            sid = len(symbol_names)
            symbol_ids[symbol] = sid
            symbol_names.append(symbol)
        return sid

    metadata: dict = {}
    seen_paths: set[str] = set()
    duplicate_documents = 0
    documents = 0
    occurrences = 0
    definitions = 0
    definitions_with_enclosing = 0
    languages = Counter()
    def_sites: dict[int, set[tuple[str, int]]] = defaultdict(set)
    relationships: list[tuple[int, int, int]] = []

    # Pass 1: metadata, definitions, relationships. The first document for a
    # path wins; overlapping TypeScript projects can emit the same file twice.
    for field, raw in iter_index(index_path):
        if field == 1:
            meta = scip_pb2.Metadata.FromString(raw)
            metadata = {
                "tool": meta.tool_info.name,
                "version": meta.tool_info.version,
                "arguments": list(meta.tool_info.arguments),
                "project_root": meta.project_root,
            }
            continue
        if field != 2:
            continue
        document = scip_pb2.Document.FromString(raw)
        path = normalise(document.relative_path)
        documents += 1
        if path in seen_paths:
            duplicate_documents += 1
            continue
        seen_paths.add(path)
        languages[document.language or "(unset)"] += 1
        for occurrence in document.occurrences:
            occurrences += 1
            if not occurrence.symbol_roles & DEFINITION:
                continue
            symbol = occurrence.symbol
            if not symbol or symbol.startswith("local "):
                continue
            line = start_line(occurrence)
            if line is None:
                continue
            definitions += 1
            if has_enclosing_range(occurrence):
                definitions_with_enclosing += 1
            def_sites[intern(symbol)].add((path, line + 1))
        for info in document.symbols:
            if not info.symbol or info.symbol.startswith("local "):
                continue
            source = intern(info.symbol)
            for relationship in info.relationships:
                if not relationship.symbol or relationship.symbol.startswith("local "):
                    continue
                flags = (
                    (REL_REFERENCE if relationship.is_reference else 0)
                    | (REL_IMPLEMENTATION if relationship.is_implementation else 0)
                    | (REL_TYPE_DEFINITION if relationship.is_type_definition else 0)
                    | (REL_DEFINITION if relationship.is_definition else 0)
                )
                relationships.append((source, intern(relationship.symbol), flags))

    # A symbol's mapped definition sites, sorted; its canonical site (for the
    # symbol-level target) is the first of them.
    mapped_defs: dict[int, list[tuple[str, int]]] = {}
    repo_unmapped_defs: set[int] = set()
    multi_file = 0
    for sid, sites in def_sites.items():
        inside = sorted(site for site in sites if site[0] in mapped)
        if inside:
            mapped_defs[sid] = inside
            if len({site[0] for site in inside}) > 1:
                multi_file += 1
        else:
            repo_unmapped_defs.add(sid)
    target_span_cache: dict[int, int] = {}

    def target_span(sid: int) -> int:
        cached = target_span_cache.get(sid)
        if cached is None:
            path, line = mapped_defs[sid][0]
            cached = spans.innermost(path, line) if spans else -1
            target_span_cache[sid] = cached
        return cached

    categories = [category(name) for name in symbol_names]

    pair_symbols: dict[tuple[str, str], set[int]] = defaultdict(set)
    pair_occurrences: Counter = Counter()
    pair_uses: set[tuple[str, str]] = set()
    symbol_edges: Counter = Counter()
    symbol_edge_categories: dict[tuple[int, int], int] = defaultdict(int)
    references = Counter()
    symbol_stats = Counter()
    calls = Counter()

    # Pass 2: references from mapped documents.
    seen_paths.clear()
    for field, raw in iter_index(index_path):
        if field != 2:
            continue
        document = scip_pb2.Document.FromString(raw)
        path = normalise(document.relative_path)
        if path in seen_paths:
            continue
        seen_paths.add(path)
        if path not in mapped:
            continue
        for occurrence in document.occurrences:
            roles = occurrence.symbol_roles
            if roles & DEFINITION:
                continue
            symbol = occurrence.symbol
            if not symbol:
                continue
            line = start_line(occurrence)
            if line is None:
                continue
            line += 1
            owner = spans.innermost(path, line) if spans else -1
            if symbol.startswith("local "):
                references["local"] += 1
                if owner >= 0:
                    calls["local_refs_in_symbols"] += 1
                continue
            references["nonlocal"] += 1
            if roles & IMPORT:
                references["import_role"] += 1
            sid = symbol_ids.get(symbol)
            sites = mapped_defs.get(sid) if sid is not None else None
            kind = categories[sid] if sid is not None else category(symbol)
            is_call_like = kind == CALLABLE and not roles & IMPORT
            if is_call_like and owner >= 0:
                calls["callable_refs_in_symbols"] += 1
            if sites is None:
                if sid is not None and sid in repo_unmapped_defs:
                    references["to_repo_unmapped"] += 1
                    if is_call_like and owner >= 0:
                        calls["to_repo_unmapped"] += 1
                else:
                    references["external"] += 1
                    if is_call_like and owner >= 0:
                        calls["external"] += 1
                continue
            references["to_mapped"] += 1
            if is_call_like and owner >= 0:
                calls["to_mapped"] += 1
            for target_file in {site[0] for site in sites}:
                if target_file == path:
                    continue
                pair = (path, target_file)
                pair_symbols[pair].add(sid)
                pair_occurrences[pair] += 1
                if kind not in (NAMESPACE, META):
                    pair_uses.add(pair)
            if spans is None:
                continue
            if owner < 0:
                symbol_stats["module_level_refs"] += 1
                continue
            target = target_span(sid)
            if target < 0:
                symbol_stats["target_not_a_symbol"] += 1
                continue
            if spans.is_self_or_ancestor(target, owner):
                symbol_stats["self_or_ancestor"] += 1
                continue
            symbol_stats["credited"] += 1
            symbol_edges[(owner, target)] += 1
            symbol_edge_categories[(owner, target)] |= 1 << kind

    relationship_counts = Counter()
    relationship_pairs: dict[tuple[int, int], int] = defaultdict(int)
    relationship_file_pairs: dict[tuple[str, str], int] = defaultdict(int)
    for source, target, flags in relationships:
        for bit, name in REL_NAMES.items():
            if flags & bit:
                relationship_counts[f"{name}:all"] += 1
        if source not in mapped_defs or target not in mapped_defs:
            continue
        for bit, name in REL_NAMES.items():
            if flags & bit:
                relationship_counts[f"{name}:in_repo"] += 1
        source_file = mapped_defs[source][0][0]
        target_file = mapped_defs[target][0][0]
        if source_file != target_file:
            relationship_file_pairs[(source_file, target_file)] |= flags
        if spans is None:
            continue
        a, b = target_span(source), target_span(target)
        if a >= 0 and b >= 0 and a != b:
            relationship_pairs[(a, b)] |= flags

    file_edges = sorted(
        [a, b, len(pair_symbols[(a, b)]), pair_occurrences[(a, b)], int((a, b) in pair_uses)]
        for (a, b) in pair_symbols
    )
    edges_out = sorted(
        [a, b, count, symbol_edge_categories[(a, b)]] for (a, b), count in symbol_edges.items()
    )
    relationship_rows = sorted([a, b, flags] for (a, b), flags in relationship_pairs.items())
    relationship_file_rows = sorted(
        [a, b, flags] for (a, b), flags in relationship_file_pairs.items()
    )
    lang_files = sorted(f for f in mapped if lang_of.get(f) == lang)
    indexed_lang_files = sum(1 for f in lang_files if f in seen_paths)
    fingerprint = hashlib.sha256(
        json.dumps([file_edges, edges_out, relationship_rows], separators=(",", ":")).encode()
    ).hexdigest()
    return {
        "metadata": metadata,
        "prefix": prefix,
        "documents": documents,
        "duplicate_documents": duplicate_documents,
        "document_languages": dict(sorted(languages.items())),
        "documents_mapped": sum(1 for p in seen_paths if p in mapped),
        "mapped_files_of_lang": len(lang_files),
        "mapped_files_of_lang_indexed": indexed_lang_files,
        "occurrences": occurrences,
        "definitions": definitions,
        "definitions_with_enclosing_range": definitions_with_enclosing,
        "symbols_defined_in_mapped_files": len(mapped_defs),
        "symbols_defined_in_multiple_mapped_files": multi_file,
        "references": dict(sorted(references.items())),
        "calls": dict(sorted(calls.items())),
        "symbol_stats": dict(sorted(symbol_stats.items())),
        "relationships": dict(sorted(relationship_counts.items())),
        "file_edges": file_edges,
        "symbol_edges": edges_out,
        "relationship_pairs": relationship_rows,
        "relationship_file_pairs": relationship_file_rows,
        "fingerprint": fingerprint,
    }


def self_test() -> None:
    """A two-file synthetic index with a known answer, run before real data."""
    index = scip_pb2.Index()
    index.metadata.tool_info.name = "synthetic"
    a = index.documents.add(relative_path="pkg/a.py", language="python")
    # module object (meta), a function f (callable) and a class C (type).
    a.occurrences.add(range=[0, 0, 0], symbol="p `pkg.a`/__init__:", symbol_roles=DEFINITION)
    a.occurrences.add(range=[1, 4, 5], symbol="p `pkg.a`/f().", symbol_roles=DEFINITION)
    a.occurrences.add(range=[4, 6, 7], symbol="p `pkg.a`/C#", symbol_roles=DEFINITION)
    info = a.symbols.add(symbol="p `pkg.a`/C#")
    info.relationships.add(symbol="p `pkg.b`/Base#", is_implementation=True)
    b = index.documents.add(relative_path="pkg/b.py", language="python")
    b.occurrences.add(range=[0, 0, 0], symbol="p `pkg.b`/__init__:", symbol_roles=DEFINITION)
    b.occurrences.add(range=[0, 5, 10], symbol="p `pkg.a`/__init__:", symbol_roles=IMPORT)
    b.occurrences.add(range=[2, 6, 10], symbol="p `pkg.b`/Base#", symbol_roles=DEFINITION)
    b.occurrences.add(
        single_line_range=scip_pb2.SingleLineRange(line=5, start_character=4, end_character=9),
        symbol="p `pkg.b`/g().",
        symbol_roles=DEFINITION,
    )
    b.occurrences.add(range=[6, 8, 9], symbol="p `pkg.a`/f().")  # call inside g
    b.occurrences.add(range=[6, 12, 13], symbol="p `pkg.a`/f().")  # second call
    b.occurrences.add(range=[7, 8, 11], symbol="npm ext 1.0 lib/x().")  # external
    b.occurrences.add(range=[7, 1, 2], symbol="local 3")
    b.occurrences.add(range=[9, 0, 1], symbol="p `pkg.a`/C#")  # module level
    path = Path(tempfile.mkdtemp()) / "self_test.scip"
    try:
        path.write_bytes(index.SerializeToString())
        files = ["pkg/a.py", "pkg/b.py", "pkg/c.py"]
        rows = [
            [0, "f", 1, 2, 3, -1, 2, False],
            [0, "C", 0, 5, 8, -1, 4, False],
            [1, "Base", 0, 3, 4, -1, 2, False],
            [1, "g", 1, 6, 9, -1, 4, False],
        ]
        result = ingest(
            path, ".", set(files), {f: "py" for f in files}, "py", Spans(files, rows)
        )
    finally:
        path.unlink(missing_ok=True)
    assert result["file_edges"] == [["pkg/b.py", "pkg/a.py", 3, 4, 1]], result["file_edges"]
    assert result["symbol_edges"] == [[3, 0, 2, 1 << CALLABLE]], result["symbol_edges"]
    assert result["calls"] == {
        "callable_refs_in_symbols": 3,
        "external": 1,
        "local_refs_in_symbols": 1,
        "to_mapped": 2,
    }, result["calls"]
    assert result["symbol_stats"] == {
        "credited": 2,
        "module_level_refs": 2,
    }, result["symbol_stats"]
    assert result["relationship_pairs"] == [[1, 2, REL_IMPLEMENTATION]], result
    assert result["relationship_file_pairs"] == [["pkg/a.py", "pkg/b.py", REL_IMPLEMENTATION]]
    assert result["mapped_files_of_lang"] == 3 and result["mapped_files_of_lang_indexed"] == 2
    print("scip_ingest self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--index", type=Path)
    parser.add_argument("--prefix", default=".")
    parser.add_argument("--map", type=Path, help="tolmap map JSON (for F)")
    parser.add_argument("--graph", type=Path, help="tolmap dump-graph JSON (for node lang)")
    parser.add_argument("--symbols", type=Path, help="tolmap symbols JSON (for spans)")
    parser.add_argument("--lang", choices=("py", "ts", "go"))
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    document = json.loads(args.map.read_text())
    files = document["F"]
    graph = json.loads(args.graph.read_text())
    lang_of = {node["file"]: node.get("lang") or graph["lang"] for node in graph["nodes"]}
    spans = None
    if args.symbols:
        spans = Spans(files, json.loads(args.symbols.read_text())["symbols"])
    result = ingest(args.index, args.prefix, set(files), lang_of, args.lang, spans)
    args.out.write_text(json.dumps(result, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
