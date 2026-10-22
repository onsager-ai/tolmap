# tolmap

Install the Linux x64 package with glibc 2.35 or newer:

```sh
npm install -g tolmap
tolmap --help
```

Or run it without a global install:

```sh
npx tolmap --help
npx tolmap build /path/to/repository --name my-repo --out out
```

The package includes its Rust executable, native Leiden/igraph libraries and
the production web viewer. It does not need Rust, CMake or a compiler on the
consumer machine. `git` must be available on `PATH` for repository builds and
`tolmap local`. Node.js >=22.14.0, libstdc++6 and libgcc-s1 are required
system runtimes (provided by ordinary Ubuntu installations).

Run `tolmap local` as an ordinary non-root user, including in containers, with
`--data` pointing to private storage outside the checkout. Root execution of
the local journey is unsupported because its worker runs as a different uid
and cannot traverse root-owned private storage. The packaged viewer is used
automatically; the server binds only to `127.0.0.1`.

Only linux-x64 with glibc >= 2.35 is supported in this release. macOS,
Windows, Linux arm64 and musl/Alpine are unsupported. See the repository's
[`docs/NPM.md`](https://github.com/onsager-ai/tolmap/blob/main/docs/NPM.md) for
local viewer usage, prerequisites, platform details and release procedure.

The linked binary is distributed under GPL-3.0-or-later; the original tolmap
source remains MIT licensed. See `LICENSE-DISTRIBUTION.md`, `LICENSE` and
`THIRD_PARTY_NOTICES.md`. Corresponding source and rebuild instructions
accompany the binary in `source/corresponding-source.tar.gz`.
