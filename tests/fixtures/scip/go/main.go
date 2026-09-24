// Package main is a tiny, dependency-free fixture for the worker image's
// SCIP smoke check (.github/workflows/scip-image-build.yml, issue #110
// P1b). It exists only to give scip-go one definition and one reference to
// put in a non-empty index.scip -- nothing here is measured for edge
// recall the way docs/FINDINGS.md finding 41's real-repository fixtures
// are.
package main

import "fmt"

// Greet is the one symbol this smoke fixture exists to let scip-go define
// and reference in the same file.
func Greet(name string) string {
	return fmt.Sprintf("hello, %s", name)
}

func main() {
	fmt.Println(Greet("scip"))
}
