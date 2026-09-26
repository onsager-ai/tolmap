#!/usr/bin/env python3
"""Score the hand-written resolver against SCIP on the nine fixtures (issue #110, finding 48).

    hand_score.py score    --name N --work DIR --repo CLONE --out JSON
    hand_score.py summary  --work DIR --out JSON --markdown MD
    hand_score.py baseline --summary JSON --out data/scip/hand_score.json [--source URL]
    hand_score.py gate     --summary JSON --baseline data/scip/hand_score.json
    hand_score.py ts-variants --work DIR
    hand_score.py rust-targets
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

Each is also reported over SCIP's **use pairs** (`*_uses`): the pairs at
least one non-namespace symbol supports (the ingest's `uses` flag). The rest
are namespace-only: the import statement `from pkg.sub import x` is itself
an occurrence of the module symbol `pkg.sub`, whose definition is the
package's `__init__.py`, so SCIP has that pair even when nothing the
`__init__` defines is used. That is the pair a package over-attribution fix
removes, so "confirmed by a use" is what the gate holds, and
`shared_namespace_only` counts the hand pairs SCIP confirms only that way.

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
- `same package` (Go, checked first): source and target share a directory.
  Go needs no import there, so no import-level resolver sees the pair.
- `member via value` (Go, checked next): hand links the source to another
  file of the target's directory, and every symbol SCIP credits the pair
  with is a method or field (the ingest's `member_only_use_pairs`). This is
  `x := pkg.New(); x.Run()`: the importer never names `Run`, so the hand
  resolver, which since finding 50 links an import only to the files that
  declare the names it selects, does not link `Run`'s file.
- `other`.

For Go the row also carries `go` (finding 50):
- `import_outcomes`: how each in-repo import resolved, from the resolver's
  own report (`TOLMAP_GO_IMPORT_REPORT`, written by `dump-graph --refs
  hand`): `narrowed` to the declaring files, `narrowed_by_build` when
  some name was declared in several files and the target build broke the
  tie (finding 54), or why it kept the whole package (`opaque`,
  `no_names`, `undeclared_name`, `unknown_declarations`, and for a name
  several files declare: `ambiguous_name`, several of them in the build;
  `excluded_only`, none; `unknown_constraint`, one whose constraint could
  not be evaluated; `importer_not_in_build`), with the most frequent
  failing names.
- `build`: how many Go files the target build has in, out and unknown
  (finding 54, from the resolver's `go-build.*.json`), with every file
  that is not in.
- `file_gap`: SCIP use pairs hand lacks although it links the source to the
  target's directory. Before finding 50 the whole-package link covered all
  of them, so this is exactly what narrowing gave up, split into
  `member_only` (reached only through a method or field) and the rest.

For TypeScript the row also carries `ts` (finding 51), from the resolver's
own per-specifier report (`TOLMAP_TS_IMPORT_REPORT`, written by `dump-graph
--refs hand`: where each specifier resolved, and the files its names reach
when followed through `export ... from` and `export *`):
- `import_outcomes`: `defined_here`, `followed`, `partly_followed`,
  `uncertain` (with the failing names), `opaque` (a namespace, side-effect
  or star form, `require()`, `import()`), `no_names`, `unresolved`.
- `pairs`: the pairs the report says the resolver links by resolving
  (`resolved`) and by following names (`followed`), each checked against
  the hand graph, and each scored against SCIP. On a binary that links the
  resolved file, `followed` is the prediction of what following would give;
  on one that follows, `resolved` is the graph it replaced.
- `change`: `followed` against `resolved`, pair by pair: the pairs
  following removes and adds, and how many of each SCIP has, by a use.
- `scip_only_uses_by_class`: SCIP use pairs hand lacks, in this order:
  - `barrel, followed`: following the source's names reaches the target.
  - `barrel, not followed`: the source imports an `index.*` file that
    passes on the target (the `re-export` rule below), but following did not
    reach it; `barrel_not_followed_by_outcome` says why, from the report.
  - `unresolved relative import` / `unresolved workspace or alias import`:
    a specifier the resolver left unresolved names the target (its path, or
    the name of the workspace package the target sits in).
  - `ambient or global`: the target is a `.d.ts`, a script with no
    top-level `import`/`export`, or declares `declare global`, and nothing
    imports it: its names are global.
  - `inferred type`, `other`: as below.
  Each barrel class also counts the pairs reached only through `import
  type` statements (`type_only`).
- `hand_only_by_class`: `source not indexed` (the indexer never read the
  source, so SCIP has no pairs from it), `followed` (following added the
  pair and SCIP does not have it) and `other`.
- `weight_variants`: `ts-variants` rebuilds the dumped graph's static
  signal from the report two ways and the job partitions each through
  `tolmap build --graph`: `share` (one import's mass of 1 shared among the
  files it reaches, what the resolver does, so it must place 100% against
  the job's own map, which checks the construction) and `per file` (1 on
  each file an import reaches, what a direct import of each would weigh,
  the alternative finding 51 measured). Each is placed against the job's
  map, the committed fixture and the SCIP fixture.

For Python the row also carries `py` (finding 53), from the resolver's own
per-statement report (`TOLMAP_PY_IMPORT_REPORT`, written by `dump-graph
--refs hand`): the files each import statement linked before module objects
and ordinary-module re-exports were followed, and the files it links now.
- `module_object_outcomes`: each module a statement binds as an object
  (`from .. import util`, `import a.b as s`): `narrowed` (every `util.x`
  credited to another file), `partly`, `defined_here`, `uncertain`,
  `unused` (no attribute use), `value` (also used bare), `ambiguous`
  (bound more than once); the last four keep the module, as before.
- `before` / `after`: each pair set scored against SCIP, with its hand-only
  classes, from one run. `before`'s fingerprint should equal the committed
  baseline's `hand_fingerprint`, which checks that it is main's graph;
  `hand_is_after` checks `after` against the dumped graph.
- `change`: `after` against `before`, pair by pair, as TypeScript's.

For Rust the row carries `rs` (issue #126, finding 57). Rust has no
product indexer: rust-analyzer compiles a repository's build scripts and
proc macros natively, and without network it degrades to `--no-deps`
(finding 55), so it is the oracle here only, run by the job itself with
network on the pinned release, on the pins `rust-targets` prints: the Rust
map fixtures in data/fixtures.toml plus RUST_ORACLE_ONLY. From the
resolver's own report (`TOLMAP_RUST_IMPORT_REPORT`, written by `dump-graph
--refs hand`):
- `outcomes`: every `use` leaf and every written-out path by outcome:
  `defined` (followed to the defining file), `module` (names selected
  through an imported module), `uncertain` / `uncertain_glob` (the chain
  stopped; the module the path named is linked), `module_unselected`,
  `glob` (a `use x::*`, deliberately unresolved), `unused` (a private `use`
  of a name in this repository the file never names: a trait used for its
  methods, or a name only a macro expands to), `unused_external` (the same,
  of another crate's name), `unnamed` (`as _`), `glob_scope` (the head may
  come from a glob or the prelude), `external`, `unresolved` (`Self::`, a
  module no crate declares), `no_module` (a file no crate reaches); and
  how many hit a cfg tie, broken or kept.
- `scip_only_uses_by_class`: SCIP use pairs hand lacks, in this order:
  `member via value` (only methods or fields support the pair: a trait
  method or a method called on a value, finding 50's class), `macro` (every
  symbol SCIP names for the pair is a macro, `name!`: macros are
  deliberately unresolved), `glob import` (the source glob-imports the
  target's module or one above it), `inferred type` (two hand hops),
  `other`.
- `hand_only_by_class`: `uncertain module` (a chain the resolver could not
  follow linked the module it named), `other`.
Rust rows are gated like the others since the owner saw the numbers
(issue #126, AskUserQuestion "Gate Rust now", session 16030105, 2026-09-26).
UNGATED_LANGS stays as the switch for a future language's first rows.

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
import posixpath
import random
import re
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
SCIP_CLASSES = ("re-export", "inherited member", "inferred type", "same package", "member via value", "other")
TS_SCIP_CLASSES = ("barrel, followed", "barrel, not followed", "unresolved relative import",
                   "unresolved workspace or alias import", "ambient or global", "inferred type", "other")
TS_HAND_CLASSES = ("source not indexed", "followed", "other")
TS_OUTCOMES = ("defined_here", "followed", "partly_followed", "uncertain", "opaque", "no_names", "unresolved")
PY_OUTCOMES = ("narrowed", "partly", "defined_here", "uncertain", "unused", "value", "ambiguous")
RS_OUTCOMES = ("defined", "module", "uncertain", "uncertain_glob", "module_unselected", "glob", "unused",
               "unused_external", "unnamed", "glob_scope", "external", "unresolved", "no_module")
RS_SCIP_CLASSES = ("member via value", "macro", "glob import", "inferred type", "other")
RS_HAND_CLASSES = ("uncertain module", "other")
# Languages whose rows are reported but not gated: Rust, until the owner
# has seen its numbers (issue #126).
UNGATED_LANGS: tuple[str, ...] = ()
# Rust repositories scored against rust-analyzer that are not map fixtures:
# finding 55's ripgrep pin (tag 14.1.1, a small multi-crate workspace).
RUST_ORACLE_ONLY = {
    "ripgrep": {"url": "https://github.com/BurntSushi/ripgrep.git",
                "commit": "0e8390a66fbcf6eeac1aeb0541b367663a597c79", "pkg": ".", "lang": "rs"},
}
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


def fixture_table() -> dict[str, dict]:
    with (ROOT / "data" / "fixtures.toml").open("rb") as handle:
        return tomllib.load(handle)


def fixture_names() -> list[str]:
    return list(fixture_table())


def rust_targets(_args) -> int:
    """`name url commit pkg` for every Rust repository the job scores:
    the Rust map fixtures, then RUST_ORACLE_ONLY."""
    rows = {name: row for name, row in fixture_table().items() if row.get("lang") == "rs"}
    rows.update(RUST_ORACLE_ONLY)
    for name, row in rows.items():
        print(name, row["url"], row["commit"], row["pkg"])
    return 0


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


def classify_scip_only(lang: str, a: str, t: str, hand_targets: dict[str, set[str]], py: PythonFiles | None,
                       member_only: set[tuple[str, str]] = frozenset()) -> str:
    direct = hand_targets.get(a, set())
    if lang == "go":
        if directory(a) == directory(t):
            return "same package"
        if (a, t) in member_only and any(directory(b) == directory(t) for b in direct):
            return "member via value"
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
                   py: PythonFiles | None, ts_report: list[list] | None = None,
                   ts_sources: TsSources | None = None, py_report: list[list] | None = None,
                   rs_report: tuple[list[list], dict] | None = None) -> dict:
    lang_of = {n["file"]: n.get("lang") or hand_graph["lang"] for n in hand_graph["nodes"]}
    hand = {(a, b) for a, b, _ in hand_graph["imports"] if a != b and lang_of.get(a) == lang}
    scip = {(a, b) for a, b, *_ in ingest["file_edges"] if a != b and lang_of.get(a) == lang}
    # The ingest's fifth column: 1 when a non-namespace symbol supports the
    # pair (eval/scip_ingest.py `pair_uses`).
    scip_uses = {(row[0], row[1]) for row in ingest["file_edges"]
                 if row[0] != row[1] and lang_of.get(row[0]) == lang and (len(row) < 5 or row[4])}
    shared = hand & scip
    shared_uses = hand & scip_uses
    # A use pair some non-member symbol supports: the source names something
    # the target declares (the ingest's `member_only_use_pairs` are the rest).
    member_only = {(a, b) for a, b in ingest.get("member_only_use_pairs", [])}
    shared_named_uses = shared_uses - member_only
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
    rows_scip = [{"a": a, "b": t, "class": classify_scip_only(lang, a, t, hand_targets, py, member_only)}
                 for a, t in scip_only]

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
        "scip_use_pairs": len(scip_uses),
        "shared_uses": len(shared_uses),
        "recall_uses": ratio(len(shared_uses), len(scip_uses)),
        "precision_uses": ratio(len(shared_uses), len(hand)),
        "shared_named_uses": len(shared_named_uses) if "member_only_use_pairs" in ingest else None,
        "shared_namespace_only": len(shared - scip_uses),
        "shared_namespace_only_to_package": sum(
            1 for _, b in shared - scip_uses if is_package_file(b)),
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
        # SCIP use pairs whose target directory hand links from the source
        # but whose file it does not: what narrowing gave up (finding 50).
        gap = sorted((a, b) for a, b in scip_uses - hand if (a, directory(b)) in hd)
        named = [{"a": a, "b": b} for a, b in gap if (a, b) not in member_only]
        row["go"] = {"file_gap": {"use_pairs": len(gap), "member_only": len(gap) - len(named),
                                  "named": len(named), "named_samples": sample(named)},
                     "member_only_available": "member_only_use_pairs" in ingest}
    if lang == "rs":
        symbols = {(a, b): names for a, b, names in ingest.get("use_pair_symbols", [])}
        row["rs"] = score_rs(lang_of, hand, scip, scip_uses, hand_targets, rs_report, member_only, symbols)
    if lang == "py" and py_report is not None:
        row["py"] = score_py(lang_of, hand, scip, scip_uses, py_report, py, scip_targets)
    if lang == "ts":
        symbols = {(a, b): names for a, b, names in ingest.get("use_pair_symbols", [])}
        row["ts"] = score_ts(lang_of, hand, scip, scip_uses, hand_targets, ts_report,
                             ts_sources or TsSources(None), symbols, member_only,
                             set(ingest.get("unindexed_lang_files", [])))
    references = ((scip_graph or {}).get("references") or {}).get(lang)
    row["references"] = references
    if references and references.get("path") == "scip":
        admitted = {(a, b) for a, b, _ in scip_graph["imports"] if a != b and lang_of.get(a) == lang}
        row["oracle_check"] = "equal" if admitted == scip else (
            f"differs: {len(admitted - scip)} only in the product graph, {len(scip - admitted)} only in the oracle")
    else:
        row["oracle_check"] = "fallback (not comparable)" if references else "no SCIP graph"
    return row


# Why a name several files declare kept the whole package (finding 54).
GO_TIE_OUTCOMES = ("ambiguous_name", "excluded_only", "unknown_constraint", "importer_not_in_build")
GO_OUTCOMES = ("narrowed", "narrowed_by_build", "opaque", "no_names", "undeclared_name") + GO_TIE_OUTCOMES \
    + ("unknown_declarations",)


def go_import_outcomes(work: Path) -> dict | None:
    """Aggregate the resolver's own per-import report (finding 50): rows of
    [file, import, outcome, failing name, package files, files linked]."""
    reports = sorted(work.glob("go-imports.*.json"))
    if not reports:
        return None
    rows = [row for path in reports for row in load(path)]
    outcomes = Counter(row[2] for row in rows)
    names = {kind: Counter(row[3] for row in rows if row[2] == kind)
             for kind in ("undeclared_name",) + GO_TIE_OUTCOMES}
    builds = [row for path in sorted(work.glob("go-build.*.json")) for row in load(path)]
    return {
        "imports": len(rows),
        "by_outcome": dict(sorted(outcomes.items())),
        # Files linked, summed over imports: what the old whole-package link
        # had against what the narrowed one has.
        "links_before": sum(row[4] for row in rows),
        "links_after": sum(row[5] for row in rows),
        "top_names": {kind: [[n, c] for n, c in sorted(counter.items(), key=lambda x: (-x[1], x[0]))[:15]]
                      for kind, counter in names.items()},
        "samples": {kind: sample([{"a": r[0], "b": r[1], "name": r[3]} for r in rows if r[2] == kind])
                    for kind in sorted(outcomes) if not kind.startswith("narrowed")},
        # Finding 54: each Go file's status for the resolver's one target.
        "build": {
            "by_status": dict(sorted(Counter(status for _, status in builds).items())),
            "not_in": sorted([file, status] for file, status in builds if status != "in"),
        } if builds else None,
    }


def district_matches(candidate: dict, reference: dict) -> list[list]:
    """The parity gate's greedy best-Jaccard district matching (the rule in
    eval/remote_build_result.py `placement`), as [reference name, candidate
    name, Jaccard, reference size, candidate size] rows; an unmatched
    district has None on the other side. For the rename table a fixture
    re-derivation records."""
    def groups(document):
        out = defaultdict(set)
        for file, node in zip(document.get("F", []), document.get("N", [])):
            out[int(node[0])].add(file)
        return out

    def name(document, district):
        return (document.get("names") or {}).get(str(district), str(district))

    cand, ref = groups(candidate), groups(reference)
    common = set().union(*cand.values()) & set().union(*ref.values()) if cand and ref else set()
    cand = {k: v & common for k, v in cand.items()}
    ref = {k: v & common for k, v in ref.items()}
    pairs = sorted(((len(c & r) / len(c | r), ci, ri) for ci, c in cand.items() for ri, r in ref.items() if c & r),
                   reverse=True)
    used_c, used_r, rows = set(), set(), []
    for jaccard, ci, ri in pairs:
        if ci in used_c or ri in used_r or jaccard < 0.35:
            continue
        used_c.add(ci)
        used_r.add(ri)
        rows.append([name(reference, ri), name(candidate, ci), round(jaccard, 2), len(ref[ri]), len(cand[ci])])
    rows += [[name(reference, ri), None, None, len(ref[ri]), None] for ri in sorted(ref) if ri not in used_r]
    rows += [[None, name(candidate, ci), None, None, len(cand[ci])] for ci in sorted(cand) if ci not in used_c]
    return rows


# --- TypeScript (finding 51) ---

TS_TOP_LEVEL_MODULE = re.compile(r"^\s*(import|export)\b", re.MULTILINE)
TS_SUFFIXES = (".d.ts", ".tsx", ".ts", ".jsx", ".js", ".mts", ".cts", ".mjs", ".cjs")


def ts_stem(path: str) -> str:
    """`a/b.ts` -> `a/b`, `a/index.ts` -> `a/index`."""
    for suffix in TS_SUFFIXES:
        if path.endswith(suffix):
            return path[: -len(suffix)]
    return path


def load_ts_report(work: Path) -> list[list] | None:
    """The resolver's own rows: [file, specifier, outcome, name, type-only,
    resolved file or None, files the followed names reach]."""
    reports = sorted(work.glob("ts-imports.*.json"))
    if not reports:
        return None
    return [row for path in reports for row in load(path)]


class TsSources:
    """What the TypeScript classes read from the clone: the workspace
    package each file sits in, and whether a file's names are global."""

    def __init__(self, repo: Path | None):
        self.repo = repo
        self._package: dict[str, str | None] = {}
        self._global: dict[str, bool] = {}

    def package_name(self, directory_path: str) -> str | None:
        if directory_path in self._package:
            return self._package[directory_path]
        name = None
        if self.repo is not None:
            manifest = self.repo / directory_path / "package.json" if directory_path else self.repo / "package.json"
            try:
                name = json.loads(manifest.read_text()).get("name")
            except (OSError, ValueError, AttributeError):
                name = None
            if name is None and directory_path:
                name = self.package_name(directory(directory_path))
        self._package[directory_path] = name
        return name

    def is_global(self, path: str) -> bool:
        if path in self._global:
            return self._global[path]
        answer = path.endswith(".d.ts")
        if not answer and self.repo is not None:
            try:
                text = (self.repo / path).read_text(errors="replace")
            except OSError:
                text = None
            if text is not None:
                answer = "declare global" in text or not TS_TOP_LEVEL_MODULE.search(text)
        self._global[path] = answer
        return answer


