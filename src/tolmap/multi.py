"""Language backends: Go and TypeScript via tree-sitter, Python via ast.

Emits the same graph shape as extract.py, plus a symbol table per file so the
map can descend from building to floor.
"""
import json, math, os, re, subprocess, sys
from collections import Counter, defaultdict
from itertools import combinations

from . import extract  # reuse git_cochange, churn, prox, blending constants

from tree_sitter import Language, Parser
import tree_sitter_go, tree_sitter_typescript

GO = Language(tree_sitter_go.language())
TS = Language(tree_sitter_typescript.language_typescript())

BRANCHY = {
    "if_statement", "for_statement", "while_statement", "switch_statement",
    "type_switch_statement", "expression_switch_statement", "select_statement",
    "catch_clause", "try_statement", "case_clause", "expression_case",
    "communication_case", "ternary_expression", "conditional_expression",
    "binary_expression",
}

# kind ids kept small: they ship to the browser for every symbol
KIND = {"class": 0, "func": 1, "method": 2, "interface": 3, "type": 4, "const": 5}


def walk(node):
    stack = [node]
    while stack:
        n = stack.pop()
        yield n
        stack.extend(reversed(n.children))


def txt(n, src):
    return src[n.start_byte:n.end_byte].decode("utf8", "ignore")


def name_of(n, src):
    f = n.child_by_field_name("name")
    if f is not None:
        return txt(f, src)
    for c in n.children:
        if c.type in ("identifier", "type_identifier", "field_identifier",
                      "property_identifier"):
            return txt(c, src)
    return None


# ---------------------------------------------------------------- Go
def go_symbols(root, src):
    out = []
    for n in walk(root):
        t = n.type
        if t == "function_declaration":
            k = "func"
        elif t == "method_declaration":
            k = "method"
        elif t == "type_spec":
            body = n.child_by_field_name("type")
            bt = body.type if body is not None else ""
            k = "interface" if bt == "interface_type" else (
                "class" if bt == "struct_type" else "type")
        else:
            continue
        nm = name_of(n, src)
        if nm:
            out.append([nm, KIND[k], n.start_point[0] + 1, n.end_point[0] + 1])
    return out


def go_imports(root, src):
    paths = []
    for n in walk(root):
        if n.type == "import_spec":
            for c in n.children:
                if c.type == "interpreted_string_literal":
                    paths.append(txt(c, src).strip('"'))
    return paths


# ---------------------------------------------------------------- TypeScript
def ts_symbols(root, src):
    out = []
    for n in walk(root):
        t = n.type
        k = None
        if t == "class_declaration":
            k = "class"
        elif t == "function_declaration":
            k = "func"
        elif t == "method_definition":
            k = "method"
        elif t == "interface_declaration":
            k = "interface"
        elif t == "type_alias_declaration":
            k = "type"
        elif t == "variable_declarator":
            v = n.child_by_field_name("value")
            if v is not None and v.type in ("arrow_function", "function_expression"):
                k = "func"
            elif n.parent is not None and n.parent.type == "lexical_declaration" \
                    and n.end_point[0] - n.start_point[0] >= 2:
                k = "const"
        if not k:
            continue
        nm = name_of(n, src)
        if nm and not nm.startswith("_"):
            out.append([nm, KIND[k], n.start_point[0] + 1, n.end_point[0] + 1])
    return out


def ts_named(root, src):
    """`import { Y } from './a'` names Y — a symbol-level reference for free."""
    out = []
    for n in walk(root):
        if n.type != "import_statement":
            continue
        srcn = n.child_by_field_name("source")
        if srcn is None:
            continue
        path = txt(srcn, src).strip("'\"")
        for c in walk(n):
            if c.type == "import_specifier":
                nm = name_of(c, src)
                if nm:
                    out.append((path, nm))
    return out


