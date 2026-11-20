"""Regenerate pinned Rust fixtures in CI, preserving names and gating parity.

Run from the repository root after building target/release/tolmap. The output
artifact contains only maps, suggested lenses, and the parity/name evidence.
Historical rust-* generator labels all select the current Rust hand resolver;
they record the last algorithm change that required each oracle's refresh.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tomllib

import seed_names


def run(*args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("fixture-record"))
    parser.add_argument(
        "--extraction-change", action="store_true",
        help="record extraction changes and gate on 95%% placement and modularity within 0.02")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    clones = Path("full-clones")
    clones.mkdir(exist_ok=True)
    built = Path("full-out")
    built.mkdir(exist_ok=True)
    manifest = tomllib.loads(Path("data/fixtures.toml").read_text())
    evidence = []
    failures = []
    for name, spec in manifest.items():
        if not spec["generator"].startswith("rust-"):
            raise SystemExit(f"{name}: unsupported generator {spec['generator']}")
        print(f"::group::{name} ({spec['generator']})", flush=True)
        repo = clones / name
        run("git", "clone", "--quiet", "--filter=blob:none", spec["url"], str(repo))
        run("git", "-C", str(repo), "checkout", "--quiet", spec["commit"])
        seed_names.seed(name, str(built))
        command = ["target/release/tolmap", "build", str(repo), "--pkg", spec["pkg"],
                   "--lang", spec["lang"], "--refs", "hand", "--name", name,
                   "--out", str(built), "--resolution", "1.1"]
        if not spec["parcels"]:
            command.append("--no-parcels")
        build = subprocess.run(command, capture_output=True, text=True)
        (args.out / f"{name}-build.log").write_text(build.stdout + build.stderr)
        print(build.stdout, flush=True)
        print(build.stderr, flush=True)
        build.check_returncode()
        parity = subprocess.run(
            ["target/release/tolmap", "parity", f"data/{name}.json", str(built / f"{name}.json")],
            capture_output=True, text=True)
        print(parity.stdout, flush=True)
        print(parity.stderr, flush=True)
        before = json.loads(Path(f"data/{name}.json").read_text())
        after = json.loads((built / f"{name}.json").read_text())
        issues = []
        moved = [key for key in sorted(before.keys() | after.keys())
                 if key not in before or key not in after or before[key] != after[key]]
        if args.extraction_change:
            # Structural parity checks can fail after an extraction change.
            # Use parity's own threshold decisions for the two acceptance gates.
            for gate in ("district placement", "modularity"):
                if not any(line.startswith(f"  PASS {gate}:") for line in parity.stdout.splitlines()):
                    issues.append(f"parity {gate} gate failed")
        else:
            # Entry selection must not affect extraction or partition inputs.
            for key in ("F", "E", "S", "U", "q"):
                if key in moved:
                    issues.append(f"non-landmark {key} changed")
            if [row[0] for row in before["N"]] != [row[0] for row in after["N"]]:
                issues.append("partition changed")
            if "names" in moved:
                issues.append(f"district names changed: {before['names']} -> {after['names']}")
            if parity.returncode:
                issues.append(f"parity exit {parity.returncode}")
        if "D" not in after:
            issues.append("missing D sidecar")
        if issues:
            failures.append(name)
        coverage = re.findall(r"generated coverage: (\{[^\n]+\})", build.stdout + build.stderr)
        evidence.append({"fixture": name, "generator": spec["generator"],
                         "commit": spec["commit"], "before_names": before["names"],
                         "after_names": after["names"], "before_keys": sorted(before),
                         "after_keys": sorted(after),
                         "before_landmarks": [
                             {"file": before["F"][row[0]], "kind": row[1],
                              "detail": row[2], "rank": row[3]} for row in before["L"]],
                         "after_landmarks": [
                             {"file": after["F"][row[0]], "kind": row[1],
                              "detail": row[2], "rank": row[3]} for row in after["L"]],
                         "moved": moved,
                         "files_removed": sorted(set(before["F"]) - set(after["F"])),
                         "files_added": sorted(set(after["F"]) - set(before["F"])),
                         "before_coverage": before.get("coverage"),
                         "after_coverage": after.get("coverage"),
                         "parity": parity.stdout,
                         "parity_exit": parity.returncode, "issues": issues,
                         "generated": json.loads(coverage[-1]) if coverage else None})
        shutil.copyfile(built / f"{name}.json", args.out / f"{name}.json")
        (args.out / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
        print("::endgroup::", flush=True)
    if (args.out / "flask.json").read_bytes() != Path("data/flask.json").read_bytes():
        run("target/release/tolmap", "lens", "suggest", "--map", str(args.out / "flask.json"),
            "--out", str(args.out / "lenses/flask-suggest"))
    (args.out / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
    run("python3", "eval/batch_stability.py", "--out", str(args.out / "stability"))
    run("python3", "eval/generated_mapping.py", "--out", str(args.out / "generated"))
    if failures:
        raise SystemExit(f"fixture gates failed: {', '.join(failures)}; see evidence.json")


if __name__ == "__main__":
    main()
