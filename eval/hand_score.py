#!/usr/bin/env python3
"""Score the hand-written resolver against SCIP on the nine fixtures (issue #110, finding 48).

    hand_score.py score    --name N --work DIR --repo CLONE --out JSON
    hand_score.py summary  --work DIR --out JSON --markdown MD
    hand_score.py baseline --summary JSON --out data/scip/hand_score.json [--source URL]
    hand_score.py gate     --summary JSON --baseline data/scip/hand_score.json
    hand_score.py self-test

Runs on a GitHub-hosted runner (ci.yml `hand-score`); the laptop does not run
corpus-scale analysis (CLAUDE.md). `self-test` is synthetic and runs in the
push-time `gate` job.

The owner's decision (session 16030105, 2026-09-25T16:56:34Z, "Tune hand, SCIP
as oracle"): `hand` stays the default and SCIP measures it. This script is the
measurement. For each fixture and language it compares two directed file-pair
sets over the same mapped files:

- **hand**: the `imports` of `tolmap dump-graph --refs hand`, the pairs the
  hand-written resolver emits (what the product's recall gate calls the hand
  pairs, src/scip_ingest.rs `gate`).
- **SCIP**: the `file_edges` of P0's oracle ingest (eval/scip_ingest.py) on the
  very index `dump-graph --refs scip` read, kept by `TOLMAP_SCIP_INDEX_DIR`. A
  pair is a reference in A to a symbol defined in B. The oracle is used rather
  than the SCIP graph's own `imports` because a language whose index falls
  below the product's 0.80 admission floor (sqlalchemy, finding 45) falls
  back and its SCIP graph holds hand's pairs; the index is still there to
  score against. Where a language is admitted, the two must agree exactly
  (finding 44), and `score` checks that on every run (`oracle_check`).

Two directions, named for this job, not the product gate:
- **recall** = SCIP pairs hand also has / SCIP pairs.
- **precision** = hand pairs SCIP also has / hand pairs. This is the number
  the product gate calls "recall" (sqlalchemy's 0.6608 in finding 45).

Pairs one side has alone are classified by heuristics, not proof; the samples
in the JSON exist so a class can be checked by reading source.

Hand-only (Python), from the importing file's own import statements, with the
resolver's rules replicated (src/extract.rs `resolve_python`):
- `star import`: `from X import *` names the target. SCIP makes no occurrence
  for it, so hand is right and SCIP blind (finding 47).
- `submodule via package`: the target is a package `__init__.py` reached only
  as the head of `from pkg import sub` where every imported name is a
  submodule. The package's own content is never used: over-attribution.
- `re-export`: the target is a package `__init__.py` reached through a name
  (`from pkg import Name`) or as a module object (`from x import pkg`,
  `import pkg.sub`). SCIP credits the file that defines what is used.
- `other`: everything else (function-level imports, a resolver miss, ...).
Go adds `package spread`: SCIP links the source to another file of the
target's directory, so the pair is `resolve_multi` spreading one import over
the whole package (finding 44). TypeScript's `re-export` is a target named
`index.*` that imports a file SCIP links the source to (the SCIP-only rule
below, seen from the other side).

`precision_counting_reexports` is finding 47's looser rule, kept as it was so
its 0.9008 for sqlalchemy stays comparable: a hand pair into a package file
counts as kept when SCIP links the source to any file under that directory.

SCIP-only, from the hand graph plus the source files:
- `re-export`: hand links the source to a package `__init__.py` (or
  `index.*`) that imports the target, directly or through up to two more
  package files under it, and the target sits under that package: both see
  the dependency, SCIP places it more exactly. (Requiring the package file
  to import the target matters: every file of a package is "under" its
  `__init__`, so directory alone would call any inherited member or
  inferred type reached from a package import a re-export.)
- `inherited member` (Python): the target is reached from the source's own
  file or one it imports by up to three class-base hops, resolved through
  each file's imports (a member used on `self` or on an instance).
- `inferred type`: hand links the source to some file that links the target
  (two import hops): the source uses a value whose type lives in the target,
  which it never imports.
- `other`.

A fixed-seed sample (`random.Random(SEED)`) of up to SAMPLE rows per class
is kept; the class counts are over every pair.

`gate` compares a run with the committed baseline and fails on a regression,
conservatively (see `gate` for what it checks and why).
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import random
import sys
import tempfile
import tomllib
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEED = 7  # src/lib.rs SEED
SAMPLE = 12
EXTENDS_DEPTH = 3
REEXPORT_DEPTH = 3
HAND_CLASSES = ("star import", "submodule via package", "re-export", "package spread", "other")
SCIP_CLASSES = ("re-export", "inherited member", "inferred type", "other")
# The classes that are hand over-attributing to a package: each such pair
# claims a dependency on a file whose own content is not what is used. They
# may only go down (CLAUDE.md: numbers must be a lower bound).
GATED_CLASSES = ("submodule via package", "re-export")


def load(path: Path) -> dict:
    return json.loads(Path(path).read_text())


def directory(path: str) -> str:
    return path.rsplit("/", 1)[0] if "/" in path else ""


def under(path: str, package_file: str) -> bool:
    """`path` sits in the directory of `package_file` (or below it)."""
    base = directory(package_file)
    return path != package_file and (path.startswith(base + "/") if base else True)


def fingerprint(pairs) -> str:
    return hashlib.sha256(json.dumps(sorted(pairs), separators=(",", ":")).encode()).hexdigest()


def fixture_names() -> list[str]:
    with (ROOT / "data" / "fixtures.toml").open("rb") as handle:
        return list(tomllib.load(handle))


# --- the Python resolver, replicated (src/extract.rs) ---

def python_head(level: int, module: str, current: str, is_pkg: bool) -> str:
    """src/extract.rs `python_head`."""
    if level == 0:
        return module
    parts = current.split(".")
    if not is_pkg:
        parts = parts[:-1]
    strip = level - 1
    keep = len(parts) - strip if strip <= len(parts) else 0
    prefix = ".".join(parts[:keep])
    if not module:
        return prefix
    return f"{prefix}.{module}" if prefix else module


class Statement:
    """One import statement, as `python_imports` records it, plus context."""

    def __init__(self, node, context: frozenset):
        self.context = context
        self.lineno = node.lineno
        if isinstance(node, ast.ImportFrom):
            self.from_ = True
            self.level = node.level or 0
            self.module = node.module or ""
        else:
            self.from_ = False
            self.level = 0
            self.module = ""
        self.names = [(alias.name, alias.asname) for alias in node.names]


class Visitor(ast.NodeVisitor):
    def __init__(self):
        self.context: list[str] = []
        self.statements: list[Statement] = []
        self.bases: list[ast.expr] = []

    def record(self, node):
        self.statements.append(Statement(node, frozenset(self.context)))

    visit_Import = record
    visit_ImportFrom = record

    def nested(self, tag, node):
        self.context.append(tag)
        self.generic_visit(node)
        self.context.pop()

    def visit_FunctionDef(self, node):
        self.nested("in-function", node)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_Try(self, node):
        self.nested("in-try", node)

    def visit_If(self, node):
        self.nested("type-checking" if "TYPE_CHECKING" in ast.unparse(node.test) else "in-if", node)

    def visit_ClassDef(self, node):
        self.bases.extend(node.bases)
        self.generic_visit(node)


def dotted(expr) -> str | None:
    if isinstance(expr, ast.Name):
        return expr.id
    if isinstance(expr, ast.Attribute):
        head = dotted(expr.value)
        return f"{head}.{expr.attr}" if head else None
    if isinstance(expr, ast.Subscript):  # Generic[T], Base[int]
        return dotted(expr.value)
    return None


class PythonFiles:
    """Each mapped Python file's imports, resolved as the hand resolver does."""

    def __init__(self, graph: dict, repo: Path):
        nodes = [n for n in graph["nodes"] if (n.get("lang") or graph["lang"]) == "py"]
        self.module_of = {n["file"]: n["module"] for n in nodes}
        self.file_of = {n["module"]: n["file"] for n in nodes}
        self.parsed: dict[str, Visitor | None] = {}
        for file in sorted(self.module_of):
            path = repo / file
            try:
                tree = ast.parse(path.read_text(errors="replace"))
            except (OSError, SyntaxError, ValueError):
                self.parsed[file] = None
                continue
            visitor = Visitor()
            visitor.visit(tree)
            self.parsed[file] = visitor
        self._extends: dict[str, set[str]] = {}

    def resolve(self, hit: str) -> str | None:
        """`resolve_python`'s per-hit rule: the module, else its parent."""
        if not hit:
            return None
        if hit in self.file_of:
            return hit
        parent = hit.rsplit(".", 1)[0] if "." in hit else None
        return parent if parent in self.file_of else None

    def vias(self, source: str, target: str) -> list[tuple[str, frozenset, int]]:
        """How each import statement of `source` links `target`: (via, context, line).

        via is `head` (the package a `from` names), `module-name` (`from X
        import n` where X.n is a module), `name` (X.n is not a module, so it
        falls to X), `star`, `import-module` or `import-parent`.
        """
        visitor = self.parsed.get(source)
        if visitor is None:
            return []
        current = self.module_of[source]
        is_pkg = source.endswith("__init__.py")
        want = self.module_of.get(target)
        out = []
        for statement in visitor.statements:
            if statement.from_:
                head = python_head(statement.level, statement.module, current, is_pkg)
                if head and self.resolve(head) == want and want != current:
                    out.append(("head", statement.context, statement.lineno))
                for name, _ in statement.names:
                    full = f"{head}.{name}" if head else name
                    if name == "*":
                        if self.resolve(head) == want:
                            out.append(("star", statement.context, statement.lineno))
                    elif full in self.file_of:
                        if full == want:
                            out.append(("module-name", statement.context, statement.lineno))
                    elif self.resolve(full) == want and want != current:
                        out.append(("name", statement.context, statement.lineno))
            else:
                for name, _ in statement.names:
                    if name in self.file_of:
                        if name == want:
                            out.append(("import-module", statement.context, statement.lineno))
                    elif self.resolve(name) == want:
                        out.append(("import-parent", statement.context, statement.lineno))
        return out

    def bindings(self, file: str) -> dict[str, str]:
        """Local name -> module it names or comes from (a `from` name that
        is not a module maps to its package)."""
        visitor = self.parsed.get(file)
        if visitor is None:
            return {}
        current = self.module_of[file]
        is_pkg = file.endswith("__init__.py")
        out = {}
        for statement in visitor.statements:
            if statement.from_:
                head = python_head(statement.level, statement.module, current, is_pkg)
                for name, alias in statement.names:
                    if name == "*":
                        continue
                    full = f"{head}.{name}" if head else name
                    module = full if full in self.file_of else self.resolve(head)
                    if module:
                        out[alias or name] = module
            else:
                for name, alias in statement.names:
                    if alias:
                        module = self.resolve(name)
                        if module:
                            out[alias] = module
                    else:
                        root = name.split(".")[0]
                        out.setdefault(root, "\0import")
        return out

    def extends(self, file: str) -> set[str]:
        """Files holding a base class of a class defined in `file`."""
        if file in self._extends:
            return self._extends[file]
        visitor = self.parsed.get(file)
        found: set[str] = set()
        if visitor is not None:
            local = self.bindings(file)
            for base in visitor.bases:
                name = dotted(base)
                if not name:
                    continue
                root = name.split(".")[0]
                bound = local.get(root)
                module = None
                if bound == "\0import":
                    # `import a.b` then `a.b.C`: the longest module prefix.
                    parts = name.split(".")
                    for cut in range(len(parts), 0, -1):
                        if ".".join(parts[:cut]) in self.file_of:
                            module = ".".join(parts[:cut])
                            break
                elif bound:
                    # `from x import m` then `m.C` descends into m's
                    # submodules only when they are modules themselves.
                    module = bound
                    for part in name.split(".")[1:]:
                        if f"{module}.{part}" in self.file_of:
                            module = f"{module}.{part}"
                        else:
                            break
                if module:
                    target = self.file_of[module]
                    if target != file:
                        found.add(target)
        self._extends[file] = found
        return found


