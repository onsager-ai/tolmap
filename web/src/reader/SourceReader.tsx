import { useEffect, useRef, useState } from "react";
import { EditorState, Compartment } from "@codemirror/state";
import { EditorView, lineNumbers, keymap, highlightSpecialChars, drawSelection } from "@codemirror/view";
import { defaultHighlightStyle, HighlightStyle, syntaxHighlighting, LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { search, searchKeymap, openSearchPanel, getSearchQuery, setSearchQuery } from "@codemirror/search";
import { useEffectiveTheme } from "@/lib/theme";
import { API_BASE } from "@/api/client";
import { useVisualViewportBox } from "@/hooks/useVisualViewportBox";
import "./reader.css";

// Preserve the grammar tags, but own their palette. Default CM6 purple/blue
// failed on dark chrome (reader visual review, 2026-10-25). These Tolmap
// tokens are checked against the actual editor background in browser CI.
const readerPalette: Record<string, string> = { "#708": "var(--accent)", "#a11": "var(--link-in)", "#164": "var(--link-in)", "#a50": "var(--dim)" };
const readerHighlightStyle = HighlightStyle.define(defaultHighlightStyle.specs.map(spec => ({
  ...spec, color: typeof spec.color === "string" ? (readerPalette[spec.color] ?? "var(--on)") : undefined,
})));

export interface Provenance { version: number; repository: string; commit: string }
interface Source extends Provenance { path: string; status: string; text: string | null; bytes: number; long_line: boolean }
const messages: Record<string, string> = {
  missing: "This file is missing from this snapshot.", expired: "Source for this snapshot has expired. No newer source was substituted.",
  binary: "Binary or non-UTF-8 source cannot be displayed.", large: "This file exceeds the 1 MiB reading limit.",
  unsupported: "Symlinks, submodules and non-regular files cannot be displayed.",
};

/** One production CM6 reader, consumed unchanged by OSS and hosted adapters.
 * Search/highlighting state stays here, outside MapView and its renderer. */
export function SourceReader({ owner, repo, path, commit, start = 1, end = start, modal = false, onClose }: {
  owner: string; repo: string; path: string; commit?: string; start?: number; end?: number; modal?: boolean; onClose(): void;
}) {
  const region = useRef<HTMLElement>(null);
  const [searchStatus, setSearchStatus] = useState("Search this file");
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<EditorView | null>(null);
  const close = useRef<HTMLButtonElement>(null);
  const [source, setSource] = useState<Source | null>(null);
  const [failure, setFailure] = useState(false);
  const [retry, setRetry] = useState(0);
  const [grammar, setGrammar] = useState("Plain text");
  const theme = useEffectiveTheme();
  const viewport = useVisualViewportBox(modal);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    close.current?.focus({ preventScroll: true });
    return () => { if (previous?.isConnected) previous.focus({ preventScroll: true }); };
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    setSource(null); setFailure(false);
    const base = `${API_BASE}/maps/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}`;
    void (async () => {
      const p = await fetch(`${base}/provenance${commit ? `?commit=${encodeURIComponent(commit)}` : ""}`, { signal: controller.signal, cache: "no-store" });
      if (!p.ok) throw new Error("provenance unavailable");
      const provenance: Provenance = await p.json();
      if (provenance.version !== 1 || !/^[a-f0-9]{40,64}$/.test(provenance.commit) || (commit && provenance.commit !== commit)) throw new Error("snapshot mismatch");
      const query = new URLSearchParams({ commit: provenance.commit, path });
      const r = await fetch(`${base}/source?${query}`, { signal: controller.signal, cache: "no-store" });
      if (!r.ok) throw new Error("source unavailable");
      const value: Source = await r.json();
      if (value.version !== 1 || value.repository !== provenance.repository || value.commit !== provenance.commit || value.path !== path) throw new Error("source mismatch");
      if (!controller.signal.aborted) setSource(value);
    })().catch(() => { if (!controller.signal.aborted) setFailure(true); });
    return () => controller.abort();
  }, [owner, repo, path, commit, retry]);
  useEffect(() => {
    if (!host.current || source?.status !== "ready" || source.text == null) return;
    setSearchStatus("Search this file");
    let disposed = false;
    const language = new Compartment();
    const view = new EditorView({ parent: host.current, state: EditorState.create({ doc: source.text, extensions: [
      EditorState.readOnly.of(true), EditorView.editable.of(false),
      // Reject every document-changing transaction, including programmatic
      // paste/drop paths. Selection/search transactions remain available.
      EditorState.changeFilter.of(() => false),
      EditorView.contentAttributes.of({ tabindex: "0", "aria-label": "Read-only source", role: "textbox", "aria-readonly": "true" }),
      // A read-only content DOM can receive native focus without synchronizing
      // its selection first. Make the committed line range available to copy
      // immediately, including keyboard focus after a deep-link reload.
      EditorView.domEventHandlers({ focus: (_event, focused) => { focused.focus(); return false; } }),
      lineNumbers(), highlightSpecialChars(), drawSelection(), search({ top: true }), keymap.of([...searchKeymap, { key: "Mod-a", run: v => { v.dispatch({ selection: { anchor: 0, head: v.state.doc.length } }); return true; } }]),
      EditorView.updateListener.of(update => {
        if (update.transactions.some(t => t.effects.some(e => e.is(setSearchQuery)))) {
          const q = getSearchQuery(update.state);
          setSearchStatus(!q.valid ? "Enter text to search" : q.getCursor(update.state).next().done ? "No results" : "Matches found");
        }
      }),
      syntaxHighlighting(readerHighlightStyle), language.of([]),
      EditorView.theme({ "&": { height: "100%", color: "var(--on)", backgroundColor: "var(--chrome2)" }, ".cm-scroller": { overflow: "auto", fontFamily: '"IBM Plex Mono", monospace', overscrollBehavior: "contain" }, ".cm-gutters": { backgroundColor: "var(--chrome)", color: "var(--dim)", borderColor: "var(--rule)" }, ".cm-panels": { backgroundColor: "var(--chrome)", color: "var(--on)" }, "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "var(--chrome)" } }, { dark: theme === "dark" }),
    ] }) });
    editor.current = view;
    const first = Math.max(1, Math.min(view.state.doc.lines, start));
    const last = Math.max(first, Math.min(view.state.doc.lines, end));
    view.dispatch({ selection: { anchor: view.state.doc.line(first).from, head: view.state.doc.line(last).to }, effects: EditorView.scrollIntoView(view.state.doc.line(first).from, { y: "center" }) });
    const description = LanguageDescription.matchFilename(languages, path);
    setGrammar(source.long_line ? "Plain text · long line protection" : description?.name ?? "Plain text");
    if (description && !source.long_line) void description.load().then(support => { if (!disposed) view.dispatch({ effects: language.reconfigure(support) }); }).catch(() => { if (!disposed) setGrammar("Plain text · highlighting unavailable"); });
    return () => { disposed = true; editor.current = null; view.destroy(); };
  }, [source, path, start, end, theme]);
  return <section ref={region} style={modal && viewport ? { position: "fixed", top: viewport.top, height: viewport.height, bottom: "auto" } : undefined} className="source-reader" data-source-reader role={modal ? "dialog" : "region"} aria-modal={modal || undefined} aria-label="Source reader" onKeyDown={e => {
    e.stopPropagation();
    if (e.defaultPrevented) return;
    if (modal && e.key === "Tab") {
      const controls = Array.from(region.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input, select, [tabindex="0"]') ?? []).filter(el => el.getClientRects().length);
      const first = controls[0], last = controls.at(-1);
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
    }
 if (e.key === "Escape" && !host.current?.querySelector(".cm-search")) { e.preventDefault(); onClose(); } }}>
    <header><button ref={close} onClick={onClose}>Return to map</button><strong>{path}</strong><button onClick={() => { if (editor.current) openSearchPanel(editor.current); }} disabled={source?.status !== "ready"}>Find in file</button></header>
    <p className="source-provenance">{source ? `${source.repository} · ${source.commit}` : "Snapshot source"} · Read only</p>
    {!source && !failure && <p role="status">Loading committed source…</p>}
    {failure && <div role="alert"><p>Source unavailable. Check access or restore this snapshot. No current-branch fallback was used.</p><button onClick={() => setRetry(n => n + 1)}>Retry source</button></div>}
    {source && source.status !== "ready" && <p role="status">{messages[source.status] ?? "Source unavailable."}</p>}
    {source?.status === "ready" && <p className="source-provenance">{grammar} · {source.bytes.toLocaleString()} bytes · Lines {start}–{end}</p>}
    {source?.status === "ready" && <p className="source-provenance" role="status" data-source-search-status>{searchStatus}</p>}
    <div className="source-editor" ref={host} />
  </section>;
}
