#!/usr/bin/env python3
"""CI-only, reproducible agent payload pilot for #303 (stdlib only).

Bytes are stdout/file bytes delivered to an agent, not internal indexer I/O or
model tokens. Calls are separate shell/tool invocations in the recorded recipe;
setup/build, status and shared provenance inspection are excluded on both sides.
The full baseline reads raw files then a grep/Python projection; filtered_only
also records the cheaper case where an agent already knows the schema.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


MAIN = "58be4cb0147c9eadaca568ac0b9c67fdb6a208b2"
DIFF = "79089b356ee69933f20195a237e92c9af20d9782"
# Source audit: rg -n 'crate::check|tolmap::check|mod check|check::' src
# then read grouped use statements and exclude comments. Include test references
# and the lib.rs declaration separately; E models name use, not mod declarations.
IMPORTERS = ["src/comparison.rs", "src/local.rs", "src/main.rs", "src/org.rs",
             "src/service/debt.rs", "src/service/lens.rs"]
IMPORTS = [f"src/{name}.rs" for name in
           ("detect", "extract", "geometry", "naming", "pipeline", "schema")]


def encoded(value):
    return (json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    binary, repo, out = args.binary.resolve(), args.repo.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    commands = []

    def run(label, argv, cwd=repo):
        result = subprocess.run([str(a) for a in argv], cwd=cwd, capture_output=True)
        (out / f"{label}.stdout").write_bytes(result.stdout)
        (out / f"{label}.stderr").write_bytes(result.stderr)
        commands.append({"label": label, "argv": [str(a) for a in argv],
                         "exit_code": result.returncode, "stdout_bytes": len(result.stdout)})
        (out / "commands.json").write_bytes(encoded(commands))
        result.check_returncode()
        return result.stdout

    def query(label, *argv):
        return run(label, [binary, *argv])

    def save(label, value):
        data = encoded(value)
        (out / f"{label}.json").write_bytes(data)
        return len(data)

    target = out / "checkout"
    run("worktree", ["git", "worktree", "add", "--detach", target, MAIN])
    main_sha = run("main-sha", ["git", "rev-parse", MAIN]).decode().strip()
    diff_sha = run("diff-sha", ["git", "rev-parse", DIFF]).decode().strip()
    map_dir = out / "map"
    query("build", "build", target, "--all-sources", "--name", "tolmap", "--out", map_dir)
    map_path = map_dir / "tolmap.json"
    symbols_path = map_dir / "tolmap.symbols.json"
    graph = json.loads(map_path.read_bytes())
    # Record the source audit used for recall, including test-only and mod lines.
    run("ground-truth-rg", ["rg", "-n", "crate::check|tolmap::check|mod check|check::",
                            "src", "--glob", "*.rs"], cwd=target)
    run("ground-truth-out", ["rg", "-n", "crate::", "src/check.rs"], cwd=target)
    entry = run("entry-manifest", ["rg", "-n", "-A", "2", r"^\[\[bin\]\]", "Cargo.toml"], cwd=target)
    overview = query("overview", "overview", "--map", map_path)
    where = query("where", "where", "src/main.rs", "--map", map_path)
    deps = query("deps", "deps", "src/check.rs", "--direction", "both", "--map", map_path)
    complete = query("deps-complete", "deps", "src/check.rs", "--direction", "both",
                     "--limit", "1000", "--map", map_path)
    check = query("check", "check", target, "--base", f"{DIFF}^1", "--head", DIFF,
                  "--all-sources", "--report-only", "--format", "json")
    debt = query("debt-full", "debt", "--map", map_path, "--format", "json")
    hot = query("hotspots", "debt", "--map", map_path, "--kind", "hotspot", "--top", "5",
                "--format", "json")
    cycles = query("cycles", "debt", "--map", map_path, "--kind", "cycle", "--format", "json")

    # Before recipes: full map/symbols/debt read, then a Python join (equivalent
    # to jq) or grep. Retain the projections so their bytes are auditable too.
    files, nodes, edges = graph["F"], graph["N"], graph["E"]
    names = graph["names"]
    def district(index):
        return names[str(nodes[index][0])]

    orient_projection = save("before-orient", {
        "districts": [{"name": names[key], "files": sum(n[0] == int(key) for n in nodes)}
                      for key in sorted(names, key=int)],
        "entries": [files[row[0]] for row in graph["L"] if row[1] == "entry"],
        "cli": {"path": "src/main.rs", "district": district(files.index("src/main.rs"))}})
    index = files.index("src/check.rs")
    dep_projection = save("before-deps", [
        {"path": files[b if a == index else a], "district": district(b if a == index else a),
         "direction": "out" if a == index else "in"}
        for a, b in edges if a == index or b == index])
    diff = run("before-diff", ["git", "diff", "--name-only", f"{DIFF}^1", DIFF])
    # Build a correct historical base map, rather than assigning the recent
    # diff to today's districts. This setup is excluded, just like check builds.
    base_tree = out / "base-checkout"
    run("base-worktree", ["git", "worktree", "add", "--detach", base_tree, f"{DIFF}^1"])
    base_dir = out / "base-map"
    query("base-build", "build", base_tree, "--all-sources", "--name", "base", "--out", base_dir)
    base_path = base_dir / "base.json"
    base = json.loads(base_path.read_bytes())
    base_files = {path: i for i, path in enumerate(base["F"])}
    impact_projection = save("before-impact", [
        {"path": path, "district": base["names"][str(base["N"][base_files[path]][0])]
         if path in base_files else None}
        for path in diff.decode().splitlines()])
    debt_doc = json.loads(debt)
    debt_projection = save("before-debt", {"hotspots": debt_doc["hotspots"][:5],
                                            "cycles": debt_doc["cycles"]})
    map_bytes = map_path.stat().st_size
    symbols_bytes = symbols_path.stat().st_size
    tasks = [
        {"task": "orient", "query_bytes": len(entry) + len(overview) + len(where), "query_calls": 3,
         "before_bytes": map_bytes + symbols_bytes + len(entry) + orient_projection, "before_calls": 4,
         "filtered_only_bytes": len(entry) + orient_projection, "filtered_only_calls": 2,
         "note": "Both inspect Cargo bin declarations; overview does not list entry landmarks"},
        {"task": "before-edit", "query_bytes": len(deps), "query_calls": 1,
         "before_bytes": map_bytes + dep_projection, "before_calls": 2,
         "filtered_only_bytes": dep_projection, "filtered_only_calls": 1,
         "complete_query_bytes": len(complete),
         "complete_query_calls": 1, "retry_total_bytes": len(deps) + len(complete),
         "retry_total_calls": 2,
         "note": "Default deps is capped at 8; complete recall requires a second call with --limit 1000"},
        {"task": "review-impact", "query_bytes": len(check), "query_calls": 1,
         "before_bytes": base_path.stat().st_size + len(diff) + impact_projection, "before_calls": 3,
         "filtered_only_bytes": len(diff) + impact_projection, "filtered_only_calls": 2,
         "note": "Baseline only locates changed files; no equivalent delta_q, changed edges or outside_graph reasoning"},
        {"task": "debt-pick", "query_bytes": len(hot) + len(cycles), "query_calls": 2,
         "before_bytes": len(debt) + debt_projection, "before_calls": 2,
         "filtered_only_bytes": debt_projection, "filtered_only_calls": 1,
         "note": "Filters retain evidence/measurement metadata; jq projection has fewer fields"},
    ]
    def recall(payload):
        doc = json.loads(payload)
        result = {"truncated": doc["truncated"], "total": doc["total"]}
        for direction, truth in (("in", IMPORTERS), ("out", IMPORTS)):
            actual = {row["path"] for row in doc["results"] if row["direction"] == direction}
            expected = set(truth)
            result[direction] = {"expected": sorted(expected), "found": sorted(actual),
                                 "misses": sorted(expected - actual),
                                 "false_positives": sorted(actual - expected),
                                 "recall": len(actual & expected) / len(expected)}
        return result
    save("measurements", {
        "binary_commit": run("binary-sha", ["git", "rev-parse", "HEAD"]).decode().strip(),
        "source_commit": main_sha, "diff_commit": diff_sha,
        "map_sha256": hashlib.sha256(map_path.read_bytes()).hexdigest(),
        "raw_bytes": {"map": map_bytes, "symbols": symbols_bytes, "debt": len(debt)},
        "tasks": tasks, "recall_default": recall(deps), "recall_complete": recall(complete),
        "declaration_edge_excluded": "src/lib.rs: pub mod check; (not a name-use import)",
        "truth_scope": "All explicit src/ references, including cfg(test); excludes comments, external crates and mod declarations",
        "calls_scope": "Independent recipes; build/status/provenance setup excluded; raw reads and projection each count as a call",
        "orientation": json.loads(overview), "cli": json.loads(where),
        "hotspots": json.loads(hot)["hotspots"], "cycles": json.loads(cycles)["cycles"],
        "check": json.loads(check)})


if __name__ == "__main__":
    main()