def go_selectors(root, src):
    """`pkg.Symbol` — the package qualifier tells us which package the name
    belongs to, so a selector expression is a symbol-level reference."""
    alias = {}
    for n in walk(root):
        if n.type == "import_spec":
            path, nm = None, None
            for c in n.children:
                if c.type == "interpreted_string_literal":
                    path = txt(c, src).strip('"')
                elif c.type in ("package_identifier", "identifier"):
                    nm = txt(c, src)
            if path:
                alias[nm or path.rstrip("/").split("/")[-1]] = path
    out = []
    for n in walk(root):
        if n.type == "selector_expression":
            op = n.child_by_field_name("operand")
            fl = n.child_by_field_name("field")
            if op is not None and fl is not None and op.type == "identifier":
                p = alias.get(txt(op, src))
                if p:
                    out.append((p, txt(fl, src)))
    return out


def ts_imports(root, src):
    paths = []
    for n in walk(root):
        if n.type in ("import_statement", "export_statement"):
            s = n.child_by_field_name("source")
            if s is not None:
                paths.append(txt(s, src).strip("'\"" ))
        elif n.type == "call_expression":
            fn = n.child_by_field_name("function")
            if fn is not None and txt(fn, src) in ("require", "import"):
                a = n.child_by_field_name("arguments")
                if a is not None:
                    for c in a.children:
                        if c.type == "string":
                            paths.append(txt(c, src).strip("'\""))
    return paths


CFG = {
    "go": dict(lang=GO, exts=(".go",), syms=go_symbols, imps=go_imports,
               skipfile=lambda f: f.endswith("_test.go")),
    "ts": dict(lang=TS, exts=(".ts",), syms=ts_symbols, imps=ts_imports,
               skipfile=lambda f: (".test." in f or ".spec." in f or f.endswith(".d.ts"))),
}
SKIPDIR = {"vendor", "node_modules", "dist", "testdata", "__tests__", "tests",
           "test", ".git", "docs", "documentation", "examples", "example",
           "third_party", "generated", "fixtures", "__pycache__"}


def source_files(repo, sub, cfg):
    root = os.path.join(repo, sub)
    out = []
    for dp, dn, fn in os.walk(root):
        dn[:] = [d for d in dn if d not in SKIPDIR and not d.startswith(".")]
        for f in fn:
            if f.endswith(cfg["exts"]) and not cfg["skipfile"](f):
                out.append(os.path.relpath(os.path.join(dp, f), repo))
    return sorted(out)


def join_slash(base, relative):
    """Join two repository-relative paths without consulting the filesystem."""
    parts = [p for p in base.split("/") if p and p != "."]
    for part in relative.split("/"):
        if not part or part == ".":
            continue
        if part == "..":
            if parts:
                parts.pop()
        else:
            parts.append(part)
    return "/".join(parts)


def strip_jsonc(text):
    """Strip JSONC comments and trailing commas, respecting string literals.

    A regex is wrong here: `"@/*": ["./*"]` contains a block-comment opener
    inside a string, and dify's `"**/*.ts"` supplies the next `*/`, swallowing
    the paths object between them.
    """
    out, i, in_string = [], 0, False
    while i < len(text):
        char = text[i]
        if in_string:
            out.append(char)
            i += 1
            if char == "\\" and i < len(text):
                out.append(text[i])
                i += 1
            elif char == '"':
                in_string = False
        elif char == '"':
            out.append(char)
            in_string = True
            i += 1
        elif text.startswith("//", i):
            i += 2
            while i < len(text) and text[i] != "\n":
                i += 1
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = len(text) if end < 0 else end + 2
        else:
            out.append(char)
            i += 1

    text = "".join(out)
    out, i, in_string = [], 0, False
    while i < len(text):
        char = text[i]
        if in_string:
            out.append(char)
            i += 1
            if char == "\\" and i < len(text):
                out.append(text[i])
                i += 1
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            out.append(char)
            in_string = True
            i += 1
            continue
        if char == ",":
            j = i + 1
            while j < len(text) and text[j].isspace():
                j += 1
            if j < len(text) and text[j] in "}]":
                i += 1
                continue
        out.append(char)
        i += 1
    return "".join(out)


def _sorted_prefixes(entries):
    entries.sort(key=lambda item: (-len(item[0]), item[0]))
    return [entry for i, entry in enumerate(entries)
            if i == 0 or entry != entries[i - 1]]


