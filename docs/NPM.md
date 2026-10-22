# npm distribution

## Install and use

The prepared `tolmap` npm package contains the release Rust CLI, its Leiden/igraph shared libraries and the production web viewer. It does not run install scripts or download binaries during installation. The package is not published yet; use these commands after the first release.

```sh
npm install -g tolmap
tolmap --help

# Or run it without a global install.
npx tolmap build /path/to/repository --name my-repository --out out
npx tolmap local /path/to/private-repository \
  --data "$HOME/.local/share/tolmap/private-repository" \
  --port 8988
```

`tolmap local` requires `--data` to name private storage outside the checkout. When `--static-dir` is omitted, the npm launcher uses its packaged production web assets. You can pass `--static-dir /path/to/web/dist` to use another built viewer. The local server binds only to `127.0.0.1`; it reads committed `HEAD`, keeps storage outside the checkout, serves same-commit source, and does not upload the repository.

Install the system runtimes `libstdc++6` and `libgcc-s1` on minimal Ubuntu images. Install `git` on `PATH` for builds from repositories and for `tolmap local`. The package itself needs Node.js 22.14.0 or newer to run its launcher. It does not require Rust, Cargo, CMake, a C/C++ compiler, igraph or libleidenalg on the consumer machine. Browser use requires a current browser on the local machine.

## Supported platform

Version 0.1.0 supports **linux-x64 with glibc 2.35 or newer**. CI builds on Ubuntu 22.04 and installs and exercises the tarball in a clean Ubuntu 22.04 consumer at glibc 2.35. The runtime guard checks `process.report` and rejects musl when no glibc version is reported.

macOS, Windows, Linux arm64, and musl/Alpine are unsupported. The package metadata also limits installation to Linux x64.

## Included licenses

The original tolmap source is MIT licensed. The linked native binary in this distribution is conveyed under **GPL-3.0-or-later**, as required by libleidenalg 0.12.0 (GPL-3.0-or-later) and compatible with igraph 1.0.0 (GPL-2.0-or-later). The package's SPDX field reflects that distribution. The original MIT LICENSE is preserved. See `LICENSE-DISTRIBUTION.md` and the included dependency notices and license texts.

Every npm tarball includes `source/corresponding-source.tar.gz` and its SHA256SUMS: exact tolmap source, native archives, vendored Rust dependencies, production web dependency packages, Ubuntu source packages for bundled runtime libraries, and reproduction instructions. CI uploads the same archive separately and prints its size. Ubuntu binary packages are mapped to their actual source package names and exact versions from dpkg metadata. No expiring CI artifact URL is used as a source offer.

## Build and release path

`.github/workflows/npm-package.yml` runs for pushes, pull requests and manual dispatch, and lints the release workflows with pinned actionlint. It builds the Linux x64 tarball from an exact commit, checks the npm and Cargo versions match, verifies dynamic-library closure and relative RPATHs, then uploads an artifact named `tolmap-npm-<sha>`. The artifact contains the `.tgz`, `SHA256SUMS`, `PACK_LIST.txt` and `BUILD_INFO.md`. Its clean-consumer matrix tests Node 22.14.0 and 24.21.0. A random host nonce, SHA and tarball hash must match a completion JSON containing every named check; an empty successful container fails. The uploaded `tolmap-consumer-<sha>-node-<version>` artifacts retain those records. The matrix checks install, help, a same-SHA repository build, offline parity, viewer assets, loopback binding, same-commit source, unsupported-platform errors and tarball contents. CI does not publish automatically.

The first package must be bootstrapped by an authenticated maintainer after the npm account is available. Trusted publishing cannot be configured until the package exists. For that one-time bootstrap, download the artifact from the accepted CI run, verify `SHA256SUMS`, confirm `BUILD_INFO.md` names the accepted source SHA, and publish that exact tarball with the maintainer's npm login:

```sh
EXPECTED_SHA=0123456789012345678901234567890123456789 # replace with the accepted run's SHA
sha256sum -c SHA256SUMS
grep -Fx -- "- Source commit: $EXPECTED_SHA" BUILD_INFO.md
npm publish ./tolmap-0.1.0.tgz --access public
```

The unscoped npm name's availability is unconfirmed and must be checked during bootstrap.

Do not claim provenance for this bootstrap publish. The Worker has not configured npm trust, GitHub environments, credentials or secrets; none is claimed to be configured.

After the package exists, a maintainer must configure npm trusted publishing for `onsager-ai/tolmap` and `.github/workflows/npm-publish.yml` using npm's [trusted publisher setup](https://docs.npmjs.com/trusted-publishers/) or `npm trust`. npm documents the GitHub-hosted runner, `id-token: write`, Node.js >= 22.14 and npm >= 11.5.1 requirements, and that the package must exist before trust can be set up ([`npm trust`](https://docs.npmjs.com/cli/v11/commands/npm-trust/)). A maintainer must also create the GitHub `npm-publish` environment and require reviewers.

Future versions use only the manual `npm-publish.yml` workflow. Dispatch it on a ref whose HEAD is the accepted commit, provide that exact 40-character SHA and the confirmation `publish tolmap@<version>`, and run the default dry run first. A real publish requires a separate dispatch with `dry_run` set to false and approval by the configured environment reviewers. The workflow rebuilds and clean-consumer verifies the same SHA, checks the tarball checksum and `BUILD_INFO.md`, then invokes npm with OIDC provenance. It rejects non-dispatch events, a SHA mismatch, a Cargo/npm version mismatch and a version already on the registry. Routine pushes and pull requests never publish.
