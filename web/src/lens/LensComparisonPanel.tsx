import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { LensComparison } from "@bindings/LensComparison";
import type { LensDelta } from "@bindings/LensDelta";
import type { LensList } from "@bindings/LensList";
import type { LensTarget } from "@bindings/LensTarget";
import { API_BASE } from "@/api/client";
import { isDebtLens, lensDisplayName } from "./debt";
import type { MapSearch } from "@/routes/search";
import "@/comparison/comparison.css";

export function useLensComparison(owner: string, repo: string, search: MapSearch, commit?: string, mapSha256?: string) {
  const query = new URLSearchParams({ base: search.base ?? "", head: search.head ?? "", lens: search.lens ?? "" });
  if (search.other_lens) query.set("other", search.other_lens);
  return useQuery({ queryKey: ["lens-comparison", owner, repo, query.toString(), commit, mapSha256], enabled: !!search.lens_compare && !!search.base && !!search.head && !!search.lens && !!commit && !!mapSha256,
    retry: false, staleTime: 0, gcTime: 0,
    queryFn: async ({ signal }) => {
      // The built-in debt lens has its own route and compares commits only.
      const resource = isDebtLens(search.lens) ? `debt?${new URLSearchParams({ base: search.base ?? "", head: search.head ?? "" })}` : `lenses?${query}`;
      const response = await fetch(`${API_BASE}/compare/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/${resource}`, { signal, cache: "no-store" });
      const value = await response.json();
      if (!response.ok) throw new Error(value.message ?? "Lens comparison unavailable");
      const result = value as LensComparison;
      if (result.version !== 1 || !result.lower_bound) throw new Error("Unsupported lens comparison version");
      if (!mapSha256 || result.base_map_sha256 !== mapSha256 || commit !== search.base || (owner !== "snapshot" && result.repository !== `${owner}/${repo}`) || result.base !== search.base || result.head !== search.head || result.lens !== search.lens || result.other !== (search.other_lens ?? search.lens)) throw new Error("Stale or mismatched lens comparison snapshot. Reload the base map.");
      return result;
    },
  });
}

function statusLabel(change: LensDelta, data: LensComparison) {
  if (data.lens !== data.other) {
    if (change.status === "removed") return `Only in ${data.lens}`;
    if (change.status === "added") return `Only in ${data.other}`;
    if (change.status === "shared") return "In both";
  }
  return { added: "Added", removed: "Removed", broken: "Broken", renamed: "Renamed", moved: "Moved", shared: "Unchanged" }[change.status];
}

function ChangeRow({ change, data, pick }: { change: LensDelta; data: LensComparison; pick(target: LensTarget): void }) {
  const specs = [...new Set([...change.before, ...change.after])];
  const renames = [...new Map(data.after.unresolved.flatMap(item => item.renamed && change.before.some(spec => spec.split("#")[0] === item.renamed!.from) ? [[item.renamed.from, item.renamed] as const] : [])).values()];
  return <li data-lens-delta={change.status}>
    <strong>{statusLabel(change, data)}</strong><small>{change.label}</small>
    <div className="lens-delta-paths">{specs.map((spec, index) => {
      const target = change.targets.find(target => target.path === spec.split("#")[0]);
      return <span key={spec}>{index > 0 && <span aria-hidden="true"> → </span>}{target ? <button onClick={() => pick(target)}>{spec}</button> : <span>{spec}</span>}</span>;
    })}</div>
    {change.status === "renamed" && <small>{renames.length ? renames.map(rename => `Unresolved at head · rename commit ${rename.commit.slice(0, 12)}`).join("; ") : `Rename at ${data.head.slice(0, 12)}`}</small>}
    {!change.targets.length && <small>No base footprint{change.districts.length ? ` · districts ${change.districts.join(", ")}` : " · unplaced"}</small>}
  </li>;
}