# --- classification ---

def classify_hand_only(lang: str, a: str, b: str, scip_targets: set[str], hand_targets: dict[str, set[str]],
                       py: PythonFiles | None) -> tuple[str, list[str]]:
    if lang == "go":
        spread = any(t != b and directory(t) == directory(b) for t in scip_targets)
        return ("package spread" if spread else "other"), []
    if lang != "py" or py is None:
        if is_package_file(b) and any(reexports(b, t, hand_targets) for t in scip_targets):
            return "re-export", []
        return "other", []
    vias = py.vias(a, b)
    context = sorted({tag for _, ctx, _ in vias for tag in ctx})
    kinds = {via for via, _, _ in vias}
    if "star" in kinds:
        return "star import", context
    if b.endswith("__init__.py"):
        if kinds == {"head"}:
            return "submodule via package", context
        if kinds & {"name", "module-name", "import-module", "import-parent"}:
            return "re-export", context
    if not vias:
        context = ["no import of target"]
    return "other", context


def is_package_file(path: str) -> bool:
    name = path.rsplit("/", 1)[-1]
    return name == "__init__.py" or name.startswith("index.")


def reexports(p: str, t: str, hand_targets: dict[str, set[str]]) -> bool:
    """Package file `p` passes on something from `t`: `p` links `t` in the
    hand graph, directly or through up to REEXPORT_DEPTH - 1 further package
    files (`pkg/__init__` importing `pkg/sub/__init__` importing the file),
    and `t` sits under `p`'s directory."""
    if not (is_package_file(p) and under(t, p)):
        return False
    frontier, seen = {p}, set()
    for _ in range(REEXPORT_DEPTH):
        step = set()
        for q in frontier - seen:
            seen.add(q)
            targets = hand_targets.get(q, set())
            if t in targets:
                return True
            step |= {x for x in targets if is_package_file(x) and under(x, p)}
        frontier = step
    return False


