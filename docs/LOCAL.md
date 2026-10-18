# Private checkout reading

`tolmap local /path/to/checkout --data /private/tolmap-store --static-dir /path/to/web/dist`

Build/install the Rust binary and production web bundle using the repository's
normal documented build prerequisites. The local command only binds 127.0.0.1;
open the printed URL on the same computer. It does not open a browser. Choose
**Detect committed source**, inspect the commit, source root, language, file count
and confidence, then confirm the detected source or all candidates above the
existing detection floor. Unsupported detection retains the page and retry.
Indexing uses the production process executor with stage updates and cancellation.
Refresh explicitly captures checkout HEAD again; history links retain old snapshots.

This command is separate from the public catalogue service. No Hub, account,
subscription, provider OAuth or upload is involved. HTTP never accepts a local
path or arbitrary repository URL. The original checkout is read only. Dirty and
untracked content is excluded, with an explicit disclosure; commit it first.
`tolmap build` continues its existing working-tree behavior and does not gain a
commit/source claim from this command.

The explicitly selected storage directory (outside the checkout) contains private
Git object copies, derived maps, symbol files and provenance. Parent directories
should belong to the user. Do not put it under web/public, data, a cloud-synced
public folder or a static web root. Source is retained until the user removes
that local store. A store is bound to one repository identity and canonical
checkout. Independent Git objects retain A even after the checkout moves to B or
GC runs. Removing storage makes that snapshot unavailable; no branch fallback.

Map JSON downloads disclose file paths, names and structure but **no source
text**. They are private artifacts even without source. The local router has no
catalogue publishing path and serves neither raw Git objects nor source trees.
Source is fetched lazily as inert text using version 1 provenance
`repository + immutable commit + relative path`. Map-only exports cannot reopen
source without the originating local store. Localhost links are not share links
and cannot be opened from another phone/computer. The optional hosted Hub has its
own explicit connection, audience and access controls.

Source accepts UTF-8 regular Git blobs up to 1 MiB. Symlinks and submodules are
refused; invalid/traversal paths are rejected before Git. Binary, missing,
expired and oversized results are explicit. Lines above 20,000 bytes use plain
text without language parsing. Mainstream language descriptions are lazy-loaded
from CodeMirror's language-data package; unsupported names and load failures use
plain text. Language highlighting does not extend indexing coverage.

Validation runs in CI under host thermal restrictions. Browser emulation and
unreviewed screenshot candidates are distinct from image review and physical
Safari/iPhone acceptance. No baseline is recorded or approved by CI.
