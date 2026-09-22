#!/usr/bin/env python3
"""Fail if fly.toml (production) and deploy/railway.staging.env (staging)
stop describing the same set of TOLMAP_* knobs.

Staging exists to be evidence about production. The moment one side gains a
limit the other does not have -- a larger clone budget, a longer job
timeout -- a repository that indexes on staging tells you nothing about
whether it indexes on prod, and nothing anywhere says so out loud. This
check is cheap and catches that at review time.

It compares KEY SETS, not values. Values are allowed to differ (the port is
forwarded from Railway's injected PORT, paths differ by platform); a value
divergence that matters is a review question, not something a script can
judge. What a script can judge is whether someone added a knob to one file
and forgot the other.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FLY = ROOT / "fly.toml"
STAGING = ROOT / "deploy" / "railway.staging.env"

# Keys that legitimately exist on one side only, each with the reason it
# does. Anything not listed here must appear in both files.
EXPECTED_ONLY_IN_STAGING = {
    # src/service/config.rs reads TOLMAP_PORT; Railway injects PORT, so
    # staging forwards it. Fly matches the compiled-in default via
    # fly.toml's internal_port and leaves the variable unset.
    "TOLMAP_PORT",
    # owner: staging first, prod after a look, 2026-09-22
    "TOLMAP_TERRAIN",
}
EXPECTED_ONLY_IN_FLY: set[str] = set()


def fly_env_keys() -> set[str]:
    with FLY.open("rb") as fh:
        return set(tomllib.load(fh).get("env", {}))


def staging_env_keys() -> set[str]:
    keys = set()
    for line in STAGING.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        m = re.match(r"^([A-Z0-9_]+)=", line)
        if m:
            keys.add(m.group(1))
    return keys


def main() -> int:
    fly, staging = fly_env_keys(), staging_env_keys()
    only_fly = fly - staging - EXPECTED_ONLY_IN_FLY
    only_staging = staging - fly - EXPECTED_ONLY_IN_STAGING

    if not only_fly and not only_staging:
        print(f"deploy env parity OK: {len(fly & staging)} shared keys")
        return 0

    print("deploy env parity FAILED -- prod and staging describe different knobs.")
    for k in sorted(only_fly):
        print(f"  {k}: in fly.toml [env], missing from deploy/railway.staging.env")
    for k in sorted(only_staging):
        print(f"  {k}: in deploy/railway.staging.env, missing from fly.toml [env]")
    print()
    print("Add it to both, or, if the divergence is deliberate, record it in")
    print("this script's EXPECTED_ONLY_IN_* sets with the reason it is one.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
