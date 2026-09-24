"""Record one worker job's protocol timeline on a remote-build runner.

This is intentionally run only by remote-build.yml, never on the maintainer's
laptop. The clone was already fetched by that workflow.
"""

import json
import shutil
import subprocess
import sys
import time
from pathlib import Path


def main() -> None:
    binary = Path(sys.argv[1]).resolve()
    clone = Path(sys.argv[2]).resolve()
    report = Path(sys.argv[3])
    slug = sys.argv[4]
    owner, repo = slug.split("/", 1)
    root = Path("worker-timeline").resolve()
    output_dir = root / "out"
    output_dir.mkdir(parents=True, exist_ok=True)
    spec = {
        "v": 1,
        "slug": slug,
        "owner": owner,
        "repo": repo,
        "source": str(clone),
        "local": True,
        "all_sources": True,
        "cache_dir": str(root / "cache"),
        "output_dir": str(output_dir),
        "clone_cache_bytes": 2147483648,
        "prune_variant": "node-relative",
        "namer": "idf",
        "namer_model": "anthropic/claude-haiku-4.5",
        "previous_maps": [],
        "names_cache": None,
    }
    started = time.monotonic()
    events = []
    with Path("worker-timeline.stderr").open("wb") as stderr:
        child = subprocess.Popen(
            [str(binary), "worker"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=stderr, text=True,
        )
        assert child.stdin is not None and child.stdout is not None
        child.stdin.write(json.dumps(spec, separators=(",", ":")) + "\n")
        child.stdin.close()
        for line in child.stdout:
            event = json.loads(line)
            events.append({"at_s": round(time.monotonic() - started, 3), **event})
        exit_code = child.wait()
    terminal = [e for e in events if e["type"] in ("result", "error")]
    stages = [
        {"id": e["stage"], "duration_s": e["duration_s"], "success": e["success"],
         "finished_at_s": e["at_s"]}
        for e in events if e["type"] == "stage_finished"
    ]
    result = {
        "slug": slug,
        "exit_code": exit_code,
        "elapsed_s": round(time.monotonic() - started, 3),
        "terminal": terminal[-1] if terminal else None,
        "stages": stages,
        "progress_events": sum(e["type"] == "progress" for e in events),
        "events": events,
    }
    report.write_text(json.dumps(result, indent=2) + "\n")
    shutil.rmtree(root)
    if exit_code != 0 or not terminal or terminal[-1]["type"] != "result":
        raise SystemExit("worker timeline job failed; see report and stderr")


if __name__ == "__main__":
    main()