def module_index(repo):
    """Collect nested Go modules and TypeScript aliases in one sorted walk."""
    go, ts = [], []
    for dp, dn, fn in os.walk(repo):
        dn[:] = sorted(d for d in dn if d not in SKIPDIR and not d.startswith("."))
        here = os.path.relpath(dp, repo).replace(os.sep, "/")
        here = "" if here == "." else here
        for filename in sorted(fn):
            path = os.path.join(dp, filename)
            if filename == "go.mod":
                try:
                    lines = open(path, encoding="utf8", errors="ignore")
                    module = next((line.split()[1] for line in lines
                                   if line.startswith("module ") and len(line.split()) > 1), None)
                    lines.close()
                except OSError:
                    module = None
                if module:
                    go.append((module, here))
            elif filename == "package.json":
                try:
                    package = json.load(open(path)).get("name")
                except (OSError, ValueError, AttributeError):
                    package = None
                if isinstance(package, str) and package:
                    ts.append((package, here))
            elif filename.startswith("tsconfig") and filename.endswith(".json"):
                try:
                    config = json.loads(strip_jsonc(open(path).read()))
                    options = config.get("compilerOptions", {})
                    paths = options.get("paths", {})
                    root = join_slash(here, options.get("baseUrl", "."))
                except (OSError, ValueError, AttributeError, TypeError):
                    continue
                if not isinstance(paths, dict):
                    continue
                for pattern, targets in sorted(paths.items()):
                    prefix = pattern[:-1] if pattern.endswith("*") else pattern
                    if not prefix or not isinstance(targets, list):
                        continue
                    for target in targets:
                        if not isinstance(target, str):
                            continue
                        target = target[:-1] if target.endswith("*") else target
                        ts.append((prefix, join_slash(root, target)))
    return {"go": _sorted_prefixes(go), "ts": _sorted_prefixes(ts)}


def strip_module_prefix(path, module):
    """Return the suffix below module, requiring equality or a `/` boundary."""
    module = module.rstrip("/")
    if not module:
        return None
    if path == module:
        return ""
    prefix = module + "/"
    return path[len(prefix):] if path.startswith(prefix) else None


def resolve_go(path, modules, bydir):
    for module, directory in modules["go"]:
        rest = strip_module_prefix(path, module)
        if rest is not None:
            return bydir.get(join_slash(directory, rest), [])
    return []


def ts_candidate(base, byfile):
    for cand in (base + ".ts", base + "/index.ts", base + ".tsx",
                 base + "/index.tsx", base + "/src/index.ts", base):
        if cand in byfile:
            return [cand]
    return []


def resolve_ts(path, src_file, modules, byfile):
    if path.startswith("."):
        base = os.path.normpath(os.path.join(os.path.dirname(src_file), path))
        return ts_candidate(base, byfile)
    for prefix, directory in modules["ts"]:
        rest = strip_module_prefix(path, prefix)
        if rest is not None:
            target = ts_candidate(join_slash(directory, rest), byfile)
            if target:
                return target
    return []