export function LensComparisonPanel({ search, result, list, update, pick, embedded = false, expanded = true, onExpand }: { search: MapSearch; result: ReturnType<typeof useLensComparison>; list?: LensList; update(patch: Partial<MapSearch>): void; pick(target: LensTarget): void; embedded?: boolean; expanded?: boolean; onExpand?(): void }) {
  const data = !result.isFetching && !result.error ? result.data : undefined;
  const [editing, setEditing] = useState(false);
  const panelRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!data || editing) return;
    // Scroll only the owning viewport. scrollIntoView can move every
    // ancestor and used to push the phone's search pill off screen.
    const panel = panelRef.current;
    const scroller = embedded ? panel?.closest<HTMLElement>("[data-sheet-body]") : panel?.querySelector<HTMLElement>(".lens-comparison-results");
    scroller?.scrollTo({ top: 0 });
  }, [data, editing, embedded]);
  const overlap = data ? data.lens !== data.other : !!search.other_lens;
  const changed = data?.changes.filter(change => change.status !== "shared") ?? [];
  const unchanged = data?.changes.filter(change => change.status === "shared") ?? [];
  const showForm = editing || !result.data;
  const rows = (changes: LensDelta[]) => (["area", "stop", "hop"] as const).map(kind => {
    const items = changes.filter(change => change.kind === kind);
    return items.length > 0 && <section key={kind}><h3>{kind === "area" ? "Area membership" : kind === "stop" ? "Stops" : "Hops"} · {items.length}</h3><ul>{items.map((change, index) => <ChangeRow key={index} change={change} data={data!} pick={pick} />)}</ul></section>;
  });
  return <aside ref={panelRef} className={`comparison-panel lens-comparison-panel${embedded ? " lens-comparison-sheet" : ""}`} aria-label="Lens comparison" data-lens-comparison>
    <header><h2>Lens comparison</h2><button onClick={() => update({ lens_compare: undefined, other_lens: undefined, compare: undefined, head: undefined, commit: search.base })}>Return to lens</button></header>
    <div className="lens-comparison-summary" data-lens-comparison-summary data-lens-comparison-state={!expanded && data ? "loaded" : undefined}>
      {expanded && <p>{overlap ? `${search.lens} ∩ ${search.other_lens}` : search.lens && lensDisplayName(search.lens)} · {search.base?.slice(0, 8)}{!overlap && search.head ? ` → ${search.head.slice(0, 8)}` : ""}</p>}
      {data && <p>{embedded && !expanded ? <button aria-label="View changes" onClick={onExpand}>{overlap ? `${changed.length} differing · ${unchanged.length} in both` : `${changed.length} changes · ${unchanged.length} unchanged`}</button> : overlap ? `${changed.length} differing · ${unchanged.length} in both` : `${changed.length} changes · ${unchanged.length} unchanged`}</p>}
      {embedded && !expanded && !data && <button onClick={onExpand}>{result.error ? "Stale or unavailable · View details" : "Set up comparison"}</button>}
    </div>
    {expanded && <>
      <div className={`lens-comparison-edit${showForm ? " is-open" : ""}`}><button aria-expanded={showForm} onClick={() => setEditing(value => !value)}>Edit comparison</button>
        {showForm && <form onSubmit={event => { event.preventDefault(); const values = new FormData(event.currentTarget); const other = String(values.get("other") ?? ""); const base = String(values.get("base")); setEditing(false); update({ lens_compare: true, compare: true, base, head: other ? base : String(values.get("head")), other_lens: other || undefined }); }}>
          <label>Base commit<input aria-label="Lens base commit" name="base" required pattern="[a-f0-9]{40}|[a-f0-9]{64}" defaultValue={search.base ?? search.commit} /></label>
          <label>Head commit<input aria-label="Lens head commit" name="head" pattern="[a-f0-9]{40}|[a-f0-9]{64}" defaultValue={search.head} placeholder="Same lens at another commit" /></label>
          {!isDebtLens(search.lens) && <label>Compare with lens at base<select name="other" aria-label="Compare with lens" defaultValue={search.other_lens ?? ""}><option value="">Same lens across commits</option>{search.other_lens && !list?.names.includes(search.other_lens) && <option value={search.other_lens}>{search.other_lens}</option>}{list?.names.filter(name => name !== search.lens).map(name => <option key={name}>{name}</option>)}</select></label>}
          <button>Compare lenses</button>
        </form>}
      </div>
      <div className="lens-comparison-results">
        {data && <div data-lens-comparison-state="loaded">
          <p className="lens-legend">{overlap ? `+ Only in ${data.other} · − Only in ${data.lens} · = In both` : "+ Added · − Removed · × Broken · ↗ Renamed · ↔ Moved"}</p>
          {rows(changed)}
          {!changed.length && <p>{overlap ? "All members, stops and hops are in both lenses." : "No changes to this lens."}</p>}
          {!!unchanged.length && <details data-lens-unchanged><summary>{unchanged.length} {overlap ? "in both" : "unchanged"}</summary>{rows(unchanged)}</details>}
          <small>Fixed base map · routes are lower bounds · new files have district placement only.</small>
          {(["before", "after"] as const).map(side => data[side].unresolved.length > 0 && <details key={side}><summary>{data[side].unresolved.length} unresolved {side}</summary><ul>{data[side].unresolved.map((item,index) => <li key={index}><strong>{item.kind}</strong> · {item.location}<p>{item.spec}: {item.detail}</p></li>)}</ul></details>)}
        </div>}
      </div>
    </>}
    {!search.base || !search.head ? <p>Choose two commits, or another lens at the base commit.</p> : result.isPending || result.isFetching ? <p role="status" data-lens-comparison-state="loading">{result.isFetching && result.data ? "Revalidating; previous lens comparison is stale." : "Computing lens comparison…"}</p> : null}
    {result.error && (expanded ? <p role="alert" data-lens-comparison-state="error">{result.error.message} <button onClick={() => { void result.refetch(); }}>Retry</button></p> : <span role="alert" className="sr-only" data-lens-comparison-state="error">{result.error.message}</span>)}
  </aside>;
}
