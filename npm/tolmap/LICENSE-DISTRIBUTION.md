# Terms for this distribution

The bundled native executable `lib/tolmap`, linked with libleidenalg 0.12.0
and igraph 1.0.0, is conveyed as a combined work under **GPL-3.0-or-later**.
The npm package's SPDX license field describes these distribution terms.
You may run, study, modify and redistribute it under those terms. There is
no warranty; see the included GPL text in `licenses/native/`.

The tolmap authors' original source remains MIT licensed. The repository's
unaltered MIT `LICENSE` is included. This distribution does not change the
license of that source when obtained separately. Third-party components retain
their copyright notices and licenses, included under `licenses/`.

Upstream grants at the actual source tags:

- [libleidenalg 0.12.0 README, license paragraph](https://github.com/vtraag/libleidenalg/blob/0.12.0/README.md#license): GPL version 3 or any later version; its LICENSE supplies the GPLv3 text.
- [igraph 1.0.0 source header](https://github.com/igraph/igraph/blob/1.0.0/src/graph/type_indexededgelist.c): GPL version 2 or any later version. We exercise that grant under GPLv3-or-later for this linked distribution. Its vendored components and notices are retained in the original source archive and copied license tree.
- Bundled Ubuntu runtime libraries are enumerated from the actual dependency
  closure, with binary and source package names, exact versions and copyright
  files in `THIRD_PARTY_NOTICES.md` and `licenses/runtime/`.
- Rust dependency sources, licenses and checksums are included in the source
  bundle's `vendor/`. Production web dependency notices are in `licenses/web/`;
  their original sources accompany the bundle in `web-packages/`.

## Corresponding source accompanies the binary

`source/corresponding-source.tar.gz` is included in this npm package, together
with its SHA256SUMS. It contains the exact tolmap source commit, Cargo.lock,
build.rs, native bridge, web source/lockfile, build and installation scripts,
vendored Rust dependencies, the exact native source archives used by the build,
and source packages for every non-system runtime library bundled from Ubuntu.
The internal `BUILD.md`, `BUILD-ENV.txt` and `SHA256SUMS` describe reproduction,
tool versions, source URLs and checksums. Extract that archive to modify and
rebuild; no source-hosting lifetime or future written offer is required.
