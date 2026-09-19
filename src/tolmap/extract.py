"""Stage 01-02: extract symbols/edges from a Python repo and blend into a weighted graph."""
import ast, json, math, os, subprocess, sys
from collections import Counter, defaultdict
from itertools import combinations

ALPHA, BETA, GAMMA, DELTA = 0.45, 0.35, 0.08, 0.12


def py_files(repo, pkg):
    root = os.path.join(repo, pkg)
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        # templates/ holds project scaffolding, not source: it is not map territory
        dirnames[:] = [d for d in dirnames if d not in
                       {"__pycache__", ".git", "tests", "test", "templates",
                        "vendor", "third_party", "migrations"}]
        for fn in filenames:
            if fn.endswith(".py"):
                out.append(os.path.relpath(os.path.join(dirpath, fn), repo))
        # keep walking
    return sorted(out)


def mod_name(relpath, pkg):
    """src/flask/app.py -> flask.app ; scrapy/core/engine.py -> scrapy.core.engine"""
    p = relpath[:-3]                      # strip .py
    parts = p.split(os.sep)
    if parts[-1] == "__init__":
        parts = parts[:-1]
    # The package root may be nested (lib/sqlalchemy, src/flask). Anchor on its
    # LAST segment: that is the name absolute imports actually use.
    root = pkg.rstrip("/").split("/")[-1]
    if root in parts:
        parts = parts[parts.index(root):]
    return ".".join(parts)


def resolve(node, cur_mod, known, is_pkg=False):
    """Yield internal module targets for an Import/ImportFrom node.

    `is_pkg` tells us whether `cur_mod` names the file's own package (the
    file is an `__init__.py`, and `mod_name()` already stripped `__init__`
    off it) or an ordinary module inside that package. A relative import
    resolves against the *containing package* -- itself, for a package
    `__init__`, or `cur_mod` minus its last segment, for anything else --
    and then strips `node.level - 1` further segments. Collapsing that
    distinction into a single `+ 1` (as this used to) is correct only for
    the `__init__` case and silently drops every other relative import:
    for `pkg.sub.mod` at level 1 it kept the whole name, so `from . import
    x` became `pkg.sub.mod.x` -- not a known module, and its parent equals
    `cur_mod` and is discarded. See issue #12.
    """
    hits = []
    if isinstance(node, ast.Import):
        for a in node.names:
            hits.append(a.name)
    elif isinstance(node, ast.ImportFrom):
        if node.level:                                   # relative import
            pkg_parts = cur_mod.split(".") if is_pkg else cur_mod.split(".")[:-1]
            strip = node.level - 1
            base = pkg_parts[: len(pkg_parts) - strip] if strip <= len(pkg_parts) else []
            prefix = ".".join(base)
            head = f"{prefix}.{node.module}" if node.module else prefix
        else:
            head = node.module or ""
        hits.append(head)
        for a in node.names:                              # from x import y (y may be a module)
            hits.append(f"{head}.{a.name}")
    out = set()
    for h in hits:
        if not h:
            continue
        if h in known:
            out.add(h)
        else:                                             # from pkg.mod import Symbol
            parent = h.rsplit(".", 1)[0]
            if parent in known:
                out.add(parent)
    out.discard(cur_mod)
    return out


IDENT_STOP = {"self", "cls", "args", "kwargs", "return", "None", "True", "False",
              "str", "int", "list", "dict", "set", "type", "object", "Exception",
              "value", "name", "key", "data", "result", "item", "obj", "i", "e"}


def identifiers(tree):
    c = Counter()
    for n in ast.walk(tree):
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            c[n.name] += 3
        elif isinstance(n, ast.Name):
            c[n.id] += 1
        elif isinstance(n, ast.Attribute):
            c[n.attr] += 1
    for w in IDENT_STOP:
        c.pop(w, None)
    return Counter({k: v for k, v in c.items() if len(k) > 2 and not k.startswith("__")})


def git_cochange(repo, files, max_commits=4000):
    """confidence(a,b) = |commits touching both| / min(|a|,|b|)"""
    fileset = set(files)
    out = subprocess.run(
        ["git", "-C", repo, "log", f"-n{max_commits}", "--no-merges",
         "--pretty=format:@%H", "--name-only"],
        capture_output=True, text=True, timeout=300).stdout
    commits, cur = [], None
    for line in out.splitlines():
        if line.startswith("@"):
            if cur:
                commits.append(cur)
            cur = set()
        elif line.strip() and cur is not None:
            if line in fileset:
                cur.add(line)
    if cur:
        commits.append(cur)

    solo = Counter()
    pair = Counter()
    for cs in commits:
        if not (2 <= len(cs) <= 40):          # skip giant sweeps: they couple everything
            for f in cs:
                solo[f] += 1
            continue
        for f in cs:
            solo[f] += 1
        for a, b in combinations(sorted(cs), 2):
            pair[(a, b)] += 1

    conf = {}
    for (a, b), n in pair.items():
        d = min(solo[a], solo[b])
        if d >= 3 and n >= 2:
            conf[(a, b)] = n / d
    return conf, solo, len(commits)


