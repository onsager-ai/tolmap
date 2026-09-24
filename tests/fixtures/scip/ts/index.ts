// A tiny, dependency-free fixture for the worker image's SCIP smoke check
// (.github/workflows/scip-image-build.yml, issue #110 P1b). It exists only
// to give scip-typescript one definition and one reference to put in a
// non-empty index.scip.
export function greet(name: string): string {
  return `hello, ${name}`;
}

console.log(greet("scip"));
