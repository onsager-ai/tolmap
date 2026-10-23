# npm distribution

## Install and use

The `tolmap` npm package contains the release Rust CLI, its Leiden/igraph shared libraries and the production web viewer. It does not run install scripts or download binaries during installation.

```sh
npm install -g tolmap
tolmap --help

# Or run it without a global install.
npx tolmap build /path/to/repository --name my-repository --out out
npx tolmap local /path/to/private-repository \
  --data "$HOME/.local/share/tolmap/private-repository" \
  --port 8988
```

Run `tolmap local` as an ordinary non-root user, including inside containers. Root execution is unsupported for this local journey: private storage is owned by the launching user, while root-launched workers drop to a separate uid that cannot traverse that storage. Do not change private directory permissions to work around this.

`tolmap local` requires `--data` to name private storage outside the checkout. When `--static-dir` is omitted, the npm launcher uses its packaged production web assets. You can pass `--static-dir /path/to/web/dist` to use another built viewer. The local server binds only to `127.0.0.1`; it reads committed `HEAD`, keeps storage outside the checkout, serves same-commit source, and does not upload the repository.

Install the system runtimes `libstdc++6` and `libgcc-s1` on minimal Ubuntu images. Install `git` on `PATH` for builds from repositories and for `tolmap local`. The package itself needs Node.js 22.14.0 or newer to run its launcher. It does not require Rust, Cargo, CMake, a C/C++ compiler, igraph or libleidenalg on the consumer machine. Browser use requires a current browser on the local machine.

## Supported platform

Version 0.1.0 supports **linux-x64 with glibc 2.35 or newer**. CI builds on Ubuntu 22.04 and installs and exercises the tarball in a clean Ubuntu 22.04 consumer at glibc 2.35. The runtime guard checks `process.report` and rejects musl when no glibc version is reported.

macOS, Windows, Linux arm64, and musl/Alpine are unsupported. The package metadata also limits installation to Linux x64.

## Included licenses

The original tolmap source is MIT licensed. The linked native binary in this distribution is conveyed under **GPL-3.0-or-later**, as required by libleidenalg 0.12.0 (GPL-3.0-or-later) and compatible with igraph 1.0.0 (GPL-2.0-or-later). The package's SPDX field reflects that distribution. The original MIT LICENSE is preserved. See `LICENSE-DISTRIBUTION.md` and the included dependency notices and license texts.

Complete corresponding source is offered at the designated network location in each npm tarball's `SOURCE.json` and kept available alongside the package, per GPLv3 §6(d). The file pins the exact durable URL, SHA-256, byte size and source commit, and lists the native and Ubuntu source components. The URL format is `https://github.com/onsager-ai/tolmap/releases/download/v<version>/tolmap-<version>-corresponding-source-<sha>.tar.gz`. Packaging generates the concrete values in `SOURCE.json` and appends them to the shipped README, distribution notice and `SOURCE-BUILD.md`; those included instructions explain download verification and rebuilding.

The separate archive contains exact tolmap source, pinned native archives, vendored Rust dependencies, production web dependency packages, Ubuntu source packages for bundled runtime libraries, and reproduction instructions. Ubuntu binary packages are mapped to their actual source package names and exact versions from dpkg metadata. CI retains this archive as a separate file for a maintainer to publish unchanged as the designated GitHub release asset before npm publication. No expiring Actions artifact URL is used as the public source offer. The npm tarball contains no `source/` directory and must be at most 64 MiB (67,108,864 bytes). `PACKAGE_SIZES.json` records the npm tarball, unpacked runtime package and separate source archive sizes.

## Build and release path

