"""Measure Rust cold/warm district retention at the acceptance corpus pins.

Run in CI after building tolmap. Clones have full history; no extraction cache
is used. Only --previous-map differs between the cold and warm head builds.
The historical Python-only experiment's HEAD-relative pins are replaced by
explicit data/fixtures.toml pins so repeated measurements are comparable.
"""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib


REPOS = [("scrapy", 300), ("django", 300), ("celery", 300), ("httpx", 200)]


def run(*args):
    return subprocess.run([str(arg) for arg in args], check=True,
                          capture_output=True, text=True).stdout.strip()


def retention(before, after):
    """The common-file Jaccard matching and tie order used by src/parity.rs."""
    old = dict(zip(before["F"], (row[0] for row in before["N"])))
    new = dict(zip(after["F"], (row[0] for row in after["N"])))
    common = set(old) & set(new)
    old_by, new_by = defaultdict(set), defaultdict(set)
    for file in common:
        old_by[old[file]].add(file)
        new_by[new[file]].add(file)
    pairs = []
    for new_id, new_files in new_by.items():
        for old_id, old_files in old_by.items():
            overlap = len(new_files & old_files) / len(new_files | old_files)
            if overlap >= 0.35:
                pairs.append((overlap, new_id, old_id))
    used_old, matches = set(), {}
    for _, new_id, old_id in sorted(pairs, reverse=True):
        if new_id not in matches and old_id not in used_old:
            matches[new_id] = old_id
            used_old.add(old_id)
    kept = sum(matches.get(new[file]) == old[file] for file in common)
    return {"keep": 100 * kept / len(common) if common else 0,
            "common_files": len(common), "matched": len(matches),
            "nd": len(set(new.values())), "q": after["q"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/tolmap"))
    parser.add_argument("--clones", type=Path, default=Path("full-clones"))
    parser.add_argument("--out", type=Path, default=Path("stability-record"))
    args = parser.parse_args()
    binary = args.binary.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    args.clones.mkdir(parents=True, exist_ok=True)
    manifest = tomllib.loads(Path("data/fixtures.toml").read_text())
    rows = []
    for name, back in REPOS:
        spec = manifest[name]
        repo = (args.clones / name).resolve()
        if not repo.exists():
            run("git", "clone", "--quiet", "--filter=blob:none", spec["url"], repo)
        head = spec["commit"]
        old = run("git", "-C", repo, "rev-parse", f"{head}~{back}")
        original = run("git", "-C", repo, "rev-parse", "HEAD")
        maps = args.out / name

        def build(label, previous=None):
            out = maps / label
            command = [str(binary), "build", str(repo), "--pkg", spec["pkg"],
                       "--lang", spec["lang"], "--refs", "hand", "--name", name,
                       "--out", str(out), "--resolution", "1.1", "--no-parcels"]
            if previous is not None:
                command.extend(["--previous-map", str(previous)])
            result = subprocess.run(command, capture_output=True, text=True)
            (args.out / f"{name}-{label}.log").write_text(result.stdout + result.stderr)
            result.check_returncode()
            path = out / f"{name}.json"
            coverage = re.findall(r"generated coverage: (\{[^\n]+\})", result.stdout + result.stderr)
            return path, json.loads(path.read_text()), json.loads(coverage[-1]) if coverage else None

        try:
            run("git", "-C", repo, "checkout", "--quiet", old)
            prior_path, prior, _ = build("old")
            run("git", "-C", repo, "checkout", "--quiet", head)
            cold_path, cold, generated = build("cold")
            warm_path, warm, _ = build("warm", prior_path)
            repeat_path, _, _ = build("repeat")
            deterministic = cold_path.read_bytes() == repeat_path.read_bytes()
            if not deterministic:
                raise RuntimeError(f"{name}: repeated cold maps differ")
            row = {"repo": name, "head": head, "old": old, "back": back,
                   "n": len(cold["F"]), "seed": 7, "resolution": 1.1,
                   "refs": "hand", "extraction_cache": False,
                   "cold": retention(prior, cold), "warm": retention(prior, warm),
                   "generated": generated, "deterministic": deterministic,
                   "cold_sha256": hashlib.sha256(cold_path.read_bytes()).hexdigest(),
                   "warm_sha256": hashlib.sha256(warm_path.read_bytes()).hexdigest()}
            rows.append(row)
            print(json.dumps(row, sort_keys=True), flush=True)
            (args.out / "batch_stability.json").write_text(json.dumps(rows, indent=2) + "\n")
        finally:
            run("git", "-C", repo, "checkout", "--quiet", original)


if __name__ == "__main__":
    main()
