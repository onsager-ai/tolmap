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

printf 'export LEIDEN_PREFIX=%q\n' "$prefix"