`.github/workflows/npm-package.yml` runs for pushes, pull requests and manual dispatch, and lints the release workflows with pinned actionlint. It builds the Linux x64 tarball from an exact commit, checks the npm and Cargo versions match, verifies dynamic-library closure and relative RPATHs, then uploads an artifact named `tolmap-npm-<sha>`. It contains the `.tgz` and separate corresponding-source archive, `SOURCE.json`, `SOURCE_SHA256SUMS`, `SHA256SUMS`, `PACKAGE_SIZES.json`, `PACK_INFO.json`, `PACK_LIST.txt` and `BUILD_INFO.md`. Its clean-consumer matrix tests Node 22.14.0 and 24.21.0 as an ordinary uid. A random host nonce, SHA and tarball hash must match a completion JSON containing every named check; an empty successful container fails. The uploaded `tolmap-consumer-<sha>-node-<version>` artifacts retain those records. All ten existing runtime checks remain: clean runtime, artifact identity, package contents, CLI, unsupported platform, offline parity, reader cancellation, real Rust repository build, viewer/assets/source/loopback, and launcher signals. An eleventh source-metadata check verifies the tarball's `SOURCE.json` against the separate archive's hash, size, commit and Ubuntu source list, enforces the size budget and excludes `source/`. The host independently verifies this metadata too. CI does not publish automatically.

The first package must be bootstrapped by an authenticated maintainer. Trusted publishing cannot be configured until the package exists. For that one-time bootstrap, download the artifact from the accepted successful exact-SHA CI run. First create the version's GitHub release/tag pointing to the accepted commit and upload the source asset under its exact `SOURCE.json` filename. Preserve the CI-built asset bytes: do not rebuild it or replace a mismatching existing asset. Then verify the downloaded artifact and the public source offer before publishing the exact npm tarball with the maintainer's npm login:

```sh
EXPECTED_SHA=0123456789012345678901234567890123456789 # replace with the accepted run's SHA
sha256sum -c SHA256SUMS
grep -Fx -- "- Source commit: $EXPECTED_SHA" BUILD_INFO.md
# Run this script from the accepted source checkout, with artifacts in $PWD.
# --release also downloads the durable source URL; 404/hash/size mismatches fail.
python3 /path/to/accepted-checkout/scripts/npm-source-metadata.py "$PWD" "$EXPECTED_SHA" --release
npm publish ./tolmap-0.1.0.tgz --access public
```

The unscoped npm name's availability is unconfirmed and must be checked during bootstrap.

Do not claim provenance for this bootstrap publish. An authenticated npm account is required for bootstrap; npm trust and the GitHub environment binding must be configured separately for future OIDC releases. This repository does not establish that any of those prerequisites are already configured.

After the package exists, a maintainer must configure npm trusted publishing for `onsager-ai/tolmap` and `.github/workflows/npm-publish.yml` using npm's [trusted publisher setup](https://docs.npmjs.com/trusted-publishers/) or `npm trust`. npm documents the GitHub-hosted runner, `id-token: write`, Node.js >= 22.14 and npm >= 11.5.1 requirements, and that the package must exist before trust can be set up ([`npm trust`](https://docs.npmjs.com/cli/v11/commands/npm-trust/)). Match the ordinary GitHub `npm-publish` environment name in the npm trust binding. Reviewer approvals are not required by this release procedure.

Future versions use only the manual `npm-publish.yml` workflow. Dispatch it on a ref whose HEAD is the accepted commit, provide that exact 40-character SHA, the successful `npm-package.yml` run ID in `package_run_id`, and the confirmation `publish tolmap@<version>`. Publish the source asset first, then run the default dry run. A real publish uses a separate dispatch with `dry_run` set to false. The workflow requires a successful same-repository branch packaging run at that SHA and downloads its exact verified artifact without rebuilding. Before either dry run or real publication it verifies the tarball, budget, source metadata and manifests, then downloads the durable release asset and checks its SHA-256 and size against both the tarball's `SOURCE.json` and `SOURCE_SHA256SUMS`. A missing asset or any mismatch fails; it never uploads or replaces an asset. Only then does it invoke npm with OIDC provenance. It also rejects non-dispatch events, a SHA mismatch, a Cargo/npm version mismatch and a version already on the registry. Same source SHA alone does not promise byte-identical archive or compiler output. Routine pushes and pull requests never publish.
