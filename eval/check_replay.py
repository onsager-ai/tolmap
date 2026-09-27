#!/usr/bin/env python3
"""Replay `tolmap check` over a repository's recent first-parent history, for
issue #170's threshold calibration. Runs inside remote-build.yml's
`check-calibrate` job, on a GitHub-hosted runner, never on the
maintainer's machine (CLAUDE.md: no local builds).

For the last `--commits` first-parent commits at `--pin` (the pin itself
first), each commit `c` whose index falls in this shard is checked as

    tolmap check <clone> --base <c>^ --head <c> --format json

which is exactly what a CI gate on the change that produced `c` would run:
for a merge commit, `c^` is the mainline before the merge, so the diff is
the whole merged change; for a squash or rebase, it is that one commit.

Shards interleave (index % shards), so every shard spans the whole window
and a slow stretch of history is spread across jobs instead of landing on
one.

Nothing is reused between checks, deliberately. On a first-parent chain
every commit is the base of exactly one check, so a stored base map
(`--base-map`) would be built once and read once: it would move the base
map build (0.5 s of django's 5.6 s, docs/CHECK.md) out of the check into a
`tolmap build` that also runs layout, which is more work, not less. The
reusable part is the extracted graph (a check's head graph is the next
check's base graph), and the check has no flag to accept one.

Writes into `--out`:
  - `<index>-<sha12>.json`: the report, byte for byte as the check printed
    it, when the check exited 0 or 1;
  - `<index>-<sha12>.log`: the check's stderr (timings, errors);
  - `commits.jsonl`: one row per checked commit, in index order: index,
    commit, base (`<c>^` resolved), subject, merge, exit_code, wall_s and
    the report's file name (null when there is none);
  - `shard.json`: what this shard was (slug, pin, shard, shards, commits).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

# A single check on django takes seconds (docs/CHECK.md); a check that has
# not finished in fifteen minutes is stuck, and is recorded as such (exit
# code null) rather than allowed to eat the job's own timeout.
CHECK_TIMEOUT_S = 900


def git(clone: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(clone), *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", required=True, type=Path, help="the tolmap binary")
    parser.add_argument("--clone", required=True, type=Path)
    parser.add_argument("--slug", required=True)
    parser.add_argument("--pin", required=True, help="the corpus pin, a full commit id")
    parser.add_argument("--commits", type=int, default=200)
    parser.add_argument("--shard", type=int, default=0)
    parser.add_argument("--shards", type=int, default=1)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument(
        "--mem-bytes",
        default=os.environ.get("BUILD_MEM_BYTES", ""),
        help="address-space cap for each check (prlimit --as), as the build job caps builds",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    args.out.mkdir(parents=True, exist_ok=True)
    chain = git(
        args.clone, "rev-list", "--first-parent", f"--max-count={args.commits + 1}", args.pin
    ).split()
    # The last commit of the chain is only ever a base: it needs its own
    # first parent to be checked, and that parent is outside the window.
    window = chain[: args.commits] if len(chain) > args.commits else chain[:-1]
    if len(window) < args.commits:
        print(
            f"{args.slug}: only {len(window)} first-parent commits with a parent "
            f"at {args.pin} (asked for {args.commits}); is the clone deep enough?",
            file=sys.stderr,
        )
    (args.out / "shard.json").write_text(
        json.dumps(
            {
                "slug": args.slug,
                "pin": args.pin,
                "shard": args.shard,
                "shards": args.shards,
                "commits": args.commits,
                "window": len(window),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )
    prefix = ["prlimit", f"--as={args.mem_bytes}", "--"] if args.mem_bytes else []
    rows = []
    for index, commit in enumerate(window):
        if index % args.shards != args.shard:
            continue
        stem = f"{index:03d}-{commit[:12]}"
        subject = git(args.clone, "log", "-1", "--format=%s", commit).strip()
        parents = git(args.clone, "log", "-1", "--format=%P", commit).split()
        base = git(args.clone, "rev-parse", f"{commit}^").strip()
        command = [
            *prefix,
            str(args.bin),
            "check",
            str(args.clone),
            "--base",
            f"{commit}^",
            "--head",
            commit,
            "--format",
            "json",
        ]
        started = time.monotonic()
        try:
            result = subprocess.run(
                command, capture_output=True, text=True, timeout=CHECK_TIMEOUT_S
            )
            code: int | None = result.returncode
            stdout, stderr = result.stdout, result.stderr
        except subprocess.TimeoutExpired as expired:
            code = None
            stdout = ""
            stderr = (expired.stderr or b"").decode(errors="replace") if isinstance(
                expired.stderr, bytes
            ) else (expired.stderr or "")
            stderr += f"\ncheck_replay: timed out after {CHECK_TIMEOUT_S}s\n"
        wall = round(time.monotonic() - started, 2)
        (args.out / f"{stem}.log").write_text(stderr)
        report = None
        if code in (0, 1) and stdout:
            report = f"{stem}.json"
            (args.out / report).write_text(stdout)
        rows.append(
            {
                "index": index,
                "commit": commit,
                "base": base,
                "subject": subject,
                "merge": len(parents) > 1,
                "exit_code": code,
                "wall_s": wall,
                "report": report,
            }
        )
        print(f"{args.slug} {stem} exit {code} in {wall}s: {subject}", flush=True)
    with (args.out / "commits.jsonl").open("w") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True) + "\n")
    failed = [row for row in rows if row["exit_code"] not in (0, 1)]
    print(
        f"{args.slug} shard {args.shard}/{args.shards}: {len(rows)} checked, "
        f"{len(failed)} without a report",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
