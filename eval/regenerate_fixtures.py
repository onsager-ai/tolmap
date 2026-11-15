"""Regenerate pinned Rust fixtures in CI, preserving names and gating parity.

Run from the repository root after building target/release/tolmap. The output
artifact contains only maps, suggested lenses, and the parity/name evidence.
Historical rust-* generator labels all select the current Rust hand resolver;
they record the last algorithm change that required each oracle's refresh.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

import seed_names


def run(*args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("fixture-record"))
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    clones = Path("full-clones")
    clones.mkdir(exist_ok=True)
    built = Path("full-out")
    built.mkdir(exist_ok=True)
    manifest = tomllib.loads(Path("data/fixtures.toml").read_text())
    evidence = []
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
        run(*command)
        parity = subprocess.run(
            ["target/release/tolmap", "parity", f"data/{name}.json", str(built / f"{name}.json")],
            capture_output=True, text=True)
        print(parity.stdout, flush=True)
        print(parity.stderr, flush=True)
        parity.check_returncode()
        before = json.loads(Path(f"data/{name}.json").read_text())
        after = json.loads((built / f"{name}.json").read_text())
        # Entry selection must not affect extraction or partition inputs.
        # Record every landmark row, including rank changes, for PR review.
        for key in ("F", "E", "S", "U", "q"):
            if before[key] != after[key]:
                raise SystemExit(f"{name}: non-landmark {key} changed")
        if [row[0] for row in before["N"]] != [row[0] for row in after["N"]]:
            raise SystemExit(f"{name}: partition changed")
        if before["names"] != after["names"]:
            raise SystemExit(f"{name}: district names changed: {before['names']} -> {after['names']}")
        if "D" not in after:
            raise SystemExit(f"{name}: missing D sidecar")
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
                         "parity": parity.stdout,
                         "parity_exit": parity.returncode})
        shutil.copyfile(built / f"{name}.json", args.out / f"{name}.json")
        print("::endgroup::", flush=True)
    if (args.out / "flask.json").read_bytes() != Path("data/flask.json").read_bytes():
        run("target/release/tolmap", "lens", "suggest", "--map", str(args.out / "flask.json"),
            "--out", str(args.out / "lenses/flask-suggest"))
    (args.out / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
