"""CI-only paired full/cold/unchanged/small-edit build, fixture pins owned by fixtures.toml."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
import tomllib

binary = str(Path(sys.argv[1]).resolve())
root = Path(sys.argv[2]).resolve()
root.mkdir(parents=True, exist_ok=True)
fixtures = tomllib.loads(Path("data/fixtures.toml").read_text())
rows = []
for name in ("flask", "django", "vue"):
    fixture = fixtures[name]
    repo = root / name
    subprocess.run(["git", "clone", "--quiet", "--filter=blob:none", fixture["url"], str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "checkout", "--quiet", fixture["commit"]], check=True)
    def run(case, cached):
        out = root / f"{name}-{case}"
        cmd = [binary, "build", str(repo), "--pkg", fixture["pkg"], "--lang", fixture["lang"], "--refs", "hand", "--namer", "idf", "--name", name, "--out", str(out)]
        if not fixture["parcels"]:
            cmd.append("--no-parcels")
        if cached:
            cmd += ["--extraction-cache", str(root / f"{name}-cache")]
        started = time.monotonic()
        result = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        elapsed = time.monotonic() - started
        (root / f"{name}-{case}.log").write_text(result.stdout + result.stderr)
        if result.returncode:
            raise RuntimeError(result.stderr[-6000:])
        stages = [json.loads(s.removeprefix("index_timing ")) for s in result.stderr.splitlines() if s.startswith("index_timing ")]
        reuse = [json.loads(s.removeprefix("index_reuse ")) for s in result.stderr.splitlines() if s.startswith("index_reuse ")]
        hashes = {suffix: hashlib.sha256((out / f"{name}{suffix}").read_bytes()).hexdigest() for suffix in (".json", ".symbols.json")}
        row = dict(repository=name, commit=fixture["commit"], case=case, wall_s=elapsed, stages=stages, reuse=reuse, hashes=hashes, seed=7, naming="empty idf cache", prior=None)
        rows.append(row)
        print(json.dumps(row), flush=True)
        return row
    baseline = run("full", False)
    cold = run("cold", True)
    unchanged = run("unchanged", True)
    assert baseline["hashes"] == cold["hashes"] == unchanged["hashes"]
    assert sum(r["reused"] for r in unchanged["reuse"]) > 0
    # A real content edit, not mtime: preserve syntax in both fixture languages.
    candidates = sorted((repo / fixture["pkg"]).rglob("*.py" if fixture["lang"] == "py" else "*.ts"))
    # Select an actually indexed file from the full map, not an excluded test.
    doc = json.loads((root / f"{name}-full" / f"{name}.json").read_text())
    file = repo / doc["F"][0]
    with file.open("a") as stream:
        stream.write("\n# incremental benchmark edit\n" if fixture["lang"] == "py" else "\n// incremental benchmark edit\n")
    edit = run("small-edit", True)
    edit_full = run("small-edit-full", False)
    assert edit["hashes"] == edit_full["hashes"]
    assert sum(r["parsed"] for r in edit["reuse"]) == 1
    (root / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
