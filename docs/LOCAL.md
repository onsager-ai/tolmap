# Personal repository library

Register committed local source or a GitHub repository, then browse the same library:

```sh
tolmap add /path/to/checkout --name engine --all-sources
tolmap add owner/name
tolmap add --scan /path/to/projects
tolmap repos
tolmap status --repo engine
tolmap refresh engine
tolmap web
# Remove tolmap-owned history/source; preserve the original checkout.
tolmap rm engine
```

The CLI and browser share one SQLite database under `$XDG_DATA_HOME/tolmap`
or `$HOME/.local/share/tolmap`. Global `--data ROOT`, `TOLMAP_DATA_DIR` or
application configuration chooses a different shared root. Keep it outside
mapped checkouts. Application configuration is `$XDG_CONFIG_HOME/tolmap/config.toml`
or `$HOME/.config/tolmap/config.toml`; [API.md](API.md) describes precedence.
Existing data is never discovered, imported or read automatically. Re-add
repositories to begin new history. Existing files elsewhere are left alone.

Run as an ordinary user. The npm package provides its production viewer;
source builds need an existing `web/dist` or `TOLMAP_STATIC_DIR` pointing to a
built bundle. Bind defaults to `127.0.0.1:8787`; `--addr HOST:PORT` overrides it.
There is no authentication. An explicit off-loopback bind exposes private maps,
source, host filesystem management and credential-backed acquisition to anyone
who can reach the listener.

Only committed source is indexed. Dirty and untracked files are excluded;
commit first, then refresh explicitly. CLI refresh works while the browser runs.
Reads retain the last complete map during indexing. Same-repository writes
return busy; failed or cancelled attempts preserve saved maps. History uses
snapshot IDs, including repeated commits. Git objects are copied independently,
so saved source survives checkout moves, deletion and GC. Missing retained source
is explicit and never substituted with another revision.

GitHub acquisition tries anonymously, then uses existing Git/gh helpers. Configure
them externally; credentials are never written into the library or passed to the
indexing worker. Local checkout indexing makes no upload. Source stays behind
same-commit Git-relative reads. Maps contain structure and symbol names and
should be treated as private. Localhost links work on this computer only.

[QUERY.md](QUERY.md) covers `--repo`, retained commits and explicit map inputs.
[CHECK.md](CHECK.md) covers advisory working-tree diff checks.
