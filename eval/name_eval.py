#!/usr/bin/env python3
"""Remote-only, sequential district-name evaluation for issue #62.

One process runs the selected corpus in order. All model builds share one
TOLMAP_NAMER_LEDGER file, so the pre-request reservations cannot race. The
Rust client caps output tokens and reserves twice listed input/output prices
before each call; failed calls keep their reservation. At $5, the next call
falls back to IDF. This is a cap in configured-price terms; the owner should
also set a $5 OpenRouter key limit to cover provider price changes.
"""
from __future__ import annotations

import collections
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parent.parent
SAMPLE = [
    "Textualize/rich", "celery/celery", "scrapy/scrapy", "pallets/flask",
    "fastapi/fastapi", "gin-gonic/gin", "honojs/hono", "vuejs/core",
    "pydantic/pydantic", "django/django", "crawlab-team/crawlab",
    "date-fns/date-fns", "etcd-io/etcd", "hashicorp/terraform", "helm/helm",
    "apache/airflow", "angular/angular", "getsentry/sentry", "go-gitea/gitea",
    "n8n-io/n8n",
]


def run(*args: str, cwd: Path | None = None) -> str:
    result = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT)
    if result.returncode:
        raise RuntimeError(f"{' '.join(args[:3])} exited {result.returncode}:\n{result.stdout[-4000:]}")
    return result.stdout


def members(doc: dict) -> dict[int, set[str]]:
    groups: dict[int, set[str]] = collections.defaultdict(set)
    for file, node in zip(doc["F"], doc["N"]):
        groups[node[0]].add(file)
    return groups


def matched(old: dict, new: dict) -> list[tuple[int, int]]:
    a, b = members(old), members(new)
    pairs = []
    for new_id, new_set in b.items():
        for old_id, old_set in a.items():
            common = len(new_set & old_set)
            if common:
                score = common / len(new_set | old_set)
                if score >= 0.35:
                    pairs.append((score, new_id, old_id))
    pairs.sort(reverse=True)
    used_new, used_old, matches = set(), set(), []
    for _, new_id, old_id in pairs:
        if new_id not in used_new and old_id not in used_old:
            matches.append((new_id, old_id))
            used_new.add(new_id)
            used_old.add(old_id)
    return matches


def rename_share(old: dict, new: dict) -> dict:
    pairs = matched(old, new)
    changed = sum(new["names"][str(n)] != old["names"][str(o)] for n, o in pairs)
    return {"matched": len(pairs), "changed": changed,
            "share": changed / len(pairs) if pairs else 0.0}


def build(binary: Path, clone: Path, args: list[str], stem: str,
          out: Path, namer: str, previous: Path | None = None) -> tuple[dict, dict]:
    out.mkdir(parents=True, exist_ok=True)
    ledger_path = Path(os.environ["TOLMAP_NAMER_LEDGER"])
    before = json.loads(ledger_path.read_text()) if ledger_path.exists() else {}
    command = [str(binary), "build", str(clone), *args, "--name", stem,
               "--out", str(out), "--namer", namer]
    if previous:
        command += ["--previous-map", str(previous)]
    start = time.monotonic()
    run(*command)
    after = json.loads(ledger_path.read_text()) if ledger_path.exists() else {}
    metrics = {key: after.get(field, 0) - before.get(field, 0)
               for key, field in (("calls", "calls"), ("prompt_tokens", "prompt_tokens"),
                                  ("completion_tokens", "completion_tokens"), ("cost_usd", "actual_usd"))}
    metrics.update({
               "wall_seconds": round(time.monotonic() - start, 2)}
    )
    return json.loads((out / f"{stem}.json").read_text()), metrics


def names(doc: dict, cache: dict) -> list[dict]:
    by_id = {str(district): cache.get(hashlib.sha1("\n".join(sorted(group)).encode()).hexdigest()[:12], {})
             for district, group in members(doc).items()}
    return [{"district": id, "name": name,
             "source": by_id.get(id, {}).get("namer", "idf"),
             "numbered": by_id.get(id, {}).get("numbered", False)}
            for id, name in sorted(doc["names"].items(), key=lambda item: int(item[0]))]


