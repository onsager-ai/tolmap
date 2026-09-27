import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useRouter, useSearch } from "@tanstack/react-router";
import { useCatalogue, useMapDocument, useDistrictSymbolsMap } from "@/data/queries";
import { MapCanvas, type MapCanvasHandle } from "@/map/MapCanvas";
import type { FrameInsets, MapRendererCallbacks } from "@/map/MapRenderer";
import { buildAdj, findRoute, type Route } from "@/map/graph";
import { D_, DESKTOP_INSETS, districtClass } from "@/map/geometry";
import { decodeDistrictSymbols, parentGlobalOf } from "@/map/symbolCards";
import type { SearchHit } from "@/map/search";
import { TopBar } from "@/components/TopBar";
import { Sidebar } from "@/components/Sidebar";
import { SearchBox } from "@/components/SearchBox";
import { SelectionPanel } from "@/components/SelectionPanel";
import { SelectionSummaryBar } from "@/components/SelectionSummaryBar";
import { RouteBox } from "@/components/RouteBox";
import { FooterStats } from "@/components/FooterStats";
import { ZoomControls } from "@/components/ZoomControls";
import { PackageLegend } from "@/components/PackageLegend";
import { LoadProgressIndicator } from "@/components/LoadProgressIndicator";
import { DetailStatusNote } from "@/components/DetailStatusNote";
import { useEarlyMapJob } from "@/api/useEarlyMapJob";
import { buildPackageLayout } from "@/map/packageLayout";
import { useEffectiveTheme } from "@/lib/theme";
import type { MapSearch } from "@/routes/search";
import { useIsNarrow } from "@/hooks/useIsNarrow";
import { usePhoneMetrics } from "@/hooks/usePhoneMetrics";
import { detentHeights, safeInsets, type Detent } from "@/map/phoneShell";
import { closeOverlay, initBack, openOverlay, OVERLAY_MARKER, popTo, type BackState, type OverlayKind } from "@/map/backStack";
import { structureDetail } from "@/map/structureCard";
import { PhoneChrome } from "@/components/phone/PhoneChrome";
import type { PathPick, StructureCardState } from "@/components/phone/SheetCards";

/** The /:owner/:repo page: assembles chrome (TopBar, Sidebar, SearchBox,
 * SelectionPanel, RouteBox, FooterStats, ZoomControls) around one MapCanvas
 * on desktop, and the phone shell (components/phone/PhoneChrome.tsx:
 * docs/UX.md §3's pill, control column and one bottom sheet) on a phone.
 * This component owns every piece of application state — selection, geo,
 * layer, route — either directly or via the URL; MapCanvas/MapRenderer only
 * ever receive it as props and report gestures back through callbacks. */