def classify_scip_only(lang: str, a: str, t: str, hand_targets: dict[str, set[str]], py: PythonFiles | None) -> str:
    direct = hand_targets.get(a, set())
    if any(reexports(p, t, hand_targets) for p in direct):
        return "re-export"
    if lang == "py" and py is not None:
        frontier, seen = {a} | direct, set()
        for _ in range(EXTENDS_DEPTH):
            step = set()
            for file in sorted(frontier - seen):
                seen.add(file)
                step |= py.extends(file)
            if t in step:
                return "inherited member"
            frontier = step
    if any(t in hand_targets.get(c, ()) for c in direct):
        return "inferred type"
    return "other"


def sample(rows: list[dict]) -> list[dict]:
    rows = sorted(rows, key=lambda r: (r["a"], r["b"]))
    return rows if len(rows) <= SAMPLE else sorted(
        random.Random(SEED).sample(rows, SAMPLE), key=lambda r: (r["a"], r["b"]))


def score_language(name: str, lang: str, hand_graph: dict, scip_graph: dict | None, ingest: dict,
                   py: PythonFiles | None) -> dict:
    lang_of = {n["file"]: n.get("lang") or hand_graph["lang"] for n in hand_graph["nodes"]}
    hand = {(a, b) for a, b, _ in hand_graph["imports"] if a != b and lang_of.get(a) == lang}
    scip = {(a, b) for a, b, *_ in ingest["file_edges"] if a != b and lang_of.get(a) == lang}
    shared = hand & scip
    hand_only = sorted(hand - scip)
    scip_only = sorted(scip - hand)
    scip_targets, hand_targets = defaultdict(set), defaultdict(set)
    for a, b in scip:
        scip_targets[a].add(b)
    for a, b in hand:
        hand_targets[a].add(b)

    rows_hand = []
    for a, b in hand_only:
        why, context = classify_hand_only(lang, a, b, scip_targets[a], hand_targets, py)
        rows_hand.append({"a": a, "b": b, "class": why, "context": context})
    rows_scip = [{"a": a, "b": t, "class": classify_scip_only(lang, a, t, hand_targets, py)} for a, t in scip_only]

    # The product gate's comparison with a package re-export counted as
    # kept: finding 47's 0.9008 for sqlalchemy.
    reexport_kept = sum(
        1 for a, b in hand
        if (a, b) in scip or (
            (b.endswith("__init__.py") or b.rsplit("/", 1)[-1].startswith("index."))
            and any(under(t, b) for t in scip_targets[a])))

    def ratio(x, y):
        return round(x / y, 4) if y else None

    row = {
        "name": name,
        "lang": lang,
        "files": sum(1 for f in lang_of.values() if f == lang),
        "files_indexed": ingest.get("mapped_files_of_lang_indexed"),
        "hand_pairs": len(hand),
        "scip_pairs": len(scip),
        "shared": len(shared),
        "recall": ratio(len(shared), len(scip)),
        "precision": ratio(len(shared), len(hand)),
        "precision_counting_reexports": ratio(reexport_kept, len(hand)),
        "hand_only": len(hand_only),
        "scip_only": len(scip_only),
        "hand_only_by_class": {c: sum(1 for r in rows_hand if r["class"] == c) for c in HAND_CLASSES},
        "scip_only_by_class": {c: sum(1 for r in rows_scip if r["class"] == c) for c in SCIP_CLASSES},
        "scip_fingerprint": fingerprint(scip),
        "hand_fingerprint": fingerprint(hand),
        "hand_only_samples": {c: sample([r for r in rows_hand if r["class"] == c]) for c in HAND_CLASSES},
        "scip_only_samples": {c: sample([r for r in rows_scip if r["class"] == c]) for c in SCIP_CLASSES},
        "hand_only_pairs": rows_hand,
    }
    if lang == "go":
        # The unit the hand Go graph asserts (src/scip_ingest.rs
        # `recall_granularity`): the target's directory.
        hd = {(a, directory(b)) for a, b in hand}
        sd = {(a, directory(b)) for a, b in scip}
        row["by_directory"] = {"hand_pairs": len(hd), "scip_pairs": len(sd), "shared": len(hd & sd),
                               "recall": ratio(len(hd & sd), len(sd)), "precision": ratio(len(hd & sd), len(hd))}
    references = ((scip_graph or {}).get("references") or {}).get(lang)
    row["references"] = references
    if references and references.get("path") == "scip":
        admitted = {(a, b) for a, b, _ in scip_graph["imports"] if a != b and lang_of.get(a) == lang}
        row["oracle_check"] = "equal" if admitted == scip else (
            f"differs: {len(admitted - scip)} only in the product graph, {len(scip - admitted)} only in the oracle")
    else:
        row["oracle_check"] = "fallback (not comparable)" if references else "no SCIP graph"
    return row


