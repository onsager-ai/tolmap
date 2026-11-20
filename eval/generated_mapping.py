"""CI evidence for generated-file projection on tolmap's all-sources map."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--out", type=Path, default=Path("generated-record"))
out = parser.parse_args().out
out.mkdir(parents=True, exist_ok=True)
binary = "target/release/tolmap"
hashes = []
# Keep the mapped checkout separate from the full-fixtures job's corpus clones,
# generated output and build products. A local clone retains the same history.
with tempfile.TemporaryDirectory(prefix="tolmap-generated-") as temporary:
    checkout = Path(temporary) / "tolmap"
    subprocess.run(["git", "clone", "--quiet", "--no-hardlinks", ".", str(checkout)], check=True)
    for attempt in range(2):
        result = subprocess.run(
            [binary, "build", str(checkout), "--all-sources", "--name", "tolmap",
             "--out", str(out), "--refs", "hand", "--no-parcels"],
            capture_output=True, text=True)
        (out / f"build-{attempt}.log").write_text(result.stdout + result.stderr)
        result.check_returncode()
        hashes.append(hashlib.sha256((out / "tolmap.json").read_bytes()).hexdigest())
if len(set(hashes)) != 1:
    raise SystemExit("repeated all-sources maps differ")
coverage = re.findall(r"generated coverage: (\{[^\n]+\})", result.stdout + result.stderr)
counts = json.loads(coverage[-1])
if counts["collapsed"] == 0:
    raise SystemExit("no ts-rs binding collapsed on tolmap")
document = json.loads((out / "tolmap.json").read_text())
for name, count in counts.items():
    if document["coverage"].get(f"generated_{name}", 0) != count:
        raise SystemExit(f"map coverage disagrees with generated {name} count")
if any(file.startswith("bindings/") for file in document["F"]):
    raise SystemExit("generated binding leaked into F")
cross_imports = [
    [document["F"][a], document["F"][b]] for a, b in document["E"]
    if document["F"][a].startswith("web/") and document["F"][b].endswith(".rs")
]
if not cross_imports:
    raise SystemExit("binding imports did not reach Rust generators")
result = subprocess.run([binary, "debt", "--map", str(out / "tolmap.json"), "--format", "json"],
                        capture_output=True, text=True)
result.check_returncode()
(out / "debt.json").write_text(result.stdout)
debt = json.loads(result.stdout)
if any(pair[key].startswith("bindings/") for pair in debt["hidden_coupling"]
       for key in ("left", "right")):
    raise SystemExit("generated binding leaked into hidden coupling")
evidence = {"commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
            "seed": 7, "resolution": 1.1, "refs": "hand", "generated": counts,
            "files": len(document["F"]), "q": document["q"],
            "sha256": hashes, "web_to_rust_imports": cross_imports,
            "hidden_coupling_pairs": len(debt["hidden_coupling"])}
(out / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
print(json.dumps(evidence, sort_keys=True), flush=True)
