"""Measure district delivery over HTTP on a remote-build runner.

Run after `tolmap build`; the legacy row points at the same map and full
symbols sibling but has no pre-split directory. No repository is parsed by
this script except the one response chosen by file size.
"""

import argparse
import gzip
import http.client
import json
import os
from pathlib import Path
import socket
import sqlite3
import statistics
import subprocess
import tempfile
import time


def request(conn, path, gzip_accepted=False):
    headers = {"Accept-Encoding": "gzip"} if gzip_accepted else {}
    start = time.perf_counter_ns()
    conn.request("GET", path, headers=headers)
    response = conn.getresponse()
    body = response.read()
    elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
    if response.status != 200:
        raise RuntimeError(f"{path}: HTTP {response.status}: {body[:200]!r}")
    return body, response.getheader("Content-Encoding"), elapsed_ms


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("map", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    map_path = args.map.resolve()
    symbols_path = map_path.with_suffix(".symbols.json")
    district_dir = map_path.with_suffix(".symbols")
    largest = max(district_dir.glob("*.json"), key=lambda path: path.stat().st_size)
    district = int(largest.stem)

    with tempfile.TemporaryDirectory() as temp:
        temp_path = Path(temp)
        old_map = temp_path / "old.json"
        old_map.symlink_to(map_path)
        old_map.with_suffix(".symbols.json").symlink_to(symbols_path)
        db_path = temp_path / "store.sqlite3"
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = os.environ.copy()
        env.update({
            "TOLMAP_BIND_ADDR": "127.0.0.1",
            "TOLMAP_PORT": str(port),
            "TOLMAP_DB_PATH": str(db_path),
            "TOLMAP_CACHE_DIR": str(temp_path / "cache"),
        })
        process = subprocess.Popen([str(args.binary.resolve()), "serve"], env=env)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError(f"service exited with {process.returncode}")
                try:
                    probe = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
                    probe.request("GET", "/api/healthz")
                    if probe.getresponse().status == 200:
                        probe.close()
                        break
                except OSError:
                    time.sleep(0.1)
            else:
                raise RuntimeError("service did not become healthy")

            with sqlite3.connect(db_path) as db:
                for repo, path in (("new", map_path), ("old", old_map)):
                    db.execute(
                        "INSERT INTO maps (slug, owner, repo, commit_sha, branch, lang, "
                        "files, districts, modularity, map_path, indexed_at) "
                        "VALUES (?, 'bench', ?, 'abc', NULL, 'py', 0, 0, 0, ?, "
                        "'2026-09-24T00:00:00Z')",
                        (f"bench/{repo}", repo, str(path)),
                    )

            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
            paths = {
                "new": f"/api/maps/bench/new/symbols?district={district}",
                "old": f"/api/maps/bench/old/symbols?district={district}",
            }
            raw, encoding, _ = request(conn, paths["new"])
            assert encoding is None
            assert raw == largest.read_bytes()
            compressed, encoding, _ = request(conn, paths["new"], True)
            assert encoding == "gzip"
            assert gzip.decompress(compressed) == raw
            legacy, _, _ = request(conn, paths["old"])
            assert json.loads(legacy) == json.loads(raw)

            samples = {"new": [], "old": []}
            for _ in range(3):
                request(conn, paths["new"])
                request(conn, paths["old"])
            for i in range(20):
                for kind in (("new", "old") if i % 2 == 0 else ("old", "new")):
                    _, _, ms = request(conn, paths[kind])
                    samples[kind].append(ms)
            conn.close()
            result = {
                "district": district,
                "raw_http_bytes": len(raw),
                "gzip_http_bytes": len(compressed),
                "new_median_ms": statistics.median(samples["new"]),
                "legacy_median_ms": statistics.median(samples["old"]),
                "samples_each": 20,
                "method": "persistent localhost HTTP/1.1, alternating warm requests",
            }
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
