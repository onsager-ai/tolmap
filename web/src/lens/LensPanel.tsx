import { useQuery } from "@tanstack/react-query";
import type { LensList } from "@bindings/LensList";
import type { LensResponse } from "@bindings/LensResponse";
import type { LensTarget } from "@bindings/LensTarget";
import { API_BASE } from "@/api/client";
import { isDebtLens, lensDisplayName, DEBT_LENS } from "./debt";
import "./lens.css";

async function resource<T>(url: string, signal: AbortSignal): Promise<T> {
  const response = await fetch(url, { signal });
  if (!response.ok) {
    const body = await response.json();
    throw new Error(body.message ?? `Lenses unavailable (${response.status})`);
  }
  return response.json();
}

export function useLenses(owner: string, repo: string, commit: string | undefined, name: string | undefined, ready: boolean) {
  const map = `${API_BASE}/maps/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`;
  const root = `${map}/lenses`;
  const suffix = commit ? `?commit=${commit}` : "";
  const list = useQuery({ queryKey: ["lenses", owner, repo, commit], enabled: ready && !!commit, retry: false,
    queryFn: ({ signal }) => resource<LensList>(root + suffix, signal) });
  const resolved = useQuery({ queryKey: ["lens", owner, repo, commit, name], enabled: ready && !!commit && !!name, retry: false,
    queryFn: ({ signal }) => resource<LensResponse>(isDebtLens(name) ? `${map}/debt${suffix}` : `${root}/${encodeURIComponent(name!)}${suffix}`, signal) });
  // A response from another snapshot must never paint over this map.
  const mismatch = resolved.data && (resolved.data.name !== name || (commit && resolved.data.commit !== commit));
  return { list, resolved, data: mismatch ? undefined : resolved.data, error: ready && !commit ? new Error("Lenses require a map with a pinned service snapshot.") : mismatch ? new Error("Lens snapshot does not match this map.") : list.error ?? resolved.error };
}

export function LensPanel({ lenses, name, select, pick, mode = "full", details, compare, walk }: {
  lenses: ReturnType<typeof useLenses>; name?: string; select(name?: string): void; pick(target: LensTarget): void;
  mode?: "full" | "picker" | "sheet"; details?(): void; compare?(): void; walk?(path: string): void;
}) {
  const { list, resolved, data, error } = lenses;
  const body = <div className="lens-body">
    {mode !== "sheet" && <label>Saved lens<select aria-label="Saved lens" value={name ?? ""} onChange={event => select(event.target.value || undefined)}>
      <option value="">None</option>
      <optgroup label="Built-in"><option value={DEBT_LENS}>Debt</option></optgroup>
      <optgroup label="Repository">
        {name && !isDebtLens(name) && !list.data?.names.includes(name) && <option value={name}>{name}</option>}
        {list.data?.names.map(item => <option key={item} value={item}>{item}</option>)}
      </optgroup>
    </select></label>}
    {error ? <p role="alert" data-lens-state="error">{error.message}</p> : list.isPending || (name && resolved.isPending) ? <p role="status">Loading lenses…</p> : !list.data?.names.length && !data ? <p data-lens-state="empty">No lenses at this snapshot.</p> : null}
    {data && compare && <button onClick={compare}>Compare this lens</button>}
    {data && mode !== "picker" && isDebtLens(name) && !data.report.areas.length && <p data-lens-state="empty">No debt findings at this snapshot. Findings are lower bounds over the map's edges.</p>}
    {data && mode !== "picker" && !(isDebtLens(name) && !data.report.areas.length) && <div data-lens-state={data.report.unresolved.length ? "unresolved" : "loaded"}>
      {isDebtLens(name) && <p className="lens-meta" data-debt-summary>Debt findings are lower bounds over the map's edges: district cycles, misplaced files, god files, hotspots and files with no inbound edge found. Measured values are in the notes.</p>}
      {data.report.paths.map(path => <section key={path.name}><h3>{path.name}</h3>{walk && path.stops.length > 0 && <button data-lens-walk-start={path.name} onClick={() => walk(path.name)}>Start walkthrough</button>}<ol>{path.stops.map((stop, index) => {
        const hop = path.hops.find(item => item.to === index);
        return <li key={index} data-lens-stop-row={index}>
          {hop && <p className="lens-meta">{hop.resolved ? hop.source === "declared" ? "Declared" : `Computed · ${hop.edge_kind}` : "Unresolved hop"}{hop.reason ? ` — ${hop.reason}` : ""}</p>}
          {stop.target ? <button onClick={() => pick(stop.target!)}>{stop.spec}</button> : <span>Unresolved stop: {stop.spec}</span>}
        </li>;
      })}</ol></section>)}
      <p className="lens-meta">{isDebtLens(name) ? "Computed" : "Saved"} at {data.commit.slice(0, 12)} · computed paths are lower bounds from available edges.</p>
      <p className="lens-legend"><i className="lens-computed" /> Computed <i className="lens-declared" /> Declared</p>
      {data.report.areas.length > 0 && <section><h3>Areas</h3>{data.report.areas.map(area => <details key={area.label}><summary><i className="lens-swatch" style={{ background: `var(--lens-${area.color})` }} />{area.label} · {area.members.length}</summary><ul>{area.members.map((member, index) => <li key={index}><button onClick={() => pick(member.target)}>{member.spec}</button></li>)}</ul></details>)}</section>}
      {data.report.notes.length > 0 && <section><h3>Notes</h3>{data.report.notes.map((note, index) => <div key={index} className="lens-note"><span className="lens-meta">{note.target ? <button onClick={() => pick(note.target!)}>{note.anchor}</button> : note.anchor}</span><p>{note.text}</p></div>)}</section>}
      {data.report.unresolved.length > 0 && <section role="status" data-lens-unresolved><h3>Unresolved · {data.report.unresolved.length}</h3><ul>{data.report.unresolved.map((item, index) => <li key={index}><strong>{item.kind}</strong> · {item.location}<p>{item.spec}: {item.detail}</p></li>)}</ul></section>}
    </div>}
  </div>;
  return mode === "sheet" ? <section className="lens-panel lens-sheet" data-lens-panel data-lens-sheet>
    <header><h2>{data?.report.title ?? (name && lensDisplayName(name))}</h2>{details && <button onClick={details}>Details</button>}</header>{body}
  </section> : <details className="lens-panel" open={!!name} data-lens-panel>
    <summary>Lenses{name ? ` · ${data?.report.title ?? lensDisplayName(name)}` : ""}</summary>{body}
  </details>;
}
