# Rebuild the accompanying distribution

This archive accompanies the npm binary under GPL-3.0-or-later. All copyright
and license files in vendor/, native archives, Ubuntu source packages and web
packages are retained. The original tolmap LICENSE is MIT. SOURCE_SHA identifies
the exact source; BUILD-ENV.txt records the build runner's tool/package versions.
SHA256SUMS covers every source input. Verify it before rebuilding:

    sha256sum -c SHA256SUMS

Use Ubuntu 22.04 x86-64 with build-essential, cmake, pkg-config, patchelf, git,
curl, Python 3.12, and the Rust toolchain version recorded in BUILD-ENV.txt.
Node 24.21.0 and pnpm 9.15.9 build the web assets. These general purpose tools,
glibc, libgcc and libstdc++ are system prerequisites, not bundled libraries.
Runtime source archives downloaded from Ubuntu (ubuntu/) carry their upstream
archives, Debian patches and .dsc signature/checksum metadata. To rebuild those
libraries, install dpkg-dev and that source package's build dependencies, run
`dpkg-source -x <name>.dsc`, then `dpkg-buildpackage -b -uc -us` inside it.
Install the resulting architecture-appropriate runtime .debs before packaging.
The exact source package names, versions and download URIs are in ubuntu/.

From this directory:

    export TOLMAP_BUILD_COMMIT=$(cat SOURCE_SHA)
    export TOLMAP_NATIVE_ARCHIVES="$PWD/native-archives"
    export LEIDEN_PREFIX="$PWD/native-install"
    cd tolmap
    mkdir -p .cargo
    # vendor/ is next to tolmap/, so replace only this generated directory path.
    sed "s|directory = .*|directory = \"../vendor\"|" ../CARGO-CONFIG.toml > .cargo/config.toml
    bash scripts/install-leiden.sh "$LEIDEN_PREFIX"
    cargo build --offline --locked --release --bin tolmap
    corepack enable
    corepack prepare pnpm@9.15.9 --activate
    pnpm --dir web install --frozen-lockfile
    pnpm --dir web exec vite build

Rust compilation is offline using the accompanied dependencies. The web build
uses its exact lockfile and the ordinary npm registry for build tooling. The
production dependency tarballs in web-packages/ are enumerated by
web-notices/packages.json (same sorted order: 0.tgz, 1.tgz, ...). The editable web
source and lockfile are in tolmap/web/.

For the npm layout, follow scripts/package-npm.sh: copy target/release/tolmap
into lib/, recursively copy all non-base `ldd` dependencies into lib/, and set
each ELF's RPATH to `$ORIGIN` with patchelf. Copy web/dist and the npm launcher.
That script's base-library allowlist is tested in a clean Ubuntu 22.04 consumer.
The source archive has no Git metadata: the script's Git-SHA guard deliberately
requires an actual checkout for official release packaging. A modification can
be run directly from target/release/tolmap, or packaged using a new Git commit
and that same script; preserve these distribution/source notices.

The exact workflow and scripts that built this release are included in
`tolmap/.github/workflows/` and `tolmap/scripts/`. The npm package's BUILD_INFO.md
records the source SHA. Rebuild instructions promise source completeness and
compatible behavior, not byte-identical compiler output across toolchains.
