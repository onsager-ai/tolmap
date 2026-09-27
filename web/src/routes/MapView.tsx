import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useRouter, useSearch } from "@tanstack/react-router";
import { useCatalogue, useMapDocument, useDistrictSymbolsMap } from "@/data/queries";
import { MapCanvas, type MapCanvasHandle } from "@/map/MapCanvas";
import type { FrameInsets, MapRendererCallbacks } from "@/map/MapRenderer";
import { buildAdj, findRoute, type Route } from "@/map/graph";
import { D_, districtClass, type Insets } from "@/map/geometry";
import type { SearchPick } from "@/map/searchResults";
import { TopBar } from "@/components/TopBar";
import { DistrictRail } from "@/components/Sidebar";
import { SearchBox } from "@/components/SearchBox";
import { ZoomControls } from "@/components/ZoomControls";
import { PackageLegend } from "@/components/PackageLegend";
import { LoadProgressIndicator } from "@/components/LoadProgressIndicator";
import { BottomLeftStack, Inspector, QualityStrip, RampLegend } from "@/components/DesktopChrome";
import { buildPackageLayout } from "@/map/packageLayout";
import { useEffectiveTheme } from "@/lib/theme";
import type { MapSearch } from "@/routes/search";
import { useLayoutProfile } from "@/hooks/useLayoutProfile";
import { usePhoneMetrics } from "@/hooks/usePhoneMetrics";
import { detentHeights, safeInsets, type Detent } from "@/map/phoneShell";
import {
  DESKTOP_GUTTER_PX,
  FULLSCREEN_SEARCH_HEIGHT_PX,
  INSPECTOR_INSET_PX,
  desktopControlSize,
  desktopLeftEdge,
  desktopSafeInsets,
  inspectorWidth,
  isPhoneShell,
  isTouchProfile,
  landscapeSafeInsets,
  type SafeArea,
} from "@/map/layoutProfile";
import { closeOverlay, initBack, openOverlay, OVERLAY_MARKER, popTo, type BackState, type OverlayKind } from "@/map/backStack";
import { structureDetail } from "@/map/structureCard";
import { PhoneChrome } from "@/components/phone/PhoneChrome";
import { SelectionCard, type PathPick, type StructureCardState } from "@/components/phone/SheetCards";

/** The /:owner/:repo page. The layout follows docs/UX.md §9's profiles
 * (map/layoutProfile.ts): the phone shell (components/phone/PhoneChrome.tsx:
 * §3's pill, control column and one sheet -- a bottom sheet in portrait, a
 * side sheet in landscape), or the desktop layout (§5: top bar, district
 * rail, the inspector, the map-quality strip and the controls around one
 * MapCanvas), which a tablet shares with a collapsible rail and 44 px
 * targets. The phone sheet and the inspector host the same cards
 * (SheetCards.tsx's SelectionCard, principle 10).
 * This component owns every piece of application state — selection, geo,
 * layer, route — either directly or via the URL; MapCanvas/MapRenderer only
 * ever receive it as props and report gestures back through callbacks. */
