#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
sha=${1:?usage: package-npm.sh <40-character-source-sha> <artifact-directory>}
artifact_dir=${2:?usage: package-npm.sh <40-character-source-sha> <artifact-directory>}
leiden_prefix=${LEIDEN_PREFIX:?LEIDEN_PREFIX must point to the pinned native install}
native_license_dir=${TOLMAP_THIRD_PARTY_LICENSES_DIR:?TOLMAP_THIRD_PARTY_LICENSES_DIR must contain native source licenses}

if [[ ! "$sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "expected a 40-character lowercase source SHA, got: $sha" >&2
  exit 1
fi
if [[ "$(git -C "$repo" rev-parse HEAD)" != "$sha" ]]; then
  echo "checkout HEAD does not match requested package SHA $sha" >&2
  exit 1
fi

rust_version=$(python3 - "$repo/Cargo.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as source:
    print(tomllib.load(source)["package"]["version"])
PY
)
npm_version=$(node -p "require('$repo/npm/tolmap/package.json').version")
if [[ "$rust_version" != "$npm_version" ]]; then
  echo "Cargo.toml version $rust_version does not match npm/tolmap/package.json $npm_version" >&2
  exit 1
fi

command -v patchelf >/dev/null || { echo 'patchelf is required' >&2; exit 1; }
command -v readelf >/dev/null || { echo 'readelf is required' >&2; exit 1; }
command -v ldd >/dev/null || { echo 'ldd is required' >&2; exit 1; }
[[ -d "$leiden_prefix/lib" && -d "$native_license_dir" ]] || {
  echo 'pinned Leiden/igraph libraries and source licenses are required' >&2
  exit 1
}

TOLMAP_BUILD_COMMIT="$sha" LEIDEN_PREFIX="$leiden_prefix" \
  cargo build --locked --release --bin tolmap --manifest-path "$repo/Cargo.toml"

artifact_dir=$(mkdir -p "$artifact_dir" && cd "$artifact_dir" && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/tolmap-npm-stage.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
mkdir -p "$stage/bin" "$stage/lib" "$stage/web" "$stage/licenses/native" "$stage/licenses/runtime"
cp "$repo/npm/tolmap/package.json" "$stage/package.json"
cp "$repo/npm/tolmap/bin/tolmap.js" "$stage/bin/tolmap.js"
# Keep documentation links valid when publishing a reviewed branch before merge.
sed "s|https://github.com/onsager-ai/tolmap/blob/main/docs/NPM.md|https://github.com/onsager-ai/tolmap/blob/$sha/docs/NPM.md|g" \
  "$repo/npm/tolmap/README.md" > "$stage/README.md"
cp "$repo/LICENSE" "$stage/LICENSE"
cp "$repo/npm/tolmap/LICENSE-DISTRIBUTION.md" "$stage/LICENSE-DISTRIBUTION.md"
cp "$repo/npm/tolmap/SOURCE-BUILD.md" "$stage/SOURCE-BUILD.md"
cp -a "$repo/web/dist" "$stage/web/dist"
cp -a "$native_license_dir"/. "$stage/licenses/native/"
cp "$repo/target/release/tolmap" "$stage/lib/tolmap"
chmod 755 "$stage/bin/tolmap.js" "$stage/lib/tolmap"

machine=$(readelf -h "$stage/lib/tolmap" | sed -n 's/^  Machine: *//p')
if [[ "$machine" != 'Advanced Micro Devices X86-64' ]]; then
  echo "expected an x86-64 executable, got: $machine" >&2
  exit 1
fi

is_base_library() {
  case "$1" in
    linux-vdso.so.1|ld-linux-x86-64.so.2|libc.so.6|libm.so.6|libdl.so.2|libpthread.so.0|librt.so.1|libresolv.so.2|libutil.so.1|libstdc++.so.6|libgcc_s.so.1)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

declare -A runtime_packages=()
runtime_sources=()
runtime_source_specs=()
copy_runtime_license() {
  local source_path=$1 owner_output owner package version copyright destination source_package source_version
  owner_output=$(dpkg-query -S "$source_path" 2>/dev/null | head -n 1 || true)
  if [[ -z "$owner_output" ]]; then
    source_path=$(readlink -f "$source_path")
    owner_output=$(dpkg-query -S "$source_path" 2>/dev/null | head -n 1 || true)
  fi
  if [[ -z "$owner_output" ]]; then
    echo "cannot identify the Debian package/license for bundled runtime library $source_path" >&2
    exit 1
  fi
  owner=${owner_output%%: *}
  package=${owner%%:*}
  if [[ -n "${runtime_packages[$package]:-}" ]]; then return; fi
  version=$(dpkg-query -W -f='${Version}' "$owner" 2>/dev/null || dpkg-query -W -f='${Version}' "$package")
  source_package=$(dpkg-query -W -f='${source:Package}' "$owner")
  source_version=$(dpkg-query -W -f='${source:Version}' "$owner")
  [[ -n "$source_package" && -n "$source_version" ]]
  runtime_source_specs+=("$source_package=$source_version")
  copyright="/usr/share/doc/$package/copyright"
  if [[ ! -f "$copyright" ]]; then
    echo "no Debian copyright text at $copyright for bundled runtime library $source_path" >&2
    exit 1
  fi
  destination="licenses/runtime/${package}-${version}-copyright"
  cp "$copyright" "$stage/$destination"
  runtime_packages[$package]=1
  runtime_sources+=("$package $version $destination https://launchpad.net/ubuntu/+source/$source_package/$source_version $source_package $source_version")
}

collect_dependencies() {
  local target=$1 listing line needed _arrow resolved destination
  listing=$(LD_LIBRARY_PATH="$leiden_prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ldd "$target") || {
    echo "ldd failed for $target" >&2
    exit 1
  }
  while IFS= read -r line; do
    if [[ "$line" == *'not found'* ]]; then
      echo "unresolved native dependency in $target: $line" >&2
      exit 1
    fi
    if [[ "$line" == *' => '* ]]; then
      read -r needed _arrow resolved _ <<< "$line"
    else
      continue
    fi
    if is_base_library "$needed"; then continue; fi
    if [[ ! -f "$resolved" ]]; then
      echo "native dependency path does not exist: $needed => $resolved" >&2
      exit 1
    fi
    destination="$stage/lib/$needed"
    if [[ ! -e "$destination" ]]; then
      cp -L "$resolved" "$destination"
      if [[ "$resolved" != "$leiden_prefix/"* ]]; then
        copy_runtime_license "$resolved"
      fi
    fi
  done <<< "$listing"
}

# ldd on the executable reports its entire dependency closure. Repeat after
# staging libraries as a guard against a copied library adding another edge.
collect_dependencies "$stage/lib/tolmap"
while :; do
  before=$(find "$stage/lib" -maxdepth 1 -type f -name '*.so*' | wc -l)
  for library in "$stage"/lib/*.so*; do
    [[ -f "$library" ]] || continue
    collect_dependencies "$library"
  done
  after=$(find "$stage/lib" -maxdepth 1 -type f -name '*.so*' | wc -l)
  [[ "$after" -gt "$before" ]] || break
done

for elf in "$stage/lib/tolmap" "$stage"/lib/*.so*; do
  [[ -f "$elf" ]] || continue
  # shellcheck disable=SC2016 # The ELF loader expands this token at runtime.
  patchelf --set-rpath '$ORIGIN' "$elf"
done

validate_dependencies() {
  local target=$1 listing line needed _arrow resolved
  listing=$(ldd "$target") || { echo "ldd failed after relocation: $target" >&2; exit 1; }
  while IFS= read -r line; do
    if [[ "$line" == *'not found'* ]]; then
      echo "unresolved dependency after relocation in $target: $line" >&2
      exit 1
    fi
    if [[ "$line" == *' => '* ]]; then
      read -r needed _arrow resolved _ <<< "$line"
      if is_base_library "$needed"; then continue; fi
      if [[ "$resolved" != "$stage/lib/"* ]]; then
        echo "non-base dependency is not bundled: $needed => $resolved" >&2
        exit 1
      fi
    fi
  done <<< "$listing"
}
validate_dependencies "$stage/lib/tolmap"
for library in "$stage"/lib/*.so*; do
  [[ -f "$library" ]] || continue
  validate_dependencies "$library"
done

node "$repo/scripts/collect-npm-web-licenses.mjs" "$repo/web" "$stage/licenses/web"
cat > "$stage/BUILD_INFO.md" <<EOF
# Build information

- Package: tolmap@$npm_version
- Source commit: $sha
- Build image: Ubuntu 22.04 x86-64 (glibc 2.35)
- Native libraries: igraph 1.0.0 and libleidenalg 0.12.0
- Web build: Node 24.21.0, pnpm 9.15.9, frozen lockfile
- Runtime scope: linux-x64 with glibc >= 2.35
EOF

{
  cat <<EOF
# Third-party notices

tolmap's Rust source and original CLI are licensed under MIT; see [LICENSE](LICENSE). The executable is dynamically linked with igraph and libleidenalg, and the production viewer bundles web dependencies. License texts are included under licenses/. See [LICENSE-DISTRIBUTION.md](LICENSE-DISTRIBUTION.md) for GPL-3.0-or-later linked-distribution terms and accompanying corresponding source.

| Component | Version | License/source | Included text |
| --- | --- | --- | --- |
| tolmap | $npm_version | MIT, https://github.com/onsager-ai/tolmap/tree/$sha | LICENSE |
| igraph | 1.0.0 | GPL-2.0-or-later, https://github.com/igraph/igraph/releases/tag/1.0.0 | licenses/native/ |
| libleidenalg | 0.12.0 | GPL-3.0-or-later, https://github.com/vtraag/libleidenalg/tree/0.12.0 | licenses/native/ |
EOF
  for source in "${runtime_sources[@]}"; do
    read -r package version file url source_package source_version <<< "$source"
    printf '| %s | %s | Debian Ubuntu source package: [%s](%s) | %s |\n' "$package" "$version" "$source_package $source_version" "$url" "$file"
  done
  cat <<'EOF'

## Production web dependencies

The exact installed versions, declared licenses, npm source tarballs and copied license files are listed in [licenses/web/THIRD_PARTY_NOTICES.md](licenses/web/THIRD_PARTY_NOTICES.md).
EOF
} > "$stage/THIRD_PARTY_NOTICES.md"

bash "$repo/scripts/npm-corresponding-source.sh" "$sha" "$artifact_dir" "$npm_version" "${runtime_source_specs[@]}"
source_asset="tolmap-$npm_version-corresponding-source-$sha.tar.gz"
python3 - "$artifact_dir" "$stage" "$sha" "$npm_version" "$source_asset" "${runtime_source_specs[@]}" <<'PY'
import json, pathlib, sys
root, stage = map(pathlib.Path, sys.argv[1:3])
sha, version, asset = sys.argv[3:6]
digest, filename = (root/'SOURCE_SHA256SUMS').read_text().split()
assert filename == asset
metadata = dict(source_commit=sha, package_version=version, source_asset=asset,
                url=f'https://github.com/onsager-ai/tolmap/releases/download/v{version}/{asset}',
                sha256=digest, size_bytes=(root/asset).stat().st_size,
                components=dict(native=[dict(name='igraph', version='1.0.0'),
                                        dict(name='libleidenalg', version='0.12.0')],
                                ubuntu=[dict(zip(('name', 'version'), spec.split('=', 1)))
                                        for spec in sorted(set(sys.argv[6:]))]))
content = json.dumps(metadata, indent=2) + '\n'
(root/'SOURCE.json').write_text(content)
(stage/'SOURCE.json').write_text(content)
# Generate the release-specific facts once; authored docs refer to SOURCE.json.
offer = (f'\n## Corresponding source for this package\n\n'
         f'- URL: {metadata["url"]}\n- SHA-256: `{metadata["sha256"]}`\n'
         f'- Size: {metadata["size_bytes"]} bytes\n- Source commit: `{sha}`\n'
         f'\nSee `SOURCE.json` and `SOURCE-BUILD.md` for verification and build instructions.\n')
for name in ('README.md', 'LICENSE-DISTRIBUTION.md', 'SOURCE-BUILD.md', 'BUILD_INFO.md'):
    with (stage/name).open('a') as doc:
        doc.write(offer)
PY
cp -a /usr/share/common-licenses "$stage/licenses/runtime/common-licenses"
cd "$stage"
npm pack --json --ignore-scripts --pack-destination "$artifact_dir" > "$artifact_dir/PACK_INFO.json"
tarball=$(python3 - "$artifact_dir" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
pack, = json.loads((root/'PACK_INFO.json').read_text())
sizes = dict(tarball_bytes=(root/pack['filename']).stat().st_size,
             unpacked_package_bytes=pack['unpackedSize'],
             source_bundle_bytes=json.loads((root/'SOURCE.json').read_text())['size_bytes'])
(root/'PACKAGE_SIZES.json').write_text(json.dumps(sizes, indent=2) + '\n')
if sizes['tarball_bytes'] > 67_108_864:
    raise SystemExit(f'npm tarball exceeds 64 MiB budget: {sizes["tarball_bytes"]} bytes')
print(pack['filename'])
PY
)
[[ "$tarball" == "tolmap-$npm_version.tgz" ]] || {
  echo "unexpected npm pack output: $tarball" >&2
  exit 1
}
tar -tzf "$artifact_dir/$tarball" > "$artifact_dir/PACK_LIST.txt"
cp "$stage/BUILD_INFO.md" "$artifact_dir/BUILD_INFO.md"
(cd "$artifact_dir" && sha256sum "$tarball" "$source_asset" > SHA256SUMS)
python3 "$repo/scripts/npm-source-metadata.py" "$artifact_dir" "$sha"
cat "$artifact_dir/SHA256SUMS"
stat --printf="npm tarball: %s bytes\n" "$artifact_dir/$tarball"
cat "$artifact_dir/PACKAGE_SIZES.json"
echo "created $artifact_dir/$tarball for $sha"
