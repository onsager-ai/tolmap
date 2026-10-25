import type { CheckFile } from "../../../bindings/CheckFile";
import type { Comparison } from "../../../bindings/Comparison";
import type { MapSearch } from "../routes/search";

export function sourceSides(file: CheckFile, base: string, head: string) {
  return [
    ...(file.status === "added" ? [] : [{ side: "base", commit: base, path: file.base_path ?? file.path }]),
    ...(file.status === "deleted" ? [] : [{ side: "head", commit: head, path: file.path }]),
  ];
}
export function comparisonQuery(search: MapSearch) {
  const query = new URLSearchParams({ base: search.base ?? "", head: search.head ?? "" });
  if (search.max_districts != null) query.set("max_districts", String(search.max_districts));
  if (search.max_dq != null) query.set("max_dq", String(search.max_dq));
  if (search.report_only) query.set("report_only", "true");
  return query;
}
export function validateComparison(value: Comparison, search: MapSearch): Comparison {
  if (value.report.version !== 1 || value.provenance.report_version !== 1) throw new Error("Unsupported comparison report version");
  if (value.report.base !== search.base || value.report.head !== search.head || value.provenance.base !== search.base || value.provenance.head !== search.head) throw new Error("Stale comparison: commit identity does not match this request");
  if (!value.report.lower_bound) throw new Error("Unsupported graph coverage contract");
  return value;
}