def unresolved_names(a: str, specifier: str, t: str, sources: TsSources) -> str | None:
    """The class of an unresolved specifier of `a` that names `t`, if it does."""
    if specifier.startswith("."):
        base = posixpath.normpath(posixpath.join(directory(a), specifier))
        base = ts_stem(base)
        stem = ts_stem(t)
        if stem in (base, f"{base}/index") or t.startswith(base + "/"):
            return "unresolved relative import"
        return None
    package = sources.package_name(directory(t))
    if package and (specifier == package or specifier.startswith(package + "/")):
        return "unresolved workspace or alias import"
    return None


def score_ts(lang_of: dict, hand: set, scip: set, scip_uses: set, hand_targets: dict,
             report: list[list] | None, sources: TsSources, symbols: dict, member_only: set,
             unindexed: set) -> dict:
    """The TypeScript block of a row (finding 51); see the module docstring."""
    out: dict = {"report": report is not None}
    if report is None:
        return out
    rows = [r for r in report if lang_of.get(r[0]) == "ts"]
    by_file = defaultdict(list)
    for r in rows:
        by_file[r[0]].append(r)
    outcomes = Counter(r[2] for r in rows)
    uncertain_names = Counter(r[3] for r in rows if r[2] in ("uncertain", "partly_followed"))
    resolved = {(r[0], r[5]) for r in rows if r[5] and r[5] != r[0]}
    followed = {(r[0], t) for r in rows for t in r[6] if t != r[0]}

    def ratio(x, y):
        return round(x / y, 4) if y else None

    def scores(pairs):
        return {"pairs": len(pairs), "shared": len(pairs & scip), "shared_uses": len(pairs & scip_uses),
                "precision": ratio(len(pairs & scip), len(pairs)), "recall": ratio(len(pairs & scip), len(scip)),
                "precision_uses": ratio(len(pairs & scip_uses), len(pairs)),
                "recall_uses": ratio(len(pairs & scip_uses), len(scip_uses))}

    removed, added = resolved - followed, followed - resolved
    out["import_outcomes"] = {
        "imports": len(rows),
        "by_outcome": dict(sorted(outcomes.items())),
        "type_only": sum(1 for r in rows if r[4]),
        "type_only_by_outcome": dict(sorted(Counter(r[2] for r in rows if r[4]).items())),
        "top_uncertain_names": [[n, c] for n, c in sorted(uncertain_names.items(),
                                                           key=lambda x: (-x[1], x[0]))[:15]],
        "samples": {kind: sample([{"a": r[0], "b": r[1], "name": r[3], "resolved": r[5]}
                                  for r in rows if r[2] == kind])
                    for kind in sorted(outcomes) if kind not in ("defined_here", "followed")},
    }
    out["pairs"] = {"resolved": scores(resolved), "followed": scores(followed),
                    "hand_is_resolved": hand == resolved, "hand_is_followed": hand == followed}
    out["change"] = {
        "removed": len(removed), "removed_scip": len(removed & scip), "removed_scip_uses": len(removed & scip_uses),
        "added": len(added), "added_scip": len(added & scip), "added_scip_uses": len(added & scip_uses),
        "added_not_scip": len(added - scip),
        "removed_scip_uses_samples": sample([{"a": a, "b": b} for a, b in removed & scip_uses]),
        "added_not_scip_samples": sample([{"a": a, "b": b} for a, b in added - scip]),
    }

    classes, barrel_why, type_only, members, samples = [], Counter(), Counter(), Counter(), defaultdict(list)
    for a, t in sorted(scip_uses - hand):
        mine = by_file.get(a, [])
        why = None
        via = [r for r in mine if t in r[6] and r[5] != t]
        if via:
            why = "barrel, followed"
            if all(r[4] for r in via):
                type_only[why] += 1
        if why is None:
            via = [r for r in mine if r[5] and r[5] != t and reexports(r[5], t, hand_targets)]
            if via:
                why = "barrel, not followed"
                barrel_why[via[0][2]] += 1
                if all(r[4] for r in via):
                    type_only[why] += 1
        if why is None:
            for r in mine:
                if r[2] == "unresolved":
                    why = unresolved_names(a, r[1], t, sources)
                    if why:
                        break
        if why is None and sources.is_global(t) and not any(r[5] == t for r in mine):
            why = "ambient or global"
        if why is None and any(t in hand_targets.get(c, ()) for c in hand_targets.get(a, ())):
            why = "inferred type"
        why = why or "other"
        classes.append(why)
        # Reached only through a method or field (`Type#member`): a value's
        # member, which the source never names (finding 50's Go class).
        members[why] += (a, t) in member_only
        samples[why].append({"a": a, "b": t, "symbols": symbols.get((a, t), []),
                             "imports": sorted({r[1] for r in mine if r[5] == t or t in r[6]
                                                or (r[5] and under(t, r[5]))})[:4]})
    out["scip_only_uses"] = len(classes)
    out["scip_only_uses_by_class"] = {c: classes.count(c) for c in TS_SCIP_CLASSES}
    out["barrel_not_followed_by_outcome"] = dict(sorted(barrel_why.items()))
    out["type_only"] = dict(sorted(type_only.items()))
    out["member_only"] = {c: members[c] for c in TS_SCIP_CLASSES}
    out["scip_only_uses_samples"] = {c: sample(samples[c]) for c in TS_SCIP_CLASSES}

    hand_only = sorted(hand - scip)
    hand_classes = ["source not indexed" if a in unindexed else "followed" if (a, b) in added else "other"
                    for a, b in hand_only]
    out["hand_only_by_class"] = {c: hand_classes.count(c) for c in TS_HAND_CLASSES}
    out["hand_only_samples"] = {c: sample([{"a": a, "b": b} for (a, b), k in zip(hand_only, hand_classes) if k == c])
                                for c in TS_HAND_CLASSES}
    return out