def render(rows: list[dict], path: Path) -> None:
    lines = ["# District name evaluation", "", "Matched districts use greedy Jaccard ≥ 0.35. Rename share is changed names / matched districts.", "",
             "| Repo | Band | IDF rename | Model rename | Collisions | Numbered | Calls | Tokens in/out | Cost USD | Model wall s |",
             "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for row in rows:
        im, mm = row["idf_renames"], row["model_renames"]
        u = row["usage"]
        lines.append(f"| {row['slug']} | {row['band']} | {im['share']:.1%} ({im['changed']}/{im['matched']}) | "
                     f"{mm['share']:.1%} ({mm['changed']}/{mm['matched']}) | {row['collisions']} | "
                     f"{row['numbered']} | {u['calls']} | {u['prompt_tokens']}/{u['completion_tokens']} | "
                     f"{u['cost_usd']:.4f} | {u['wall_seconds']:.1f} |")
    lines += ["", "## By band", "", "| Band | Repos | Calls | Tokens in/out | Cost USD | Model wall s |",
              "|---|---:|---:|---:|---:|---:|"]
    for band in ("small", "medium", "large", "ultra"):
        subset = [row["usage"] for row in rows if row["band"] == band]
        lines.append(f"| {band} | {len(subset)} | {sum(u['calls'] for u in subset)} | "
                     f"{sum(u['prompt_tokens'] for u in subset)}/{sum(u['completion_tokens'] for u in subset)} | "
                     f"{sum(u['cost_usd'] for u in subset):.4f} | {sum(u['wall_seconds'] for u in subset):.1f} |")
    lines += ["", "## Per-map usage", "", "| Repo | Commit | Calls | Tokens in/out | Cost USD | Wall s |",
              "|---|---|---:|---:|---:|---:|"]
    for row in rows:
        for commit, usage in row["map_usage"].items():
            lines.append(f"| {row['slug']} | {commit} | {usage['calls']} | "
                         f"{usage['prompt_tokens']}/{usage['completion_tokens']} | "
                         f"{usage['cost_usd']:.4f} | {usage['wall_seconds']:.1f} |")
    lines += ["", "## Side-by-side current names", ""]
    for row in rows:
        lines += [f"### {row['slug']}", "", "| District | IDF | Model | Source | Numbered |",
                  "|---:|---|---|---|---:|"]
        idf = {entry["district"]: entry for entry in row["idf_names"]}
        model = {entry["district"]: entry for entry in row["model_names"]}
        for district in sorted(idf, key=int):
            m = model[district]
            lines.append(f"| {district} | {idf[district]['name']} | {m['name']} | {m['source']} | {m['numbered']} |")
        lines.append("")
    path.write_text("\n".join(lines) + "\n")


def main() -> int:
    if not os.environ.get("OPENROUTER_API_KEY"):
        raise SystemExit("OPENROUTER_API_KEY repository secret is required for name-eval")
    work = Path(os.environ.get("NAME_EVAL_WORK", "name-eval-work")).resolve()
    work.mkdir(exist_ok=True)
    os.environ["TOLMAP_NAMER_BUDGET_USD"] = "5"
    os.environ["TOLMAP_NAMER_LEDGER"] = str(work / "spend.json")
    binary = Path(os.environ.get("TOLMAP_BIN", "bin-primary/tolmap")).resolve()
    manifest = {entry["slug"]: entry for entry in tomllib.loads((ROOT / "eval/corpus.toml").read_text())["repo"]}
    rows = []
    for slug in SAMPLE:
        entry = manifest[slug]
        stem = slug.replace("/", "__")
        clone = work / "clone"
        if clone.exists():
            shutil.rmtree(clone)
        clone.mkdir()
        run("git", "init", "--quiet", str(clone))
        run("git", "-C", str(clone), "remote", "add", "origin", f"https://github.com/{slug}.git")
        run("git", "-C", str(clone), "fetch", "--quiet", "--filter=blob:none", "--depth=4000", "origin", entry["commit"])
        run("git", "-C", str(clone), "checkout", "--quiet", "--detach", "FETCH_HEAD")
        old = run("git", "-C", str(clone), "rev-parse", "HEAD~300").strip()
        run("git", "-C", str(clone), "checkout", "--quiet", "--detach", old)
        output = work / stem
        output.mkdir()
        args = entry.get("args", ["--all-sources"])
        idf_old, _ = build(binary, clone, args, stem, output / "idf", "idf")
        model_old, model_old_usage = build(binary, clone, args, stem, output / "model", "model")
        model_old_cache = json.loads((output / "model" / f"{stem}.names.json").read_text())
        idf_previous = output / "idf.previous.json"
        model_previous = output / "model.previous.json"
        idf_previous.write_text(json.dumps(idf_old))
        model_previous.write_text(json.dumps(model_old))
        run("git", "-C", str(clone), "checkout", "--quiet", "--detach", entry["commit"])
        idf_new, _ = build(binary, clone, args, stem, output / "idf", "idf", idf_previous)
        model_new, model_new_usage = build(binary, clone, args, stem, output / "model", "model", model_previous)
        idf_cache = json.loads((output / "idf" / f"{stem}.names.json").read_text())
        model_cache = json.loads((output / "model" / f"{stem}.names.json").read_text())
        old_model_names = names(model_old, model_old_cache)
        model_names = names(model_new, model_cache)
        old_values = list(model_old["names"].values())
        values = list(model_new["names"].values())
        row = {"slug": slug, "band": entry["band"], "commit": entry["commit"],
               "previous_commit": old, "idf_renames": rename_share(idf_old, idf_new),
               "model_renames": rename_share(model_old, model_new),
               "collisions": len(old_values) - len(set(old_values)) + len(values) - len(set(values)),
               "numbered": sum(item["numbered"] for item in old_model_names + model_names),
               "idf_names": names(idf_new, idf_cache), "model_names": model_names,
               "map_usage": {"previous": model_old_usage, "current": model_new_usage},
               "usage": {key: model_old_usage[key] + model_new_usage[key] for key in model_new_usage}}
        rows.append(row)
        (work / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
        render(rows, work / "report.md")
        shutil.rmtree(clone)
        print(f"{slug}: model {row['model_renames']['share']:.1%}, IDF {row['idf_renames']['share']:.1%}, cost ${row['usage']['cost_usd']:.4f}", flush=True)
    if any(row["collisions"] or row["model_renames"]["share"] > row["idf_renames"]["share"] for row in rows):
        raise SystemExit("name-eval regression: collision or model rename share exceeds IDF; see artifact")
    return 0


if __name__ == "__main__":
    sys.exit(main())