def score(args) -> int:
    work = args.work
    hand_graph = load(work / "hand.graph.json")
    scip_path = work / "scip.graph.json"
    scip_graph = load(scip_path) if scip_path.is_file() else None
    languages = sorted({n.get("lang") or hand_graph["lang"] for n in hand_graph["nodes"]})
    py = PythonFiles(hand_graph, args.repo) if "py" in languages else None
    rows = []
    for lang in languages:
        ingest_path = work / f"{lang}.ingest.json"
        if not ingest_path.is_file():
            rows.append({"name": args.name, "lang": lang, "status": "no index"})
            continue
        row = score_language(args.name, lang, hand_graph, scip_graph, load(ingest_path), py)
        row["status"] = "scored"
        rows.append(row)
    # District churn of this binary's hand map against the committed
    # fixture, so a resolver change is measured against the maps it moves.
    hand_map = work / "map" / f"{args.name}.json"
    committed = ROOT / "data" / f"{args.name}.json"
    if hand_map.is_file() and committed.is_file():
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from remote_build_result import placement
        built = load(hand_map)
        fraction, delta = placement(built, load(committed))
        for row in rows:
            row["hand_map_vs_fixture"] = {"placement": round(fraction, 4), "q_delta": round(delta, 4),
                                          "q": built["q"], "districts": len({int(n[0]) for n in built["N"]}),
                                          "edges": len(built.get("E", []))}
    args.out.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
    brief = ("name", "lang", "status", "hand_pairs", "scip_pairs", "shared", "recall", "precision",
             "hand_only_by_class", "scip_only_by_class", "oracle_check", "hand_map_vs_fixture")
    for row in rows:
        print(json.dumps({k: row[k] for k in brief if k in row}, sort_keys=True))
    return 0


