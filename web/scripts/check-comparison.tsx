import assert from "node:assert/strict";
import { sourceSides, comparisonQuery, validateComparison } from "../src/comparison/model";
import { validateMapSearch } from "../src/routes/search";
import type { CheckFile } from "../../bindings/CheckFile";
import type { Comparison } from "../../bindings/Comparison";
// Rendering is browser-verified separately; this script covers the protocol
// and navigation combinations without duplicating check's math in TypeScript.
const base = "a".repeat(40), head = "b".repeat(40);
for (const status of ["added", "deleted", "modified", "renamed"] as const) {
  const f: CheckFile = {path:"new.py",base_path:status === "renamed" ? "old.py" : null,status,district:1,placed:false};
  const sides = sourceSides(f, base, head);
  assert.deepEqual(sides.map(s => s.side), status === "added" ? ["head"] : status === "deleted" ? ["base"] : ["base", "head"]);
  for (const s of sides) {
    assert.equal(s.commit, s.side === "base" ? base : head);
    assert.equal(s.path, s.side === "base" && status === "renamed" ? "old.py" : "new.py");
  }
}
for (const thresholds of [{}, {max_dq:0.002}, {max_districts:2}, {report_only:true}]) {
  const search = validateMapSearch({base,head,compare:true,...thresholds});
  const roundTrip = validateMapSearch(Object.fromEntries(comparisonQuery(search)));
  assert.equal(roundTrip.base, base); assert.equal(roundTrip.head, head);
  assert.equal(roundTrip.max_dq, search.max_dq); assert.equal(roundTrip.max_districts, search.max_districts); assert.equal(roundTrip.report_only, search.report_only);
}
const search = validateMapSearch({base,head});
const c = {report:{version:1,base,head,lower_bound:true},provenance:{report_version:1,base,head}} as Comparison;
assert.equal(validateComparison(c, search), c);
assert.throws(() => validateComparison({...c,report:{...c.report,version:9}}, search), /version/);
assert.throws(() => validateComparison(c, {...search,head:base}), /Stale/);
console.log("comparison source sides, URL round-trip and stale/version checks passed");
