"""Deterministically materialise a synthetic Python repository whose map
contains all three district classes issue #34 ("islands") introduces --
mainland, island and unconnected -- which none of the nine acceptance
fixtures in `data/` can exercise (eight have zero sub-1% districts and
prometheus has three, per `src/geometry.rs`'s classification doc comment).

Why generated, not cloned: same reasoning as
`eval/gen_synthetic_polyglot_fixture.py` -- there is no reference
implementation of this feature to check a real repository's map against
(the Python reference predates issue #34 entirely), so a small, scripted
repository whose structure is fully known lets the classification counts be
stated up front and then checked, rather than eyeballed after the fact.

Layout (371 files total, one Python source at the repo root, `--pkg .`):

    core/hub.py, core/mod_0.py .. mod_119.py         120 files, mainland
    web/hub.py, web/mod_0.py .. mod_119.py           120 files, mainland
    worker/hub.py, worker/mod_0.py .. mod_119.py     120 files, mainland
    islands/reporting/{collector,formatter,writer}.py  3 files, island
    islands/billing/{ledger,invoice,receipt}.py        3 files, island
    unfiled/group_a/orphan_0.py, orphan_1.py           2 files, unconnected
    unfiled/group_b/orphan_0.py, orphan_1.py, orphan_2.py  3 files, unconnected

371 files total. `MAINLAND_SHARE_PERCENT` (src/geometry.rs) is 1%, i.e. a
district needs >= 4 files here (4 * 100 = 400 >= 371) to be mainland -- each
of the three packages above clears that by 30x (120 files), and both
islands (3 files each, 3 * 100 = 300 < 371) sit comfortably under it with
room for the file count to drift a little without crossing the boundary.

Three real district classes, not left to chance:

- **Mainland.** Each of `core`, `web`, `worker` is a hub-and-spoke package:
  one `hub.py` every other module in the package imports, and nothing else.
  A star graph has no better cut than "the whole package", so Leiden
  recovers each package as one district -- the same shape flask's and
  httpx's real (much smaller) districts already have, just larger.
- **Island.** `islands/reporting` and `islands/billing` are each a 3-file
  import chain (`collector -> formatter -> writer`,
  `ledger -> invoice -> receipt`) with no import in or out of the
  directory -- a genuine community, below the mainland threshold, that
  `src/geometry.rs::classify_districts` must find via `data.imports` (the
  same edges the map exports as `E`) rather than merge into anything else.
  Two, not one, so the fixture also exercises the ring-placement code
  (`geometry::relocate_offshore`) spacing more than one district around it.
- **Unconnected.** `unfiled/group_a` and `unfiled/group_b` hold plain
  modules with no import statement anywhere in them, in two different
  directories so they do not all collapse into one district via
  `pipeline::merge_tiny`'s same-directory sibling fallback -- exercising
  more than one unconnected group, again for the ring code's sake. Nested
  two directories deep (`unfiled/group_a`, not a bare top-level
  `group_a`) deliberately: `merge_tiny::parent_target` computes a tiny
  community's "parent directory" as `directory_name(directory)`, and
  `directory_name` on a bare top-level directory name (no `/` in it) is
  `""` -- the empty prefix every file's path starts with. A first version
  of this fixture used bare top-level orphan directories and watched
  `parent_target`'s counter-mode vote fold both of them wholesale into
  `core` (the encounter-order tie-break's winner among the three
  same-sized mainland packages), 125 files misnamed "unfiled_b &
  unfiled_a". One extra path segment gives each group a non-empty parent
  (`"unfiled"`) that nothing else lives under, so the fallback finds no
  candidate and leaves them alone.

Every file's identifiers are unique to its own module (`Hub{Core,Web,
Worker}`, `collect_report`/`format_report`/`write_report`, ...) so the
`semantic` signal's IDF vocabulary does not accidentally bridge two
districts that are supposed to be independent -- the same discipline
`eval/gen_synthetic_polyglot_fixture.py`'s filler functions use.

Everything is added in one commit. Deliberately: `git_cochange` (see
HANDOFF.md's gotchas) skips any commit touching more than 40 files, and this
commit touches 371, so `cochange` is uniformly zero here and the map is
built from `static` (imports), `proximity` (directory layout) and
`semantic` alone -- one fewer signal to reason about when checking that an
island's or an unconnected file's classification came from the edges this
docstring says it should.

    python eval/gen_synthetic_islands_fixture.py --out /tmp/synthetic-islands

Prints the resulting HEAD SHA and the `tolmap build` invocation to use.
Exits non-zero (via subprocess) if git is unavailable or a command fails.
"""
import argparse
import os
import subprocess
import sys

ENV = {
    "GIT_AUTHOR_NAME": "tolmap-fixture",
    "GIT_AUTHOR_EMAIL": "fixture@tolmap.invalid",
    "GIT_COMMITTER_NAME": "tolmap-fixture",
    "GIT_COMMITTER_EMAIL": "fixture@tolmap.invalid",
}

COMMIT_DATE = "2026-01-01T00:00:00"

# One mainland package per name: a hub every spoke module imports, and
# nothing else -- see the module docstring for why a star topology is the
# point, not an oversight.
MAINLAND_PACKAGES = ["core", "web", "worker"]
MAINLAND_SPOKES_PER_PACKAGE = 119  # + 1 hub.py each = 120 files/package.


