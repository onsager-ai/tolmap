"""Materialise the 51-file TypeScript alias-resolution CI fixture.

Every internal import in this repository is a side-effect import through the
same tsconfig alias (`@/moduleNNN`). There are no relative imports, shared
identifiers, or qualifying co-change pairs. Before issue #41's resolver fix,
tolmap therefore sees an empty graph and aborts at blend; after the fix the 50
hand-counted alias imports form a chain across all 51 files and the map builds.

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

    env = dict(os.environ)
    env.update(ENV)
    subprocess.run(["git", "add", "-A"], cwd=out_dir, env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    subprocess.run(["git", "commit", "--quiet", "-m", "add alias-only TypeScript package"],
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
    print(f"files: {FILE_COUNT}; internal alias imports: {FILE_COUNT - 1}")


if __name__ == "__main__":
    main()