TS_VARIANTS = ("share", "per-file")


def load_rs_report(work: Path) -> tuple[list[list], dict] | None:
    """The Rust resolver's rows (finding 57): [file, "use" | "extern_crate"
    | "path", path, local name, outcome, cfg tie ("kept" | "broken" |
    null), files linked, inline-module scope], and each file's module path."""
    reports = sorted(work.glob("rust-imports.*.json"))
    if not reports:
        return None
    rows = [row for path in reports for row in load(path)]
    modules = {}
    for path in sorted(work.glob("rust-modules.*.json")):
        modules.update({file: module for file, module in load(path)})
    return rows, modules


def rs_absolute(segments: list[str], own: str) -> str | None:
    """A `use` path made absolute from the module it is written in, for
    the classifier: `crate`, `self` and `super` only (a leading crate name
    or local item is not told apart here)."""
    parts = own.split("::")
    head, rest = segments[0], segments[1:]
    if head == "crate":
        path = parts[:1]
    elif head == "self":
        path = parts
    elif head == "super":
        if len(parts) < 2:
            return None
        path = parts[:-1]
    else:
        return None
    for segment in rest:
        if segment == "super":
            if len(path) < 2:
                return None
            path = path[:-1]
        else:
            path = path + [segment]
    return "::".join(path)


