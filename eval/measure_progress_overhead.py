"""Balanced warm-build timing on a standard remote-build runner."""

import hashlib
import json
import os
import shlex
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path


def digest(path: Path) -> str:
    checksum = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def main() -> None:
    stem = sys.argv[1]
    args = json.loads(os.environ["REPO_ARGS_JSON"]) + shlex.split(os.environ.get("EXTRA_ARGS", ""))
    binaries = {
        "progress": Path("bin-primary/tolmap").resolve(),
        "main": Path("bin-compare/tolmap").resolve(),
    }
    samples = []
    for position, label in enumerate(("main", "progress", "progress", "main")):
        out = Path(f"overhead-{position}")
        out.mkdir()
        command = ["prlimit", f"--as={os.environ['BUILD_MEM_BYTES']}", "--",
                   str(binaries[label]), "build", str(Path("clone").resolve()),
                   *args, "--name", stem, "--out", str(out)]
        started = time.perf_counter()
        with Path(f"overhead-{position}.log").open("wb") as log:
            completed = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT,
                                       check=False)
        elapsed = time.perf_counter() - started
        if completed.returncode != 0:
            raise SystemExit(f"{label} replay {position} exited {completed.returncode}")
        samples.append({"order": position, "ref": label, "wall_s": round(elapsed, 3),
                        "map_sha256": digest(out / f"{stem}.json"),
                        "symbols_sha256": digest(out / f"{stem}.symbols.json")})
        shutil.rmtree(out)
    main = statistics.median(row["wall_s"] for row in samples if row["ref"] == "main")
    progress = statistics.median(row["wall_s"] for row in samples if row["ref"] == "progress")
    report = {"samples": samples, "main_median_s": main,
              "progress_median_s": progress,
              "overhead_pct": round(100 * (progress / main - 1), 3),
              "byte_identical": len({(r["map_sha256"], r["symbols_sha256"]) for r in samples}) == 1}
    Path("progress_overhead.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if not report["byte_identical"]:
        raise SystemExit("paired warm builds differed in map or symbols bytes")


if __name__ == "__main__":
    main()
