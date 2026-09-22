#!/usr/bin/env python3
"""Pin, clone, and build the repository evaluation corpus.

The normal input is eval/corpus.toml.  During candidate screening, pass a
newline-delimited list of owner/repo slugs with ``--candidates``; their
default-branch HEADs are resolved once, written to builds.json, and used for
the build.  This keeps pinning and measuring in the same resumable path.

Maps, clones, logs, and build metadata live below $TOLMAP_CORPUS_DIR (default
~/.cache/tolmap-corpus), never in this repository.  A successful map is
reused only when builds.json says it came from the manifest's exact commit.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "eval" / "corpus.toml"
DEFAULT_CORPUS_DIR = Path(
    os.environ.get("TOLMAP_CORPUS_DIR", "~/.cache/tolmap-corpus")
).expanduser()
DEFAULT_BINARY = ROOT / "target" / "release" / "tolmap"
MIN_FREE_BYTES = 25 * 1024**3


@dataclass(frozen=True)
class Repo:
    slug: str
    commit: str
    band: str
    args: tuple[str, ...]

    @property
    def stem(self) -> str:
        return self.slug.replace("/", "__")


def load_manifest(path: Path) -> list[Repo]:
    with path.open("rb") as handle:
        raw = tomllib.load(handle)
    entries = raw.get("repo", [])
    if not isinstance(entries, list):
        raise SystemExit(f"{path}: expected [[repo]] entries")
    repos = []
    for entry in entries:
        args = entry.get("args", ["--all-sources"])
        repos.append(Repo(entry["slug"], entry["commit"], entry["band"], tuple(args)))
    return repos


def run_checked(command: list[str], **kwargs) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=True, text=True, **kwargs)


def resolve_head(slug: str) -> str:
    proc = run_checked(
        ["git", "ls-remote", f"https://github.com/{slug}.git", "HEAD"],
        capture_output=True,
    )
    fields = proc.stdout.split()
    if len(fields) < 2 or len(fields[0]) != 40:
        raise RuntimeError(f"could not resolve default-branch HEAD for {slug}")
    return fields[0]


def load_candidates(path: Path, jobs: int, previous: dict[str, dict]) -> list[Repo]:
    candidates = []
    for raw in path.read_text().splitlines():
        fields = raw.split("#", 1)[0].split()
        if not fields:
            continue
        if fields[0] in {"small", "medium", "large", "ultra", "candidate"}:
            band, slug, *build_args = fields
        else:
            band, slug, build_args = "candidate", fields[0], fields[1:]
        if band not in {"small", "medium", "large", "ultra", "candidate"}:
            raise SystemExit(f"{path}: unknown candidate band {band!r}")
        candidates.append((slug, band, tuple(build_args or ["--all-sources"])))
    slugs = [slug for slug, _, _ in candidates]
    pinned = {
        slug: previous[slug]["commit"]
        for slug in slugs
        if len(previous.get(slug, {}).get("commit", "")) == 40
    }
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = {
            pool.submit(resolve_head, slug): slug
            for slug in slugs
            if slug not in pinned
        }
        for future in concurrent.futures.as_completed(futures):
            slug = futures[future]
            pinned[slug] = future.result()
            print(f"{slug}: pinned {pinned[slug]}", flush=True)
    return [Repo(slug, pinned[slug], band, build_args) for slug, band, build_args in candidates]


def git(repo: Path, *args: str, **kwargs) -> subprocess.CompletedProcess[str]:
    return run_checked(["git", "-C", str(repo), *args], **kwargs)


def prepare_clone(spec: Repo, clone: Path) -> None:
    if (clone / ".git").is_dir():
        dirty = git(clone, "status", "--porcelain", capture_output=True).stdout.strip()
        if dirty:
            raise RuntimeError("dirty cached clone; refusing to overwrite it")
    elif clone.exists():
        raise RuntimeError(f"clone path exists but is not a git repository: {clone}")
    else:
        clone.parent.mkdir(parents=True, exist_ok=True)
        run_checked(
            [
                "git",
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                f"https://github.com/{spec.slug}.git",
                str(clone),
            ],
            stdout=subprocess.DEVNULL,
        )
    have = subprocess.run(
        ["git", "-C", str(clone), "cat-file", "-e", f"{spec.commit}^{{commit}}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ).returncode == 0
    if not have:
        git(clone, "fetch", "--filter=blob:none", "origin", spec.commit)
    # The clone is deliberately not shallow. extract.rs walks at most 4,000
    # commits, while the service's 200,000 setting is an admission limit and
    # does not change that extraction walk (src/service/config.rs).
    git(clone, "checkout", "--quiet", "--detach", spec.commit)


def time_fields(path: Path) -> tuple[float | None, float | None]:
    elapsed = None
    peak_mb = None
    if not path.exists():
        return elapsed, peak_mb
    for line in path.read_text(errors="replace").splitlines():
        key, _, value = line.partition(": ")
        if "Maximum resident set size" in key:
            peak_mb = float(value.strip()) / 1024.0
        elif "Elapsed (wall clock) time" in key:
            parts = value.strip().split(":")
            try:
                elapsed = sum(float(part) * 60**index for index, part in enumerate(reversed(parts)))
            except ValueError:
                pass
    return elapsed, peak_mb


def run_timed(
    command: list[str],
    log_path: Path,
    time_path: Path,
    timeout: int,
    memory_gib: float,
    env: dict[str, str],
) -> tuple[int, bool, float, float | None, float | None]:
    started = time.monotonic()
    with log_path.open("w") as log:
        proc = subprocess.Popen(
            [
                "/usr/bin/time",
                "-v",
                "-o",
                str(time_path),
                "/usr/bin/prlimit",
                f"--as={int(memory_gib * 1024**3)}",
                "--",
                *command,
            ],
            stdout=log,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
            env=env,
        )
        timed_out = False
        try:
            returncode = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                returncode = proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                returncode = proc.wait()
    measured_wall, peak_mb = time_fields(time_path)
    return returncode, timed_out, time.monotonic() - started, measured_wall, peak_mb


def failure_tail(path: Path, lines: int = 8) -> str:
    if not path.exists():
        return "no build output"
    tail = path.read_text(errors="replace").splitlines()[-lines:]
    return " | ".join(part.strip() for part in tail if part.strip())[-1200:]


def load_results(path: Path) -> dict[str, dict]:
    if not path.exists():
        return {}
    with path.open() as handle:
        raw = json.load(handle)
    return raw.get("repos", raw)


def save_results(path: Path, results: dict[str, dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as handle:
        json.dump({"version": 1, "repos": results}, handle, indent=2, sort_keys=True)
        handle.write("\n")
        temp = Path(handle.name)
    temp.replace(path)


def map_matches_pin(spec: Repo, map_path: Path, results: dict[str, dict]) -> bool:
    row = results.get(spec.slug, {})
    return (
        map_path.is_file()
        and row.get("commit") == spec.commit
        and row.get("args") == list(spec.args)
        and row.get("status") == "built"
    )


def build_one(
    spec: Repo,
    corpus: Path,
    binary: Path,
    timeout: int,
    memory_gib: float,
    ld_library_path: str,
) -> dict:
    clone = corpus / "clones" / spec.stem
    maps = corpus / "maps"
    logs = corpus / "logs"
    maps.mkdir(parents=True, exist_ok=True)
    logs.mkdir(parents=True, exist_ok=True)
    log_path = logs / f"{spec.stem}.log"
    time_path = logs / f"{spec.stem}.time"
    result = {
        "slug": spec.slug,
        "commit": spec.commit,
        "band": spec.band,
        "args": list(spec.args),
        "status": "failed",
    }
    try:
        prepare_clone(spec, clone)
        with tempfile.TemporaryDirectory(prefix=f"tolmap-{spec.stem}-", dir=corpus) as out:
            command = [
                str(binary),
                "build",
                str(clone),
                *spec.args,
                "--name",
                spec.stem,
                "--out",
                out,
            ]
            env = dict(os.environ)
            env_path = env.get("LD_LIBRARY_PATH", "")
            env["LD_LIBRARY_PATH"] = ld_library_path + (os.pathsep + env_path if env_path else "")
            code, timed_out, observed, measured, peak_mb = run_timed(
                command, log_path, time_path, timeout, memory_gib, env
            )
            result.update(
                build_seconds=measured if measured is not None else observed,
                peak_rss_mb=peak_mb,
                returncode=code,
            )
            built = Path(out) / f"{spec.stem}.json"
            if timed_out:
                raise RuntimeError(f"timed out after {timeout}s")
            if code != 0:
                raise RuntimeError(f"build exited {code}: {failure_tail(log_path)}")
            if not built.is_file():
                raise RuntimeError("build succeeded without producing its map")
            with built.open() as handle:
                files = len(json.load(handle)["F"])
            shutil.move(str(built), maps / f"{spec.stem}.json")
            result.update(status="built", files=files)
    except Exception as exc:  # one repository must not stop the corpus run
        result["reason"] = str(exc)
    return result


def maybe_evict(clone: Path, map_path: Path) -> None:
    free = shutil.disk_usage(clone.parent).free
    if free >= MIN_FREE_BYTES or not map_path.is_file() or not clone.exists():
        return
    # Both paths are fully resolved under corpus-controlled directories; do
    # not accept a glob or unresolved environment variable as a delete target.
    shutil.rmtree(clone)
    print(f"disk below 25 GiB: evicted completed clone {clone.name}", flush=True)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--candidates", type=Path, help="newline-delimited owner/repo slugs to pin and build")
    parser.add_argument("--corpus-dir", type=Path, default=DEFAULT_CORPUS_DIR)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--band", choices=("small", "medium", "large", "ultra", "candidate"))
    parser.add_argument("--timeout", type=int, default=3600)
    parser.add_argument("--memory-gb", type=float, default=24.0)
    parser.add_argument("--force", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.jobs < 1 or args.timeout < 1 or args.memory_gb <= 0:
        raise SystemExit("--jobs, --timeout, and --memory-gb must be positive")
    corpus = args.corpus_dir.expanduser().resolve()
    binary = args.binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"release binary not found: {binary}")
    corpus.mkdir(parents=True, exist_ok=True)
    results_path = corpus / "builds.json"
    results = load_results(results_path)
    repos = load_candidates(args.candidates, args.jobs, results) if args.candidates else load_manifest(args.manifest)
    if args.candidates:
        for repo in repos:
            current = results.get(repo.slug)
            if current and current.get("status") in {"built", "failed"}:
                continue
            results[repo.slug] = {
                "slug": repo.slug,
                "commit": repo.commit,
                "band": repo.band,
                "args": list(repo.args),
                "status": "pending",
            }
        # Candidate HEADs become pins before the first clone begins. A later
        # resume reads these exact SHAs even if the upstream default branches
        # have advanced in the meantime.
        save_results(results_path, results)
    if args.band:
        repos = [repo for repo in repos if repo.band == args.band]
    pending = []
    for spec in repos:
        map_path = corpus / "maps" / f"{spec.stem}.json"
        if not args.force and map_matches_pin(spec, map_path, results):
            print(f"{spec.slug}: skip (map already built at {spec.commit})")
        else:
            pending.append(spec)

    lock = threading.Lock()
    ld_library_path = str(Path.home() / ".local" / "leiden" / "lib")

    def record(spec: Repo) -> dict:
        print(f"{spec.slug}: building {spec.commit}", flush=True)
        result = build_one(spec, corpus, binary, args.timeout, args.memory_gb, ld_library_path)
        with lock:
            results[spec.slug] = result
            save_results(results_path, results)
        print(f"{spec.slug}: {result['status']}" + (f" ({result.get('files')} files)" if result["status"] == "built" else f": {result.get('reason')}"), flush=True)
        maybe_evict(corpus / "clones" / spec.stem, corpus / "maps" / f"{spec.stem}.json")
        return result

    regular = [repo for repo in pending if repo.band != "ultra"]
    ultras = [repo for repo in pending if repo.band == "ultra"]
    failures = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for result in pool.map(record, regular):
            failures += result["status"] != "built"
    # Ultra repositories do not overlap one another or a smaller build; their
    # working trees and geometry phases are the corpus's largest memory users.
    for spec in ultras:
        failures += record(spec)["status"] != "built"
    print(f"finished {len(pending)} build(s): {failures} failure(s); metadata {results_path}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
