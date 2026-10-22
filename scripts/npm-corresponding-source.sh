#!/usr/bin/env bash
# CI only. Ship source with every binary instead of relying on expiring CI URLs.
set -euo pipefail
sha=${1:?source SHA}
artifacts=${2:?artifact directory}
shift 2
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
source_dir="$work/corresponding-source"
mkdir -p "$source_dir/tolmap" "$source_dir/native-archives" "$source_dir/ubuntu" "$source_dir/web-packages"
git -C "$repo" archive "$sha" Cargo.toml Cargo.lock build.rs native src web scripts npm LICENSE README.md .github/workflows | tar -x -C "$source_dir/tolmap"
cp -a "${TOLMAP_NATIVE_SOURCE_DIR:?native archives required}"/. "$source_dir/native-archives/"
# All crates, with their original license/copyright files and cargo checksums.
(cd "$repo" && cargo vendor --locked "$source_dir/vendor") > "$source_dir/CARGO-CONFIG.toml"
(cd "$repo" && cargo metadata --locked --format-version 1) > "$source_dir/RUST-PACKAGES.json"
# Source package names/versions come from dpkg source:Package/source:Version,
# never guessed from binary names (libgmp10, for example, comes from gmp).
if [[ "$#" -gt 0 ]]; then
  sudo sed -n 's/^deb /deb-src /p' /etc/apt/sources.list | sudo tee /etc/apt/sources.list.d/tolmap-source.list >/dev/null
  sudo apt-get update
  for spec in "$@"; do
    printf '%s\n' "$spec" >> "$source_dir/ubuntu/PACKAGES.txt"
    (cd "$source_dir/ubuntu" && apt-get source --download-only "$spec")
    (cd "$source_dir/ubuntu" && apt-get --print-uris source "$spec") >> "$source_dir/ubuntu/URIS.txt"
  done
fi
# The web notice generator records the exact installed production closure.
node "$repo/scripts/collect-npm-web-licenses.mjs" "$repo/web" "$source_dir/web-notices"
python3 - "$source_dir" <<'PY'
import json, pathlib, sys, urllib.request
root=pathlib.Path(sys.argv[1])
packages=json.loads((root/'web-notices/packages.json').read_text())
for i, package in enumerate(packages):
    url=package['source']
    urllib.request.urlretrieve(url, root/'web-packages'/f'{i}.tgz')
PY
{
  printf 'Source commit: %s\n' "$sha"
  rustc --version
  cargo --version
  cmake --version
  gcc --version
  node --version
  pnpm --version
  dpkg-query -W build-essential cmake patchelf pkg-config libc6 libstdc++6 libgcc-s1
} > "$source_dir/BUILD-ENV.txt"
cp "$repo/npm/tolmap/SOURCE-BUILD.md" "$source_dir/BUILD.md"
printf '%s\n' "$sha" > "$source_dir/SOURCE_SHA"
(cd "$source_dir" && find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS)
tar -czf "$artifacts/corresponding-source.tar.gz" -C "$work" corresponding-source
(cd "$artifacts" && sha256sum corresponding-source.tar.gz > SOURCE_SHA256SUMS)
stat --printf='Corresponding source: %s bytes\n' "$artifacts/corresponding-source.tar.gz"
