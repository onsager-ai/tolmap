# Repository hooks

The primary checkout stays on `main`. New code changes happen in a git worktree beside it:

```sh
git worktree add -b <branch> ../tolmap-wt-<slug> origin/main
```

## Why

Much of what this repo is read for lives in the working tree rather than behind a ref: the frozen reference implementation under `src/tolmap/` that the port is checked against, the nine acceptance fixtures in `data/`, the eval scripts, and `docs/FINDINGS.md`, which records what has already been falsified by measurement. A person, an editor, or an agent subprocess that reads the checkout while it sits on a feature branch reads a version of all of that which may not be what `main` says — and the failure is silent, because every file is still there and still plausible.

Pinning the root to `main` means the default thing to read is the true thing. Worktrees cost nothing here: several branches in this repo were already developed that way.

## Enabling the hooks

`core.hooksPath` is local configuration and cannot be committed, so each clone enables it once:

```sh
git config core.hooksPath "$(git rev-parse --show-toplevel)/.githooks"
```

The absolute path is deliberate. A relative `core.hooksPath` is resolved against the current working directory rather than the repository root, which makes it behave inconsistently across worktrees and subdirectories.

Verify it is active:

```sh
git config --get core.hooksPath
```

## What `post-checkout` does

If the **primary** worktree ends up on a branch other than `main`:

- with a clean tree, it switches back to `main` and explains why;
- with tracked changes uncommitted, it refuses to revert and tells you to commit or stash first, because the checkout has already swapped your files and an automatic revert could confuse real work.

Untracked files do not count as dirty (`git status --porcelain -uno`). They survive a branch switch untouched, and Git refuses the switch outright if it would clobber one, so they are never at risk from the corrective checkout. This is the one place the hook differs from the ostrom-hub original it was ported from, which counts them.

It never acts in a linked worktree. The hook compares `--git-dir` with `--git-common-dir`; those differ in a worktree, so branch work there is untouched even though `core.hooksPath` is shared repository-wide.

## What this cannot do

It cannot *prevent* a checkout. Git has no `pre-checkout` hook, so `post-checkout` runs after the switch and reverting is the only mechanism available. A `reference-transaction` hook exiting non-zero on the `HEAD` update does not abort `git checkout` either — that was established in ostrom-hub by direct test and taken on trust here, not re-tested.

Git does natively refuse to check out a branch that another worktree already holds, which covers part of the same risk without any hook.
