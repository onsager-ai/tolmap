#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} != --consumer ]]; then
  [[ $(id -u) == 0 ]]
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y --no-install-recommends ca-certificates curl git iproute2 python3 xz-utils libstdc++6 libgcc-s1 passwd procps strace
  rm -rf /var/lib/apt/lists/*
  curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" | tar -xJ -C /opt
  # Match the host's non-root uid so the private output mount stays writable
  # without changing its owner/mode or any production source-store permissions.
  [[ ${CONSUMER_UID:?} =~ ^[0-9]+$ && $CONSUMER_UID -ne 0 ]]
  useradd --uid "$CONSUMER_UID" --create-home consumer
  exec runuser --preserve-environment -u consumer -- env HOME=/home/consumer \
    bash /repo/scripts/npm-consumer-check.sh --consumer
fi
export PATH="/opt/node-v${NODE_VERSION}-linux-x64/bin:$PATH"
[[ $(id -u) != 0 && $(id -u) == "$CONSUMER_UID" ]]
id
python3 - <<'PY'
import json, os, subprocess
from pathlib import Path
Path('/tmp/consumer-identity.json').write_text(json.dumps(dict(
    uid=os.geteuid(), gid=os.getegid(), description=subprocess.check_output(['id'], text=True).strip())))
PY
consumer_exit() {
  local status=$?
  trap - EXIT
  if [[ $status -ne 0 ]]; then
    python3 /repo/scripts/npm-consumer-diagnostics.py "$status" || true
  fi
  exit "$status"
}
trap consumer_exit EXIT
node --version | grep -Fx "v${NODE_VERSION}"
node -p 'process.report.getReport().header.glibcVersionRuntime' | grep -Fx 2.35
printf '{}\n' > /tmp/consumer-checks.json
passed() {
  python3 - "$1" <<'PY'
import json, sys
from pathlib import Path
p=Path('/tmp/consumer-checks.json'); checks=json.loads(p.read_text())
checks[sys.argv[1]]='passed'; p.write_text(json.dumps(checks))
PY
  echo "PASS $1"
}
for tool in cargo rustc gcc g++ cmake; do
  if command -v "$tool"; then echo "unexpected compiler: $tool" >&2; exit 1; fi
done
if ldconfig -p | grep -Ei 'lib(igraph|libleidenalg)'; then exit 1; fi
ldconfig -p | grep -F libstdc++.so.6
ldconfig -p | grep -F libgcc_s.so.1
passed clean-runtime
cd /artifacts
sha256sum -c SHA256SUMS
tarball=$(awk '$2 ~ /\.tgz$/ {print $2}' SHA256SUMS)
[[ -n "$tarball" ]]
grep -Fx -- "- Source commit: $EXPECTED_SHA" BUILD_INFO.md
tar -xOf "$tarball" package/BUILD_INFO.md > /tmp/tarball-build-info.md
cmp BUILD_INFO.md /tmp/tarball-build-info.md
passed artifact-identity
python3 /repo/scripts/npm-source-metadata.py /artifacts "$EXPECTED_SHA" > /tmp/source-metadata-result.json
passed source-metadata
while IFS= read -r member; do
  case "$member" in
    package/package.json|package/README.md|package/LICENSE|package/LICENSE-DISTRIBUTION.md|package/THIRD_PARTY_NOTICES.md|package/BUILD_INFO.md|package/SOURCE.json|package/SOURCE-BUILD.md) ;;
    package/bin/*|package/lib/*|package/web/dist/*|package/licenses/*) ;;
    *) echo "unexpected package path: $member" >&2; exit 1 ;;
  esac
done < PACK_LIST.txt
passed package-contents
mkdir /tmp/consumer
cd /tmp/consumer
printf '{"private":true}\n' > package.json
npm install --no-audit --no-fund --no-save "/artifacts/$tarball"
cli="$PWD/node_modules/.bin/tolmap"
"$cli" --help
"$cli" build --help
"$cli" local --help
passed cli
for override in TOLMAP_TEST_PLATFORM=darwin TOLMAP_TEST_ARCH=arm64 TOLMAP_TEST_GLIBC_VERSION=2.34 TOLMAP_TEST_GLIBC_VERSION=musl; do
  if env "$override" "$cli" --help >/tmp/unsupported.log 2>&1; then exit 1; fi
  grep -F 'supports linux-x64 with glibc >= 2.35' /tmp/unsupported.log
done
passed unsupported-platform
for name in flask httpx; do
  "$cli" build --graph "/repo/data/ci/${name}.graph.json" --name "$name" --out parity-out --refs hand --resolution 1.1
  "$cli" parity "/repo/data/${name}.json" "parity-out/${name}.json"
done
passed offline-parity
mkdir -p tests/fixtures scripts
cp -a /repo/tests/fixtures/service_identity tests/fixtures/service_identity
cp /repo/scripts/local-reader-fixture.py scripts/
TOLMAP_BINARY="$cli" TOLMAP_TEST_PACKAGED_DEFAULT_STATIC=1 \
  TOLMAP_TEST_WRITE_TRACE=/tmp/reader-worker-writes.log timeout 900 python3 scripts/local-reader-fixture.py
passed reader-cancellation
# A writable same-SHA checkout, with no host git safe-directory dependency.
git -c safe.directory=/repo clone --no-hardlinks /repo /tmp/source
[[ "$(git -C /tmp/source rev-parse HEAD)" == "$EXPECTED_SHA" ]]
"$cli" build /tmp/source --pkg src --lang rs --name tolmap-consumer-smoke --out real-build-out --refs hand
python3 - <<'PY'
import json
from pathlib import Path
p=Path('real-build-out'); d=json.loads((p/'tolmap-consumer-smoke.json').read_text())
counts={'files':len(d['F']), 'districts':len(d['districts'])}
assert counts['files'] >= 20 and counts['districts'] >= 1, counts
symbols=list(p.glob('*symbols*.json')); assert symbols, list(p.iterdir())
for s in symbols: assert json.loads(s.read_text())
Path('/tmp/build-counts.json').write_text(json.dumps(counts))
print(counts)
PY
passed real-rs-build
python3 /repo/scripts/npm-viewer-check.py
passed viewer-assets-source-loopback
TOLMAP_BINARY="$cli" python3 /repo/scripts/npm-launcher-check.py
passed launcher-signals
python3 /repo/scripts/npm-consumer-result.py write
echo 'PASS consumer completion record written'
