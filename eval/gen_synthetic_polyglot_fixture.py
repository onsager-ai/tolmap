"""Deterministically materialise the synthetic Go+TypeScript polyglot fixture
step 2 of the polyglot union-extraction work (docs/FINDINGS.md finding 13)
measures against, and that CI's `polyglot-report` job runs on.

Why generated, not cloned: the nine acceptance fixtures in `data/` pin a real
upstream repository by commit SHA, which is the right oracle when one exists
(the frozen Python reference built each of them once). Polyglot has no such
oracle -- the reference maps one language per run -- so there is nothing to
clone, and cloning something incidentally polyglot would hand CI a fixture
whose "correct" cross-language numbers nobody could state. A small, scripted
repository whose structure is fully known lets `docs/FINDINGS.md` state what
the numbers *should* look like (two real static-edge clusters, one
deliberately shared identifier bridging them, one deliberately linked
co-change pair) and then check the tool actually finds it.

Layout (50 files total -- 25 Go, 25 TypeScript -- well under the 600-file
semantic-candidate-sweep threshold `src/extract.rs::finish_graph` documents,
so the full O(n^2) semantic sweep runs and cross-language semantic bridging
is actually exercised, not switched off by size; also well over
`--all-sources`' 25-file/5%-share floor, so `--all-sources` actually selects
both languages here rather than nothing):

    go.mod                          module example.com/synth
    cmd/synth/main.go               package main; imports beta
    internal/alpha/render.go        package alpha
    internal/alpha/shape.go         package alpha -- defines Widget
    internal/alpha/filler0..9.go    isolated filler, no edges (see below)
    internal/beta/service.go        package beta; imports alpha, uses alpha.Widget
    internal/beta/handler.go        package beta; imports alpha
    internal/beta/filler0..9.go     isolated filler, no edges
    package.json, tsconfig.json
    src/index.ts                imports service
    src/service.ts              imports render, widget
    src/render.ts               imports widget
    src/widget.ts               defines Widget (deliberately named to
                                     match the Go side -- see below)
    src/filler0..20.ts           isolated filler, no edges

Three deliberate signals, not left to chance, plus filler that deliberately
carries none:

- **Shared vocabulary.** `Widget` is the concept both `internal/alpha/shape.go`
  and `src/widget.ts` are built around, and each of those files' import
  neighbours (`internal/beta/service.go`, `src/render.ts`) reference it
  too -- 4 of the 9 hand-authored files, comfortably inside
  `semantic_vectors`'s [2, count*0.5] document-frequency window (2 to 25 of
  50), so it survives the IDF filter and produces genuine cross-language
  cosine similarity instead of being filtered out as either too rare or too
  common.
- **Scripted co-change.** `internal/alpha/shape.go` and `src/widget.ts`
  are committed together three times (the "Widget" commits below), giving
  each a solo commit count of 3 and a co-occurrence count of 3 -- clears
  `git_history`'s `denominator >= 3 && count >= 2` floor with room to spare,
  producing a real (not edge-case) `cochange` value between a Go file and a
  TypeScript file.

Every other file is added in one shared "scaffold" commit, so its solo commit
count (1) stays well under the co-change floor -- the deliberate pair above
is the only source of co-change, not scaffolding noise.

Git identity and dates are pinned so the generated repository's commit SHAs
are themselves reproducible, though nothing in the map pipeline reads a SHA
directly (`git_history` only reads `git log --name-only`'s file lists) --
pinning them is a cleanliness property, not a functional requirement.

    python eval/gen_synthetic_polyglot_fixture.py --out /tmp/synthetic-polyglot

Prints the resulting HEAD SHA. Exits non-zero (via subprocess) if git is
unavailable or a command fails.
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

FILES_SCAFFOLD = {
    "go.mod": "module example.com/synth\n\ngo 1.22\n",
    "cmd/synth/main.go": """package main

import "example.com/synth/internal/beta"

func main() {
\tbeta.Serve()
}
""",
    "internal/alpha/render.go": """// Package alpha holds the small rendering primitives beta serves.
package alpha