export function MapView() {
  const { owner, repo } = useParams({ strict: false }) as { owner: string; repo: string };
  const search = useSearch({ strict: false }) as MapSearch;
  const navigate = useNavigate();
  const { data: catalogue } = useCatalogue();
  const { data: doc, isLoading, isError, error, source } = useMapDocument(owner, repo);

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
  const { map: districtSymbolsMap, loading: symbolsLoadingDistricts } = useDistrictSymbolsMap(owner, repo, source, [
    ...wantedDistricts,
  ]);

  const canvasRef = useRef<MapCanvasHandle>(null);
  const mapAreaRef = useRef<HTMLDivElement>(null);
  const [route, setRoute] = useState<Route | null>(null);
  const [previewDirectory, setPreviewDirectory] = useState<{ repo: string; path: string } | null>(null);

  // ---------- the layout profile (docs/UX.md §9) ----------
  const profile = useLayoutProfile();
  // `narrow` is the phone shell, in either orientation.
  const narrow = isPhoneShell(profile);
  const landscape = profile === "landscape";
  const touch = isTouchProfile(profile);
  // §9: the landscape side sheet is open (360) or collapsed (0).
  const [sideOpen, setSideOpen] = useState(true);
  // §9: a tablet's rail is collapsible from the top bar. It starts open
  // where the map keeps room beside it and the inspector (a landscape
  // tablet), collapsed on a portrait one.
  const [railOpen, setRailOpen] = useState(() => typeof window === "undefined" || window.innerWidth >= 900);
  // §5: the inspector shows the phone sheet's Half content; "All N" symbols
  // asks for Full, as it does on the phone.
  const [inspectorDetent, setInspectorDetent] = useState<Detent>("half");

  // ---------- the phone shell (docs/UX.md §3) ----------
  const router = useRouter();
  const metrics = usePhoneMetrics();
  const safeArea: SafeArea = { top: metrics.safeTop, right: metrics.safeRight ?? 0, bottom: metrics.safeBottom, left: metrics.safeLeft ?? 0 };
  const heights = detentHeights(metrics);
  const [detent, setDetentState] = useState<Detent>("peek");
  const [searchOpen, setSearchOpen] = useState(false);
  const [layersOpen, setLayersOpen] = useState(false);
  const [quality, setQuality] = useState(false);
  const [indexTab, setIndexTab] = useState<"districts" | "folders">("districts");
  const [structure, setStructure] = useState<StructureCardState | null>(null);
  const [pathPick, setPathPick] = useState<PathPick | null>(null);
  const [pathEnds, setPathEnds] = useState<[number, number] | null>(null);
  // Issue #82 A1 scope item 5 (fullscreen), desktop and tablet only (§3: no
  // fullscreen on phones). Declared here because the frame below depends on
  // it; the rest is further down.
  const [isFullscreen, setIsFullscreen] = useState(false);
  // Something for the card to show: the phone sheet's card instead of its
  // overview, and on desktop and tablet the open inspector (§5).
  const cardShown = (sel: number | null, selD: number | null) =>
    !!pathPick || !!structure || quality || sel != null || selD != null;

  // docs/UX.md §3.3 and §5: the safe rectangle from the real chrome. `frame`
  // is the resting layout, which the level-of-detail scale is measured
  // against, so opening a sheet or the inspector never changes what the map
  // draws; `safe` follows the chrome as it is now.
  //  - Phone portrait: `frame` is pill + Peek; `safe` follows the sheet.
  //  - Phone landscape (§9): the map right of the side sheet; `frame` with
  //    the sheet open, `safe` as it is (collapsed widens it).
  //  - Desktop and tablet (§5): the map box minus the quality strip and the
  //    controls, and minus the inspector when it is open (`safe` only).
  const peekInsets = safeInsets(metrics, heights.peek);
  const desktopInsets = (inspector: boolean): Insets =>
    desktopSafeInsets({ profile, inspector, rail: profile === "desktop" || railOpen, fullscreen: isFullscreen, safe: safeArea });
  /** The insets a selection lands in: the sheet at Peek, the side sheet
   * open, the inspector open. */
  const selectionInsets = (): FrameInsets =>
    !narrow
      ? { frame: desktopInsets(false), safe: desktopInsets(true), centreInSafe: true }
      : landscape
        ? { frame: landscapeSafeInsets(true, safeArea), safe: landscapeSafeInsets(true, safeArea), centreInSafe: true }
        : { frame: peekInsets, safe: peekInsets, centreInSafe: true };
  /** Before a camera move made together with a selection (which lands the
   * sheet at Peek, §3.2, opens the side sheet, or opens the inspector): the
   * renderer frames into that rect now, not after React commits. */
  function framePeekNow() {
    canvasRef.current?.setInsets(selectionInsets());
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
  // AREA (below), not the whole page: TopBar and the rail are its siblings,
  // so the Fullscreen API path hides them simply by not being part of what
  // the browser paints while fullscreen is active -- no separate
  // show/hide logic needed for that path. The CSS fallback (no element
  // Fullscreen API -- iPad Safari, an embedding that forbids it) instead
  // covers them with `fixed inset-0`, the same technique the prototype's own
  // `.mapbox.fs` used (arch20 -- see its setFS/reAspect); either way
  // `isFullscreen` is the one source of truth the JSX below reads, so React
  // chrome and the CSS class agree. docs/UX.md §9: the fallback covers the
  // whole screen, notch included, and the top bar that carried the top
  // safe-area inset is gone, so everything it floats (search, the
  // inspector, the controls, the strip) clears the insets itself.

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
  const hasCard = cardShown(sel, selD);
  // At Full the phone sheet covers the map up to the pill, leaving no rect
  // to frame into; a camera move made then (Zoom to district from the card)
  // frames as at Half, so it lands where the map shows again once the sheet
  // comes down.
  const frameInsets: FrameInsets = !narrow
    ? { frame: desktopInsets(false), safe: desktopInsets(hasCard), centreInSafe: true }
    : landscape
      ? { frame: landscapeSafeInsets(true, safeArea), safe: landscapeSafeInsets(sideOpen, safeArea), centreInSafe: true }
      : { frame: peekInsets, safe: safeInsets(metrics, heights[detent === "full" ? "half" : detent]), centreInSafe: true };

  // The selected FILE's district symbols, decoded once per fetch -- what
  // SelectionPanel's outline tree/external references and the breadcrumb's
  // class/method chain both read. `undefined` (not yet fetched, or this map
  // has no symbols sibling at all) degrades silently: every reader below
  // just sees `null` and renders what it would have before this feature.
  const selFileDistrict = doc && sel != null ? D_(doc, sel) : null;
  const selSymbolsDoc = selFileDistrict != null ? districtSymbolsMap.get(selFileDistrict) : undefined;

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
    // Scope item 3: "setting all levels" -- file, symbol, and dropping
    // whatever else (a bare district, a directory highlight, the old `sym`)
    // was selected, in one URL update.
    updateSearch({ file: doc.F[fileIdx], sym: undefined, hsym: global, d: undefined, dir: undefined });
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
    updateSearch({ file: doc.F[i], sym: opts.symbol, hsym: undefined, d: undefined, dir: undefined });
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
    updateSearch({ file: undefined, sym: undefined, d, dir: undefined });
  }
  // Jumps straight to "nothing selected" -- the breadcrumb's own repo
  // segment, a card's close button, Esc, and an empty-map tap (§3.5).
  function clearAll() {
    updateSearch({ file: undefined, sym: undefined, hsym: undefined, d: undefined, dir: undefined });
  }
  // Issue #82 A1 scope item 2 made an empty map tap step back one level at
  // a time (symbol -> its file -> the file's district -> nothing), and
  // docs/UX.md phase 2 kept that on desktop while the phone cleared in one
  // tap (its departure 6). §7.1 rule 5 ("Tap on empty map clears the
  // selection (§3.5)") is an interaction rule for both profiles, so phase 5
  // applies it everywhere: one tap, one step -- the selection clears; the
  // camera stays. Stepping up a level is the breadcrumb's job (each segment
  // selects its level without moving the view).

  /** docs/UX.md §4.8: a search result selects its district, file or
   * symbol and brings it into view (framed above the sheet at Peek on a
   * phone); a district pans like a District index row, a file or symbol
   * like any other selection (pan only if off screen, issue #82 A1). */
  function pickSearchResult(pick: SearchPick) {
    if (pick.kind === "district") {
      selectDistrict(pick.d);
      framePeekNow();
      canvasRef.current?.panToDistrict(pick.d);
    } else {
      selectFile(pick.i, pick.kind === "symbol" ? { symbol: pick.s } : {});
    }
  }

  function selectDirectory(path?: string) {
    setPreviewDirectory(null);
    setRoute(null);
    updateSearch({ file: undefined, sym: undefined, d: undefined, dir: path });
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
  // and replaces whatever card (a road, map quality) was showing. In
  // landscape it also opens a collapsed side sheet (§9); the inspector
  // shows the new card at Half (§5).
  const selKey = `${sel}|${selSym}|${selHSym}|${selD}|${search.dir ?? ""}`;
  const prevSelKeyRef = useRef(selKey);
  useEffect(() => {
    if (prevSelKeyRef.current === selKey) return;
    prevSelKeyRef.current = selKey;
    setQuality(false);
    setStructure(null);
    setInspectorDetent("half");
    if (!narrow) return;
    changeDetent("peek");
    if (sel != null || selD != null) setSideOpen(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selKey]);
  // Map quality is a Full-sheet view on a phone; the sheet leaving
  // Half/Full ends it. (The desktop inspector has no detent to leave.)
  useEffect(() => {
    if (narrow && detent === "peek") setQuality(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
    // Landscape: the side sheet's box does not change with its detent (§9).
    if (!narrow || landscape || !doc || detent !== "half" || prev !== "peek") return;
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
  // Leaving the phone layout (a resize) closes phone overlays. (A phone
  // turned sideways stays in the phone shell, §9, and keeps them.)
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
  /** docs/UX.md §3.5 and §7.1 rule 5, every profile: a tap on empty map
   * clears the selection (one tap, one step -- no zoom), and returns the
   * phone's sheet to Peek or closes the desktop inspector. A road card, the
   * map-quality card or a path waiting for its end are the step it takes
   * first. */
  function emptyTap() {
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
    if (!narrow) setQuality(false);
    else changeDetent("peek");
  }
  /** §5: Esc closes the inspector -- whatever card it shows goes, and the
   * selection with it (as its close button does). */
  function closeInspector() {
    setStructure(null);
    setQuality(false);
    clearPath();
    if (sel != null || selD != null) clearAll();
  }
  // Esc on desktop and tablet: closes the inspector (§5), else leaves the
  // CSS fullscreen fallback. The native fullscreen path exits on Esc in the
  // browser itself and fires fullscreenchange (handled above);
  // exitFullscreen()'s own guard makes calling it redundantly harmless. A
  // key already handled (search's own Esc closes its dropdown) or typed
  // into a field is not ours.
  const escRef = useRef<() => boolean>(() => false);
  escRef.current = () => {
    if (hasCard) {
      closeInspector();
      return true;
    }
    if (isFullscreen) {
      void exitFullscreen();
      return true;
    }
    return false;
  };
  useEffect(() => {
    if (narrow) return;
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || t.closest("input, textarea, select"))) return;
      if (escRef.current()) e.preventDefault();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [narrow]);

  const rendererCallbacks: MapRendererCallbacks = {
    onSelectFile: (i) => (pathPick && !pathEnds ? completePath(i) : selectFile(i)),
    onSelectSymbol: (i, s) => selectFile(i, { symbol: s }),
    onSelectHierSymbol: (g) => selectHierSymbol(g),
    onSelectDistrict: (d) => {
      if (pathPick && !pathEnds) return; // a path's end is a file
      selectDistrict(d);
    },
    // docs/UX.md §3.5 / §7.1 rule 5: an empty tap clears, on every profile.
    onClearSelection: () => emptyTap(),
    // §7.1 rule 7: a road card never stays put while the map moves.
    onDragStart: () => {
      if (narrow) setStructure(null);
    },
    onPanDismiss: () => {
      if (narrow && detent !== "peek") changeDetent("peek");
    },
    // Road, street and neighborhood cards are sheet content on a phone
    // (§3.2); desktop and tablet keep the renderer's own hover card, which
    // a tap pins (hover stays as it is, §5).
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
      <div className="relative h-full overflow-hidden bg-[var(--canvas)]" data-phone-shell data-profile={profile} data-touch="">
        <div ref={mapAreaRef} className="absolute inset-0">
          {mapCanvas}
        </div>
        <PhoneChrome
          doc={doc}
          packageLayout={packageLayout}
          packageGrouping={packageGrouping}
          landscape={landscape}
          sideOpen={sideOpen}
          onToggleSide={() => setSideOpen((v) => !v)}
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
          onSearchPick={(pick: SearchPick) => {
            closeSearch();
            clearPath();
            // Picking what is already selected still lands the sheet at
            // Peek (§4.8), which the selection-change effect alone would not.
            changeDetent("peek");
            pickSearchResult(pick);
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

  // ---------- desktop and tablet (docs/UX.md §5, §9) ----------
  // One tree for both, so a window crossing 1100 px keeps its MapCanvas (and
  // its view): only the rail's toggle, the inspector's width and the
  // targets' size differ.
  const tablet = profile === "tablet";
  const railShown = !isFullscreen && (!tablet || railOpen);
  const controlSize = desktopControlSize(profile);
  const cardProps = {
    doc,
    packageLayout,
    sel,
    selSym,
    selD,
    selHSym,
    symbolsDoc: selSymbolsDoc,
    symbolsLoading: selSymbolsLoading,
    adj,
    radj,
    detent: inspectorDetent,
    onDetent: (d: Detent) => setInspectorDetent(d === "full" ? "full" : "half"),
    quality,
    onCloseQuality: () => setQuality(false),
    structure,
    onCloseStructure: () => setStructure(null),
    pathPick,
    pathEnds,
    route,
    onPath: (i: number, dir: "from" | "to") => {
      setRoute(null);
      setPathEnds(null);
      setPathPick({ anchor: i, dir });
    },
    onPathCancel: clearPath,
    onClearSelection: () => {
      clearPath();
      clearAll();
    },
    onSelectFile: (i: number) => selectFile(i),
    onSelectSymbol: (i: number, sy: number) => selectFile(i, { symbol: sy }),
    onSelectHierSymbol: selectHierSymbol,
    onHoverHierSymbol: (g: number | null) => canvasRef.current?.hoverSymbol(g),
    onSelectDistrict: selectDistrict,
    onZoomDistrict: (d: number) => canvasRef.current?.zoomDistrict(d),
    onSelectDirectory: selectDirectory,
    onBreadcrumbRepo: clearAll,
    onBreadcrumbFile: (i: number) => updateSearch({ file: doc.F[i], sym: undefined, hsym: undefined, d: undefined, dir: undefined }),
  };
  const searchBox = <SearchBox doc={doc} onPick={pickSearchResult} touch={touch} />;

  return (
    <div className="flex h-full flex-col" data-desktop-shell data-profile={profile} data-touch={touch ? "" : undefined}>
      {!isFullscreen && (
        <TopBar
          catalogue={catalogue}
          doc={doc}
          owner={owner}
          repo={repo}
          layer={search.layer}
          onLayer={(l) => updateSearch({ layer: l })}
          search={searchBox}
          rail={tablet ? { open: railOpen, onToggle: () => setRailOpen((v) => !v) } : undefined}
          touch={touch}
        />
      )}
      <div className="relative flex min-h-0 flex-1">
        {railShown && (
          <DistrictRail
            doc={doc}
            packageLayout={packageLayout}
            tab={activeDirectory ? "folders" : indexTab}
            onTab={setIndexTab}
            selected={selD ?? (sel != null ? D_(doc, sel) : null)}
            activeDirectory={activeDirectory}
            // Issue #82 "district index": a district row's key-file line
            // (most imported / entry / links a bridge) selects the file,
            // panning only to bring it out from under the inspector or in
            // from off screen (selectFile() is pan-only for every caller,
            // per A1).
            onPickKeyFile={(i) => selectFile(i)}
            onSelectDistrict={(d) => {
              // Issue #82 A1: a rail row SELECTS the district (mainland and
              // island rows alike) and pans to it if it's off screen -- or,
              // §5, under the inspector it opens -- never zooming in.
              // selectDistrict() itself stays pan-free (it's shared with map
              // taps and the breadcrumb, which must never move the view);
              // this is the one place that adds the pan, because a rail
              // target genuinely can be off screen.
              selectDistrict(d);
              framePeekNow();
              canvasRef.current?.panToDistrict(d);
            }}
            onSelectDirectory={selectDirectory}
          />
        )}
        <div
          ref={mapAreaRef}
          data-map-area
          className={`relative min-w-0 flex-1 overflow-hidden bg-[var(--canvas)]${isFullscreen ? " fixed inset-0 z-50" : ""}`}
        >
          {mapCanvas}
          {/* Fullscreen: no top bar, so search floats in the map's top-left
              corner, below the top safe inset (§9: the fallback covers the
              notch, and search must not sit under it). */}
          {isFullscreen && (
            <div
              data-fullscreen-search
              className="absolute z-40"
              style={{
                top: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-top, 0px))`,
                left: desktopLeftEdge(true),
                width: `min(460px, calc(100% - ${2 * DESKTOP_GUTTER_PX}px - ${hasCard ? inspectorWidth(profile) + INSPECTOR_INSET_PX : 0}px - env(safe-area-inset-left, 0px) - env(safe-area-inset-right, 0px)))`,
                height: FULLSCREEN_SEARCH_HEIGHT_PX,
              }}
            >
              {searchBox}
            </div>
          )}
          {hasCard && (
            <Inspector profile={profile} fullscreen={isFullscreen}>
              <SelectionCard {...cardProps} />
            </Inspector>
          )}
          <BottomLeftStack atScreenEdge={isFullscreen || !railShown}>
            {search.layer === "p" && (
              <PackageLegend
                grouping={packageGrouping}
                auto={search.depth == null}
                minDepth={packageLayout.minDepth}
                maxDepth={packageLayout.maxDepth}
                onDepth={(depth) => updateSearch({ depth: depth === packageLayout.autoDepth ? undefined : depth })}
              />
            )}
            <RampLegend layer={search.layer} maxCh={maxCh} maxCx={maxCx} />
            <QualityStrip doc={doc} unconnected={packageLayout.unconnectedFiles.length} onOpen={() => setQuality(true)} touch={touch} />
          </BottomLeftStack>
          <ZoomControls
            onZoomIn={() => canvasRef.current?.zoomBy(1.6)}
            onZoomOut={() => canvasRef.current?.zoomBy(1 / 1.6)}
            onFit={() => canvasRef.current?.fit(true)}
            isFullscreen={isFullscreen}
            onToggleFullscreen={toggleFullscreen}
            size={controlSize}
            style={{
              right: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-right, 0px))`,
              bottom: `calc(${DESKTOP_GUTTER_PX}px + env(safe-area-inset-bottom, 0px))`,
            }}
          />
        </div>
      </div>
    </div>
  );
}