# --- summary, baseline, gate ---

def pct(value) -> str:
    return "—" if value is None else f"{value:.4f}"


def markdown(rows: list[dict]) -> str:
    scored = [r for r in rows if r.get("status") == "scored"]
    out = ["### Pairs", "",
           "| fixture | lang | hand pairs | SCIP pairs | shared | recall | precision | precision, re-exports kept | product path (gate recall) | oracle check | hand map vs fixture |",
           "|---|---|---:|---:|---:|---:|---:|---:|---|---|---|"]
    for r in rows:
        if r.get("status") != "scored":
            out.append(f"| {r['name']} | {r['lang']} | {r.get('status')} |" + " |" * 8)
            continue
        ref = r.get("references") or {}
        path = f"{ref.get('path')} ({ref.get('reason')}, {ref.get('recall')})" if ref else "—"
        churn = r.get("hand_map_vs_fixture")
        churn = f"{100 * churn['placement']:.1f}%, Δq {churn['q_delta']:.4f}" if churn else "—"
        out.append(f"| {r['name']} | {r['lang']} | {r['hand_pairs']:,} | {r['scip_pairs']:,} | {r['shared']:,} "
                   f"| {pct(r['recall'])} | {pct(r['precision'])} | {pct(r['precision_counting_reexports'])} "
                   f"| {path} | {r['oracle_check']} | {churn} |")
        if "by_directory" in r:
            d = r["by_directory"]
            out.append(f"| {r['name']} (by directory) | {r['lang']} | {d['hand_pairs']:,} | {d['scip_pairs']:,} "
                       f"| {d['shared']:,} | {pct(d['recall'])} | {pct(d['precision'])} | | | | |")
    out += ["", "### Hand-only pairs by class", "",
            "| fixture | lang | hand-only | " + " | ".join(HAND_CLASSES) + " |",
            "|---|---|---:|" + "---:|" * len(HAND_CLASSES)]
    total = Counter()
    for r in scored:
        out.append(f"| {r['name']} | {r['lang']} | {r['hand_only']:,} | "
                   + " | ".join(f"{r['hand_only_by_class'][c]:,}" for c in HAND_CLASSES) + " |")
        total.update(r["hand_only_by_class"])
    out.append(f"| **all** | | {sum(r['hand_only'] for r in scored):,} | "
               + " | ".join(f"{total[c]:,}" for c in HAND_CLASSES) + " |")
    out += ["", "### SCIP-only pairs by class", "",
            "| fixture | lang | SCIP-only | " + " | ".join(SCIP_CLASSES) + " |",
            "|---|---|---:|" + "---:|" * len(SCIP_CLASSES)]
    total = Counter()
    for r in scored:
        out.append(f"| {r['name']} | {r['lang']} | {r['scip_only']:,} | "
                   + " | ".join(f"{r['scip_only_by_class'][c]:,}" for c in SCIP_CLASSES) + " |")
        total.update(r["scip_only_by_class"])
    out.append(f"| **all** | | {sum(r['scip_only'] for r in scored):,} | "
               + " | ".join(f"{total[c]:,}" for c in SCIP_CLASSES) + " |")
    return "\n".join(out) + "\n"