def churn(repo, files, max_commits=4000):
    out = subprocess.run(
        ["git", "-C", repo, "log", f"-n{max_commits}", "--no-merges",
         "--pretty=format:@", "--name-only"],
        capture_output=True, text=True, timeout=300).stdout
    c = Counter()
    fs = set(files)
    for line in out.splitlines():
        if line in fs:
            c[line] += 1
    return c


def complexity(tree):
    """crude cyclomatic proxy: branch points + function count"""
    n = 0
    for x in ast.walk(tree):
        if isinstance(x, (ast.If, ast.For, ast.While, ast.ExceptHandler,
                          ast.With, ast.Assert, ast.BoolOp)):
            n += 1
        elif isinstance(x, (ast.FunctionDef, ast.AsyncFunctionDef)):
            n += 1
    return n


KIND = {"class": 0, "func": 1, "method": 2, "interface": 3, "type": 4, "const": 5}


def py_uses(tree, cur_mod, known, f_of, cur_file):
    """Symbol-level references, recovered from the import statements themselves.

    `from a.b import Y` names Y explicitly, and `import a.b as m` followed by
    `m.Y` names it too. This is an approximation of a reference graph — it misses
    calls through a variable — but it is exact about what it does report, and it
    costs nothing extra to collect.
    """
    is_pkg = os.path.basename(cur_file) == "__init__.py"
    out, alias = [], {}
    for n in ast.walk(tree):
        if isinstance(n, ast.ImportFrom):
            tgts = resolve(n, cur_mod, known, is_pkg)
            if not tgts:
                continue
            # Emit every candidate module. Which one actually defines the name is
            # decided later against the symbol tables — picking here would be a
            # guess, and `from pkg import X` legitimately resolves to both the
            # package and the submodule.
            for t in sorted(tgts):
                tf = f_of[t]
                if tf == cur_file:
                    continue
                for a in n.names:
                    if a.name != "*":
                        out.append((tf, a.name))
        elif isinstance(n, ast.Import):
            for a in n.names:
                if a.name in known:
                    alias[a.asname or a.name.split(".")[-1]] = f_of[a.name]
    # `from . import interfaces` binds a MODULE, not a symbol, and the code then
    # writes `interfaces.Dialect`. Without this the dominant intra-package style
    # in many libraries is invisible.
    for n in ast.walk(tree):
        if isinstance(n, ast.ImportFrom):
            if n.level:
                pkg_parts = cur_mod.split(".") if is_pkg else cur_mod.split(".")[:-1]
                strip = n.level - 1
                base = pkg_parts[: len(pkg_parts) - strip] if strip <= len(pkg_parts) else []
                head = ".".join(base + ([n.module] if n.module else []))
            else:
                head = n.module or ""
            for a in n.names:
                full = f"{head}.{a.name}" if head else a.name
                if full in known:
                    alias[a.asname or a.name] = f_of[full]
    if alias:
        for n in ast.walk(tree):
            if isinstance(n, ast.Attribute) and isinstance(n.value, ast.Name):
                tf = alias.get(n.value.id)
                if tf and tf != cur_file:
                    out.append((tf, n.attr))
    return out


def py_symbols(tree):
    """Top-level classes and functions, plus methods one level in — the units a
    person actually navigates to. Deeper nesting is detail, not an address."""
    out = []
    for n in tree.body:
        if isinstance(n, ast.ClassDef):
            out.append([n.name, KIND["class"], n.lineno, getattr(n, "end_lineno", n.lineno)])
            for m in n.body:
                if isinstance(m, (ast.FunctionDef, ast.AsyncFunctionDef)) \
                        and not m.name.startswith("__"):
                    out.append([f"{n.name}.{m.name}", KIND["method"], m.lineno,
                                getattr(m, "end_lineno", m.lineno)])
        elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            out.append([n.name, KIND["func"], n.lineno, getattr(n, "end_lineno", n.lineno)])
        elif isinstance(n, (ast.Assign, ast.AnnAssign)):
            span = getattr(n, "end_lineno", n.lineno) - n.lineno
            tgt = n.targets[0] if isinstance(n, ast.Assign) else n.target
            if span >= 2 and isinstance(tgt, ast.Name):
                out.append([tgt.id, KIND["const"], n.lineno,
                            getattr(n, "end_lineno", n.lineno)])
    return sorted(out, key=lambda x: x[2])[:60]


def prox(a, b):
    pa, pb = a.split("/")[:-1], b.split("/")[:-1]
    shared = 0
    for x, y in zip(pa, pb):
        if x == y:
            shared += 1
        else:
            break
    depth = max(len(pa), len(pb), 1)
    return shared / depth


