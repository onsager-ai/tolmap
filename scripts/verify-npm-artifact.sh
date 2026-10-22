#!/usr/bin/env bash
set -euo pipefail

artifact_dir=${1:?usage: verify-npm-artifact.sh <artifact-directory> <repository-checkout> <source-sha>}
repo=${2:?usage: verify-npm-artifact.sh <artifact-directory> <repository-checkout> <source-sha>}
sha=${3:?usage: verify-npm-artifact.sh <artifact-directory> <repository-checkout> <source-sha>}
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid source SHA: $sha" >&2; exit 1; }
[[ "$(git -C "$repo" rev-parse HEAD)" == "$sha" ]] || { echo 'consumer checkout does not match artifact SHA' >&2; exit 1; }

docker run --rm --init \
  --volume "$artifact_dir:/artifacts:ro" \
  --volume "$repo:/repo:ro" \
  --env EXPECTED_SHA="$sha" \
  ubuntu:22.04 bash -s <<'CONTAINER'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends ca-certificates curl git iproute2 python3 xz-utils
rm -rf /var/lib/apt/lists/*

node_version=24.21.0
node_directory="/opt/node-v${node_version}-linux-x64"
curl -fsSL "https://nodejs.org/dist/v${node_version}/node-v${node_version}-linux-x64.tar.xz" \
  | tar -xJ -C /opt
export PATH="$node_directory/bin:$PATH"
node --version | grep -Fx "v${node_version}"
npm --version
node -p "process.report.getReport().header.glibcVersionRuntime" | grep -Fx '2.35'

for tool in cargo rustc gcc g++ cmake; do
  if command -v "$tool" >/dev/null 2>&1; then
    echo "consumer image unexpectedly has $tool" >&2
    exit 1
  fi
done
if ldconfig -p | grep -Eiq 'lib(igraph|libleidenalg)'; then
  echo 'consumer image unexpectedly has a system igraph/Leiden library' >&2
  exit 1
fi
ldconfig -p | grep -F 'libstdc++.so.6' >/dev/null
ldconfig -p | grep -F 'libgcc_s.so.1' >/dev/null

cd /artifacts
sha256sum -c SHA256SUMS
tarball=$(awk '$2 ~ /\.tgz$/ {print $2}' SHA256SUMS)
[[ -n "$tarball" ]]
grep -Fx -- "- Source commit: $EXPECTED_SHA" BUILD_INFO.md
tar -xOf "$tarball" package/BUILD_INFO.md > /tmp/tarball-build-info.md
cmp BUILD_INFO.md /tmp/tarball-build-info.md

while IFS= read -r member; do
  case "$member" in
    package/package.json|package/README.md|package/LICENSE|package/THIRD_PARTY_NOTICES.md|package/BUILD_INFO.md) ;;
    package/bin/*|package/lib/*|package/web/dist/*|package/licenses/*) ;;
    *) echo "unexpected tarball path: $member" >&2; exit 1 ;;
  esac
done < PACK_LIST.txt
if grep -E '(^|/)(node_modules|target|\.git)(/|$)|(^|/)Cargo\.(toml|lock)$' PACK_LIST.txt; then
  echo 'tarball contains a build tree or source checkout file' >&2
  exit 1
fi

mkdir -p /tmp/tolmap-consumer
cd /tmp/tolmap-consumer
printf '{"private":true}\n' > package.json
npm install --no-audit --no-fund --no-save "/artifacts/$tarball"

./node_modules/.bin/tolmap --help
./node_modules/.bin/tolmap build --help
./node_modules/.bin/tolmap local --help

if TOLMAP_TEST_PLATFORM=darwin ./node_modules/.bin/tolmap --help >/tmp/unsupported.log 2>&1; then
  echo 'unsupported-platform override unexpectedly succeeded' >&2
  exit 1
fi
grep -F 'supports linux-x64 with glibc >= 2.35' /tmp/unsupported.log

mkdir -p parity-out
for name in flask httpx; do
  ./node_modules/.bin/tolmap build \
    --graph "/repo/data/ci/${name}.graph.json" \
    --name "$name" \
    --out parity-out \
    --refs hand \
    --resolution 1.1
  ./node_modules/.bin/tolmap parity "/repo/data/${name}.json" "parity-out/${name}.json"
done

mkdir -p tests/fixtures scripts
cp -a /repo/tests/fixtures/service_identity tests/fixtures/service_identity
cp /repo/scripts/local-reader-fixture.py scripts/local-reader-fixture.py
TOLMAP_BINARY="$PWD/node_modules/.bin/tolmap" \
TOLMAP_TEST_PACKAGED_DEFAULT_STATIC=1 \
  timeout 900 python3 scripts/local-reader-fixture.py

./node_modules/.bin/tolmap build /repo \
  --pkg src --lang rust --name tolmap-consumer-smoke --out real-build-out --refs hand
python3 - <<'PY'
import json
from pathlib import Path
document = json.loads(Path("real-build-out/tolmap-consumer-smoke.json").read_text())
assert document.get("F"), "real repository build produced no file records"
print(f"real repository build emitted {len(document['F'])} file records")
PY

ss_output=$(ss -ltnH 'sport = :8988')
grep -F '127.0.0.1:8988' <<< "$ss_output"
if grep -Eq '0\.0\.0\.0:8988|\[::\]:8988' <<< "$ss_output"; then
  echo 'local viewer is listening on a non-loopback address' >&2
  exit 1
fi
curl -fsS http://127.0.0.1:8988/ > /tmp/tolmap-index.html
asset=$(node -e 'const s=require("fs").readFileSync("/tmp/tolmap-index.html","utf8"); const m=s.match(/(?:src|href)="([^\"]*\/assets\/[^\"]+\.(?:js|css))"/); if(!m) process.exit(1); console.log(m[1]);')
curl -fsS "http://127.0.0.1:8988$asset" -o /dev/null
snapshot=$(python3 -c 'import json; print(json.load(open(".local-reader/fixture.json"))["snapshotA"])')
commit=$(python3 -c 'import json; print(json.load(open(".local-reader/fixture.json"))["a"])')
curl -fsS "http://127.0.0.1:8988/api/maps/snapshot/$snapshot/provenance" > /tmp/tolmap-provenance.json
python3 - "$commit" <<'PY'
import json, sys
assert json.load(open("/tmp/tolmap-provenance.json"))["commit"] == sys.argv[1]
PY
curl -fsS "http://127.0.0.1:8988/api/maps/snapshot/$snapshot/source?path=shop/cart.py&commit=$commit" > /tmp/tolmap-source.json
python3 - <<'PY'
import json
source = json.load(open("/tmp/tolmap-source.json"))
assert source["status"] == "ready" and "def " in source["text"]
PY

echo 'PASS clean consumer install, CLI, source build, parity, packaged viewer assets, same-commit source and loopback bind'
CONTAINER
