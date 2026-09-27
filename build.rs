use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=native/leiden_bridge.cpp");
    println!("cargo:rerun-if-env-changed=LEIDEN_PREFIX");
    emit_build_commit();

    let prefix = env::var_os("LEIDEN_PREFIX")
        .map(PathBuf::from)
        .or_else(discover_system_prefix)
        .unwrap_or_else(|| {
            panic!(
                "libleidenalg/igraph not found; run scripts/install-leiden.sh and set LEIDEN_PREFIX"
            )
        });
    let include = prefix.join("include");
    let lib = prefix.join("lib");
    if !include.join("libleidenalg/Optimiser.h").is_file()
        || !include.join("igraph/igraph.h").is_file()
    {
        panic!(
            "LEIDEN_PREFIX={} has no compatible headers",
            prefix.display()
        );
    }

    cc::Build::new()
        .cpp(true)
        .std("c++14")
        .include(&include)
        .file("native/leiden_bridge.cpp")
        .warnings(false)
        .compile("tolmap_leiden_bridge");

    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=dylib=libleidenalg");
    println!("cargo:rustc-link-lib=dylib=igraph");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
}

fn discover_system_prefix() -> Option<PathBuf> {
    ["/usr/local", "/usr"]
        .into_iter()
        .map(Path::new)
        .find(|p| p.join("include/libleidenalg/Optimiser.h").is_file())
        .map(Path::to_path_buf)
}

/// Build identity (docs/WORKER_TIER.md §3.6): bakes `TOLMAP_BUILD_COMMIT`
/// into the binary as a compile-time env var (`option_env!` in
/// `service::workers::build_commit`), so a mismatched worker build stays
/// idle rather than racing the master to write a map (#153's departure 5,
/// which this closes: `hello.build.commit` used to be the executable's own
/// SHA-256 because nothing else was available).
///
/// Preference order, all emitted through `cargo:rustc-env` (never left to
/// a bare `option_env!` on the *caller's* shell variable, which Cargo does
/// not track -- a later build with a different `TOLMAP_BUILD_COMMIT` and no
/// source change would otherwise keep the stale one linked in):
///   1. `TOLMAP_BUILD_COMMIT` from this build's own environment -- what CI
///      and the Dockerfile pass as a build arg (`docs/API.md`).
///   2. `git rev-parse HEAD`, run here, when a checkout is available. This
///      also covers a worktree checkout (CLAUDE.md's convention): `.git`
///      there is a file, not a directory, and the branch ref this crate
///      must watch for a rebuild lives in the *common* git dir, not the
///      worktree's own -- `git rev-parse --git-path`/`--git-common-dir`
///      resolve both without this file having to parse `.git` by hand.
///   3. Absent: `option_env!` then yields `None` and `build_commit()` falls
///      back to `"unknown"` at run time, which `same_build` never matches,
///      even to another `"unknown"`.
///
/// No `-dirty` suffix (optional per the brief): every path that bakes a
/// commit in for real -- CI and the Dockerfile -- builds from a clean
/// checkout, and a `git status --porcelain` dirty check has no single file
/// to watch for `rerun-if-changed`, so it would go stale between builds in
/// a way this file's other triggers do not. A developer's own dirty
/// checkout still gets a real commit (last-committed HEAD), just not a
/// flagged one; that is no worse than every other build.rs value here,
/// which also describes the checkout, not the working tree.
fn emit_build_commit() {
    println!("cargo:rerun-if-env-changed=TOLMAP_BUILD_COMMIT");
    if let Ok(commit) = env::var("TOLMAP_BUILD_COMMIT") {
        let commit = commit.trim();
        if !commit.is_empty() {
            println!("cargo:rustc-env=TOLMAP_BUILD_COMMIT={commit}");
            return;
        }
    }
    if let Some(commit) = git_head_commit() {
        println!("cargo:rustc-env=TOLMAP_BUILD_COMMIT={commit}");
    }
    // Neither: leave the env var unset. `option_env!` then sees `None` and
    // `build_commit()` falls back to `"unknown"` -- no `cargo:rustc-env` at
    // all here, rather than emitting the literal string `"unknown"`, so a
    // later build that gains a real commit is not held back by this one
    // having "set" the variable to a placeholder.
}

/// `git rev-parse HEAD`, plus the `rerun-if-changed` triggers that keep it
/// from going stale: `.git`'s own `HEAD` file (this checkout's own, which
/// for a worktree is `.git/worktrees/<name>/HEAD`, found by
/// `--git-path HEAD` rather than assumed), and, when `HEAD` names a branch,
/// that branch's ref file in the *common* git dir (`--git-common-dir`),
/// since that is where a worktree's branch refs actually live and moving
/// the branch (a new commit, a fetch that fast-forwards it) touches that
/// file, not the per-worktree `HEAD`. `None` wherever `git` is not on
/// `PATH`, this is not a git checkout at all (an extracted source
/// tarball), or any command fails -- never a build error.
fn git_head_commit() -> Option<String> {
    if let Some(head_path) = git_stdout(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head_path}");
    }
    if let Some(branch) = git_stdout(&["symbolic-ref", "-q", "--short", "HEAD"]) {
        if let Some(common_dir) = git_stdout(&["rev-parse", "--git-common-dir"]) {
            let common_dir = Path::new(&common_dir);
            println!(
                "cargo:rerun-if-changed={}",
                common_dir.join("refs/heads").join(&branch).display()
            );
            // A branch with no loose ref file (just repacked, or fetched
            // straight into packed-refs) still has to invalidate the build
            // when it moves.
            println!(
                "cargo:rerun-if-changed={}",
                common_dir.join("packed-refs").display()
            );
        }
    }
    git_stdout(&["rev-parse", "HEAD"])
}

fn git_stdout(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}