def build(repo, pkg, out_path):
    files = py_files(repo, pkg)
    mods, trees, idents, loc, cplx, syms = {}, {}, {}, {}, {}, {}
    for f in files:
        try:
            src = open(os.path.join(repo, f), encoding="utf-8", errors="ignore").read()
            t = ast.parse(src)
        except SyntaxError:
            continue
        m = mod_name(f, pkg)
        mods[m] = f
        trees[f] = t
        idents[f] = identifiers(t)
        loc[f] = src.count("\n") + 1
        cplx[f] = complexity(t)
        syms[f] = py_symbols(t)

    known = set(mods)
    files = [mods[m] for m in mods]
    f_of = {m: f for m, f in mods.items()}

    # --- static import edges ---
    # Keep BOTH an undirected weight (for clustering) and the directed edge
    # (for routing): "what does this import" is the only signal that supports an
    # honest path between two files. A co-change edge is not a route.
    static = Counter()
    directed = Counter()
    fanin = Counter()
    uses = set()
    for m, f in mods.items():
        is_pkg = os.path.basename(f) == "__init__.py"
        for node in ast.walk(trees[f]):
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                for tgt in resolve(node, m, known, is_pkg):
                    tf = f_of[tgt]
                    if tf == f:
                        continue
                    static[tuple(sorted((f, tf)))] += 1
                    directed[(f, tf)] += 1        # f imports tf
                    fanin[tf] += 1
        for tf, nm in py_uses(trees[f], m, known, f_of, f):
            uses.add((f, tf, nm))

    # --- co-change ---
    conf, solo, ncommits = git_cochange(repo, files)

    # --- semantic: tf-idf cosine over identifiers ---
    df = Counter()
    for f in files:
        for w in idents[f]:
            df[w] += 1
    N = len(files)
    vecs = {}
    for f in files:
        v = {}
        for w, c in idents[f].items():
            if df[w] < 2 or df[w] > N * 0.5:
                continue
            v[w] = (1 + math.log(c)) * math.log(N / df[w])
        norm = math.sqrt(sum(x * x for x in v.values())) or 1.0
        vecs[f] = {w: x / norm for w, x in v.items()}

    def cos(a, b):
        va, vb = vecs[a], vecs[b]
        if len(va) > len(vb):
            va, vb = vb, va
        return sum(x * vb.get(w, 0.0) for w, x in va.items())

    # --- blend ---
    cand = set(static) | set(conf)
    # Add semantic neighbours so purely-conceptual pairs can surface. This is
    # O(n^2); above a few hundred files, restrict the sweep to pairs that already
    # share a directory, which is where conceptual neighbours actually cluster.
    if len(files) <= 600:
        sweep = combinations(files, 2)
    else:
        bydir = defaultdict(list)
        for f in files:
            bydir["/".join(f.split("/")[:-1])].append(f)
        sweep = (p for fs in bydir.values() for p in combinations(fs, 2))
    for a, b in sweep:
        k = (a, b) if a < b else (b, a)
        if k in cand:
            continue
        if cos(a, b) > 0.28:
            cand.add(k)

    smax = max(static.values()) if static else 1
    edges = []
    for (a, b) in sorted(cand):
        s = static.get((a, b), 0) / smax
        cc = min(conf.get((a, b), 0.0), 1.0)
        pr = prox(a, b)
        se = cos(a, b)
        w = ALPHA * s + BETA * cc + GAMMA * pr + DELTA * se
        if w < 0.02:
            continue
        edges.append({"a": a, "b": b, "w": round(w, 5),
                      "static": round(s, 4), "cochange": round(cc, 4),
                      "prox": round(pr, 4), "sem": round(se, 4)})

    ch = churn(repo, files)
    nodes = [{"f": f, "loc": loc[f], "cplx": cplx[f],
              "churn": ch.get(f, 0), "fanin": fanin.get(f, 0),
              "mod": mod_name(f, pkg)} for f in sorted(files)]

    data = {"repo": os.path.basename(repo), "pkg": pkg,
            "imports": [[a, b, n] for (a, b), n in sorted(directed.items())],
            "symbols": {f: syms[f] for f in files if syms.get(f)},
            "uses": sorted(uses),
            "params": {"alpha": ALPHA, "beta": BETA, "gamma": GAMMA, "delta": DELTA},
            "commits_scanned": ncommits,
            "nodes": nodes, "edges": edges}
    json.dump(data, open(out_path, "w"), indent=1)

    only_cc = sum(1 for e in edges if e["static"] == 0 and e["cochange"] > 0.15)
    print(f"{os.path.basename(repo):8s} files={len(files):4d} edges={len(edges):5d} "
          f"commits={ncommits:4d} static_pairs={len(static):4d} "
          f"cochange_pairs={len(conf):4d} edges_cochange_only={only_cc:4d}")


if __name__ == "__main__":
    build(sys.argv[1], sys.argv[2], sys.argv[3])
