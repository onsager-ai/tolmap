"""Materialise the 53-file TypeScript alias-resolution CI fixture.

The original package is unchanged: 50 side-effect imports through the root
tsconfig's `@/moduleNNN` alias form a chain across 51 files. A second two-file
package declares the same `@/*` prefix and imports its own `module001`; this is
the duplicate-prefix collision that must resolve against the nearer tsconfig,
not the root package's equally named file. There are no relative imports,
shared identifiers, or qualifying co-change pairs.

Before issue #41's first resolver fix, tolmap sees an empty graph and aborts at
blend. The repo-global version of that fix sees 51 edges but sends the root
package's `module000` import into the second package. The scope-aware resolver
sees the same 51 edges without inventing that cross-package relationship.

The fixture is generated locally rather than committed as an extracted graph
because the source-to-graph resolution is the behavior under test. Git identity
and dates are fixed so its single scaffold commit is reproducible too.

    python eval/gen_synthetic_module_resolution_fixture.py --out /tmp/module-resolution
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
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00",
}

FILE_COUNT = 51
SECOND_PACKAGE = "packages/secondary"
TOTAL_SOURCE_FILES = FILE_COUNT + 2


def write(root, relative, contents):
    path = os.path.join(root, relative)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as handle:
        handle.write(contents)


def source(index):
    import_line = (f'import "@/module{index + 1:03d}";\n\n'
                   if index + 1 < FILE_COUNT else "")
    return (f"{import_line}"
            f"export const value{index:03d}: number = {index};\n")


def generate(out_dir):
    os.makedirs(out_dir, exist_ok=True)
    subprocess.run(
        ["git", "init", "--quiet", "--initial-branch=main", out_dir], check=True)
    write(out_dir, "package.json",
          '{\n  "name": "module-resolution-fixture",\n  "private": true\n}\n')
    write(out_dir, "tsconfig.json", """{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {"@/*": ["src/*"]}
  }
}
""")
    for index in range(FILE_COUNT):
        write(out_dir, f"src/module{index:03d}.ts", source(index))
    write(out_dir, f"{SECOND_PACKAGE}/package.json",
          '{\n  "name": "module-resolution-secondary",\n  "private": true\n}\n')
    write(out_dir, f"{SECOND_PACKAGE}/tsconfig.json", """{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {"@/*": ["src/*"]}
  }
}
""")
    write(out_dir, f"{SECOND_PACKAGE}/src/main.ts",
          'import "@/module001";\n\nexport const secondaryMain = 1;\n')
    write(out_dir, f"{SECOND_PACKAGE}/src/module001.ts",
          "export const secondaryValue = 1;\n")

    env = dict(os.environ)
    env.update(ENV)
    subprocess.run(["git", "add", "-A"], cwd=out_dir, env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    subprocess.run(["git", "commit", "--quiet", "-m", "add scoped alias-only TypeScript packages"],
                   cwd=out_dir, env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    return subprocess.run(
        ["git", "-C", out_dir, "rev-parse", "HEAD"], capture_output=True,
        text=True, check=True).stdout.strip()


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True,
                        help="directory to materialise (must not be non-empty)")
    args = parser.parse_args(argv)
    if os.path.exists(args.out) and os.listdir(args.out):
        sys.exit(f"{args.out} already exists and is not empty")
    sha = generate(args.out)
    print(f"synthetic module-resolution fixture generated at {args.out}, HEAD {sha}")
    print(f"files: {TOTAL_SOURCE_FILES}; internal alias imports: {FILE_COUNT}")


if __name__ == "__main__":
    main()