def build(repo, sub, kind, out_path):
    cfg = CFG[kind]
    parser = Parser(cfg["lang"])
    files = source_files(repo, sub, cfg)

    trees, loc, cplx, idents, syms = {}, {}, {}, {}, {}
    for f in files:
        raw = open(os.path.join(repo, f), "rb").read()
        try:
            t = parser.parse(raw)
        except Exception:
            continue
        trees[f] = t
        loc[f] = raw.count(b"\n") + 1
        c, ids = 0, Counter()
        for n in walk(t.root_node):
            if n.type in BRANCHY:
                c += 1
            elif n.type in ("identifier", "type_identifier", "field_identifier",
                            "property_identifier"):
                w = txt(n, raw)
                if len(w) > 2:
                    ids[w] += 1
        cplx[f] = c
        idents[f] = ids
        syms[f] = sorted(cfg["syms"](t.root_node, raw), key=lambda s: s[2])[:60]

    files = [f for f in files if f in trees]
    byfile = set(files)
    bydir = defaultdict(list)
    for f in files:
        bydir[os.path.dirname(f)].append(f)
    modules = module_index(repo)

    # ---- imports (directed) ----
    static, directed, fanin = Counter(), Counter(), Counter()
    uses = set()
    for f in files:
        raw = open(os.path.join(repo, f), "rb").read()
        for p in cfg["imps"](trees[f].root_node, raw):
            tgts = (resolve_go(p, modules, bydir) if kind == "go"
                    else resolve_ts(p, f, modules, byfile))
            if not tgts:
                continue
            # A Go import names a package, i.e. a whole directory. Spread one
            # unit of weight across its files so a wide package does not
            # outvote a precise TypeScript import.
            share = 1.0 / len(tgts)
            for tf in tgts:
                if tf == f:
                    continue
                static[tuple(sorted((f, tf)))] += share
                directed[(f, tf)] += share
                fanin[tf] += share
        refs = (go_selectors(trees[f].root_node, raw) if kind == "go"
                else ts_named(trees[f].root_node, raw))
        for p, nm in refs:
            tgts = (resolve_go(p, modules, bydir) if kind == "go"
                    else resolve_ts(p, f, modules, byfile))
            for tf in tgts:
                if tf != f:
                    uses.add((f, tf, nm))

    conf, solo, ncommits = extract.git_cochange(repo, files)

    df = Counter()
    for f in files:
        for w in idents[f]:
            df[w] += 1
    N = max(len(files), 1)
    vecs = {}
    for f in files:
        v = {}
        for w, c in idents[f].items():
            if df[w] < 2 or df[w] > N * 0.5:
                continue
            v[w] = (1 + math.log(c)) * math.log(N / df[w])
        nrm = math.sqrt(sum(x * x for x in v.values())) or 1.0
        vecs[f] = {w: x / nrm for w, x in v.items()}

    def cos(a, b):
        va, vb = vecs[a], vecs[b]
        if len(va) > len(vb):
            va, vb = vb, va
        return sum(x * vb.get(w, 0.0) for w, x in va.items())

    cand = set(static) | set(conf)
    if len(files) <= 600:
        sweep = combinations(files, 2)
    else:
        sweep = (p for fs in bydir.values() for p in combinations(fs, 2))
    for a, b in sweep:
        k = (a, b) if a < b else (b, a)
        if k not in cand and cos(a, b) > 0.28:
            cand.add(k)

    smax = max(static.values()) if static else 1
    edges = []
    for (a, b) in sorted(cand):
        s = static.get((a, b), 0) / smax
        cc = min(conf.get((a, b), 0.0), 1.0)
        pr = extract.prox(a, b)
        se = cos(a, b)
        w = extract.ALPHA * s + extract.BETA * cc + extract.GAMMA * pr + extract.DELTA * se
        if w < 0.02:
            continue
        edges.append({"a": a, "b": b, "w": round(w, 5), "static": round(s, 4),
                      "cochange": round(cc, 4), "prox": round(pr, 4), "sem": round(se, 4)})

    ch = extract.churn(repo, files)
    nodes = [{"f": f, "loc": loc[f], "cplx": cplx[f], "churn": ch.get(f, 0),
              "fanin": round(fanin.get(f, 0), 2), "mod": f} for f in sorted(files)]

    data = {"repo": os.path.basename(repo), "pkg": sub, "lang": kind,
            "params": {"alpha": extract.ALPHA, "beta": extract.BETA,
                       "gamma": extract.GAMMA, "delta": extract.DELTA},
            "commits_scanned": ncommits,
            "imports": [[a, b, round(n, 3)] for (a, b), n in sorted(directed.items())],
            "symbols": {f: syms[f] for f in files if syms[f]},
            "uses": sorted(uses),
            "nodes": nodes, "edges": edges}
    json.dump(data, open(out_path, "w"), separators=(",", ":"))
    nsym = sum(len(v) for v in syms.values())
    print(f"{os.path.basename(repo):11s} {kind:3s} files={len(files):4d} "
          f"symbols={nsym:5d} symbol-refs={len(uses):6d} commits={ncommits}")


if __name__ == "__main__":
    build(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])