def summary(args) -> int:
    rows = []
    for name in fixture_names():
        path = args.work / name / "score.json"
        if not path.is_file():
            rows.append({"name": name, "lang": "?", "status": "not scored"})
            continue
        rows.extend(load(path))
    args.out.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
    table = markdown(rows)
    print(table)
    if args.markdown:
        args.markdown.write_text(table)
    return 0 if all(r.get("status") == "scored" for r in rows) else 1


BASELINE_FIELDS = ("name", "lang", "files", "hand_pairs", "scip_pairs", "shared", "recall", "precision",
                   "precision_counting_reexports", "hand_only", "scip_only", "hand_only_by_class",
                   "scip_only_by_class", "scip_fingerprint", "hand_fingerprint", "by_directory")


def baseline(args) -> int:
    rows = [r for r in load(args.summary) if r.get("status") == "scored"]
    document = {
        "source": args.source,
        "gated_classes": list(GATED_CLASSES),
        "rows": [{k: r[k] for k in BASELINE_FIELDS if k in r} for r in rows],
    }
    args.out.write_text(json.dumps(document, indent=1, sort_keys=True) + "\n")
    print(f"wrote {args.out}: {len(rows)} rows")
    return 0


def gate(args) -> int:
    """Fail on a regression against the committed baseline, and only on one.

    Conservative on purpose. It checks three things per fixture and language:
    - The oracle has not moved: the SCIP pair set's fingerprint equals the
      baseline's. The indexers and the fixture pins are fixed, so a change
      means an indexer, a pin or the mapped file set changed, and every
      other number is then not comparable; re-baseline with a finding.
    - Over-attribution may only go down: the `submodule via package` and
      `re-export` hand-only counts may not rise. Each such pair claims a
      dependency on a package `__init__` whose own content is not what is
      used, against CLAUDE.md's "numbers must be a lower bound".
    - Confirmed pairs may only go up: `shared` may not fall. Hand losing a
      pair SCIP confirms is the lower bound shrinking.
    It does not gate `star import` (hand is right, SCIP blind), `other`
    (heuristic and mixed), the SCIP-only classes (they move when hand adds a
    correct pair, which is progress) or the ratios, which follow from the
    counts it does gate. An improvement never fails; it is printed so the
    baseline can be lowered in the same change, with a finding.
    """
    base = {(r["name"], r["lang"]): r for r in load(args.baseline)["rows"]}
    rows = {(r["name"], r["lang"]): r for r in load(args.summary)}
    failed, improved = [], []
    for key, b in sorted(base.items()):
        r = rows.get(key)
        label = f"{key[0]} ({key[1]})"
        if r is None or r.get("status") != "scored":
            failed.append(f"{label}: not scored")
            continue
        problems = []
        if r["scip_fingerprint"] != b["scip_fingerprint"]:
            problems.append(f"the SCIP pair set changed ({b['scip_pairs']} → {r['scip_pairs']} pairs); "
                            "the oracle moved, so re-baseline with a finding")
        for c in GATED_CLASSES:
            was, now = b["hand_only_by_class"][c], r["hand_only_by_class"][c]
            if now > was:
                problems.append(f"hand-only `{c}` rose {was} → {now}")
            elif now < was:
                improved.append(f"{label}: hand-only `{c}` {was} → {now}")
        if r["shared"] < b["shared"]:
            problems.append(f"confirmed pairs fell {b['shared']} → {r['shared']}")
        elif r["shared"] > b["shared"]:
            improved.append(f"{label}: confirmed pairs {b['shared']} → {r['shared']}")
        print(f"{'FAIL' if problems else 'PASS'} {label}: recall {pct(b['recall'])} → {pct(r['recall'])}, "
              f"precision {pct(b['precision'])} → {pct(r['precision'])}, "
              f"hand-only {b['hand_only']} → {r['hand_only']}, SCIP-only {b['scip_only']} → {r['scip_only']}"
              + (" -- " + "; ".join(problems) if problems else ""))
        if problems:
            failed.append(label)
    for line in improved:
        print(f"improved {line} (lower the baseline in this change)")
    if failed:
        print(f"hand-score gate failed: {', '.join(failed)}")
        return 1
    print("hand-score gate passed")
    return 0


