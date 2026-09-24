"""A tiny, dependency-free fixture for the worker image's SCIP smoke check
(.github/workflows/scip-image-build.yml, issue #110 P1b). It exists only to
give scip-python (Pyright) one definition and one reference to put in a
non-empty index.scip.
"""


def greet(name: str) -> str:
    return f"hello, {name}"


if __name__ == "__main__":
    print(greet("scip"))
