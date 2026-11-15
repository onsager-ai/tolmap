"""CI-only landmark refresh at data/fixtures.toml pins.

PR #302's general regeneration is not on main yet. This scoped variant keeps
all committed fields except L byte-equivalent, verifies the rebuilt partition
and extraction exactly, and records every landmark change. It never refreshes
Flask lenses when the Flask fixture is unchanged.
"""
import argparse
import json
from pathlib import Path
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
        print(f"::group::{name} ({spec['generator']})", flush=True)
        if not spec["generator"].startswith("rust-"):
            raise SystemExit(f"{name}: unsupported generator")
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
        original = Path(f"data/{name}.json")
        before = json.loads(original.read_text())
        after = json.loads((built / f"{name}.json").read_text())
        for key in ("F", "E", "S", "U", "names", "q"):
            if before[key] != after[key]:
                raise SystemExit(f"{name}: non-landmark {key} changed")
        if [row[0] for row in before["N"]] != [row[0] for row in after["N"]]:
            raise SystemExit(f"{name}: partition changed")
        # Compare all original structural fields with only the intentionally
        # changed L replaced. The product parity command remains unmodified.
        expected = dict(before, L=after["L"])
        candidate = args.out / f"{name}.json"
        if before["L"] == after["L"]:
            candidate.write_bytes(original.read_bytes())
        else:
            candidate.write_text(json.dumps(expected, separators=(",", ":"), ensure_ascii=False))
        parity = subprocess.run(
            ["target/release/tolmap", "parity", str(candidate), str(built / f"{name}.json")],
            capture_output=True, text=True)
        print(parity.stdout, flush=True)
        print(parity.stderr, flush=True)
        parity.check_returncode()
        describe = lambda rows: [{"file": before["F"][row[0]], "kind": row[1],
                                  "detail": row[2], "rank": row[3]} for row in rows]
        evidence.append({"fixture": name, "commit": spec["commit"],
                         "placement": "100.0%", "delta_q": 0,
                         "before": describe(before["L"]), "after": describe(after["L"]),
                         "parity": parity.stdout, "parity_exit": parity.returncode})
        print("::endgroup::", flush=True)
    flask_changed = (args.out / "flask.json").read_bytes() != Path("data/flask.json").read_bytes()
    if flask_changed:
        raise SystemExit("Flask changed unexpectedly; do not refresh its lens baseline")
    (args.out / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