export function MapView() {
  const { owner, repo } = useParams({ strict: false }) as { owner: string; repo: string };
  const search = useSearch({ strict: false }) as MapSearch;
  const navigate = useNavigate();
  const { data: catalogue } = useCatalogue();
  // docs/UX.md §12: a map opened before its job's Detail phase finished
  // (`?job=`) is pinned to that job's commit, and asks for symbols only once
  // the job is done.
  const early = useEarlyMapJob(search.job);
  const { data: doc, isLoading, isError, error, source } = useMapDocument(owner, repo, {
    commit: early.commit,
    wait: early.waiting,
  });

  // Issue #82 C2 scope item 1: districts whose symbols the map/sidebar
  // currently want. Two sources add to this set and it only ever grows for
  // the life of one loaded document (TanStack Query's own staleTime:
  // Infinity cache means asking again is free) -- MapRenderer reports a
  // newly-gated file's district through onNeedSymbols (debounced below), and
  // the effect right after this state declaration adds the SELECTED file's
  // district unconditionally and immediately (spec: "or when a file in it is
  // selected" -- that's a deliberate, synchronous action, not something to
  // wait 200ms on).
  const [wantedDistricts, setWantedDistricts] = useState<Set<number>>(new Set());
  const pendingDistrictsRef = useRef<Set<number>>(new Set());
  const flushTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onNeedSymbols = (d: number) => {
    pendingDistrictsRef.current.add(d);
    if (flushTimerRef.current == null) {
      flushTimerRef.current = setTimeout(() => {
        flushTimerRef.current = null;
        const toAdd = pendingDistrictsRef.current;
        pendingDistrictsRef.current = new Set();
        setWantedDistricts((prev) => {
          let changed = false;
          const next = new Set(prev);
          for (const d2 of toAdd) {
            if (!next.has(d2)) {
              next.add(d2);
              changed = true;
            }
          }
          return changed ? next : prev;
        });
      }, 200); // debounced (spec item 7): a burst of pan/zoom frames collapses to one flush
    }
  };
  useEffect(() => {
    return () => {
      if (flushTimerRef.current != null) clearTimeout(flushTimerRef.current);
    };
  }, []);
  const { map: districtSymbolsMap, loading: symbolsLoadingDistricts } = useDistrictSymbolsMap(
    owner,
    repo,
    source,
    [...wantedDistricts],
    { commit: early.commit, ready: !early.waiting && !early.detailPending },
  );
  // The job ended: the map's URL is the plain one again (a shared link
  // should not watch a finished job), while the commit stays pinned.
  useEffect(() => {
    if (search.job && early.settled) updateSearch({ job: undefined });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search.job, early.settled]);

  const canvasRef = useRef<MapCanvasHandle>(null);
  const mapAreaRef = useRef<HTMLDivElement>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  const [, setSideOpen] = useState(false); // the old phone drawer is gone (docs/UX.md §3); kept as a no-op setter for the shared select paths
  const [routeFrom, setRouteFrom] = useState<number | null>(null);
  const [route, setRoute] = useState<Route | null>(null);
  const [unconnectedRepo, setUnconnectedRepo] = useState<string | null>(null);
  const [previewDirectory, setPreviewDirectory] = useState<{ repo: string; path: string } | null>(null);

  // ---------- the phone shell (docs/UX.md §3) ----------
  const narrow = useIsNarrow();
  const router = useRouter();
  const metrics = usePhoneMetrics();
  const heights = detentHeights(metrics);
  const [detent, setDetentState] = useState<Detent>("peek");
  const [searchOpen, setSearchOpen] = useState(false);
  const [layersOpen, setLayersOpen] = useState(false);
  const [quality, setQuality] = useState(false);
  const [indexTab, setIndexTab] = useState<"districts" | "folders">("districts");
  const [structure, setStructure] = useState<StructureCardState | null>(null);
  const [pathPick, setPathPick] = useState<PathPick | null>(null);
  const [pathEnds, setPathEnds] = useState<[number, number] | null>(null);
  // docs/UX.md §3.3: the safe rectangle from the real chrome. `frame` is the
  // resting layout (pill + Peek), which the level-of-detail scale is
  // measured against; `safe` follows the sheet. Desktop keeps its margins
  // until phase 5.
  const peekInsets = safeInsets(metrics, heights.peek);
  // At Full the sheet covers the map up to the pill, leaving no rect to
  // frame into; a camera move made then (Zoom to district from the card)
  // frames as at Half, so it lands where the map shows again once the sheet
  // comes down.
  const frameInsets: FrameInsets = narrow
    ? { frame: peekInsets, safe: safeInsets(metrics, heights[detent === "full" ? "half" : detent]), centreInSafe: true }
    : { frame: DESKTOP_INSETS, safe: DESKTOP_INSETS, centreInSafe: false };
  /** Before a camera move made together with a selection (which always
   * lands the sheet at Peek, §3.2): the renderer frames into the Peek rect
   * now, not after React commits. */
  function framePeekNow() {
    if (narrow) canvasRef.current?.setInsets({ frame: peekInsets, safe: peekInsets, centreInSafe: true });
  }
  // Issue #82 "chrome follows the theme": buildPackageLayout resolves every
  // `--pN`/`--canvas` custom property it needs ONCE, into a plain colour
  // array per depth (map/packageLayout.ts's own cssCache) -- correct for a
  // page load, stale the moment a System/Light/Dark toggle changes those
  // custom properties without a reload. Keying this memo on `effectiveTheme`
  // too (lib/theme.ts already invalidates the underlying cache on every
  // theme change) forces a fresh build, which is also what actually gets
  // MapCanvas to repaint: its own effect already lists `packageGrouping` in
  // its dependency array, so a new object reference here re-triggers
  // renderer.render() for every layer, not just "p" -- geometry.ts's own
  // districtColor/ramp caches are invalidated the same way, but nothing else
  // in this component's props changes on a theme toggle to trigger a
  // repaint for THEM without this.
  const effectiveTheme = useEffectiveTheme();
  // effectiveTheme is a deliberate cache-busting key, not a value buildPackageLayout reads directly.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const packageLayout = useMemo(() => (doc ? buildPackageLayout(doc) : null), [doc, effectiveTheme]);

  // Issue #82 A1 scope item 5 (fullscreen). Fullscreened element is the map
  // AREA (below), not the whole page: TopBar and Sidebar are its siblings,
  // so the Fullscreen API path hides them simply by not being part of what
  // the browser paints while fullscreen is active -- no separate
  // show/hide logic needed for that path. The CSS fallback (no element
  // Fullscreen API -- iOS Safari) instead covers them with `fixed inset-0`,
  // the same technique the prototype's own `.mapbox.fs` used (arch20 -- see
  // its setFS/reAspect); either way `isFullscreen` is the one source of
  // truth the JSX below reads, so React chrome and the CSS class agree.
  const [isFullscreen, setIsFullscreen] = useState(false);

  useEffect(() => {
    function onFsChange() {
      setIsFullscreen(!!document.fullscreenElement && document.fullscreenElement === mapAreaRef.current);
    }
    document.addEventListener("fullscreenchange", onFsChange);
    return () => document.removeEventListener("fullscreenchange", onFsChange);
  }, []);

  async function exitFullscreen() {
    if (document.fullscreenElement) {
      try {
        await document.exitFullscreen();
      } catch {
        /* already exiting, or the browser refused -- state is set below either way */
      }
    }
    setIsFullscreen(false);
  }
  async function enterFullscreen() {
    const el = mapAreaRef.current;
    if (el?.requestFullscreen) {
      try {
        await el.requestFullscreen();
        return; // onFsChange (above) flips isFullscreen once the browser confirms
      } catch {
        // iPhone Safari has no element Fullscreen API at all (requestFullscreen
        // is undefined there, so this branch is never reached on it) -- this
        // catch is for a DESKTOP browser that has the API but refuses the
        // call (e.g. not called from a direct user gesture). Either way, fall
        // through to the CSS-only mode rather than doing nothing.
      }
    }
    setIsFullscreen(true);
  }
  function toggleFullscreen() {
    if (isFullscreen) void exitFullscreen();
    else void enterFullscreen();
  }
  // Esc: the native path already exits via the browser and fires
  // fullscreenchange (handled above); this covers the CSS fallback, where
  // there is no native fullscreen state for Esc to exit on its own.
  // exitFullscreen()'s own guard makes calling it redundantly (native path,
  // already exiting) harmless.
  useEffect(() => {
    if (!isFullscreen) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") void exitFullscreen();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isFullscreen]);

  // radj (imported-by) is new here: SelectionPanel's "links" line and
  // MapRenderer's own selection-links feature both need it, and buildAdj
  // already computes both from one pass over doc.E -- discarding radj and
  // having MapRenderer separately rebuild it (it does, for the map surface
  // itself) would be a second identical scan of doc.E on this side too.
  const { adj, radj } = useMemo(() => (doc ? buildAdj(doc) : { adj: new Map(), radj: new Map() }), [doc]);
  const { maxCh, maxCx } = useMemo(() => {
    if (!doc) return { maxCh: 1, maxCx: 1 };
    return {
      maxCh: Math.max(1, ...doc.N.map((r) => r[5])),
      maxCx: Math.max(1, ...doc.N.map((r) => r[4])),
    };
  }, [doc]);

  // Desktop used to auto-select the top landmark on load (the reference did
  // this too: `if(R.L.length && !NARROW()) select(R.L[0][0],true)`), landing
  // every fresh /:owner/:repo view with something already highlighted; a
  // phone never did (that was the whole point of the NARROW() check).
  // Removed (review finding on the readable-overview PR): once selection
  // started dimming non-neighbour files (that PR's own change), the
  // auto-select made a FRESH desktop load of django/django open with 685 of
  // 851 dots dimmed -- the overview auto-obstructing itself before a reader
  // had asked for anything. The reference could afford landing on a
  // landmark because selection had no visual cost there; it does now, so
  // desktop opens unobstructed the same as phone always has. A `?file=`
  // deep link is unaffected -- it's an explicit selection, and dimming is
  // the correct, requested behaviour for one.
  const sel = doc && search.file ? (() => { const i = doc.F.indexOf(search.file!); return i >= 0 ? i : null; })() : null;
  const selSym = sel != null && search.sym != null ? search.sym : null;
  // Issue #82 C2: `hsym`, a GLOBAL hierarchical-symbol index -- a separate
  // URL param and index space from `sym` above (search.ts's own doc
  // comment). Both can be present in the URL at once only transiently (a
  // stale link); selHSym is read independently and a card selection always
  // clears `sym` (selectHierSymbol below), so the two never compete for
  // what the breadcrumb/sidebar actually show.
  const selHSym = sel != null && search.hsym != null ? search.hsym : null;
  const selD = sel == null && search.d != null ? search.d : null;

  // The selected FILE's district symbols, decoded once per fetch -- what
  // SelectionPanel's outline tree/external references and the breadcrumb's
  // class/method chain both read. `undefined` (not yet fetched, or this map
  // has no symbols sibling at all) degrades silently: every reader below
  // just sees `null` and renders what it would have before this feature.
  const selFileDistrict = doc && sel != null ? D_(doc, sel) : null;
  const selSymbolsDoc = selFileDistrict != null ? districtSymbolsMap.get(selFileDistrict) : undefined;
  const selDecoded = useMemo(() => (selSymbolsDoc ? decodeDistrictSymbols(selSymbolsDoc) : null), [selSymbolsDoc]);

  // Issue #82 C2 follow-up: true while this file's district symbols are
  // still fetching, OR haven't been added to `wantedDistricts` yet -- the
  // effect right below adds it synchronously, but the render in between has
  // no query for it at all yet, which would otherwise read as "definitely no
  // symbols" for one frame. SelectionPanel shows a single "loading
  // symbols…" line for either case rather than flashing its old flat list,
  // falling back to that list only once the district is confirmed to have
  // none (query settled, still no data).
  const selSymbolsLoading =
    selFileDistrict != null &&
    !selSymbolsDoc &&
    (!wantedDistricts.has(selFileDistrict) || symbolsLoadingDistricts.has(selFileDistrict));

  // Spec item 1's other trigger ("or when a file in it is selected"): add
  // the selected file's district to `wantedDistricts` immediately, not on
  // the renderer's own 200ms-debounced onNeedSymbols path -- a deliberate
  // file selection deserves its outline tree right away, not after whatever
  // zoom-driven bursts happen to be in flight.
  useEffect(() => {
    if (selFileDistrict == null) return;
    setWantedDistricts((prev) => (prev.has(selFileDistrict) ? prev : new Set(prev).add(selFileDistrict)));
  }, [selFileDistrict]);

  // Resolves a card tap's GLOBAL symbol index back to its file, across every
  // district currently loaded -- the renderer only ever reports a symbol it
  // just decoded and drew, so this always has an answer for it.
  const globalSymbolFile = useMemo(() => {
    const m = new Map<number, number>();
    for (const raw of districtSymbolsMap.values()) {
      raw.symbol_indices.forEach((g, idx) => m.set(g, raw.symbols[idx][0]));
    }
    return m;
  }, [districtSymbolsMap]);

  function selectHierSymbol(global: number) {
    if (!doc) return;
    const fileIdx = globalSymbolFile.get(global);
    if (fileIdx == null) return;
    setUnconnectedRepo(null);
    // Scope item 3: "setting all levels" -- file, symbol, and dropping
    // whatever else (a bare district, a directory highlight, the old `sym`)
    // was selected, in one URL update.
    updateSearch({ file: doc.F[fileIdx], sym: undefined, hsym: global, d: undefined, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    framePeekNow();
    if (districtClass(doc.districts[String(D_(doc, fileIdx))]) !== "unconnected") canvasRef.current?.panTo(fileIdx);
  }

  function updateSearch(patch: Partial<typeof search>) {
    navigate({
      to: ".",
      search: (prev: Record<string, unknown>) => ({ ...prev, sel: undefined, ...patch }),
      replace: true,
      // docs/UX.md §3.4: a selection change replaces the URL of whatever
      // entry is on top -- possibly one the back stack pushed. Keeping its
      // history state keeps its overlay marker (map/backStack.ts).
      state: true,
    });
  }

  // Issue #82 A1 scope item 1 ("selecting never moves the map"): there is no
  // more `fly` option. Every selection path -- a map tap, a sidebar pick, a
  // search result, a deep link, a breadcrumb segment -- goes through
  // panTo/panToDistrict now (MapRenderer's own pan-only methods), which are
  // no-ops when the target is already on screen. A map tap's target is
  // always already on screen by construction, so calling panTo
  // unconditionally here is exactly equivalent to the old "fly: false" for
  // that case, and correctly brings an off-screen sidebar/search/deep-link
  // target into view (at the CURRENT zoom) for the others -- one function,
  // no flag to thread through every caller.
  function selectFile(i: number, opts: { symbol?: number } = {}) {
    if (!doc) return;
    setUnconnectedRepo(null);
    updateSearch({ file: doc.F[i], sym: opts.symbol, hsym: undefined, d: undefined, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
    framePeekNow();
    if (districtClass(doc.districts[String(doc.N[i][0])]) !== "unconnected") canvasRef.current?.panTo(i);
  }
  // selectDistrict deliberately never pans on its own: it's reused by the
  // map's own district-polygon tap (already on screen), by SelectionPanel's
  // internal "near" neighbour and breadcrumb links (selecting a level that
  // was already reachable from the current card, so there's nothing new to
  // bring into view), and by the breadcrumb's own district segment, all of
  // which the spec requires to move the view NOT AT ALL, not "pan if
  // needed." The sidebar's district row is the one caller that DOES need
  // panning (its target can be genuinely off screen) -- see the dedicated
  // wrapper passed to <Sidebar> below, which calls this and then pans.
  function selectDistrict(d: number) {
    setUnconnectedRepo(null);
    updateSearch({ file: undefined, sym: undefined, d, dir: undefined });
    setPanelOpen(true);
    setSideOpen(false);
  }
  // Jumps straight to "nothing selected" -- the breadcrumb's own repo
  // segment, and an explicit clear. Distinct from stepBackSelection below,
  // which is one level at a time.
  function clearAll() {
    setUnconnectedRepo(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined, dir: undefined });
    setPanelOpen(false);
  }
  // Issue #82 A1 scope item 2: an empty map tap used to clear the whole
  // selection in one step; now it steps back exactly one level --
  // symbol -> its file -> the file's district -> nothing -- matching the
  // prototype's own click handler (arch20.body.html's plain
  // `svg.addEventListener("click", ...)"`, not the file-hit one). A
  // directory highlight and the unconnected-files view aren't part of that
  // repo/district/file/symbol hierarchy, but each still reduces to "select
  // one level up" the same way: dropped in one tap, panel closed on the
  // step that reaches "nothing." An unconnected file's own "district" isn't
  // a place shown anywhere else in the UI (Sidebar never lists it, see
  // districtClass's "unconnected" branch), so stepping back from one skips
  // the district level entirely rather than landing on a card nothing else
  // can reach.
  function stepBackSelection() {
    if (!doc) return;
    // Issue #82 C2 scope item 3: "symbol -> parent symbol -> file -> district
    // -> none" -- one level of NESTING at a time for a card selection,
    // unlike the flat `sym` list just below (the map's own `S`, which has no
    // parent to step through). Checked first (deepest), same priority `sym`
    // already had.
    if (selHSym != null) {
      const parent = selDecoded ? parentGlobalOf(selDecoded, selHSym) : null;
      updateSearch({ hsym: parent ?? undefined });
      return;
    }
    if (selSym != null) {
      updateSearch({ sym: undefined });
      return;
    }
    if (sel != null) {
      const d = D_(doc, sel);
      if (districtClass(doc.districts[String(d)]) === "unconnected") clearAll();
      else updateSearch({ file: undefined, sym: undefined, d, dir: undefined });
      return;
    }
    if (selD != null) {
      clearAll();
      return;
    }
    if (search.dir) {
      updateSearch({ dir: undefined });
      setPanelOpen(false);
      return;
    }
    if (unconnectedRepo) {
      setUnconnectedRepo(null);
      setPanelOpen(false);
      return;
    }
    // Already at "nothing": matches the prototype's own `else return;`.
  }

  function selectDirectory(path?: string) {
    setUnconnectedRepo(null);
    setPreviewDirectory(null);
    setRoute(null);
    setRouteFrom(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined, dir: path });
    setPanelOpen(true);
    setSideOpen(false);
  }

  // ---------- phone: back stack (docs/UX.md §3.4, map/backStack.ts) ----------
  // Opening an overlay pushes one history entry through the ROUTER's own
  // history (so TanStack's `__TSR_index` stays the one index both sides
  // count with); OS back pops them last-opened-first. Selection changes keep
  // replacing the URL (updateSearch), so after a back the URL is
  // reconciled to the selection that is actually live: an entry pushed
  // earlier still carries the URL it was pushed with, and landing on it must
  // not resurrect an older selection (back is not a selection-history
  // scrubber). Only a popped `sel` entry clears the selection.
  const backRef = useRef<BackState | null>(null);
  // True while a back press is being carried past dead entries (skipBack):
  // the entries it passes through carry older URLs, and the page must
  // neither adopt their selection as the live one nor push a new entry for
  // it mid-traversal (a push there would cut the traversal short).
  const skippingRef = useRef(false);
  const liveSearchRef = useRef(search);
  if (!skippingRef.current) liveSearchRef.current = search;
  const mapPathRef = useRef(router.history.location.pathname);
  mapPathRef.current = router.history.location.pathname;
  const historyIndex = () => Number((router.history.location.state as unknown as Record<string, unknown>).__TSR_index ?? 0);
  function pushOverlay(kind: OverlayKind) {
    if (!narrow) return;
    const cur = backRef.current ?? initBack(historyIndex(), null, false);
    const { state, push } = openOverlay(cur, kind, historyIndex());
    backRef.current = state;
    if (push) router.history.push(router.history.location.href, { [OVERLAY_MARKER]: kind } as never);
  }
  function dropOverlay(kind: OverlayKind) {
    if (backRef.current) backRef.current = closeOverlay(backRef.current, kind);
  }
  function changeDetent(d: Detent) {
    if (d === detent) return;
    if (d === "peek") dropOverlay("sheet");
    else pushOverlay("sheet");
    setDetentState(d);
  }
  function openSearch() {
    pushOverlay("search");
    setSearchOpen(true);
  }
  function closeSearch() {
    dropOverlay("search");
    setSearchOpen(false);
  }
  function openLayers() {
    pushOverlay("layers");
    setLayersOpen(true);
  }
  function closeLayers() {
    dropOverlay("layers");
    setLayersOpen(false);
  }
  function clearPath() {
    setPathPick(null);
    setPathEnds(null);
    setRoute(null);
    setRouteFrom(null);
  }
  const undoRef = useRef<(kind: OverlayKind) => void>(() => {});
  undoRef.current = (kind) => {
    if (kind === "search") setSearchOpen(false);
    else if (kind === "layers") setLayersOpen(false);
    else if (kind === "sheet") setDetentState("peek");
    else {
      // The URL half of clearing the selection is the reconcile below.
      setStructure(null);
      clearPath();
    }
  };
  const phoneHasSelection = sel != null || selD != null || !!search.dir || !!structure || !!pathPick;
  const phoneHasSelectionRef = useRef(phoneHasSelection);
  phoneHasSelectionRef.current = phoneHasSelection;
  // Mount, and every repository switch (a new map is a new stack): adopt the
  // entry the page is on -- after a reload, possibly one of ours.
  useEffect(() => {
    if (!narrow) return;
    skippingRef.current = false;
    const st = router.history.location.state as unknown as Record<string, unknown>;
    backRef.current = initBack(historyIndex(), st[OVERLAY_MARKER], phoneHasSelectionRef.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [narrow, owner, repo]);
  // A selection (a deep link included) is one entry: back clears it before
  // it leaves the map.
  useEffect(() => {
    if (!narrow || skippingRef.current) return;
    if (phoneHasSelection) pushOverlay("sel");
    else dropOverlay("sel");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phoneHasSelection, narrow]);
  useEffect(() => {
    if (!narrow) return;
    return router.history.subscribe(({ location, action }) => {
      if (action.type !== "BACK" && action.type !== "FORWARD" && action.type !== "GO") return;
      if (location.pathname !== mapPathRef.current) {
        skippingRef.current = false;
        return;
      }
      const st = location.state as unknown as Record<string, unknown>;
      const idx = Number(st.__TSR_index ?? 0);
      const cur = backRef.current ?? initBack(idx, null, false);
      const res = popTo(cur, idx, st[OVERLAY_MARKER], phoneHasSelectionRef.current);
      backRef.current = res.state;
      for (const kind of res.undo) undoRef.current(kind);
      const live = liveSearchRef.current;
      const desired = res.undo.includes("sel")
        ? { ...live, file: undefined, sym: undefined, hsym: undefined, d: undefined, dir: undefined }
        : live;
      // A second pop can arrive (skipBack) before React re-renders.
      liveSearchRef.current = desired;
      // Skipping on: the entry this press finally lands on reconciles (and
      // if that is the previous page, there is nothing of ours to fix).
      skippingRef.current = res.skipBack;
      if (res.skipBack) router.history.back();
      else navigate({ to: ".", search: desired, replace: true, state: true });
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [narrow]);

  // docs/UX.md §3.2: every selection opens the sheet at Peek, never higher,
  // and replaces whatever card (a road, map quality) was showing.
  const selKey = `${sel}|${selSym}|${selHSym}|${selD}|${search.dir ?? ""}`;
  const prevSelKeyRef = useRef(selKey);
  useEffect(() => {
    if (prevSelKeyRef.current === selKey) return;
    prevSelKeyRef.current = selKey;
    if (!narrow) return;
    changeDetent("peek");
    setQuality(false);
    setStructure(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selKey]);
  // Map quality is a Full-sheet view; the sheet leaving Half/Full ends it.
  useEffect(() => {
    if (detent === "peek") setQuality(false);
  }, [detent]);
  // docs/UX.md §3.3: raising the sheet eases the camera only if the
  // selection would otherwise be covered (panTo is a no-op when it is in
  // the new safe rect). Runs after MapCanvas has applied the new insets
  // (a child's layout effect runs before this parent's effect).
  const prevDetentRef = useRef(detent);
  useEffect(() => {
    const prev = prevDetentRef.current;
    prevDetentRef.current = detent;
    // Only on the way up to Half: at Full the map is covered, and moving it
    // there (into a rect that no longer exists) would only leave it
    // displaced when the sheet comes back down.
    if (!narrow || !doc || detent !== "half" || prev !== "peek") return;
    if (sel != null) {
      if (districtClass(doc.districts[String(D_(doc, sel))]) !== "unconnected") canvasRef.current?.panTo(sel);
    } else if (selD != null) canvasRef.current?.panToDistrict(selD);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detent]);
  // docs/UX.md §4.7: while a path waits for its other end, a file tap picks
  // that file directly.
  useEffect(() => {
    canvasRef.current?.setDirectFileTaps(!!pathPick && !pathEnds);
  }, [pathPick, pathEnds]);
  // Leaving the phone layout (a rotation, a resize) closes phone overlays.
  useEffect(() => {
    if (narrow) return;
    setSearchOpen(false);
    setLayersOpen(false);
    setDetentState("peek");
    setStructure(null);
    setPathPick(null);
    setPathEnds(null);
  }, [narrow]);

  /** §4.7: the second file of a path. */
  function completePath(i: number) {
    if (!doc || !pathPick || i === pathPick.anchor) return;
    const ends: [number, number] = pathPick.dir === "from" ? [pathPick.anchor, i] : [i, pathPick.anchor];
    setPathEnds(ends);
    setRoute(findRoute(doc, adj, ends[0], ends[1]));
  }
  /** docs/UX.md §3.5 on a phone: a tap on empty map clears the selection
   * (one tap, one step -- no zoom), and returns the sheet to Peek. A road
   * card or a path waiting for its end are the step it takes first. */
  function phoneEmptyTap() {
    if (pathPick && !pathEnds) return;
    if (structure) {
      setStructure(null);
      return;
    }
    if (sel != null || selD != null || search.dir || pathPick) {
      clearPath();
      clearAll();
      return;
    }
    changeDetent("peek");
  }

  const rendererCallbacks: MapRendererCallbacks = {
    onSelectFile: (i) => (narrow && pathPick && !pathEnds ? completePath(i) : selectFile(i)),
    onSelectSymbol: (i, s) => selectFile(i, { symbol: s }),
    onSelectHierSymbol: (g) => selectHierSymbol(g),
    onSelectDistrict: (d) => {
      if (narrow && pathPick && !pathEnds) return; // a path's end is a file
      selectDistrict(d);
    },
    // docs/UX.md §3.5: on a phone an empty tap clears; desktop keeps issue
    // #82 A1's one-level step back until its own phase (5).
    onClearSelection: () => (narrow ? phoneEmptyTap() : stepBackSelection()),
    // §7.1 rule 7: a road card never stays put while the map moves.
    onDragStart: () => {
      if (narrow) setStructure(null);
    },
    onPanDismiss: () => {
      if (narrow && detent !== "peek") changeDetent("peek");
    },
    onTapStructure: (key, lines) => {
      if (!narrow || !doc) return false;
      const detail = structureDetail(doc, key);
      if (!detail) return false;
      setStructure({ key, lines, detail });
      changeDetent("peek");
      return true;
    },
    onSelectDirectory: (path) => selectDirectory(path),
    onPreviewDirectory: (path) => setPreviewDirectory(path && doc ? { repo: doc.repo, path } : null),
    onNeedSymbols: (d) => onNeedSymbols(d),
  };

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center bg-[var(--chrome)] text-small text-[var(--dim)]">
        <LoadProgressIndicator progressKey={`map:${owner}/${repo}`} label={`${owner}/${repo}`} />
      </div>
    );
  }
  if (isError || !doc || !packageLayout) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 bg-[var(--chrome)] p-6 text-center text-small text-[var(--link-out)]">
        <p>couldn't load {owner}/{repo}.</p>
        <p className="text-[var(--dim)]">{error instanceof Error ? error.message : "not in the catalogue"}</p>
      </div>
    );
  }

  const packageDepth = Math.max(
    packageLayout.minDepth,
    Math.min(packageLayout.maxDepth, search.depth ?? packageLayout.autoDepth),
  );
  const packageGrouping = packageLayout.groupings.get(packageDepth)!;
  const activeDirectory = search.dir && packageLayout.filesByDirectory.has(search.dir) ? search.dir : undefined;
  const previewPath = previewDirectory?.repo === doc.repo ? previewDirectory.path : undefined;
  const highlightedPath = previewPath ?? activeDirectory;
  const folderFiles = highlightedPath ? (packageLayout.filesByDirectory.get(highlightedPath) ?? null) : null;
  const folderOnlyIslands = highlightedPath ? packageLayout.islandOnlyDirectories.has(highlightedPath) : false;

  const mapCanvas = (
    <MapCanvas
      doc={doc}
      geo={search.geo}
      layer={search.layer}
      sel={sel}
      selSym={selSym}
      selD={selD}
      route={route}
      packageGrouping={packageGrouping}
      folderFiles={folderFiles}
      folderOnlyIslands={folderOnlyIslands}
      folderLabels={packageLayout.folderLabels}
      activeDirectory={activeDirectory}
      districtSymbols={districtSymbolsMap}
      selHSym={selHSym}
      insets={frameInsets}
      callbacks={rendererCallbacks}
      handleRef={canvasRef}
    />
  );

  if (narrow) {
    // docs/UX.md §3: the map is full screen; everything else floats over it
    // or lives in the one sheet. No fullscreen mode on phones (§3).
    return (
      <div className="relative h-full overflow-hidden bg-[var(--canvas)]" data-phone-shell>
        <div ref={mapAreaRef} className="absolute inset-0">
          {mapCanvas}
        </div>
        <DetailStatusNote pending={early.detailPending} failed={early.detailFailed} narrow />
        <PhoneChrome
          doc={doc}
          packageLayout={packageLayout}
          packageGrouping={packageGrouping}
          catalogue={catalogue}
          owner={owner}
          repo={repo}
          layer={search.layer}
          depthAuto={search.depth == null}
          maxCh={maxCh}
          maxCx={maxCx}
          sel={sel}
          selSym={selSym}
          selD={selD}
          selHSym={selHSym}
          symbolsDoc={selSymbolsDoc}
          symbolsLoading={selSymbolsLoading}
          adj={adj}
          radj={radj}
          activeDirectory={activeDirectory}
          heights={heights}
          viewportHeight={metrics.height}
          safeTop={metrics.safeTop}
          detent={detent}
          onDetent={changeDetent}
          searchOpen={searchOpen}
          onOpenSearch={openSearch}
          onCloseSearch={closeSearch}
          onSearchPick={(hit: SearchHit) => {
            closeSearch();
            clearPath();
            selectFile(hit.i, hit.s != null ? { symbol: hit.s } : {});
          }}
          layersOpen={layersOpen}
          onOpenLayers={openLayers}
          onCloseLayers={closeLayers}
          quality={quality}
          onQuality={(open) => {
            setQuality(open);
            changeDetent(open ? "full" : "peek");
          }}
          indexTab={activeDirectory ? "folders" : indexTab}
          onIndexTab={(t) => {
            setIndexTab(t);
            if (detent === "peek") changeDetent("half");
          }}
          structure={structure}
          onCloseStructure={() => setStructure(null)}
          pathPick={pathPick}
          pathEnds={pathEnds}
          route={route}
          onPath={(i, dir) => {
            setRoute(null);
            setPathEnds(null);
            setPathPick({ anchor: i, dir });
            changeDetent("peek");
          }}
          onPathCancel={clearPath}
          onZoomIn={() => canvasRef.current?.zoomBy(1.6)}
          onZoomOut={() => canvasRef.current?.zoomBy(1 / 1.6)}
          onFit={() => canvasRef.current?.fit(true)}
          onLayer={(l) => updateSearch({ layer: l })}
          onDepth={(depth) => updateSearch({ depth: depth === packageLayout.autoDepth ? undefined : depth })}
          onClearSelection={() => {
            clearPath();
            clearAll();
          }}
          onSelectFile={(i) => selectFile(i)}
          onSelectSymbol={(i, s) => selectFile(i, { symbol: s })}
          onSelectHierSymbol={selectHierSymbol}
          onSelectDistrict={selectDistrict}
          onPickDistrict={(d) => {
            selectDistrict(d);
            framePeekNow();
            canvasRef.current?.panToDistrict(d);
          }}
          onPickKeyFile={(i) => selectFile(i)}
          onZoomDistrict={(d) => canvasRef.current?.zoomDistrict(d)}
          onSelectDirectory={selectDirectory}
          onBreadcrumbRepo={clearAll}
          onBreadcrumbFile={(i) => updateSearch({ file: doc.F[i], sym: undefined, hsym: undefined, d: undefined, dir: undefined })}
        />
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {!isFullscreen && (
        <TopBar
          catalogue={catalogue}
          owner={owner}
          repo={repo}
          layer={search.layer}
          onLayer={(l) => updateSearch({ layer: l })}
        />
      )}
      <div className="relative flex min-h-0 flex-1">
        {!isFullscreen && (
          <Sidebar
            doc={doc}
            packageLayout={packageLayout}
            onPickKeyFile={(i) => {
              // Issue #82 "district index": a district row's key-file line
              // (most imported / entry / links a bridge) selects the file
              // with no view move -- the same pan-free contract the old
              // onPickLandmark/onPickHub handlers had (selectFile() itself
              // is pan-only for every caller, per A1).
              setSideOpen(false);
              selectFile(i);
            }}
            onSelectDistrict={(d) => {
              setSideOpen(false);
              // Issue #82 A1: a sidebar row now SELECTS the district (both
              // mainland and island rows alike -- the old mainland-only
              // "fly, don't select" affordance from issue #63 is gone, since
              // this issue's own spec explicitly lists "the sidebar
              // (district rows...)" as a place selecting must never zoom)
              // and pans to it if it's off screen, instead of always
              // zooming in. selectDistrict() itself stays pan-free (it's
              // shared with map taps and the breadcrumb, which must never
              // move the view at all); this wrapper is the one place that
              // adds the pan, because a sidebar target genuinely can be off
              // screen.
              selectDistrict(d);
              canvasRef.current?.panToDistrict(d);
            }}
          />
        )}
        <div
          ref={mapAreaRef}
          className={`relative min-w-0 flex-1 bg-[var(--canvas)]${isFullscreen ? " fixed inset-0 z-50" : ""}`}
        >
          {mapCanvas}
          <DetailStatusNote pending={early.detailPending} failed={early.detailFailed} narrow={false} />
          <SearchBox
            doc={doc}
            onPick={(hit: SearchHit) => selectFile(hit.i, hit.s != null ? { symbol: hit.s } : {})}
          />
          {isFullscreen ? (
            <SelectionSummaryBar
              doc={doc}
              sel={sel}
              selSym={selSym}
              selD={selD}
              selHSym={selHSym}
              symbolsDoc={selSymbolsDoc}
              adj={adj}
              radj={radj}
              onDetails={() => {
                void exitFullscreen();
                setPanelOpen(true);
              }}
            />
          ) : (
            <SelectionPanel
              doc={doc}
              sel={sel}
              selSym={selSym}
              selD={selD}
              selHSym={selHSym}
              symbolsDoc={selSymbolsDoc}
              symbolsLoading={selSymbolsLoading}
              adj={adj}
              radj={radj}
              packageLayout={packageLayout}
              activeDirectory={activeDirectory}
              showUnconnected={unconnectedRepo === doc.repo && sel == null && selD == null}
              open={panelOpen}
              onToggleOpen={() => setPanelOpen((v) => !v)}
              onSelectFile={(i) => selectFile(i)}
              onSelectSymbol={(i, s) => selectFile(i, { symbol: s })}
              onSelectHierSymbol={selectHierSymbol}
              onHoverHierSymbol={(g) => canvasRef.current?.hoverSymbol(g)}
              onSelectDistrict={selectDistrict}
              onZoomDistrict={(d) => canvasRef.current?.zoomDistrict(d)}
              onRouteFrom={(i) => setRouteFrom(i)}
              onRouteTo={(i) => {
                if (routeFrom == null) {
                  setRouteFrom(i);
                  return;
                }
                setRoute(findRoute(doc, adj, routeFrom, i));
              }}
              onSelectDirectory={selectDirectory}
              onBreadcrumbRepo={clearAll}
              onBreadcrumbFile={(i) => updateSearch({ file: doc.F[i], sym: undefined, hsym: undefined, d: undefined, dir: undefined })}
            />
          )}
          {!isFullscreen && search.layer === "p" && (
            <PackageLegend
              grouping={packageGrouping}
              auto={search.depth == null}
              minDepth={packageLayout.minDepth}
              maxDepth={packageLayout.maxDepth}
              onDepth={(depth) => updateSearch({ depth: depth === packageLayout.autoDepth ? undefined : depth })}
            />
          )}
          {!isFullscreen && (
            <FooterStats doc={doc} layer={search.layer} maxCh={maxCh} maxCx={maxCx}
              unconnectedCount={packageLayout.unconnectedFiles.length}
              mobileHidden={panelOpen}
              onOpenUnconnected={() => { updateSearch({ file: undefined, sym: undefined, d: undefined, dir: undefined }); setUnconnectedRepo(doc.repo); setPanelOpen(true); setSideOpen(false); }} />
          )}
          <RouteBox
            doc={doc}
            routeFrom={routeFrom}
            route={route}
            onClear={() => {
              setRoute(null);
              setRouteFrom(null);
            }}
          />
          <ZoomControls
            onZoomIn={() => canvasRef.current?.zoomBy(1.6)}
            onZoomOut={() => canvasRef.current?.zoomBy(1 / 1.6)}
            onFit={() => canvasRef.current?.fit(true)}
            isFullscreen={isFullscreen}
            onToggleFullscreen={toggleFullscreen}
          />
        </div>
      </div>
    </div>
  );
}