def score_rs(lang_of: dict, hand: set, scip: set, scip_uses: set, hand_targets: dict,
             report: tuple[list[list], dict] | None, member_only: set,
             symbols: dict | None = None) -> dict:
    """Finding 57's block: the resolver's outcomes and the pairs one side
    has alone, by class."""
    if report is None:
        return {"report": False}
    rows, modules = report
    rows = [r for r in rows if lang_of.get(r[0]) == "rs"]
    outcomes = {"use": Counter(), "path": Counter()}
    ties = {"use": Counter(), "path": Counter()}
    for r in rows:
        kind = "path" if r[1] == "path" else "use"
        outcomes[kind][r[4]] += 1
        if r[5]:
            ties[kind][r[5]] += 1
    globs = defaultdict(set)
    for r in rows:
        if r[1] == "use" and r[4] == "glob" and modules.get(r[0]):
            own = modules[r[0]] + ("::" + r[7] if len(r) > 7 and r[7] else "")
            target = rs_absolute(r[2].split("::"), own)
            if target:
                globs[r[0]].add(target)
    uncertain = {(r[0], t) for r in rows if r[4].startswith("uncertain") for t in r[6]}

    def scip_class(a: str, t: str) -> str:
        if (a, t) in member_only:
            return "member via value"
        names = (symbols or {}).get((a, t))
        if names and all(name.endswith("!") for name in names):
            return "macro"
        module = modules.get(t)
        if module and any(module == g or module.startswith(g + "::") for g in globs.get(a, ())):
            return "glob import"
        if any(t in hand_targets.get(c, ()) for c in hand_targets.get(a, ())):
            return "inferred type"
        return "other"

    missing = [{"a": a, "b": t, "class": scip_class(a, t), "symbols": (symbols or {}).get((a, t), [])}
               for a, t in sorted(scip_uses - hand)]
    hand_only = [{"a": a, "b": b, "class": "uncertain module" if (a, b) in uncertain else "other"}
                 for a, b in sorted(hand - scip)]
    return {
        "report": True,
        "files_in_a_crate": sum(1 for f, lang in lang_of.items() if lang == "rs" and modules.get(f)),
        "files": sum(1 for lang in lang_of.values() if lang == "rs"),
        "outcomes": {k: {o: v.get(o, 0) for o in RS_OUTCOMES} for k, v in outcomes.items()},
        "ties": {k: dict(sorted(v.items())) for k, v in ties.items()},
        "scip_only_uses": len(missing),
        "scip_only_uses_by_class": {c: sum(1 for m in missing if m["class"] == c) for c in RS_SCIP_CLASSES},
        "scip_only_uses_samples": {c: sample([m for m in missing if m["class"] == c]) for c in RS_SCIP_CLASSES},
        "hand_only_by_class": {c: sum(1 for h in hand_only if h["class"] == c) for c in RS_HAND_CLASSES},
        "hand_only_samples": {c: sample([h for h in hand_only if h["class"] == c]) for c in RS_HAND_CLASSES},
    }


def load_py_report(work: Path) -> list[list] | None:
    """The Python resolver's own rows (finding 53): [file, statement index,
    files linked before module objects and ordinary-module re-exports were
    followed, files linked now, module objects: [local name, module file,
    outcome, attributes used, attributes whose chain was uncertain]]."""
    reports = sorted(work.glob("py-imports.*.json"))
    if not reports:
        return None
    return [row for path in reports for row in load(path)]


def score_py(lang_of: dict, hand: set, scip: set, scip_uses: set, report: list[list] | None,
             py: PythonFiles | None, scip_targets: dict) -> dict:
    """Finding 53's block: both graphs of one binary, from its report.

    `before` is what main's resolver linked (packages only, no module
    objects), `after` what this one links. Each is checked, and scored
    against SCIP here, so the before and after numbers come from one run
    and one oracle. `before_fingerprint` should equal the committed
    baseline's `hand_fingerprint` (main's graph): that is the check that
    `before` really is main's resolution.
    """
    if report is None:
        return {"report": False}
    before = {(r[0], t) for r in report for t in r[2] if lang_of.get(r[0]) == "py" and t != r[0]}
    after = {(r[0], t) for r in report for t in r[3] if lang_of.get(r[0]) == "py" and t != r[0]}

    def ratio(x, y):
        return round(x / y, 4) if y else None

    def scores(pairs):
        targets = defaultdict(set)
        for a, b in pairs:
            targets[a].add(b)
        classes = Counter(classify_hand_only("py", a, b, scip_targets[a], targets, py)[0]
                          for a, b in sorted(pairs - scip))
        return {"pairs": len(pairs), "shared": len(pairs & scip), "shared_uses": len(pairs & scip_uses),
                "recall": ratio(len(pairs & scip), len(scip)), "precision": ratio(len(pairs & scip), len(pairs)),
                "recall_uses": ratio(len(pairs & scip_uses), len(scip_uses)),
                "precision_uses": ratio(len(pairs & scip_uses), len(pairs)),
                "hand_only_by_class": {c: classes.get(c, 0) for c in HAND_CLASSES},
                "fingerprint": fingerprint(pairs)}

    removed, added = before - after, after - before
    objects = [o for r in report if lang_of.get(r[0]) == "py" for o in r[4]]
    outcomes = Counter(o[2] for o in objects)
    uncertain = Counter(name for o in objects for name in o[4])
    changed = [r for r in report if lang_of.get(r[0]) == "py"
               and set(r[2]) - {r[0]} != set(r[3]) - {r[0]}]
    by_object = sum(1 for r in changed if any(o[2] in ("narrowed", "partly") for o in r[4]))
    return {
        "report": True,
        "statements": sum(1 for r in report if lang_of.get(r[0]) == "py"),
        "statements_changed": len(changed),
        "statements_changed_by_a_module_object": by_object,
        "statements_changed_by_names_only": len(changed) - by_object,
        "module_objects": len(objects),
        "module_object_outcomes": {k: outcomes.get(k, 0) for k in PY_OUTCOMES},
        "uncertain_attributes_top": uncertain.most_common(SAMPLE),
        "hand_is_after": "equal" if after == hand else (
            f"differs: {len(after - hand)} only in the report, {len(hand - after)} only in the graph"),
        "before": scores(before),
        "after": scores(after),
        "change": {
            "removed": len(removed), "removed_scip": len(removed & scip),
            "removed_scip_uses": len(removed & scip_uses),
            "removed_to_package": sum(1 for _, b in removed if b.endswith("__init__.py")),
            "added": len(added), "added_scip": len(added & scip), "added_scip_uses": len(added & scip_uses),
            "added_not_scip": len(added - scip),
            "removed_scip_uses_samples": sample([{"a": a, "b": b} for a, b in removed & scip_uses]),
            "added_not_scip_samples": sample([{"a": a, "b": b} for a, b in added - scip]),
        },
    }


