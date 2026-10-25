"""CI-only bounded extraction controls and debugger stack sampling.

Keep the reader acceptance sources byte-for-byte identical; neither the probe
limit nor the production journey's 90-second limit is a tuning parameter.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

out = Path("extraction-profile")
out.mkdir(exist_ok=True)
sources = {
    "normal": 'def measured():\n    return 7\n' + '# performance fixture line 739\n' * 30000 + '# PERF_NEEDLE_739\n',
    "long": 'LONG = "' + 'x' * 900000 + 'PERF_NEEDLE_739"\n',
}
report = {"sha": os.environ.get("GITHUB_SHA"), "probeLimitSeconds": 30,
          "profile": {"debug": "line-tables-only", "lto": "thin", "codegenUnits": 1,
                      "features": "default (no feature overrides)", "command": "cargo build --locked --release --bin tolmap"},
          "runs": []}
for name, source in sources.items():
    repo = out / name
    repo.mkdir(exist_ok=True)
    path = repo / "fixture.py"
    path.write_bytes(source.encode())
    assert path.stat().st_size == {"normal": 930047, "long": 900025}[name]
    for label, binary in (("release", sys.argv[1]), ("release-debug", sys.argv[2])):
        graph = out / f"{name}-{label}.graph.json"
        command = [str(Path(binary).resolve()), "dump-graph", str(repo.resolve()),
                   "--pkg", ".", "--lang", "py", "--refs", "hand", "--out", str(graph)]
        start = time.monotonic()
        with (out / f"{name}-{label}.log").open("w") as log:
            process = subprocess.Popen(command, stdout=log, stderr=log, start_new_session=True)
            samples = []
            if label == "release-debug" and name == "normal":
                for index in range(5):
                    time.sleep(0.5)
                    if process.poll() is not None:
                        break
                    sample = subprocess.run(
                        ["sudo", "gdb", "-q", "-batch", "-p", str(process.pid),
                         "-ex", "set pagination off", "-ex", "thread apply all bt 20", "-ex", "detach"],
                        capture_output=True, text=True, timeout=4)
                    (out / f"sample-{index}.txt").write_text((sample.stdout + sample.stderr)[-24000:])
                    samples.append({"index": index, "status": sample.returncode})
            timed_out = False
            try:
                process.wait(timeout=max(0.01, 30 - (time.monotonic() - start)))
            except subprocess.TimeoutExpired:
                timed_out = True
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
        report["runs"].append({"fixture": name, "build": label,
            "sourceBytes": path.stat().st_size, "sourceSha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "seconds": time.monotonic() - start, "timeout": timed_out,
            "exit": process.returncode, "samples": samples,
            "graphSha256": hashlib.sha256(graph.read_bytes()).hexdigest() if graph.exists() else None})
(out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
