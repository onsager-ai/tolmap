import { useQuery } from "@tanstack/react-query";
import type { CheckFile } from "../../../bindings/CheckFile";
import type { MapSearch } from "../routes/search";
import { API_BASE } from "../api/client";
import { comparisonQuery, sourceSides, validateComparison } from "./model";
import type { Comparison } from "../../../bindings/Comparison";
import "./comparison.css";

export function useComparison(owner: string, repo: string, search: MapSearch) {
  const query = comparisonQuery(search).toString();
  return useQuery({ queryKey: ["comparison", owner, repo, query], enabled: !!search.compare && !!search.base && !!search.head,
    queryFn: async ({ signal }) => {
      const r = await fetch(`${API_BASE}/compare/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}?${query}`, { signal, cache: "no-store" });
      if (!r.ok) { const e = await r.json().catch(() => null); throw new Error(e?.message ?? "Comparison unavailable"); }
      return validateComparison(await r.json() as Comparison, search);
    }, retry: false, staleTime: 0, gcTime: 0,
  });
}

export function ComparisonPanel({ search, data, pending, refreshing, error, update, retry, pick, read }: {
  search: MapSearch; data?: Comparison; pending: boolean; refreshing: boolean; error: Error | null;
  update(patch: Partial<MapSearch>): void; retry(): void; pick(file?: string, district?: number): void; read(path: string, commit: string): void;
}) {
  const report = !error && !refreshing ? data?.report : undefined;
  const provenance = data?.provenance;
  function href(path: string, commit: string) {
    const query = comparisonQuery(search);
    query.set("compare", "true"); query.set("file", path); query.set("read", "true"); query.set("source_commit", commit);
    return `?${query}`;
  }
  function files(items: CheckFile[]) {
    return <ul>{items.map(f => <li key={f.path}>
      <button className="impact-path" onClick={() => pick(f.base_path ?? f.path)}>{f.base_path ? `${f.base_path} → ` : ""}{f.path}</button>
      <small>{f.status}{f.placed ? " · placed by imports; no base footprint" : ""}</small>
      <span className="impact-sides">{sourceSides(f, search.base!, search.head!).map(side => <a key={side.side} onClick={e => { if (e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey) { e.preventDefault(); read(side.path, side.commit); } }} href={href(side.path, side.commit)}>{side.side} source</a>)}</span>
    </li>)}</ul>;
  }
  return <aside className="comparison-panel" aria-label="Change impact" onKeyDown={e => {
    e.stopPropagation();
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if ((e.target as HTMLElement).matches("input,select")) return;
    const controls = Array.from(e.currentTarget.querySelectorAll<HTMLElement>("button,a,input"));
    const i = controls.indexOf(document.activeElement as HTMLElement);
    if (i >= 0) { e.preventDefault(); controls[(i + (e.key === "ArrowDown" ? 1 : controls.length - 1)) % controls.length]?.focus(); }
  }}>
    <header><h2>Change impact</h2><button aria-label="Close comparison" onClick={() => update({ compare: undefined, base: undefined, head: undefined, source_commit: undefined, read: undefined })}>Return to map</button></header>
    <form onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); update({ compare: true, base: String(form.get("base")), head: String(form.get("head")), max_districts: form.get("max_districts") ? Number(form.get("max_districts")) : undefined, max_dq: form.get("max_dq") ? Number(form.get("max_dq")) : undefined, report_only: form.get("report_only") === "on", read: undefined, source_commit: undefined }); }}>
      <label>Base commit<input name="base" aria-label="Base commit" required pattern="[a-f0-9]{40}|[a-f0-9]{64}" defaultValue={search.base ?? search.commit} /></label>
      <label>Head commit<input name="head" aria-label="Head commit" required pattern="[a-f0-9]{40}|[a-f0-9]{64}" defaultValue={search.head} /></label>
      <label>Max districts<input name="max_districts" type="number" min="0" step="1" defaultValue={search.max_districts} /></label>
      <label>Max Δq drop<input name="max_dq" type="number" min="0" step="any" defaultValue={search.max_dq} /></label>
      <label><input name="report_only" type="checkbox" defaultChecked={search.report_only} />Report only (clear thresholds)</label>
      <button type="submit">Compare commits</button>
    </form>
    {(pending || refreshing) && search.base && search.head && <p role="status">{refreshing ? "Revalidating comparison; previous result is stale." : "Computing comparison…"}</p>}
    {error && <p role="alert">{error.message}. <button onClick={retry}>Retry</button></p>}
    {report && provenance && <>
      <p className="impact-commit">{report.base} → {report.head}</p>
      <p>Structural signal: {report.verdict}. Δq {report.delta_q}; {report.districts_crossed} districts crossed.</p>
      <p>Lower bound: only resolvable graph references are measured. This verdict does not establish safety or grant permission.</p>
      <p>Thresholds ({report.thresholds.source}): districts {report.thresholds.max_districts ?? "off"}, Δq drop {report.thresholds.max_dq ?? "off"}.</p>
      <p>Coverage: {report.base_files} base files / {report.base_districts} districts; {provenance.indexed_changes} indexed changes; {provenance.outside_graph.length} changes outside the graph.</p>
      <h3>Affected districts</h3><ul>{report.districts.map(d => <li key={d.id}><button onClick={() => pick(undefined, d.id)}>{d.name} · {d.changed_files} changed files</button></li>)}</ul>
      <h3>Changed files</h3>{files(report.files)}
      <h3>Unplaced files</h3><ul>{report.unplaced_files.map(f => <li key={f}>{f} · no base district</li>)}</ul>
      <h3>Outside the graph</h3>{files(provenance.outside_graph)}
      {(["added", "removed"] as const).map(kind => <section key={kind}><h3>Cross-district edges {kind}</h3><ul>{report[`edges_${kind}`].map((edge, i) => <li key={i}>{edge.source} → {edge.target}<br />District {edge.source_district} → {edge.target_district} · weight {edge.weight} · {edge.static_import ? "static import" : "other graph signals"}</li>)}</ul></section>)}
      <p>Edge weights compare within their own graph only.</p>
      <h3>Landmark touches</h3><ul>{report.landmark_touches.map(l => <li key={`${l.file}:${l.kind}`}><button onClick={() => pick(l.file)}>{l.file} · {l.kind}</button><p>{l.detail}</p></li>)}</ul>
      <details><summary>Immutable provenance</summary><p>Comparison {data!.id}</p><p>Base map: {provenance.base_map_source} · SHA-256 {provenance.base_map_sha256}</p><p>tolmap {provenance.tolmap_version} / {provenance.build_revision}; report v{provenance.report_version}; resolution {provenance.resolution}; prune {provenance.prune}; references {provenance.refs}; seed {provenance.seed}; created {provenance.created_at} (Unix seconds).</p></details>
    </>}
  </aside>;
}