def ts_variants(args) -> int:
    """Write `variants/{share,per-file}.graph.json` for a TypeScript fixture:
    the dumped hand graph with its static signal recomputed from the
    resolver's report (see the module docstring, `weight_variants`)."""
    work = args.work
    report = load_ts_report(work)
    if report is None:
        print("no TypeScript report; nothing to write")
        return 0
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from scip_churn import Graph, assemble
    graph = Graph(load(work / "hand.graph.json"))
    per_file, share = defaultdict(float), defaultdict(float)
    for row in report:
        a, targets = row[0], row[6]
        if a not in graph.index or not targets:
            continue
        for t in targets:
            if t == a or t not in graph.index:
                continue
            # src/extract.rs `parse_multi`: static edges are undirected, and
            # the share divides by every file the import reaches, the source
            # included.
            per_file[graph.key(a, t)] += 1.0
            share[graph.key(a, t)] += 1.0 / len(targets)
    out = work / "variants"
    out.mkdir(parents=True, exist_ok=True)
    for name, static in (("share", share), ("per-file", per_file)):
        document = assemble(graph, graph, dict(static), graph.raw["imports"])
        (out / f"{name}.graph.json").write_text(json.dumps(document, separators=(",", ":")))
    print(f"wrote {len(TS_VARIANTS)} variants: {len(share)} static pairs")
    return 0


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
        row = score_language(args.name, lang, hand_graph, scip_graph, load(ingest_path), py,
                             load_ts_report(work) if lang == "ts" else None, TsSources(args.repo),
                             load_py_report(work) if lang == "py" else None,
                             load_rs_report(work) if lang == "rs" else None)
        if lang == "go":
            row["go"]["import_outcomes"] = go_import_outcomes(work)
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
        fixture = load(committed)
        fraction, delta = placement(built, fixture)
        # Agreement with the SCIP fixture (data/scip, the `--refs scip` map
        # at the same pin), for this hand map and for the committed hand
        # fixture: whether a resolver change moves hand's districts toward
        # the oracle's or away from them (finding 51). Placement, not
        # correctness, as everywhere else.
        scip_summary = ROOT / "data" / "scip" / f"{args.name}.json"
        towards = None
        if scip_summary.is_file():
            from scip_fixtures import summary_as_map
            oracle = summary_as_map(load(scip_summary))
            now, now_delta = placement(built, oracle)
            was, was_delta = placement(fixture, oracle)
            towards = {"fixture": round(was, 4), "fixture_q_delta": round(was_delta, 4),
                       "hand_map": round(now, 4), "hand_map_q_delta": round(now_delta, 4)}
        variants = {}
        for variant in TS_VARIANTS:
            path = work / "variants" / "maps" / f"{args.name}.{variant}.json"
            if not path.is_file():
                continue
            candidate = load(path)
            entry = {"q": candidate["q"], "districts": len({int(n[0]) for n in candidate["N"]})}
            for label, reference in (("vs_hand_map", built), ("vs_fixture", fixture)) + (
                    (("vs_scip_fixture", oracle),) if towards else ()):
                placed, q_delta = placement(candidate, reference)
                entry[label] = [round(placed, 4), round(q_delta, 4)]
            variants[variant] = entry
        for row in rows:
            if variants and row.get("ts") is not None:
                row["ts"]["weight_variants"] = variants
            row["hand_map_vs_fixture"] = {"placement": round(fraction, 4), "q_delta": round(delta, 4),
                                          "q": built["q"], "districts": len({int(n[0]) for n in built["N"]}),
                                          "edges": len(built.get("E", [])),
                                          "districts_matched": district_matches(built, fixture)}
            row["placement_vs_scip_fixture"] = towards
    args.out.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
    brief = ("name", "lang", "status", "hand_pairs", "scip_pairs", "shared", "recall", "precision",
             "hand_only_by_class", "scip_only_by_class", "oracle_check", "hand_map_vs_fixture",
             "placement_vs_scip_fixture")
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
    out += ["", "### Placement against the SCIP fixture (data/scip)", "",
            "| fixture | committed hand fixture | this hand map |", "|---|---|---|"]
    for r in scored:
        v = r.get("placement_vs_scip_fixture")
        if v and r is next(x for x in scored if x["name"] == r["name"]):
            out.append(f"| {r['name']} | {100 * v['fixture']:.1f}%, Δq {v['fixture_q_delta']:.4f} "
                       f"| {100 * v['hand_map']:.1f}%, Δq {v['hand_map_q_delta']:.4f} |")
    out += ["", "### Against SCIP's use pairs (namespace-only pairs set aside)", "",
            "| fixture | lang | SCIP use pairs | shared by a use | recall (uses) | precision (uses) | confirmed only by a namespace (to a package) |",
            "|---|---|---:|---:|---:|---:|---:|"]
    for r in scored:
        out.append(f"| {r['name']} | {r['lang']} | {r['scip_use_pairs']:,} | {r['shared_uses']:,} "
                   f"| {pct(r['recall_uses'])} | {pct(r['precision_uses'])} "
                   f"| {r['shared_namespace_only']:,} ({r['shared_namespace_only_to_package']:,}) |")
    go_rows = [r for r in scored if r.get("go")]
    if go_rows:
        out += ["", "### Go imports (findings 50 and 54)", "",
                "| fixture | imports | " + " | ".join(k.replace("_", " ") for k in GO_OUTCOMES)
                + " | files linked, whole package → narrowed | SCIP use pairs given up (member only / named) "
                "| files in / out / unknown build |",
                "|---|---:|" + "---:|" * len(GO_OUTCOMES) + "---|---|---|"]
        for r in go_rows:
            o = r["go"].get("import_outcomes") or {}
            by = o.get("by_outcome", {})
            gap = r["go"]["file_gap"]
            links = f"{o['links_before']:,} → {o['links_after']:,}" if o else "—"
            build = (o.get("build") or {}).get("by_status")
            builds = " / ".join(f"{build.get(k, 0):,}" for k in ("in", "out", "unknown")) if build else "—"
            out.append(f"| {r['name']} | {o.get('imports', 0):,} | "
                       + " | ".join(f"{by.get(k, 0):,}" for k in GO_OUTCOMES)
                       + f" | {links} | {gap['use_pairs']:,} ({gap['member_only']:,} / {gap['named']:,}) "
                       f"| {builds} |")
    ts_rows = [r for r in scored if (r.get("ts") or {}).get("report")]
    if ts_rows:
        out += ["", "### TypeScript imports (finding 51)", "",
                "| fixture | specifiers | " + " | ".join(TS_OUTCOMES) + " | type-only |",
                "|---|---:|" + "---:|" * (len(TS_OUTCOMES) + 1)]
        for r in ts_rows:
            o = r["ts"]["import_outcomes"]
            out.append(f"| {r['name']} | {o['imports']:,} | "
                       + " | ".join(f"{o['by_outcome'].get(k, 0):,}" for k in TS_OUTCOMES)
                       + f" | {o['type_only']:,} |")
        out += ["", "| fixture | pairs | hand pairs | shared by a use | precision (uses) | recall (uses) "
                "| precision | recall | equals hand |", "|---|---|---:|---:|---:|---:|---:|---:|---|"]
        for r in ts_rows:
            p = r["ts"]["pairs"]
            for key, label in (("resolved", "resolved (linking the module)"),
                               ("followed", "followed (linking the definitions)")):
                x = p[key]
                out.append(f"| {r['name']} | {label} | {x['pairs']:,} | {x['shared_uses']:,} "
                           f"| {pct(x['precision_uses'])} | {pct(x['recall_uses'])} | {pct(x['precision'])} "
                           f"| {pct(x['recall'])} | {p['hand_is_' + key]} |")
        out += ["", "| fixture | removed | removed, SCIP has | removed, SCIP has by a use | added "
                "| added, SCIP has by a use | added, SCIP lacks |", "|---|---:|---:|---:|---:|---:|---:|"]
        for r in ts_rows:
            c = r["ts"]["change"]
            out.append(f"| {r['name']} | {c['removed']:,} | {c['removed_scip']:,} | {c['removed_scip_uses']:,} "
                       f"| {c['added']:,} | {c['added_scip_uses']:,} | {c['added_not_scip']:,} |")
        out += ["", "| fixture | SCIP use pairs hand lacks | " + " | ".join(TS_SCIP_CLASSES) + " |",
                "|---|---:|" + "---:|" * len(TS_SCIP_CLASSES)]
        for r in ts_rows:
            t = r["ts"]
            out.append(f"| {r['name']} | {t['scip_only_uses']:,} | "
                       + " | ".join(f"{t['scip_only_uses_by_class'][c]:,}" for c in TS_SCIP_CLASSES) + " |")
            out.append(f"| {r['name']}, member only | {sum(t['member_only'].values()):,} | "
                       + " | ".join(f"{t['member_only'][c]:,}" for c in TS_SCIP_CLASSES) + " |")
        variant_rows = [(r, v, x) for r in ts_rows for v, x in (r["ts"].get("weight_variants") or {}).items()]
        if variant_rows:
            out += ["", "| fixture | static weight | districts | q | vs this job's map | vs committed fixture "
                    "| vs SCIP fixture |", "|---|---|---:|---:|---|---|---|"]
            for r, v, x in variant_rows:
                cell = lambda key: f"{100 * x[key][0]:.1f}%, Δq {x[key][1]:.4f}" if key in x else "—"
                out.append(f"| {r['name']} | {v} | {x['districts']} | {x['q']:.4f} | {cell('vs_hand_map')} "
                           f"| {cell('vs_fixture')} | {cell('vs_scip_fixture')} |")
        out += ["", "| fixture | hand-only | " + " | ".join(TS_HAND_CLASSES) + " |",
                "|---|---:|" + "---:|" * len(TS_HAND_CLASSES)]
        for r in ts_rows:
            h = r["ts"]["hand_only_by_class"]
            out.append(f"| {r['name']} | {sum(h.values()):,} | " + " | ".join(f"{h[c]:,}" for c in TS_HAND_CLASSES)
                       + " |")
    py_rows = [r for r in scored if (r.get("py") or {}).get("report")]
    if py_rows:
        out += ["", "### Python module objects and module re-exports (finding 53)", "",
                "| fixture | statements | changed (module object / names only) | module objects | "
                + " | ".join(PY_OUTCOMES) + " | equals hand |",
                "|---|---:|---|---:|" + "---:|" * len(PY_OUTCOMES) + "---|"]
        for r in py_rows:
            x = r["py"]
            out.append(f"| {r['name']} | {x['statements']:,} | {x['statements_changed']:,} "
                       f"({x['statements_changed_by_a_module_object']:,} / {x['statements_changed_by_names_only']:,}) "
                       f"| {x['module_objects']:,} | "
                       + " | ".join(f"{x['module_object_outcomes'][k]:,}" for k in PY_OUTCOMES)
                       + f" | {x['hand_is_after']} |")
        out += ["", "| fixture | graph | hand pairs | shared by a use | recall (uses) | precision (uses) "
                "| recall | precision | submodule via package | re-export | other | fingerprint |",
                "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|"]
        for r in py_rows:
            for key in ("before", "after"):
                x = r["py"][key]
                h = x["hand_only_by_class"]
                out.append(f"| {r['name']} | {key} | {x['pairs']:,} | {x['shared_uses']:,} "
                           f"| {pct(x['recall_uses'])} | {pct(x['precision_uses'])} | {pct(x['recall'])} "
                           f"| {pct(x['precision'])} | {h['submodule via package']:,} | {h['re-export']:,} "
                           f"| {h['other']:,} | {x['fingerprint'][:12]} |")
        out += ["", "| fixture | removed | removed, SCIP has | removed, SCIP has by a use | removed, to a package "
                "| added | added, SCIP has by a use | added, SCIP lacks |", "|---|---:|---:|---:|---:|---:|---:|---:|"]
        for r in py_rows:
            c = r["py"]["change"]
            out.append(f"| {r['name']} | {c['removed']:,} | {c['removed_scip']:,} | {c['removed_scip_uses']:,} "
                       f"| {c['removed_to_package']:,} | {c['added']:,} | {c['added_scip_uses']:,} "
                       f"| {c['added_not_scip']:,} |")
    rs_rows = [r for r in scored if (r.get("rs") or {}).get("report")]
    if rs_rows:
        out += ["", "### Rust (finding 57; reported, not gated)", "",
                "| fixture | kind | " + " | ".join(o.replace("_", " ") for o in RS_OUTCOMES) + " | cfg ties broken / kept |",
                "|---|---|" + "---:|" * len(RS_OUTCOMES) + "---|"]
        for r in rs_rows:
            x = r["rs"]
            for kind in ("use", "path"):
                t = x["ties"].get(kind, {})
                out.append(f"| {r['name']} | {kind} | " + " | ".join(f"{x['outcomes'][kind][o]:,}" for o in RS_OUTCOMES)
                           + f" | {t.get('broken', 0):,} / {t.get('kept', 0):,} |")
        out += ["", "| fixture | files (in a crate) | SCIP use pairs hand lacks | " + " | ".join(RS_SCIP_CLASSES)
                + " | hand-only | " + " | ".join(RS_HAND_CLASSES) + " |",
                "|---|---|---:|" + "---:|" * len(RS_SCIP_CLASSES) + "---:|" + "---:|" * len(RS_HAND_CLASSES)]
        for r in rs_rows:
            x = r["rs"]
            out.append(f"| {r['name']} | {x['files']:,} ({x['files_in_a_crate']:,}) | {x['scip_only_uses']:,} | "
                       + " | ".join(f"{x['scip_only_uses_by_class'][c]:,}" for c in RS_SCIP_CLASSES)
                       + f" | {sum(x['hand_only_by_class'].values()):,} | "
                       + " | ".join(f"{x['hand_only_by_class'][c]:,}" for c in RS_HAND_CLASSES) + " |")
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
    table = fixture_table()
    # Scored repositories that are not map fixtures (RUST_ORACLE_ONLY) follow
    # the fixtures, in name order.
    names = list(table) + sorted(p.name for p in args.work.iterdir()
                                 if (p / "score.json").is_file() and p.name not in table)
    for name in names:
        path = args.work / name / "score.json"
        if not path.is_file():
            lang = (table.get(name) or {}).get("lang", "?")
            rows.append({"name": name, "lang": lang, "status": "not scored"})
            continue
        rows.extend(load(path))
    args.out.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
    text = markdown(rows)
    print(text)
    if args.markdown:
        args.markdown.write_text(text)
    # An ungated language failing to score is reported, not a failure.
    return 0 if all(r.get("status") == "scored" or r.get("lang") in UNGATED_LANGS for r in rows) else 1