def class_name(package):
    return "Hub" + package.capitalize()


def hub_source(package):
    return f'''"""Hub module for the {package!r} mainland district.

Every spoke module in this package imports only this file, so the
package's internal graph is a single star -- there is no better cut than
"the whole package", which is exactly the shape a mainland district's
internal structure should have here. See
eval/gen_synthetic_islands_fixture.py's module docstring.
"""


class {class_name(package)}:
    """The one thing every {package} module depends on."""

    def value(self):
        return "{package}"
'''


def spoke_source(package, index):
    cls = class_name(package)
    return f'''from {package}.hub import {cls}


def compute_{package}_{index}():
    """Spoke {index} of the {package!r} package -- imports the hub and
    nothing else."""
    return {cls}().value()
'''


# Two islands: a 3-file import chain each, fully self-contained (no import
# in or out of the directory). `writer.py`/`receipt.py` are the chain's
# leaf and import nothing, matching how a real small feature module
# bottoms out.
ISLAND_REPORTING = {
    "islands/reporting/writer.py": '''"""Bottom of the reporting chain -- writes a report, imports nothing."""


def write_report(payload):
    return f"report: {payload}"
''',
    "islands/reporting/formatter.py": '''from islands.reporting.writer import write_report


def format_report(rows):
    """Middle of the reporting chain: formatter -> writer."""
    return write_report(", ".join(rows))
''',
    "islands/reporting/collector.py": '''from islands.reporting.formatter import format_report


def collect_report(rows):
    """Top of the reporting chain: collector -> formatter -> writer, the
    two real internal edges this island exists to carry."""
    return format_report(rows)
''',
}

ISLAND_BILLING = {
    "islands/billing/receipt.py": '''"""Bottom of the billing chain -- renders a receipt, imports nothing."""


def render_receipt(total):
    return f"receipt: {total}"
''',
    "islands/billing/invoice.py": '''from islands.billing.receipt import render_receipt


def issue_invoice(total):
    """Middle of the billing chain: invoice -> receipt."""
    return render_receipt(total)
''',
    "islands/billing/ledger.py": '''from islands.billing.invoice import issue_invoice


def post_ledger_entry(total):
    """Top of the billing chain: ledger -> invoice -> receipt, the second
    island's two real internal edges."""
    return issue_invoice(total)
''',
}

# Two separate directories so pipeline::merge_tiny's same-directory sibling
# fallback does not fold every unconnected file into one district -- see
# the module docstring.
UNFILED_GROUP_A_COUNT = 2
UNFILED_GROUP_B_COUNT = 3


def orphan_source(group, index):
    return f'''"""Orphan {index} in {group!r} -- deliberately isolated: no
import statement anywhere in this file, and a docstring/identifier
distinctive enough that the semantic signal does not accidentally bridge
it to another district (see the module docstring). This is the
"unconnected" class: a file the graph never connects to anything, not a
community that merely missed the mainland threshold.
"""


def orphan_{group}_{index}():
    return "{group}-{index}"
'''


def write(root, rel, contents):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(contents)


def run(cmd, cwd, date):
    env = dict(os.environ)
    env.update(ENV)
    env["GIT_AUTHOR_DATE"] = date
    env["GIT_COMMITTER_DATE"] = date
    subprocess.run(cmd, cwd=cwd, env=env, check=True,
                    stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)


def generate(out_dir):
    os.makedirs(out_dir, exist_ok=True)
    subprocess.run(["git", "init", "--quiet", "--initial-branch=main", out_dir], check=True)

    for package in MAINLAND_PACKAGES:
        write(out_dir, f"{package}/hub.py", hub_source(package))
        for index in range(MAINLAND_SPOKES_PER_PACKAGE):
            write(out_dir, f"{package}/mod_{index}.py", spoke_source(package, index))

    for rel, contents in {**ISLAND_REPORTING, **ISLAND_BILLING}.items():
        write(out_dir, rel, contents)

    for index in range(UNFILED_GROUP_A_COUNT):
        write(out_dir, f"unfiled/group_a/orphan_{index}.py", orphan_source("group_a", index))
    for index in range(UNFILED_GROUP_B_COUNT):
        write(out_dir, f"unfiled/group_b/orphan_{index}.py", orphan_source("group_b", index))

    # One commit, deliberately: it touches every file this fixture has
    # (371), well past git_cochange's 40-file skip floor, so cochange is
    # uniformly zero and does not become a fourth thing to reason about
    # alongside static/proximity/semantic. See the module docstring.
    run(["git", "add", "-A"], out_dir, COMMIT_DATE)
    run(["git", "commit", "--quiet", "-m",
         "islands fixture: 3 mainland packages, 2 islands, 2 unconnected groups"],
        out_dir, COMMIT_DATE)

    sha = subprocess.run(["git", "-C", out_dir, "rev-parse", "HEAD"],
                          capture_output=True, text=True, check=True).stdout.strip()
    return sha


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, help="directory to materialise the repo into (must not exist)")
    args = ap.parse_args(argv)

    if os.path.exists(args.out) and os.listdir(args.out):
        sys.exit(f"{args.out} already exists and is not empty")

    sha = generate(args.out)
    print(f"synthetic islands fixture generated at {args.out}, HEAD {sha}")
    print("build with: tolmap build <out> --pkg . --lang py")


if __name__ == "__main__":
    main()
