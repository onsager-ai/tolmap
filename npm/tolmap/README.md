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
`tolmap local`.

Only linux-x64 with glibc >= 2.35 is supported in this release. macOS,
Windows, Linux arm64 and musl/Alpine are unsupported. See the repository's
[`docs/NPM.md`](https://github.com/onsager-ai/tolmap/blob/main/docs/NPM.md) for
local viewer usage, prerequisites, platform details and release procedure.

The Rust CLI is MIT licensed. The package also includes GPL-licensed native
libraries and third-party web dependency notices; see `LICENSE` and
`THIRD_PARTY_NOTICES.md` before redistribution.