BASELINE_FIELDS = ("name", "lang", "files", "hand_pairs", "scip_pairs", "shared", "recall", "precision",
                   "precision_counting_reexports", "scip_use_pairs", "shared_uses", "recall_uses",
                   "precision_uses", "shared_named_uses", "shared_namespace_only",
                   "shared_namespace_only_to_package",
                   "hand_only", "scip_only", "hand_only_by_class",
                   "scip_only_by_class", "scip_fingerprint", "hand_fingerprint", "by_directory", "go", "ts",
                   "rs")


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
    - Pairs confirmed by a use may only go up: `shared_uses` may not fall.
      Hand losing a pair SCIP confirms by a use is the lower bound
      shrinking. A pair SCIP has only through a namespace symbol is the
      import statement naming a package, not a use of anything the
      package's `__init__` defines; gating on it would block removing the
      very over-attribution the second check asks to go down. (This was
      learned the hard way: the first version gated all of `shared`, and
      the package fix's first run tripped it on 85 celery pairs, every one
      of them namespace-only. `shared` is still reported.)
    For Go the third check holds `shared_named_uses` instead, once both
    the baseline and the run have it: the shared use pairs some non-member
    symbol supports, where the importer names what the target declares.
    Go's resolver links an import to the files declaring the names the
    importer selects (finding 50); a pair SCIP supports only through a
    method or field reached on a value (`x := pkg.New(); x.Run()`) is one
    no syntax-level resolver can name, and the whole-package link held it
    only by linking every file. This was also learned from a run: finding
    50's first run lost exactly 24 such pairs on prometheus (1,279 → 1,255
    by a use), all member-only, and none named. `shared_uses` is still
    reported. Python and TypeScript keep the `shared_uses` gate: their
    resolvers link by import statement, so a member-only pair there is
    still an import the file makes.
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
        if key[1] in UNGATED_LANGS:
            # Recorded so a later change can be compared, not gated until
            # the owner has seen the numbers (issue #126).
            if r is None or r.get("status") != "scored":
                print(f"REPORT {label}: not scored (not gated)")
            else:
                print(f"REPORT {label} (not gated): recall {pct(b['recall'])} → {pct(r['recall'])}, "
                      f"precision {pct(b['precision'])} → {pct(r['precision'])}, "
                      f"recall (uses) {pct(b['recall_uses'])} → {pct(r['recall_uses'])}, "
                      f"precision (uses) {pct(b['precision_uses'])} → {pct(r['precision_uses'])}, "
                      f"SCIP fingerprint {'unchanged' if r['scip_fingerprint'] == b['scip_fingerprint'] else 'changed'}")
            continue
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
        held = "shared_uses"
        if key[1] == "go" and b.get("shared_named_uses") is not None \
                and r.get("shared_named_uses") is not None:
            held = "shared_named_uses"
        what = "pairs confirmed by a named use" if held == "shared_named_uses" else "pairs confirmed by a use"
        if r[held] < b[held]:
            problems.append(f"{what} fell {b[held]} → {r[held]}")
        elif r[held] > b[held]:
            improved.append(f"{label}: {what} {b[held]} → {r[held]}")
        print(f"{'FAIL' if problems else 'PASS'} {label}: recall {pct(b['recall'])} → {pct(r['recall'])}, "
              f"precision {pct(b['precision'])} → {pct(r['precision'])}, "
              f"recall (uses) {pct(b['recall_uses'])} → {pct(r['recall_uses'])}, "
              f"precision (uses) {pct(b['precision_uses'])} → {pct(r['precision_uses'])}, "
              f"shared {b['shared']} → {r['shared']} (by a use {b['shared_uses']} → {r['shared_uses']}), "
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
        # One pair SCIP has only through a namespace symbol (the import
        # statement naming the module); every other pair is a use.
        namespace = {("pkg/cli.py", "pkg/util.py")}
        edges = [p + [1, 1, 0 if tuple(p) in namespace else 1] for p in scip]
        row = score_language("synthetic", "py", graph, None, {"file_edges": edges}, py)
        by_pair = {(r["a"], r["b"]): r["class"] for r in row["hand_only_pairs"]}
        # `from . import util` + `from . import Engine`: the statement naming
        # Engine is a re-export; util alone would be `submodule via package`.
        assert by_pair == {
            ("pkg/__init__.py", "pkg/_api.py"): "star import",
            ("pkg/cli.py", "pkg/__init__.py"): "re-export",
            ("pkg/star.py", "pkg/__init__.py"): "star import",
        }, by_pair
        assert row["scip_only_by_class"] == {"re-export": 0, "inherited member": 1, "inferred type": 1,
                                             "same package": 0, "member via value": 0,
                                             "other": 0}, row["scip_only_by_class"]
        assert (row["hand_pairs"], row["scip_pairs"], row["shared"]) == (8, 7, 5), row
        assert row["recall"] == round(5 / 7, 4) and row["precision"] == round(5 / 8, 4), row
        assert (row["scip_use_pairs"], row["shared_uses"], row["shared_namespace_only"]) == (6, 4, 1), row
        assert row["shared_namespace_only_to_package"] == 0, row
        # Finding 53's report: `after` is the graph; `before` differs on one
        # statement, whose name used to reach types.py and now core.py.
        py_report = [
            ["pkg/__init__.py", 0, ["pkg/core.py"], ["pkg/core.py"], []],
            ["pkg/__init__.py", 1, ["pkg/_api.py"], ["pkg/_api.py"], []],
            ["pkg/core.py", 0, ["pkg/base.py"], ["pkg/base.py"], []],
            ["pkg/base.py", 0, ["pkg/types.py"], ["pkg/types.py"], []],
            ["pkg/cli.py", 0, ["pkg/util.py"], ["pkg/util.py"], [["util", "pkg/util.py", "defined_here", 1, []]]],
            ["pkg/cli.py", 1, ["pkg/__init__.py"], ["pkg/__init__.py"], []],
            ["pkg/cli.py", 2, ["pkg/types.py"], ["pkg/core.py"], []],
            ["pkg/star.py", 0, ["pkg/__init__.py", "pkg/star.py"], ["pkg/__init__.py"], []],
        ]
        x = score_language("synthetic", "py", graph, None, {"file_edges": edges}, py,
                           py_report=py_report)["py"]
        assert x["hand_is_after"] == "equal", x["hand_is_after"]
        assert x["after"]["fingerprint"] == row["hand_fingerprint"], x["after"]
        assert (x["before"]["pairs"], x["before"]["shared_uses"], x["after"]["shared_uses"]) == (8, 3, 4), x
        change = {k: v for k, v in x["change"].items() if not k.endswith("samples")}
        assert change == {"removed": 1, "removed_scip": 0, "removed_scip_uses": 0, "removed_to_package": 0,
                          "added": 1, "added_scip": 1, "added_scip_uses": 1, "added_not_scip": 0}, change
        assert (x["statements"], x["statements_changed"], x["statements_changed_by_names_only"]) == (8, 1, 1), x
        assert x["module_object_outcomes"]["defined_here"] == 1 and x["module_objects"] == 1, x

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
        # Go, finding 50: hand links a/x.go to b/one.go only; SCIP also has
        # b/two.go through a method (member only) and b/three.go through a
        # package-level name, and a/y.go in a/x.go's own package.
        go_graph = {"lang": "go", "nodes": [{"file": f, "lang": "go"} for f in
                                            ("a/x.go", "a/y.go", "b/one.go", "b/three.go", "b/two.go")],
                    "imports": [["a/x.go", "b/one.go", 1.0]]}
        go_ingest = {"file_edges": [["a/x.go", "a/y.go", 1, 1, 1], ["a/x.go", "b/one.go", 1, 1, 1],
                                    ["a/x.go", "b/three.go", 1, 1, 1], ["a/x.go", "b/two.go", 1, 1, 1]],
                     "member_only_use_pairs": [["a/x.go", "b/two.go"]]}
        go_row = score_language("synthetic", "go", go_graph, None, go_ingest, None)
        assert go_row["scip_only_by_class"] == {"re-export": 0, "inherited member": 0, "inferred type": 0,
                                                "same package": 1, "member via value": 1,
                                                "other": 1}, go_row["scip_only_by_class"]
        gap = go_row["go"]["file_gap"]
        assert (gap["use_pairs"], gap["member_only"], gap["named"]) == (2, 1, 1), gap
        (repo / "go-imports.root.json").write_text(json.dumps([
            ["a/x.go", "m/b", "narrowed", None, 3, 1], ["a/y.go", "m/b", "undeclared_name", "Run", 3, 3],
            ["a/y.go", "m/c", "opaque", None, 2, 2], ["a/z.go", "m/c", "narrowed_by_build", None, 2, 1],
            ["a/z_windows.go", "m/c", "importer_not_in_build", "Open", 2, 2]]))
        (repo / "go-build.root.json").write_text(json.dumps([
            ["a/x.go", "in"], ["a/z_windows.go", "out"], ["m/c/e.go", "unknown"]]))
        outcomes = go_import_outcomes(repo)
        assert outcomes["by_outcome"] == {"importer_not_in_build": 1, "narrowed": 1, "narrowed_by_build": 1,
                                          "opaque": 1, "undeclared_name": 1}, outcomes
        assert (outcomes["links_before"], outcomes["links_after"]) == (12, 9), outcomes
        assert outcomes["top_names"]["undeclared_name"] == [["Run", 1]], outcomes
        assert outcomes["top_names"]["importer_not_in_build"] == [["Open", 1]], outcomes
        assert "narrowed_by_build" not in outcomes["samples"], outcomes
        assert outcomes["build"] == {"by_status": {"in": 1, "out": 1, "unknown": 1},
                                     "not_in": [["a/z_windows.go", "out"], ["m/c/e.go", "unknown"]]}, outcomes

        # TypeScript (finding 51): a barrel `src/index.ts` passing on `a.ts`
        # by name and `b.ts` by `export *`, an unresolved relative import, a
        # global declaration file and a two-hop type.
        ts_files = ("src/a.ts", "src/b.ts", "src/bench.ts", "src/c.ts", "src/index.ts", "src/missing.ts", "src/other.ts",
                    "src/use.ts", "types/global.d.ts")
        ts_graph = {"lang": "ts", "nodes": [{"file": f, "lang": "ts"} for f in ts_files],
                    "imports": [["src/use.ts", "src/index.ts", 1.0], ["src/index.ts", "src/a.ts", 1.0],
                                ["src/index.ts", "src/b.ts", 1.0], ["src/other.ts", "src/c.ts", 1.0],
                                ["src/c.ts", "src/a.ts", 1.0], ["src/bench.ts", "src/index.ts", 1.0]]}
        ts_report = [
            ["src/bench.ts", "./index", "followed", None, False, "src/index.ts", ["src/a.ts"]],
            ["src/c.ts", "./a", "defined_here", None, False, "src/a.ts", ["src/a.ts"]],
            ["src/index.ts", "./a", "defined_here", None, False, "src/a.ts", ["src/a.ts"]],
            ["src/index.ts", "./b", "opaque", None, False, "src/b.ts", ["src/b.ts"]],
            ["src/other.ts", "./c", "defined_here", None, True, "src/c.ts", ["src/c.ts"]],
            ["src/use.ts", "./index", "followed", None, True, "src/index.ts", ["src/a.ts"]],
            ["src/use.ts", "./missing", "unresolved", None, False, None, []],
        ]
        ts_ingest = {"file_edges": [
            ["src/c.ts", "src/a.ts", 1, 1, 1], ["src/index.ts", "src/a.ts", 1, 1, 1],
            ["src/index.ts", "src/b.ts", 1, 1, 1], ["src/other.ts", "src/a.ts", 1, 1, 1],
            ["src/other.ts", "src/b.ts", 1, 1, 1], ["src/other.ts", "src/c.ts", 1, 1, 1],
            ["src/use.ts", "src/a.ts", 1, 1, 1], ["src/use.ts", "src/b.ts", 1, 1, 1],
            ["src/use.ts", "src/index.ts", 1, 1, 0], ["src/use.ts", "src/missing.ts", 1, 1, 1],
            ["src/use.ts", "types/global.d.ts", 1, 1, 1]],
            "use_pair_symbols": [["src/use.ts", "src/a.ts", ["npm pkg 1 src/a.ts/f()."]]],
            "member_only_use_pairs": [["src/other.ts", "src/b.ts"]],
            # The indexer never read src/bench.ts: its hand pairs are not
            # evidence against hand.
            "unindexed_lang_files": ["src/bench.ts"]}
        ts_row = score_language("synthetic", "ts", ts_graph, None, ts_ingest, None, ts_report, TsSources(None))
        ts = ts_row["ts"]
        assert ts["scip_only_uses_by_class"] == {
            "barrel, followed": 1, "barrel, not followed": 1, "unresolved relative import": 1,
            "unresolved workspace or alias import": 0, "ambient or global": 1, "inferred type": 1,
            "other": 1}, ts["scip_only_uses_by_class"]
        assert ts["barrel_not_followed_by_outcome"] == {"followed": 1}, ts["barrel_not_followed_by_outcome"]
        assert ts["type_only"] == {"barrel, followed": 1, "barrel, not followed": 1}, ts["type_only"]
        assert ts["pairs"]["hand_is_resolved"] and not ts["pairs"]["hand_is_followed"], ts["pairs"]
        change = {k: v for k, v in ts["change"].items() if not k.endswith("samples")}
        assert change == {"removed": 2, "removed_scip": 1, "removed_scip_uses": 0, "added": 2, "added_scip": 1,
                          "added_scip_uses": 1, "added_not_scip": 1}, change
        assert ts["import_outcomes"]["by_outcome"] == {"defined_here": 3, "followed": 2, "opaque": 1,
                                                       "unresolved": 1}, ts["import_outcomes"]
        assert ts["hand_only_by_class"] == {"source not indexed": 1, "followed": 0, "other": 0}, \
            ts["hand_only_by_class"]
        assert ts["member_only"]["other"] == 1 and sum(ts["member_only"].values()) == 1, ts["member_only"]
        followed_sample = ts["scip_only_uses_samples"]["barrel, followed"][0]
        assert followed_sample["symbols"] == ["npm pkg 1 src/a.ts/f()."], followed_sample
        assert unresolved_names("src/x/use.ts", "../lib", "src/lib/index.ts", TsSources(None)) \
            == "unresolved relative import"
        assert unresolved_names("src/use.ts", "./lib", "src/other.ts", TsSources(None)) is None

        # The gate: Go holds pairs confirmed by a named use, once both sides
        # carry it; Python keeps holding every pair confirmed by a use.
        def gate_rows(lang, shared_uses, named):
            return [{"name": "x", "lang": lang, "status": "scored", "scip_fingerprint": "f", "scip_pairs": 1,
                     "hand_only_by_class": {c: 0 for c in HAND_CLASSES}, "shared_uses": shared_uses,
                     "shared_named_uses": named, "recall": 0, "precision": 0, "recall_uses": 0,
                     "precision_uses": 0, "shared": 0, "hand_only": 0, "scip_only": 0}]

        def run_gate(base, run):
            (repo / "base.json").write_text(json.dumps({"rows": base}))
            (repo / "run.json").write_text(json.dumps(run))
            return gate(argparse.Namespace(baseline=repo / "base.json", summary=repo / "run.json"))
        assert run_gate(gate_rows("go", 10, 8), gate_rows("go", 9, 8)) == 0
        assert run_gate(gate_rows("go", 10, 8), gate_rows("go", 10, 7)) == 1
        assert run_gate(gate_rows("go", 10, None), gate_rows("go", 9, 8)) == 1
        assert run_gate(gate_rows("py", 10, 8), gate_rows("py", 9, 8)) == 1
        # Rust is reported, never gated (issue #126), even on a fall.
        assert run_gate(gate_rows("rs", 10, 8), gate_rows("rs", 2, 1)) == 0

        # Rust (finding 57): the report's outcomes and both classifiers.
        rs_files = ["src/lib.rs", "src/a.rs", "src/b.rs", "src/b/c.rs", "src/d.rs"]
        rs_graph = {"lang": "rs", "nodes": [{"file": f, "lang": "rs"} for f in rs_files],
                    "imports": [["src/lib.rs", "src/a.rs", 1], ["src/a.rs", "src/b.rs", 1],
                                ["src/lib.rs", "src/d.rs", 1]]}
        rs_rows = [
            ["src/lib.rs", "use", "crate::a::X", "X", "defined", None, ["src/a.rs"], ""],
            ["src/lib.rs", "use", "crate::d::Y", "Y", "uncertain", None, ["src/d.rs"], ""],
            ["src/lib.rs", "use", "crate::b", None, "glob", None, [], ""],
            ["src/a.rs", "path", "super::b::f", None, "defined", "broken", ["src/b.rs"], ""],
            ["src/a.rs", "use", "crate::gone::Z", "Z", "unused", None, [], ""],
        ]
        rs_modules = {"src/lib.rs": "demo", "src/a.rs": "demo::a", "src/b.rs": "demo::b",
                      "src/b/c.rs": "demo::b::c", "src/d.rs": "demo::d"}
        rs_ingest = {"file_edges": [
            ["src/lib.rs", "src/a.rs", 1, 1, 1], ["src/a.rs", "src/b.rs", 1, 1, 1],
            ["src/lib.rs", "src/b/c.rs", 1, 1, 1], ["src/lib.rs", "src/b.rs", 1, 1, 1],
            ["src/a.rs", "src/d.rs", 1, 1, 1], ["src/b.rs", "src/a.rs", 1, 1, 1],
            ["src/d.rs", "src/b.rs", 1, 1, 1]],
            "member_only_use_pairs": [["src/b.rs", "src/a.rs"]],
            "use_pair_symbols": [["src/d.rs", "src/b.rs", ["rust-analyzer cargo demo 0.1.0 b/made!"]]]}
        rs_row = score_language("synthetic", "rs", rs_graph, None, rs_ingest, None,
                                rs_report=(rs_rows, rs_modules))
        rs = rs_row["rs"]
        assert rs["outcomes"]["use"]["defined"] == 1 and rs["outcomes"]["use"]["glob"] == 1 \
            and rs["outcomes"]["use"]["unused"] == 1 and rs["outcomes"]["path"]["defined"] == 1, rs["outcomes"]
        assert rs["ties"] == {"use": {}, "path": {"broken": 1}}, rs["ties"]
        # lib.rs globs `crate::b`, and b.rs and b/c.rs are under it; b.rs ->
        # a.rs is member-only; a.rs -> d.rs has no hand path at all.
        assert rs["scip_only_uses_by_class"] == {
            "member via value": 1, "macro": 1, "glob import": 2, "inferred type": 0, "other": 1}, \
            rs["scip_only_uses_by_class"]
        assert rs["hand_only_by_class"] == {"uncertain module": 1, "other": 0}, rs["hand_only_by_class"]
        assert rs_absolute(["super", "b"], "demo::a") == "demo::b"
        assert rs_absolute(["crate", "x", "super", "y"], "demo::a") == "demo::y"
        assert rs_absolute(["other_crate", "x"], "demo::a") is None
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
    run = commands.add_parser("ts-variants")
    run.add_argument("--work", type=Path, required=True, help="hand.graph.json and ts-imports.*.json")
    run.set_defaults(run=ts_variants)
    commands.add_parser("rust-targets").set_defaults(run=rust_targets)
    commands.add_parser("self-test").set_defaults(run=self_test)
    args = parser.parse_args()
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