// Render turns a plain string into the fixture's one "formatted" shape.
func Render(label string) string {
\treturn "<" + label + ">"
}
""",
    "internal/beta/service.go": """package beta

import "example.com/synth/internal/alpha"

// Serve builds a Widget and renders it -- the intra-Go static edge this
// fixture exists to carry (beta -> alpha).
func Serve() string {
\tw := alpha.NewWidget("root")
\treturn alpha.Render(w.Label)
}
""",
    "internal/beta/handler.go": """package beta

import "example.com/synth/internal/alpha"

// Handle is a second beta -> alpha edge, in a different beta file, so the
// alpha package's fan-in comes from more than one importer.
func Handle(label string) string {
\treturn alpha.Render(label)
}
""",
    "package.json": "{\n  \"name\": \"synth-web\",\n  \"private\": true\n}\n",
    "tsconfig.json": "{\n  \"compilerOptions\": { \"strict\": true }\n}\n",
    "src/index.ts": """import { service } from "./service";

export function main(): string {
  return service("root");
}
""",
    "src/service.ts": """import { renderWidget } from "./render";
import { Widget } from "./widget";

// service -> render and service -> widget, the TypeScript-side mirror of
// beta -> alpha above (two importers of the shared Widget concept).
export function service(label: string): string {
  const widget = new Widget(label);
  return renderWidget(widget);
}
""",
    "src/render.ts": """import { Widget } from "./widget";

// render -> widget: the second TypeScript static edge, matching handler.go's
// second beta -> alpha edge on the Go side.
export function renderWidget(widget: Widget): string {
  return `<${widget.label}>`;
}
""",
}

# Committed separately (and three times over, with a trivial edit each time)
# so this Go/TypeScript pair -- and only this pair -- clears the co-change
# floor. See the module docstring.
WIDGET_GO_STAGES = [
    """package alpha

// Widget is the shared concept this fixture deliberately names the same on
// both sides of the language boundary, so semantic_vectors sees real
// cross-language vocabulary overlap instead of two disjoint corpora.
type Widget struct {
\tLabel string
}

// NewWidget constructs a Widget from a label.
func NewWidget(label string) *Widget {
\treturn &Widget{Label: label}
}
""",
    """package alpha

// Widget is the shared concept this fixture deliberately names the same on
// both sides of the language boundary, so semantic_vectors sees real
// cross-language vocabulary overlap instead of two disjoint corpora.
type Widget struct {
\tLabel string
}

// NewWidget constructs a Widget from a label.
func NewWidget(label string) *Widget {
\treturn &Widget{Label: label}
}

// Rename lets Widget's second commit touch the same file as the first
// without changing its exported shape.
func (w *Widget) Rename(label string) {
\tw.Label = label
}
""",
    """package alpha

// Widget is the shared concept this fixture deliberately names the same on
// both sides of the language boundary, so semantic_vectors sees real
// cross-language vocabulary overlap instead of two disjoint corpora.
type Widget struct {
\tLabel string
}

// NewWidget constructs a Widget from a label.
func NewWidget(label string) *Widget {
\treturn &Widget{Label: label}
}

// Rename lets Widget's second commit touch the same file as the first
// without changing its exported shape.
func (w *Widget) Rename(label string) {
\tw.Label = label
}

// String is Widget's third commit, completing the three-commit co-change
// pair with src/widget.ts.
func (w *Widget) String() string {
\treturn w.Label
}
""",
]

WIDGET_TS_STAGES = [
    """// Widget is the shared concept this fixture deliberately names the same on
// both sides of the language boundary, so semantic_vectors sees real
// cross-language vocabulary overlap instead of two disjoint corpora.
export class Widget {
  label: string;

  constructor(label: string) {
    this.label = label;
  }
}
""",
    """// Widget is the shared concept this fixture deliberately names the same on
// both sides of the language boundary, so semantic_vectors sees real
// cross-language vocabulary overlap instead of two disjoint corpora.
export class Widget {
  label: string;

  constructor(label: string) {
    this.label = label;
  }

  rename(label: string): void {
    this.label = label;
  }
}
""",
    """// Widget is the shared concept this fixture deliberately names the same on
