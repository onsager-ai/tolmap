#!/usr/bin/env bash
# Generate the complete map set the app prefers, into <repo>/.maps.
#
# The committed fixtures in data/ are the acceptance corpus, not a demo set:
# seven of the nine were recorded with --no-parcels and carry no `P` block, so
# the app's "plots" geometry has nothing to draw for them. This rebuilds all
# nine from the pinned clones WITH parcels, and adds a map of tolmap itself.
#
# The naming cache is seeded from each committed fixture first, so district
# names come back verbatim rather than being re-derived by the IDF fallback --
# name drift invalidates every spatial memory a reader has built, which is
# worse than a mediocre name (naming.py, finding 4).
#
#   web/scripts/generate-maps.sh [--repos DIR]
#
# --repos names a directory of clones (DIR/<name>), default $TOLMAP_FIXTURE_REPOS.
# A repo that is absent is skipped with a warning rather than cloned: this is a
# convenience for local development, and eval/verify_fixtures.py is the thing
# that clones and checks pins.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
MANIFEST="$ROOT/data/fixtures.toml"
OUT="$ROOT/.maps"
REPOS=${TOLMAP_FIXTURE_REPOS:-}
PY=${TOLMAP_PYTHON:-"$ROOT/.venv/bin/python"}

while [ $# -gt 0 ]; do
  case "$1" in
    --repos) REPOS=$2; shift 2;;
    *) echo "unknown argument: $1" >&2; exit 2;;
  esac
done

[ -f "$MANIFEST" ] || {
  echo "no $MANIFEST — the fixture manifest names each repo's pkg and lang, and this script reads it" >&2
  exit 1
}
[ -n "$REPOS" ] || { echo "no --repos DIR and no \$TOLMAP_FIXTURE_REPOS" >&2; exit 2; }
[ -x "$PY" ] || { echo "no interpreter at $PY (set \$TOLMAP_PYTHON)" >&2; exit 2; }

mkdir -p "$OUT"
cd "$ROOT"

names=$("$PY" - "$MANIFEST" <<'PYEOF'
import sys, tomllib
with open(sys.argv[1], "rb") as fh:
    print("\n".join(f"{k} {v['pkg']} {v['lang']}" for k, v in tomllib.load(fh).items()))
PYEOF
)

# Seed every naming cache before building anything: name_districts() keys its
# cache on a fingerprint of the membership, so a seeded miss is the correct
# signal that membership genuinely changed.
# shellcheck disable=SC2086
PYTHONPATH=src "$PY" eval/seed_names.py "$OUT" $(echo "$names" | cut -d' ' -f1)

while read -r name pkg lang; do
  if [ ! -d "$REPOS/$name" ]; then
    echo "skip $name: no clone at $REPOS/$name" >&2
    continue
  fi
  PYTHONPATH=src "$PY" -m tolmap.cli build "$REPOS/$name" \
    --pkg "$pkg" --lang "$lang" --name "$name" --out "$OUT" | tail -1
done <<< "$names"

# tolmap's own map. It has no import edges today because extract.resolve()
# drops every `from . import x` and this package uses nothing else (issue #12);
# the map is still a real one, and it is the shape a small repo genuinely has.
PYTHONPATH=src "$PY" -m tolmap.cli build "$ROOT" \
  --pkg src/tolmap --lang py --name tolmap --out "$OUT" | tail -1

echo "maps in $OUT"
