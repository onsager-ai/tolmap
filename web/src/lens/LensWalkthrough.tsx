import type { LensReport } from "@bindings/LensReport";
import type { LensTarget } from "@bindings/LensTarget";
import type { ResolvedHop } from "@bindings/ResolvedHop";
import type { ResolvedPath } from "@bindings/ResolvedPath";
import "./lens.css";

/** Issue #248: one step of a lens walkthrough, `?path=<name>&step=<n>`
 * (1-based). `null` when the path is not in this report. */
export function walkthroughStep(report: LensReport | undefined, name: string | undefined, step: number | undefined) {
  const path = name && step ? report?.paths.find(item => item.name === name) : undefined;
  if (!path || !path.stops.length) return null;
  const index = Math.min(path.stops.length, Math.max(1, step!)) - 1;
  return { path, index, stop: path.stops[index], hop: path.hops.find(item => item.to === index) };
}

/** The targets a step frames besides its stop: the previous stop and the
 * files of a computed route arriving at it. */
export function walkthroughHop(path: ResolvedPath, index: number, files: readonly string[], district: (file: number) => number): LensTarget[] {
  const hop = path.hops.find(item => item.to === index);
  const from = path.stops[index - 1]?.target;
  if (!hop?.resolved || !from) return [];
  const via = hop.source === "computed" ? hop.via.map(via => files.indexOf(via)).filter(file => file >= 0)
    .map(file => ({ file, path: files[file], district: district(file), symbol: null })) : [];
  return [from, ...via];
}

const sameTarget = (a: LensTarget, b: LensTarget) => a.file === b.file
  && (a.symbol?.name ?? null) === (b.symbol?.name ?? null) && (a.symbol?.start ?? null) === (b.symbol?.start ?? null);

const EDGE_WORDS: Record<string, string> = { imports: "imports", "imported-by": "imported by", undirected: "linked by imports", "same-file": "same file" };

function Provenance({ report, path, index, hop }: { report: LensReport; path: ResolvedPath; index: number; hop?: ResolvedHop }) {
  if (index === 0) return <p className="lens-meta" data-lens-walk-hop="start">Start of the path.</p>;
  if (!hop?.resolved) {
    const reason = report.unresolved.find(item => item.location === `paths.${path.name}.hops[${index - 1}]`);
    return <p className="lens-meta" data-lens-walk-hop="unresolved">Unresolved hop from stop {index}{reason ? ` — ${reason.detail}` : hop?.source === "declared" && hop.reason ? ` — declared: ${hop.reason}` : " — a stop on it is unresolved."}</p>;
  }
  if (hop.source === "declared") return <p className="lens-meta" data-lens-walk-hop="declared"><i className="lens-declared" /> Declared hop from stop {index}{hop.reason ? ` — ${hop.reason}` : ""}</p>;
  return <p className="lens-meta" data-lens-walk-hop="computed"><i className="lens-computed" /> Computed hop from stop {index} · {EDGE_WORDS[hop.edge_kind ?? ""] ?? hop.edge_kind} · route from map edges (lower bound){hop.via.length > 2 ? `: ${hop.via.join(" → ")}` : ""}</p>;
}

/** The walkthrough card: position, controls, hop provenance, the stop's
 * unresolved reason and its anchored notes. The file or symbol card itself
 * is the ordinary selection card the chrome shows with it. */
export function LensWalkthrough({ report, name, step, go, exit, keyboard }: {
  report: LensReport; name: string; step: number; go(step: number): void; exit(): void; keyboard?: boolean;
}) {
  const current = walkthroughStep(report, name, step);
  if (!current) {
    return <section className="lens-walkthrough" data-lens-walkthrough data-lens-walk-state="missing" role="status">
      <p>Path “{name}” is not in this lens.</p>
      <button onClick={exit}>End walkthrough</button>
    </section>;
  }
  const { path, index, stop, hop } = current;
  const last = path.stops.length - 1;
  const unresolved = !stop.target ? report.unresolved.find(item => item.location === `paths.${path.name}.stops[${index}]`) : undefined;
  const notes = stop.target ? report.notes.filter(note => note.target && sameTarget(note.target, stop.target!)) : [];
  const state = !stop.target ? "unresolved" : index === 0 ? "first" : index === last ? "last" : "middle";
  return <section className="lens-walkthrough" data-lens-walkthrough data-lens-walk-state={state} data-lens-walk-step={index + 1} aria-label="Lens walkthrough">
    <header>
      <h3>{path.name}</h3>
      <span className="lens-meta" aria-live="polite">Step {index + 1} of {path.stops.length}</span>
    </header>
    <div className="lens-walk-controls">
      <button data-lens-walk="previous" disabled={index === 0} onClick={() => go(index)}>Previous</button>
      <button data-lens-walk="next" disabled={index === last} onClick={() => go(index + 2)}>Next</button>
      <button data-lens-walk="exit" onClick={exit}>End</button>
    </div>
    <Provenance report={report} path={path} index={index} hop={hop} />
    {stop.target
      ? <p className="lens-walk-stop" data-lens-walk-stop={stop.spec}>{stop.spec}</p>
      : <div role="status" data-lens-walk-unresolved>
          <p className="lens-walk-stop">Unresolved stop: {stop.spec}</p>
          <p>{unresolved ? `${unresolved.kind} · ${unresolved.detail}` : "This stop did not resolve at this snapshot."} The map stays where it is.</p>
        </div>}
    {notes.map((note, i) => <p key={i} className="lens-note" data-lens-walk-note>{note.text}</p>)}
    {keyboard && <p className="lens-meta">← and → also step.</p>}
  </section>;
}