# --- self-test: a synthetic package with one pair of every class ---

def self_test(_args) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        repo = Path(tmp)
        files = {
            "pkg/__init__.py": "from .core import Engine\nfrom ._api import *\nVERSION = 1\n",
            "pkg/_api.py": "def get():\n    pass\n",
            "pkg/core.py": "from .base import Base\nclass Engine(Base):\n    pass\n",
            "pkg/base.py": "from .types import Kind\nclass Base:\n    kind: Kind\n",
            "pkg/types.py": "class Kind:\n    pass\n",
            "pkg/util.py": "x = 1\n",
            "pkg/cli.py": "from . import util\nfrom . import Engine\nfrom .core import Engine as E\n",
            "pkg/star.py": "from pkg import *\n",
        }
        for path, text in files.items():
            (repo / path).parent.mkdir(parents=True, exist_ok=True)
            (repo / path).write_text(text)
        module = {f: f[:-3].replace("/", ".").removesuffix(".__init__") for f in files}
        nodes = [{"file": f, "module": module[f], "lang": "py"} for f in sorted(files)]
        hand = [
            ["pkg/__init__.py", "pkg/core.py", 1.0], ["pkg/__init__.py", "pkg/_api.py", 1.0],
            ["pkg/core.py", "pkg/base.py", 1.0], ["pkg/base.py", "pkg/types.py", 1.0],
            ["pkg/cli.py", "pkg/__init__.py", 2.0], ["pkg/cli.py", "pkg/util.py", 1.0],
            ["pkg/cli.py", "pkg/core.py", 1.0], ["pkg/star.py", "pkg/__init__.py", 1.0],
        ]
        scip = [
            ["pkg/__init__.py", "pkg/core.py"], ["pkg/core.py", "pkg/base.py"], ["pkg/base.py", "pkg/types.py"],
            ["pkg/cli.py", "pkg/util.py"], ["pkg/cli.py", "pkg/core.py"],
            # Engine's inherited member, and Base.kind's type through core.
            ["pkg/cli.py", "pkg/base.py"], ["pkg/core.py", "pkg/types.py"],
        ]
        graph = {"lang": "py", "nodes": nodes, "imports": hand}
        py = PythonFiles(graph, repo)
        row = score_language("synthetic", "py", graph, None, {"file_edges": [p + [1, 1, 0] for p in scip]}, py)
        by_pair = {(r["a"], r["b"]): r["class"] for r in row["hand_only_pairs"]}
        # `from . import util` + `from . import Engine`: the statement naming
        # Engine is a re-export; util alone would be `submodule via package`.
        assert by_pair == {
            ("pkg/__init__.py", "pkg/_api.py"): "star import",
            ("pkg/cli.py", "pkg/__init__.py"): "re-export",
            ("pkg/star.py", "pkg/__init__.py"): "star import",
        }, by_pair
        assert row["scip_only_by_class"] == {"re-export": 0, "inherited member": 1, "inferred type": 1,
                                             "other": 0}, row["scip_only_by_class"]
        assert (row["hand_pairs"], row["scip_pairs"], row["shared"]) == (8, 7, 5), row
        assert row["recall"] == round(5 / 7, 4) and row["precision"] == round(5 / 8, 4), row

        # Without the Engine import the package link is submodule-only.
        (repo / "pkg/cli.py").write_text("from . import util\nfrom .core import Engine as E\n")
        py = PythonFiles(graph, repo)
        why, _ = classify_hand_only("py", "pkg/cli.py", "pkg/__init__.py", set(), {}, py)
        assert why == "submodule via package", why
        # A re-exported name SCIP credits to the defining file.
        assert classify_scip_only("py", "pkg/cli.py", "pkg/core.py",
                                  {"pkg/cli.py": {"pkg/__init__.py"}, "pkg/__init__.py": {"pkg/core.py"}},
                                  py) == "re-export"
        # ... and not when the package file never imports it.
        assert classify_scip_only("py", "pkg/cli.py", "pkg/types.py",
                                  {"pkg/cli.py": {"pkg/__init__.py"}, "pkg/__init__.py": {"pkg/core.py"}},
                                  py) == "other"
        # Go: a hand pair spread to a sibling file of the one SCIP names.
        assert classify_hand_only("go", "a/x.go", "b/one.go", {"b/two.go"}, {}, None) == ("package spread", [])
        assert classify_hand_only("go", "a/x.go", "b/one.go", {"c/two.go"}, {}, None) == ("other", [])
    print("hand_score self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("score")
    run.add_argument("--name", required=True)
    run.add_argument("--work", type=Path, required=True,
                     help="hand.graph.json, scip.graph.json, <lang>.ingest.json, map/<name>.json")
    run.add_argument("--repo", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.set_defaults(run=score)
    run = commands.add_parser("summary")
    run.add_argument("--work", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--markdown", type=Path)
    run.set_defaults(run=summary)
    run = commands.add_parser("baseline")
    run.add_argument("--summary", type=Path, required=True)
    run.add_argument("--out", type=Path, required=True)
    run.add_argument("--source", default=None, help="the Actions run the scores came from")
    run.set_defaults(run=baseline)
    run = commands.add_parser("gate")
    run.add_argument("--summary", type=Path, required=True)
    run.add_argument("--baseline", type=Path, required=True)
    run.set_defaults(run=gate)
    commands.add_parser("self-test").set_defaults(run=self_test)
    args = parser.parse_args()
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