// both sides of the language boundary, so semantic_vectors sees real
// cross-language vocabulary overlap instead of two disjoint corpora.
export class Widget {
  label: string;

  constructor(label: string) {
    this.label = label;
  }

  rename(label: string): void {
    this.label = label;
  }

  toString(): string {
    return this.label;
  }
}
""",
]

WIDGET_GO_PATH = "internal/alpha/shape.go"
WIDGET_TS_PATH = "src/widget.ts"

COMMIT_DATES = [
    "2026-01-01T00:00:00",
    "2026-01-02T00:00:00",
    "2026-01-03T00:00:00",
    "2026-01-04T00:00:00",
]

# `--all-sources` (detect::ALL_SOURCES_MIN_FILES = 25, detect::
# ALL_SOURCES_MIN_SHARE = 5%) has to actually select both languages here --
# spec step 1 item 8 requires three `build --all-sources` runs on this
# fixture to be byte-identical, and a fixture below the floor would make
# `--all-sources` select nothing at all. The 9 hand-authored files above
# carry every deliberate signal (the Widget bridge, the beta -> alpha /
# service -> widget static edges, the scripted co-change pair); these are
# plain filler, added once in the scaffold commit so they cannot affect any
# solo/co-change count, purely to push both languages' file counts to 25 (>=
# ALL_SOURCES_MIN_FILES, and 25/50 = 50% clears ALL_SOURCES_MIN_SHARE with
# room to spare). They deliberately carry no imports of their own -- adding
# real edges between them would blur the fixture's three deliberately-placed
# signals with incidental ones nobody chose on purpose.
FILLER_GO_PER_PACKAGE = 10  # -> internal/alpha, internal/beta: +20 files
FILLER_TS_COUNT = 21  # -> src/: +21 files
# hand-authored (5 go + 4 ts) + filler (20 go + 21 ts) = 25 go + 25 ts = 50.


def filler_go(package, index):
    return f"""package {package}

// Filler{index} pads this fixture's file count past --all-sources' floor
// (see the module docstring) -- deliberately isolated, no imports, no
// relation to the Widget bridge this fixture actually exists to measure.
func Filler{index}() int {{
\treturn {index}
}}
"""


def filler_ts(index):
    return f"""// Filler{index} pads this fixture's file count past --all-sources' floor
// (see the module docstring) -- deliberately isolated, no imports, no
// relation to the Widget bridge this fixture actually exists to measure.
export function filler{index}(): number {{
  return {index};
}}
"""


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

    for rel, contents in FILES_SCAFFOLD.items():
        write(out_dir, rel, contents)
    for index in range(FILLER_GO_PER_PACKAGE):
        write(out_dir, f"internal/alpha/filler{index}.go", filler_go("alpha", index))
        write(out_dir, f"internal/beta/filler{index}.go", filler_go("beta", index))
    for index in range(FILLER_TS_COUNT):
        write(out_dir, f"src/filler{index}.ts", filler_ts(index))
    run(["git", "add", "-A"], out_dir, COMMIT_DATES[0])
    run(["git", "commit", "--quiet", "-m", "scaffold: cmd/main, alpha/render, beta, web (no Widget yet)"],
        out_dir, COMMIT_DATES[0])

    for index, (go_body, ts_body) in enumerate(zip(WIDGET_GO_STAGES, WIDGET_TS_STAGES)):
        write(out_dir, WIDGET_GO_PATH, go_body)
        write(out_dir, WIDGET_TS_PATH, ts_body)
        run(["git", "add", "--", WIDGET_GO_PATH, WIDGET_TS_PATH], out_dir, COMMIT_DATES[index + 1])
        stage_name = ["introduce", "extend", "finish"][index]
        run(["git", "commit", "--quiet", "-m",
             f"{stage_name} Widget (go+ts together -- deliberate co-change pair)"],
            out_dir, COMMIT_DATES[index + 1])

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
    print(f"synthetic polyglot fixture generated at {args.out}, HEAD {sha}")
    print("sources: --pkg . --lang go --pkg src --lang ts")


if __name__ == "__main__":
    main()
