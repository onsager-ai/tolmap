#!/usr/bin/env bash
set -euo pipefail

prefix=${1:-"$PWD/.native/leiden"}
work=$(mktemp -d "${TMPDIR:-/tmp}/tolmap-leiden.XXXXXX")
trap 'rm -rf -- "$work"' EXIT

igraph_version=1.0.0
leiden_version=0.12.0

curl -fsSL --retry 3 \
  "https://github.com/igraph/igraph/releases/download/${igraph_version}/igraph-${igraph_version}.tar.gz" \
  -o "$work/igraph.tar.gz"
curl -fsSL --retry 3 \
  "https://github.com/vtraag/libleidenalg/archive/refs/tags/${leiden_version}.tar.gz" \
  -o "$work/libleidenalg.tar.gz"
tar -xzf "$work/igraph.tar.gz" -C "$work"
tar -xzf "$work/libleidenalg.tar.gz" -C "$work"

cmake -S "$work/igraph-${igraph_version}" -B "$work/build-igraph" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DBUILD_SHARED_LIBS=ON \
  -DIGRAPH_ENABLE_TLS=OFF \
  -DIGRAPH_ENABLE_LTO=OFF
cmake --build "$work/build-igraph" --parallel
cmake --install "$work/build-igraph"

cmake -S "$work/libleidenalg-${leiden_version}" -B "$work/build-leiden" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DCMAKE_PREFIX_PATH="$prefix" \
  -DBUILD_SHARED_LIBS=ON
cmake --build "$work/build-leiden" --parallel
cmake --install "$work/build-leiden"

# The npm distribution includes the licenses for the exact native source
# archives used to build it. Keep this opt-in so the ordinary CI/developer
# prefix has no extra files and retains the same build behavior.
if [[ -n "${TOLMAP_THIRD_PARTY_LICENSES_DIR:-}" ]]; then
  mkdir -p "$TOLMAP_THIRD_PARTY_LICENSES_DIR"
  copy_source_license() {
    local project=$1 version=$2 source_dir=$3 license_file count=0
    while IFS= read -r -d '' license_file; do
      cp "$license_file" "$TOLMAP_THIRD_PARTY_LICENSES_DIR/${project}-${version}-$(basename "$license_file")"
      count=$((count + 1))
    done < <(find "$source_dir" -maxdepth 2 -type f \
      \( -iname 'COPYING' -o -iname 'COPYING.*' -o -iname 'LICENSE' -o -iname 'LICENSE.*' \) \
      -print0)
    if [[ "$count" -eq 0 ]]; then
      echo "no license file found in $source_dir" >&2
      exit 1
    fi
  }
  copy_source_license igraph "$igraph_version" "$work/igraph-${igraph_version}"
  copy_source_license libleidenalg "$leiden_version" "$work/libleidenalg-${leiden_version}"
fi

printf 'export LEIDEN_PREFIX=%q\n' "$prefix"
