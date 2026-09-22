// Imperative SVG renderer, ported from the <script> block of
// viewer/template.html. This is the ONE piece of the app that is not React:
// see docs/ARCHITECTURE.md, "Rendering: do not put nodes in the React tree".
// State flows in through render(state); interaction (tap on a district, a
// file, a symbol, or empty space) flows out through the callbacks passed to
// the constructor. React owns selection/geo/layer state and the URL; this
// class only draws and reports gestures.
import type { LandmarkRow, MapDocument } from "@/types";
import { BUILD_ZOOM, DOT_DENSITY_FLOOR, KCOL, KIND, PARCEL_ZOOM, type Geo, type Layer } from "./constants";
import {
  CH,
  CX_,
  D_,
  FI,
  LOC,
  RECT,
  districtClass,
  districtColor,
  districtWorldArea,
  fitScale as fitScaleOf,
  fullFitScale as fullFitScaleOf,
  mainlandBounds,
  px as pxOf,
  ramp,
  rooms,
  stripRows,
  symbolsOf,
  tmCentre,
} from "./geometry";
import { computeBlast, type Route } from "./graph";

declare global {
  interface Window {
    /** Issue #51: set by web/scripts/perf-bench.mjs via page.addInitScript,
     * before the app boots. Gates the performance.mark/measure pair in
     * draw() below -- see its comment for why that can't be unconditional. */
    __TOLMAP_PERF__?: boolean;
  }
}

export type TerrainSelection = {
  kind: "subdistrict" | "parcel";
  district: number;
  index: number;
};

const NS = "http://www.w3.org/2000/svg";
function el<K extends keyof SVGElementTagNameMap>(
  name: K,
  attrs: Record<string, string | number> = {},
): SVGElementTagNameMap[K] {
  const e = document.createElementNS(NS, name) as SVGElementTagNameMap[K];
  for (const k in attrs) e.setAttribute(k, String(attrs[k]));
  return e;
}

export interface MapRenderState {
  doc: MapDocument;
  geo: Geo;
  layer: Layer;
  sel: number | null;
  selSym: number | null;
  selD: number | null;
  selTerrain: TerrainSelection | null;
  route: Route | null;
}

export interface MapRendererCallbacks {
  onSelectDistrict(d: number): void;
  onSelectFile(i: number): void;
  onSelectSymbol(i: number, s: number): void;
  onSelectSubdistrict(d: number, index: number): void;
  onSelectParcel(d: number, index: number): void;
  onClearSelection(): void;
  /** Fired once a drag has actually moved the map, so React can dismiss
   * transient chrome (the mobile drawer, a suggestion list) the way a real
   * map app does. */
  onDragStart?(): void;
}

export class MapRenderer {
  private svg: SVGSVGElement;
  private callbacks: MapRendererCallbacks;
  private state: MapRenderState | null = null;

  private maxLoc = 1;
  private maxCh = 1;
  private maxCx = 1;

  // Issue #48: per-district file-dot prominence order, and each district's
  // world-space area, precomputed once per document in loadDocument() --
  // not per frame, since a pinch-zoom redraws dozens of times a second and
  // sorting every district's members each time would defeat the point of
  // thinning dots for performance. draw() only ever does an O(1) map lookup
  // (fileRank, districtArea) plus a multiply by k² per file; see dotFactor.
  private districtOrder = new Map<number, number[]>();
  private districtArea = new Map<number, number>();
  private fileRank = new Map<number, number>();

  private k = 1;
  private tx = 0;
  private ty = 0;
  private VW = 1000;
  private VH = 700;

  private dragging = false;
  private lx = 0;
  private ly = 0;
  private moved = 0;
  private pts = new Map<number, [number, number]>();
  private pinch: { d: number; k: number; m: [number, number] } | null = null;
  private lastTap = 0;
  private tapped = false;
  private animId: number | null = null;

  // Issue #51: transform-during-gesture state -- what preview()/
  // driftExceeded()/gestureFrame() further down actually read and write;
  // see those methods for why. rootG/drawnK/drawnTx/drawnTy are the <g>
  // paint() last drew and the (k, tx, ty) it drew it at. previewRaf/
  // settleTimer are a gesture's outstanding work; lastDriftRedraw
  // rate-limits the mid-gesture forced paint().
  private rootG: SVGGElement | null = null;
  private drawnK = 1;
  private drawnTx = 0;
  private drawnTy = 0;
  private previewRaf: number | null = null;
  private settleTimer: ReturnType<typeof setTimeout> | null = null;
  private lastDriftRedraw = 0;
  // Tuned against web/scripts/perf-bench.mjs on the corpus maps (numbers in
  // the PR description). SETTLE_MS=140: short enough that a pause mid-drag
  // reads as "the map is live," not "the map is stuck," long enough that a
  // 40-step scripted drag (one pointermove roughly every frame) never fires
  // it between two moves -- and see pointerDown/endPointer for why it also
  // has to be short enough that an unrelated LATER tap's own click always
  // beats a timer re-armed from it.
  //
  // DRIFT_PAN_FRACTION/DRIFT_ZOOM_LO/DRIFT_ZOOM_HI are this renderer's own
  // choice, tuned on the bench -- the issue gave "panned > 1/3 viewport or r
  // outside [0.67, 1.5]" as an example ("e.g."), not a spec. They're
  // deliberately asymmetric on zoom: paint() only ever draws what's near the
  // viewport at paint time (#49's culling), so panning OR zooming OUT
  // (r < DRIFT_ZOOM_LO) can both reveal genuinely blank map beyond what was
  // drawn -- that has to stay tight. Zooming IN (r > 1) can't reveal blank
  // map; it only magnifies a SUBSET of what's already painted, at a dot
  // density (#48) that goes stale-sparse rather than stale-blank until the
  // next real paint() catches up -- an acceptable transient, so
  // DRIFT_ZOOM_HI is set far looser than DRIFT_ZOOM_LO instead of mirroring
  // it. (Re-benched with DRIFT_ZOOM_HI=4 on dify/n8n/aws-sdk-go-v2: see the
  // PR description for the zoom numbers.)
  private static readonly SETTLE_MS = 140;
  private static readonly DRIFT_PAN_FRACTION = 1 / 3;
  private static readonly DRIFT_ZOOM_LO = 0.67;
  private static readonly DRIFT_ZOOM_HI = 4;
  private static readonly DRIFT_REDRAW_MS = 250;

  private readonly TOUCH = matchMedia("(pointer: coarse)").matches;
  private readonly onPointerDown = (e: PointerEvent) => this.pointerDown(e);
  private readonly onPointerMove = (e: PointerEvent) => this.pointerMove(e);
  private readonly onPointerUp = (e: PointerEvent) => this.endPointer(e);
  private readonly onWheel = (e: WheelEvent) => this.wheel(e);
  private readonly onClick = (e: MouseEvent) => this.click(e);

  constructor(svg: SVGSVGElement, callbacks: MapRendererCallbacks) {
    this.svg = svg;
    this.callbacks = callbacks;
    svg.addEventListener("pointerdown", this.onPointerDown);
    svg.addEventListener("pointermove", this.onPointerMove);
    svg.addEventListener("pointerup", this.onPointerUp);
    svg.addEventListener("pointercancel", this.onPointerUp);
    svg.addEventListener("wheel", this.onWheel, { passive: false });
    svg.addEventListener("click", this.onClick);
  }

  destroy() {
    if (this.animId != null) cancelAnimationFrame(this.animId);
    this.cancelPendingGestureWork();
    this.svg.removeEventListener("pointerdown", this.onPointerDown);
    this.svg.removeEventListener("pointermove", this.onPointerMove);
    this.svg.removeEventListener("pointerup", this.onPointerUp);
    this.svg.removeEventListener("pointercancel", this.onPointerUp);
    this.svg.removeEventListener("wheel", this.onWheel);
    this.svg.removeEventListener("click", this.onClick);
  }

  /** Call once per repo/document change, before the first render(). Resets
   * the per-repo maxima used to scale node radius and the churn/complexity
   * ramps. Route-finding (BFS over the import graph) lives in map/graph.ts
   * and is called from React (MapView), not here — the renderer only draws
   * whatever Route object it's handed in render(state). */
  loadDocument(doc: MapDocument) {
    this.maxLoc = Math.max(1, ...doc.N.map((r) => r[3]));
    this.maxCh = Math.max(1, ...doc.N.map((r) => r[5]));
    this.maxCx = Math.max(1, ...doc.N.map((r) => r[4]));

    // Prominence order per district (issue #48): landmark files first (the
    // map already treats them as a district's headline files), then the
    // same FI + LOC/50 score drawLabels uses to decide which file labels
    // earn the budget at low zoom -- so a district's first dots to reveal
    // are the same files that would earn a label, not an unrelated ranking.
    // Ties are broken by file index explicitly: two files can tie on every
    // ranking term (zero fan-in, zero loc), and CLAUDE.md's determinism
    // rule means that has to be spelled out rather than left to whatever
    // order doc.N happens to iterate in (stable-sort would preserve that
    // order today, but "today's iteration order" is not a rule).
    this.districtOrder.clear();
    this.districtArea.clear();
    this.fileRank.clear();
    const isLandmark = new Set(doc.L.map(([i]) => i));
    const byDistrict = new Map<number, number[]>();
    for (let i = 0; i < doc.N.length; i++) {
      const d = D_(doc, i);
      if (!byDistrict.has(d)) byDistrict.set(d, []);
      byDistrict.get(d)!.push(i);
    }
    for (const [d, members] of byDistrict) {
      members.sort((a, b) => {
        const la = isLandmark.has(a) ? 1 : 0;
        const lb = isLandmark.has(b) ? 1 : 0;
        if (la !== lb) return lb - la;
        const sa = FI(doc, a) + LOC(doc, a) / 50;
        const sb = FI(doc, b) + LOC(doc, b) / 50;
        if (sa !== sb) return sb - sa;
        return a - b;
      });
      this.districtOrder.set(d, members);
      members.forEach((i, rank) => this.fileRank.set(i, rank));
    }
    for (const key in doc.districts) {
      this.districtArea.set(+key, districtWorldArea(doc.districts[key]));
    }
  }

  /** issue #48: how much of file `i`'s dot to draw this frame, from 0 (skip
   * the element entirely -- the perf win, and a tap there falls through to
   * the district path underneath, data-k="d:...") to 1 (full strength).
   * Callers that need a file drawn unconditionally (landmarks, the current
   * selection, a route or blast set) must check that themselves first --
   * this function only ever answers the density question. */
  private dotFactor(i: number): number {
    const { doc } = this.state!;
    const d = D_(doc, i);
    const district = doc.districts[String(d)];
    // Unconnected districts carry no polygon at all -- geometry.rs empties
    // their `blob` (districtClass's doc comment) -- so there is no area to
    // budget against. They are already the map's most de-emphasised places
    // (no polygon, no road, no label: drawLabels skips them outright), and
    // a file's dot is its ONLY trace on the map; thinning it would erase
    // the file rather than declutter a place, so the simplest defensible
    // rule is to never budget these districts at all.
    if (districtClass(district) === "unconnected" || district.size <= 0) return 1;
    const area = this.districtArea.get(d) ?? 0;
    const edge = (area * this.k * this.k) / DOT_DENSITY_FLOOR;
    // Fast path: once the budget covers every file in the district there is
    // nothing to rank or fade, and every acceptance fixture + crawlab lives
    // here at the fit zoom (DOT_DENSITY_FLOOR's comment) -- skip straight to
    // "draw everything" so the DOM stays byte-identical to the pre-#48
    // renderer there, rather than asymptotically approaching 1 through the
    // fade math below.
    if (edge >= district.size) return 1;
    const rank = this.fileRank.get(i) ?? district.size;
    // Fade band: the last ~half of the CURRENT (continuous, unfloored)
    // budget ramps in rather than popping, so a pinch-zoom fills a district
    // in gradually instead of one file blinking on at a time. Using the
    // continuous `edge` (not Math.floor(edge)) is what makes the ramp itself
    // continuous -- floored, every whole-number crossing would still be a
    // pop for whichever one file sits at that exact rank.
    const fadeWidth = Math.max(1, edge * 0.5);
    return Math.min(1, Math.max(0, (edge - rank) / fadeWidth));
  }

  /** VW/VH track the canvas element's own box, not the window — the sidebar
   * and mobile drawer both change available width without a window resize
   * firing, and the reference's window-resize listener under-reacted to
   * exactly that case.
   *
   * This used to always re-fit (hence the `anim` parameter this method no
   * longer takes). That was wrong: MapCanvas's ResizeObserver re-subscribed
   * on every selection/layer/route change, and observe() always delivers one
   * "initial size" callback on subscribe whether or not the box actually
   * changed — so tapping a file dot, tapping it away, or switching layers
   * each re-fit the map back to its opening view. MapCanvas no longer
   * re-subscribes for that reason, but resize() still has to hold up its own
   * end: a GENUINE resize (rotation, the phone URL bar showing/hiding, the
   * sidebar opening) must not discard whatever the viewer was looking at
   * either. So instead of fitting, this keeps the world point that was under
   * the viewport centre still under the centre, and keeps k — clamped,
   * because a smaller viewport can raise fitScale()'s floor out from under
   * the old k. Fitting stays explicit: MapCanvas's repoKey effect (a new
   * document has no "current view" worth preserving) and the fit button. */
  resize(vw: number, vh: number) {
    const newVW = Math.max(360, vw);
    const newVH = Math.max(300, vh);
    if (newVW === this.VW && newVH === this.VH) return;
    const prevVW = this.VW;
    const prevVH = this.VH;
    this.VW = newVW;
    this.VH = newVH;
    if (!this.state) return;
    // Issue #51: a resize can land mid-gesture (the phone URL bar can hide
    // while a finger is still down, mid-drag). Cancel whatever preview/settle
    // work is pending against the OLD viewport before repainting at the new
    // one, the same as fit()'s non-anim path and render() do — otherwise a
    // leftover settle timer could fire a redundant repaint a moment later, or
    // a leftover preview transform could still be mid-flight when draw()
    // below replaces rootG out from under it. draw() then re-establishes
    // drawnK/drawnTx/drawnTy from the (k, tx, ty) computed here, so nothing
    // stale is left for a later gestureFrame() to compare against.
    this.cancelPendingGestureWork();
    const worldX = (prevVW / 2 - this.tx) / this.k;
    const worldY = (prevVH / 2 - this.ty) / this.k;
    this.k = this.clampK(this.k);
    this.tx = this.VW / 2 - worldX * this.k;
    this.ty = this.VH / 2 - worldY * this.k;
    this.draw();
  }

  render(state: MapRenderState) {
    this.state = state;
    // Issue #51: a real state change (selection, layer, geo, a route...)
    // always gets a full paint(), never a transform -- preview() only ever
    // moves geometry that's still valid for the CURRENT state. Cancel
    // whatever gesture-settling work was pending so it can't fire moments
    // later against a state this paint() has already superseded.
    this.cancelPendingGestureWork();
    this.draw();
  }

  // ---------- viewport ----------
  private fitScale(): number {
    if (!this.state) return 1;
    return fitScaleOf(this.state.doc, this.state.geo, this.VW, this.VH);
  }
  /** Floor for how far a viewer can zoom OUT, based on the FULL extent
   * (mainland + offshore rings) -- never the default framing. Without this,
   * `clampK`'s `fitScale()*0.5` floor would be a fraction of a view that
   * itself excludes the offshore rings (see `frameBounds`), so scrolling or
   * pinching out could never reach far enough to see an island or
   * unconnected district at all (issue #34, requirement 2: offshore is
   * reachable, not clamped away). */
  private fullScale(): number {
    if (!this.state) return 1;
    return fullFitScaleOf(this.state.doc, this.state.geo, this.VW, this.VH);
  }
  /** The box the default view and the "fit" control frame: mainland only
   * (issue #34) -- see `mainlandBounds`'s doc comment in geometry.ts for why
   * framing the full extent by default would bury the actual map (n8n:
   * mainland is 27.2% of the full extent) under its own offshore rings. */
  private frameBounds() {
    if (!this.state) return [0, 0, 1, 1] as [number, number, number, number];
    return mainlandBounds(this.state.doc, this.state.geo);
  }
  /** `state` is for MapCanvas's repoKey effect only: on a repo change it has
   * to fit the NEW document, but frameBounds()/draw() both read `this.state`,
   * and `this.state` is only otherwise updated by render() -- which paints.
   * Setting it here, inline with the one paint fit() already does, is what
   * makes "reset derived indices and fit before the first paint of the new
   * document" (see that effect's comment) actually true, rather than the
   * effect's OWN fit() call framing whatever document `this.state` still
   * held from the PREVIOUS repo (React runs a component's layout effects in
   * declaration order within a commit, so the later state-render effect
   * hasn't updated `this.state` yet when this one runs) -- painted over a
   * beat later by that state-render effect's own render(), which doesn't
   * refit, leaving the NEW document's geometry drawn at the OLD document's
   * transform. A prior version of this bug shipped invisibly: the
   * ResizeObserver effect this fix (fix/keep-view-on-select) stopped
   * re-subscribing on every prop change used to deliver a spurious extra
   * resize()-then-fit() shortly after, which re-fit against the by-then-
   * current `this.state` and papered over it. Every other caller passes no
   * `state` and gets exactly today's behaviour. */
  fit(anim: boolean, state?: MapRenderState) {
    if (state) this.state = state;
    const b = this.frameBounds();
    const pad = 46;
    const s = Math.min((this.VW - 2 * pad) / (b[2] - b[0] || 1), (this.VH - 2 * pad) / (b[3] - b[1] || 1));
    const nx = pad + ((this.VW - 2 * pad) - (b[2] - b[0]) * s) / 2 - b[0] * s;
    const ny = pad + ((this.VH - 2 * pad) - (b[3] - b[1]) * s) / 2 - b[1] * s;
    if (anim) this.glide(s, nx, ny);
    else {
      this.cancelPendingGestureWork();
      this.k = s;
      this.tx = nx;
      this.ty = ny;
      this.draw();
    }
  }
  private glide(nk: number, ntx: number, nty: number, ms = 520) {
    const k0 = this.k;
    const x0 = this.tx;
    const y0 = this.ty;
    const t0 = performance.now();
    const ease = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) {
      this.cancelPendingGestureWork();
      this.k = nk;
      this.tx = ntx;
      this.ty = nty;
      this.draw();
      return;
    }
    if (this.animId != null) cancelAnimationFrame(this.animId);
    const step = (now: number) => {
      const u = Math.min(1, (now - t0) / ms);
      const e = ease(u);
      this.k = k0 + (nk - k0) * e;
      this.tx = x0 + (ntx - x0) * e;
      this.ty = y0 + (nty - y0) * e;
      if (u < 1) {
        // Issue #51: glide()'s intermediate frames are a gesture too -- move
        // the drawn <g> with a transform (or force a real paint() if the
        // drift bound says the culled content is going stale), the same as
        // a drag/pinch/wheel frame, rather than rebuilding the SVG on every
        // one of glide's ~30 animation frames.
        this.gestureFrame();
        this.animId = requestAnimationFrame(step);
      } else {
        // Animation complete: always a real paint(), not a transform left
        // to the settle timer, so the view is exactly right the instant the
        // glide visibly stops rather than up to SETTLE_MS later.
        this.animId = null;
        this.cancelPendingGestureWork();
        this.draw();
      }
    };
    this.animId = requestAnimationFrame(step);
  }
  flyTo(i: number, zoomTo?: number) {
    if (!this.state) return;
    const p = this.px(i);
    const nk = zoomTo || Math.max(this.k, this.fitScale() * 3.2);
    this.glide(nk, this.VW / 2 - p[0] * nk, this.VH / 2 - p[1] * nk);
  }
  /** Fly all the way into room level, for a search hit on a symbol — the
   * reference does this as a special case (`fitScale()*Math.max(8,
   * BUILD_ZOOM+3)`) so picking a class or function from search lands you
   * looking at its room, not just its file's plot. */
  flyToDetail(i: number) {
    this.flyTo(i, this.fitScale() * Math.max(8, BUILD_ZOOM + 3));
  }
  zoomDistrict(d: number) {
    if (!this.state) return;
    const district = this.state.doc.districts[d];
    let c = this.state.geo !== "t" ? district.c : tmCentre(this.state.doc, d);
    // tmCentre folds min/max over every N row whose district matches `d`; a
    // district with no member row would leave its accumulator at the
    // untouched +-1e9 sentinel, averaging to a wrong-but-finite (0,0) --
    // not NaN, but just as useless a place to fly to. Empirically
    // unreachable today (every id `compact()` emits already carries >=1
    // file, mainland or not, per geometry.rs), guarded anyway: `district.c`
    // is always a valid, finite coordinate either way -- mainland's own
    // centroid, or an island/unconnected's ring position
    // (geometry.rs::relocate_offshore) -- so it's a correct fallback for a
    // literal NaN too, if that invariant is ever violated from elsewhere.
    if (!Number.isFinite(c[0]) || !Number.isFinite(c[1])) c = district.c;
    const narrow = window.innerWidth <= 820;
    const nk = this.fitScale() * (narrow ? 2.2 : 2.6);
    this.glide(nk, this.VW / 2 - c[0] * nk, this.VH / 2 - c[1] * nk);
  }
  private clampK(v: number) {
    // The lower bound is whichever scale is smaller, so a viewer can always
    // zoom out far enough to see the full extent -- islands, unconnected,
    // everything -- even though the default view only frames mainland.
    // Requirement 2 (issue #34): offshore districts are reachable by
    // zooming out, not clamped away. On a pre-#34 fixture (no `class`
    // anywhere) mainland IS the full extent, so fullScale() === fitScale()
    // and this is exactly today's `fitScale() * 0.5` floor.
    const lo = Math.min(this.fitScale() * 0.5, this.fullScale());
    return Math.max(lo, Math.min(this.fitScale() * 40, v));
  }
  zoomBy(f: number) {
    const nk = this.clampK(this.k * f);
    this.glide(nk, this.VW / 2 - (this.VW / 2 - this.tx) * (nk / this.k), this.VH / 2 - (this.VH / 2 - this.ty) * (nk / this.k), 240);
  }

  private px(i: number): [number, number] {
    if (!this.state) return [0, 0];
    return pxOf(this.state.doc, this.state.geo, i);
  }
  private X(v: number) {
    return v * this.k + this.tx;
  }
  private Y(v: number) {
    return v * this.k + this.ty;
  }
  private S(v: number) {
    return v * this.k;
  }
  private tint(i: number): string {
    const { doc, layer } = this.state!;
    if (layer === "d") return districtColor(D_(doc, i));
    if (layer === "c") return ramp(Math.min(1, CH(doc, i) / (this.maxCh || 1)));
    return ramp(Math.min(1, CX_(doc, i) / (this.maxCx || 1)));
  }
  private narrow() {
    return window.innerWidth <= 820;
  }

  // ---------- draw ----------
  // Issue #51: performance.mark/measure around paint(), so
  // web/scripts/perf-bench.mjs can read draw() cost straight off the
  // Performance timeline via performance.getEntriesByName("tolmap:draw").
  // Gated behind window.__TOLMAP_PERF__ (set by the bench via
  // page.addInitScript, before any app code runs): the User Timing buffer
  // has no eviction, so leaving this unconditional would leak one
  // "tolmap:draw" measure per frame for the life of a real session -- a
  // slow, silent memory leak in every visitor's tab for a number only the
  // bench ever reads.
  private draw() {
    if (!window.__TOLMAP_PERF__) {
      this.paint();
      return;
    }
    performance.mark("tolmap:draw:start");
    this.paint();
    performance.mark("tolmap:draw:end");
    performance.measure("tolmap:draw", "tolmap:draw:start", "tolmap:draw:end");
  }

  private paint() {
    const state = this.state;
    const svg = this.svg;
    svg.setAttribute("viewBox", `0 0 ${this.VW} ${this.VH}`);
    svg.textContent = "";
    // A real paint() bakes fresh geometry at the CURRENT (k, tx, ty), so any
    // in-flight preview transform is now stale by definition -- clear the
    // class that scopes the non-scaling-stroke rule (see index.css) and
    // forget the previous root, rather than leaving a removed <g> referenced.
    svg.classList.remove("previewing");
    this.rootG = null;
    if (!state) return;
    const { doc, geo, layer, sel, selSym, selD, route } = state;
    const g = el("g", {});
    svg.appendChild(g);
    // Issue #51: remember exactly what this paint() drew at, so a later
    // gesture frame can move `g` with a transform (preview()) instead of
    // rebuilding it, and so settle()/driftExceeded() know how far the
    // transform has stretched from the geometry actually baked into the DOM.
    this.rootG = g;
    this.drawnK = this.k;
    this.drawnTx = this.tx;
    this.drawnTy = this.ty;
    const zf0 = this.k / this.fitScale();

    if (geo !== "t") {
      doc.roads.forEach(([a, b, w]) => {
        const p = doc.districts[String(a)].c;
        const q = doc.districts[String(b)].c;
        g.appendChild(
          el("line", {
            x1: this.X(p[0]),
            y1: this.Y(p[1]),
            x2: this.X(q[0]),
            y2: this.Y(q[1]),
            stroke: "var(--coast)",
            "stroke-width": (0.5 + 3 * w).toFixed(1),
            "stroke-linecap": "round",
            "stroke-opacity": 0.5,
          }),
        );
      });
      for (const d in doc.districts) {
        const on = selD === +d;
        // Islands are real places but not the map's subject (issue #34): a
        // thinner, fainter outline is the de-emphasis mechanism here,
        // rather than a second colour scheme (districtColor stays the only
        // source of hue, per spec). Unconnected districts never draw
        // anything in this loop regardless -- the Rust side already
        // emptied their `blob` (geometry.rs), so `.forEach` below is a
        // no-op for them and they need no explicit case.
        const faint = districtClass(doc.districts[d]) === "island";
        doc.districts[d].blob.forEach((poly) => {
          const path = el("path", {
            d: "M" + poly.map((q) => this.X(q[0]).toFixed(1) + " " + this.Y(q[1]).toFixed(1)).join("L") + "Z",
            fill: districtColor(+d),
            "fill-opacity": layer === "d" ? (on ? 0.3 : faint ? 0.07 : 0.14) : on ? 0.16 : faint ? 0.03 : 0.06,
            stroke: on ? "var(--hot)" : districtColor(+d),
            "stroke-width": on ? 2.6 : faint ? 0.9 : 1.5,
            "stroke-opacity": on ? 1 : faint ? 0.4 : 0.7,
            "stroke-linejoin": "round",
            class: "hit",
            "pointer-events": "all",
            "data-k": "d:" + d,
          });
          const t = el("title", {});
          t.textContent = `${doc.names[d]} — ${doc.districts[d].size} files`;
          path.appendChild(t);
          g.appendChild(path);
        });
      }
      this.drawTerrain(g, zf0);
    }

    const CELL = geo === "p" && doc.P && zf0 > PARCEL_ZOOM;
    const ROOMS = geo === "p" && zf0 > BUILD_ZOOM;
    const defs = CELL ? el("defs", {}) : null;
    if (defs) g.appendChild(defs);
    const blast = computeBlast(doc, sel, selSym);
    const dim = route ? new Set(route.path) : blast ? new Set([...blast.set, sel!]) : null;
    // Issue #48: files that must never be thinned or culled, whatever their
    // district's dot budget says -- landmarks, the current selection, and
    // anything a route or blast radius is highlighting. `dim`, despite the
    // name, is exactly "the set to keep at full strength" whenever it's
    // non-null (see its use below); folding it in here means this list
    // can't drift from what the existing dim/undim opacity logic already
    // treats as important, including a search/flyTo landing on `sel` --
    // flyTo/flyToDetail don't need their own zoom-reveals-target logic
    // because the file they land on is always in this set.
    const alwaysDrawn = new Set<number>(doc.L.map(([i]) => i));
    if (sel != null) alwaysDrawn.add(sel);
    if (dim) for (const i of dim) alwaysDrawn.add(i);
    for (let i = 0; i < doc.N.length; i++) {
      if (CELL && doc.P![String(i)]) {
        const p = this.px(i);
        const cx = this.X(p[0]);
        const cy = this.Y(p[1]);
        if (cx < -260 || cx > this.VW + 260 || cy < -260 || cy > this.VH + 260) continue;
        this.plot(g, defs!, i, dim, ROOMS);
        continue;
      }
      const p = this.px(i);
      let node: SVGElement;
      if (geo !== "t") {
        // Density budget (issue #48): below the floor, don't create the
        // element at all -- that's the perf win, and it's also what makes
        // hit-testing fall through to the district path underneath for
        // free (there is no invisible dot left to swallow the tap).
        const factor = alwaysDrawn.has(i) ? 1 : this.dotFactor(i);
        if (factor <= 0) continue;
        const r = Math.max(1.1, (1.6 + 5.2 * Math.sqrt(LOC(doc, i) / this.maxLoc)) * Math.sqrt(this.k / this.fitScale()));
        // Cull margin covers the dot's own radius plus its touch hit-stroke
        // halo (up to 16px, see stroke-width below) -- the same treatment
        // the CELL/parcel path above already gives its (larger) plots.
        const cx = this.X(p[0]);
        const cy = this.Y(p[1]);
        const margin = r + 20;
        if (cx < -margin || cx > this.VW + margin || cy < -margin || cy > this.VH + margin) continue;
        const baseOpacity = dim && !dim.has(i) ? 0.2 : 0.85;
        // Keep the exact pre-#48 numeric literal when factor is 1 (the
        // common case, and required for the no-change proof's byte-for-byte
        // DOM comparison on the nine fixtures + crawlab at fit zoom) --
        // only files actually being thinned pay for the extra rounding.
        node = el("circle", {
          cx: cx.toFixed(1),
          cy: cy.toFixed(1),
          r: r.toFixed(2),
          fill: this.tint(i),
          "fill-opacity": factor === 1 ? baseOpacity : Math.round(baseOpacity * factor * 1000) / 1000,
          class: "hit",
          stroke: "transparent",
          "stroke-width": this.TOUCH ? 16 : 0,
          "pointer-events": "all",
          "data-k": "f:" + i,
        });
      } else {
        // Treemap tiles are left ungated by the #48 budget: unlike a
        // fixed-radius dot, a treemap rect already occupies exactly this
        // file's share of its district's box, sized down and never
        // overlapping (the Math.max(...,0.6) floor below is the only
        // shrink limit) -- a dense district degenerates into a fine but
        // legible mosaic of colour rather than the illegible scatter of
        // overlapping circles the issue measured. Still viewport-culled.
        const r = RECT(doc, i);
        const rx = this.X(r[0]);
        const ry = this.Y(r[1]);
        const rw = Math.max(this.S(r[2]) - 1.2, 0.6);
        const rh = Math.max(this.S(r[3]) - 1.2, 0.6);
        if (rx + rw < -20 || rx > this.VW + 20 || ry + rh < -20 || ry > this.VH + 20) continue;
        node = el("rect", {
          x: rx.toFixed(1),
          y: ry.toFixed(1),
          width: rw.toFixed(1),
          height: rh.toFixed(1),
          rx: 1.4,
          fill: this.tint(i),
          "fill-opacity": dim && !dim.has(i) ? 0.18 : layer === "d" ? 0.8 : 0.92,
          class: "hit",
          stroke: "transparent",
          "stroke-width": this.TOUCH ? 12 : 0,
          "pointer-events": "all",
          "data-k": "f:" + i,
        });
      }
      const t = el("title", {});
      t.textContent = `${doc.F[i]}\n${LOC(doc, i)} loc · churn ${CH(doc, i)} · cplx ${CX_(doc, i)}`;
      node.appendChild(t);
      g.appendChild(node);
    }

    // blast radius: a thread from every file that names this symbol
    if (blast && sel != null) {
      const o = this.px(sel);
      const ox = this.X(o[0]);
      const oy = this.Y(o[1]);
      blast.files.forEach((j) => {
        const q = this.px(j);
        g.appendChild(
          el("line", {
            x1: this.X(q[0]).toFixed(1),
            y1: this.Y(q[1]).toFixed(1),
            x2: ox.toFixed(1),
            y2: oy.toFixed(1),
            stroke: "var(--hot)",
            "stroke-width": 1,
            "stroke-opacity": 0.34,
            "pointer-events": "none",
          }),
        );
      });
      blast.files.forEach((j) => {
        const q = this.px(j);
        g.appendChild(
          el("circle", {
            cx: this.X(q[0]).toFixed(1),
            cy: this.Y(q[1]).toFixed(1),
            r: 4.6,
            fill: "none",
            stroke: "var(--hot)",
            "stroke-width": 1.8,
            "stroke-opacity": 0.9,
            "pointer-events": "none",
          }),
        );
      });
    }

    if (route) {
      const pts = route.path.map((i) => {
        const p = this.px(i);
        return [this.X(p[0]), this.Y(p[1])];
      });
      g.appendChild(
        el("polyline", {
          points: pts.map((q) => q[0].toFixed(1) + "," + q[1].toFixed(1)).join(" "),
          fill: "none",
          stroke: "var(--hot)",
          "stroke-width": 2.6,
          "stroke-linejoin": "round",
          "stroke-linecap": "round",
          "stroke-opacity": 0.92,
        }),
      );
      pts.forEach((q, n) =>
        g.appendChild(
          el("circle", {
            cx: q[0].toFixed(1),
            cy: q[1].toFixed(1),
            r: n === 0 || n === pts.length - 1 ? 6 : 3.6,
            fill: "var(--hot)",
            stroke: "var(--canvas)",
            "stroke-width": 1.6,
          }),
        ),
      );
    }

    this.drawLabels(g, alwaysDrawn);

    // landmark pins
    const zf = this.k / this.fitScale();
    doc.L.forEach(([i, why, detail, rank]: LandmarkRow) => {
      if (zf > 3.4 && why === "capital") return;
      const p = this.px(i);
      const cx = this.X(p[0]);
      const cy = this.Y(p[1]);
      if (cx < -30 || cx > this.VW + 30 || cy < -30 || cy > this.VH + 30) return;
      const gg = el("g", { class: "hit", "data-k": "f:" + i });
      gg.appendChild(
        el("path", {
          d: `M ${cx.toFixed(1)} ${cy.toFixed(1)} l -8 -12 a 9.5 9.5 0 1 1 16 0 z`,
          fill: "#111A1E",
          stroke: "#fff",
          "stroke-width": 1.25,
        }),
      );
      const t = el("text", {
        x: cx.toFixed(1),
        y: (cy - 12.5).toFixed(1),
        "font-size": 9.5,
        fill: "#fff",
        "text-anchor": "middle",
        "font-family": "IBM Plex Mono, monospace",
        "font-weight": 600,
      });
      t.textContent = String(rank);
      gg.appendChild(t);
      const tt = el("title", {});
      tt.textContent = `${why} — ${doc.F[i]}\n${detail}`;
      gg.appendChild(tt);
      g.appendChild(gg);
    });

    if (sel != null) this.ring(g, sel);
  }

  /** Terrain stays inside the imperative surface and uses the same delegated
   * `data-k` click path as files and districts. In particular, the invisible
   * hit strokes below widen on coarse pointers without adding a touch-only
   * event path (the three pointer bugs documented at the bottom of this
   * file apply to these targets too). */
  private drawTerrain(g: SVGGElement, zoom: number) {
    const { doc, selTerrain } = this.state!;
    if (!doc.terrain || zoom < 1.25) return;
    for (const [districtKey, terrain] of Object.entries(doc.terrain)) {
      const district = +districtKey;
      const color = districtColor(district);
      terrain.subdistricts.forEach((subdistrict, index) => {
        const selected =
          selTerrain?.kind === "subdistrict" && selTerrain.district === district && selTerrain.index === index;
        subdistrict.blob.forEach((polygon) => {
          const path = el("path", {
            d: "M" + polygon.map((point) => `${this.X(point[0]).toFixed(1)} ${this.Y(point[1]).toFixed(1)}`).join("L") + "Z",
            fill: color,
            "fill-opacity": selected ? 0.28 : 0.07,
            stroke: selected ? "var(--hot)" : color,
            "stroke-width": selected ? 2.4 : 1.1,
            "stroke-opacity": selected ? 1 : 0.82,
            "stroke-dasharray": selected ? "none" : "4 2",
            "stroke-linejoin": "round",
            class: "hit",
            "pointer-events": "all",
            "data-k": `sd:${district}:${index}`,
          });
          const title = el("title");
          title.textContent = `${doc.names[districtKey]} · ${subdistrict.suffix} — ${subdistrict.members.length} files`;
          path.appendChild(title);
          g.appendChild(path);
        });
        if (zoom > 1.7) {
          const label = el("text", {
            x: this.X(subdistrict.c[0]).toFixed(1),
            y: this.Y(subdistrict.c[1]).toFixed(1),
            "font-size": 9.5,
            "text-anchor": "middle",
            fill: "var(--ink)",
            "fill-opacity": 0.82,
            "font-family": "IBM Plex Mono, monospace",
            "paint-order": "stroke",
            stroke: "var(--canvas)",
            "stroke-width": 3.2,
            "stroke-linejoin": "round",
            class: "hit",
            "pointer-events": "all",
            "data-k": `sd:${district}:${index}`,
          });
          label.textContent = `${doc.names[districtKey]} · ${subdistrict.suffix}`;
          g.appendChild(label);
        }
      });

      if (zoom > 2.4) {
        terrain.parcels.forEach((parcel, index) => {
          const [x, y, width, height] = parcel.rect;
          const selected = selTerrain?.kind === "parcel" && selTerrain.district === district && selTerrain.index === index;
          g.appendChild(
            el("rect", {
              x: this.X(x).toFixed(1),
              y: this.Y(y).toFixed(1),
              width: Math.max(0.5, this.S(width)).toFixed(1),
              height: Math.max(0.5, this.S(height)).toFixed(1),
              fill: color,
              "fill-opacity": selected ? 0.34 : 0.13,
              stroke: selected ? "var(--hot)" : color,
              "stroke-width": selected ? 2.1 : 0.8,
              "stroke-opacity": 0.8,
              "pointer-events": "none",
            }),
          );
          const hit = el("rect", {
            x: this.X(x).toFixed(1),
            y: this.Y(y).toFixed(1),
            width: Math.max(0.5, this.S(width)).toFixed(1),
            height: Math.max(0.5, this.S(height)).toFixed(1),
            fill: "transparent",
            stroke: "transparent",
            "stroke-width": this.TOUCH ? 12 : 2,
            class: "hit",
            "pointer-events": "all",
            "data-k": `p:${district}:${index}`,
          });
          const title = el("title");
          title.textContent = `${parcel.address} — ${parcel.members.length} files`;
          hit.appendChild(title);
          g.appendChild(hit);
          if (zoom > 4 && this.S(width) > 30 && this.S(height) > 12) {
            const label = el("text", {
              x: this.X(x + width / 2).toFixed(1),
              y: (this.Y(y + height / 2) + 3).toFixed(1),
              "font-size": 8.5,
              "text-anchor": "middle",
              fill: "var(--ink)",
              "fill-opacity": 0.8,
              "font-family": "IBM Plex Mono, monospace",
              "paint-order": "stroke",
              stroke: "var(--canvas)",
              "stroke-width": 3,
              "pointer-events": "none",
            });
            label.textContent = parcel.address;
            g.appendChild(label);
          }
        });
      }

      terrain.arterials.forEach((arterial) => {
        const origin = this.px(arterial.file);
        const commands = arterial.links
          .map((neighbour) => {
            const target = this.px(neighbour);
            return `M${this.X(origin[0]).toFixed(1)} ${this.Y(origin[1]).toFixed(1)}L${this.X(target[0]).toFixed(1)} ${this.Y(target[1]).toFixed(1)}`;
          })
          .join("");
        if (!commands) return;
        g.appendChild(
          el("path", {
            d: commands,
            fill: "none",
            stroke: color,
            "stroke-width": Math.min(3.4, 1.3 + Math.log2(arterial.stranded + 1) * 0.22).toFixed(1),
            "stroke-opacity": 0.45,
            "stroke-linecap": "round",
            "pointer-events": "none",
          }),
        );
        const hit = el("path", {
          d: commands,
          fill: "none",
          stroke: "transparent",
          "stroke-width": this.TOUCH ? 18 : 7,
          "stroke-linecap": "round",
          class: "hit",
          "pointer-events": "stroke",
          "data-k": `f:${arterial.file}`,
        });
        const title = el("title");
        title.textContent = `${doc.F[arterial.file]} — arterial, strands ${arterial.stranded} files`;
        hit.appendChild(title);
        g.appendChild(hit);
      });
    }
  }

  // Label budget: districts first, then files by importance, skipping
  // collisions.
  private drawLabels(g: SVGGElement, alwaysDrawn: Set<number>) {
    const { doc, geo } = this.state!;
    const placed: [number, number, number, number][] = [];
    const hits = (x: number, y: number, w: number, h: number) =>
      placed.some((r) => !(x + w < r[0] || x > r[0] + r[2] || y + h < r[1] || y > r[1] + r[3]));
    const put = (x: number, y: number, txt: string, size: number, op: number, weight?: number, dk?: number) => {
      const w = txt.length * size * 0.62;
      const h = size * 1.25;
      if (hits(x - w / 2, y - h, w, h)) return false;
      placed.push([x - w / 2, y - h, w, h]);
      const t = el("text", {
        x: x.toFixed(1),
        y: y.toFixed(1),
        "font-size": size,
        "text-anchor": "middle",
        fill: "var(--ink)",
        "fill-opacity": op,
        "font-family": "IBM Plex Mono, monospace",
        "paint-order": "stroke",
        stroke: "var(--canvas)",
        "stroke-width": 3.2,
        "stroke-linejoin": "round",
      });
      if (weight) t.setAttribute("font-weight", String(weight));
      if (dk != null) {
        t.setAttribute("class", "hit");
        t.setAttribute("pointer-events", "all");
        t.setAttribute("data-k", "d:" + dk);
      }
      t.textContent = txt;
      g.appendChild(t);
      return true;
    };
    const narrow = this.narrow();
    const zf = this.k / this.fitScale();
    // Mainland labels claim the shared collision budget first; an island's
    // `put()` below only succeeds where that leaves room -- "reduced
    // priority within the existing label budget" (issue #34), not a second
    // pass or a bigger one. At real density (n8n had 269 islands crowded
    // onto one ring before finding 15's resolver fix, 52 at cb04469;
    // measured on a 60-island synthetic stress fixture to overlap well
    // before they'd stop colliding on screen) this is what keeps the
    // result a sparse, legible scatter of names instead of a solid
    // unreadable band of overlapping text. Unconnected districts are
    // skipped outright: the Rust side gave them no polygon because they
    // aren't places (geometry.rs), so labelling them here would strand a
    // name and a "N files" line out on the unconnected ring where nothing
    // is drawn to attach it to.
    const ids = Object.keys(doc.districts).sort((a, b) => {
      const pa = districtClass(doc.districts[a]) === "island" ? 1 : 0;
      const pb = districtClass(doc.districts[b]) === "island" ? 1 : 0;
      return pa - pb;
    });
    for (const d of ids) {
      const cls = districtClass(doc.districts[d]);
      if (cls === "unconnected") continue;
      const isIsland = cls === "island";
      const c = geo !== "t" ? doc.districts[d].c : tmCentre(doc, +d);
      const x = this.X(c[0]);
      const y = this.Y(c[1]);
      if (x < 0 || x > this.VW || y < 0 || y > this.VH) continue;
      const size = narrow ? Math.min(13, 10 + zf) : Math.min(17, 12 + zf);
      // Islands read as minor: smaller, dimmer, lighter weight, and no "N
      // files" subtitle -- with up to hundreds of them on a real repo, a
      // second line per label would be its own kind of clutter even after
      // the priority sort above thins the count that gets placed at all.
      put(x, y, doc.names[d], isIsland ? size * 0.75 : size, isIsland ? 0.5 : 0.82, isIsland ? 500 : 600, +d);
      if (zf < 1.8 && !narrow && !isIsland) put(x, y + 13, doc.districts[d].size + " files", 9.5, 0.45);
    }
    // file labels appear as you zoom in — the budget grows with scale
    if (geo === "p" && zf > BUILD_ZOOM) return; // plots label themselves
    const budget = Math.round(Math.min(narrow ? 18 : 60, Math.max(0, (zf - 1.5) * (narrow ? 10 : 26))));
    if (budget > 0) {
      const order = [...doc.N.keys()].sort((a, b) => (FI(doc, b) + LOC(doc, b) / 50) - (FI(doc, a) + LOC(doc, a) / 50));
      let n = 0;
      for (const i of order) {
        if (n >= budget) break;
        // Issue #48: never label a file whose dot the density budget hid --
        // a floating name with nothing under it reads as a bug, not a
        // place. Treemap ("t") dots are left ungated (see the draw() loop),
        // so this check only ever applies to "r"/"p".
        if (geo !== "t" && !alwaysDrawn.has(i) && this.dotFactor(i) <= 0) continue;
        const p = this.px(i);
        const x = this.X(p[0]);
        const y = this.Y(p[1]);
        if (x < 10 || x > this.VW - 10 || y < 14 || y > this.VH - 6) continue;
        if (put(x, y - 9, doc.F[i].split("/").pop()!, 10, 0.82)) n++;
      }
    }
  }

  private ring(g: SVGGElement, i: number) {
    const { doc, geo } = this.state!;
    const p = this.px(i);
    if (geo !== "t") {
      g.appendChild(
        el("circle", {
          cx: this.X(p[0]).toFixed(1),
          cy: this.Y(p[1]).toFixed(1),
          r: (4 + 5.2 * Math.sqrt(LOC(doc, i) / this.maxLoc) * Math.sqrt(this.k / this.fitScale()) + 3).toFixed(1),
          fill: "none",
          stroke: "var(--hot)",
          "stroke-width": 2.3,
        }),
      );
    } else {
      const r = RECT(doc, i);
      g.appendChild(
        el("rect", {
          x: (this.X(r[0]) - 2.5).toFixed(1),
          y: (this.Y(r[1]) - 2.5).toFixed(1),
          width: (this.S(r[2]) + 3).toFixed(1),
          height: (this.S(r[3]) + 3).toFixed(1),
          rx: 3,
          fill: "none",
          stroke: "var(--hot)",
          "stroke-width": 2.3,
        }),
      );
    }
  }

  // A plot is the file's actual share of its district: a weighted-Voronoi
  // cell, solved so that AREA tracks line count. Rooms are laid inside it and
  // clipped to its boundary, so a file's classes divide exactly the land the
  // file owns.
  private plot(g: SVGGElement, defs: SVGDefsElement, i: number, dim: Set<number> | null, roomsOn: boolean) {
    const { doc, sel, selSym, layer } = this.state!;
    const poly = doc.P![String(i)];
    const faded = !!(dim && !dim.has(i));
    let d = "M";
    let x0 = 1e9;
    let y0 = 1e9;
    let x1 = -1e9;
    let y1 = -1e9;
    for (let n = 0; n < poly.length; n++) {
      const X_ = this.X(poly[n][0]);
      const Y_ = this.Y(poly[n][1]);
      d += (n ? "L" : "") + X_.toFixed(1) + " " + Y_.toFixed(1);
      if (X_ < x0) x0 = X_;
      if (X_ > x1) x1 = X_;
      if (Y_ < y0) y0 = Y_;
      if (Y_ > y1) y1 = Y_;
    }
    d += "Z";
    const dcol = districtColor(D_(doc, i));
    const sy = symbolsOf(doc, i);
    const showRooms = roomsOn && sy.length > 0 && x1 - x0 > 26 && y1 - y0 > 20;

    g.appendChild(
      el("path", {
        d,
        fill: showRooms ? "var(--canvas)" : this.tint(i),
        "fill-opacity": faded ? 0.12 : showRooms ? 0.96 : layer === "d" ? 0.5 : 0.82,
        stroke: dcol,
        "stroke-width": showRooms ? 1.1 : 0.8,
        "stroke-opacity": faded ? 0.2 : 0.55,
        "stroke-linejoin": "round",
        class: "hit",
        "pointer-events": "all",
        "data-k": "f:" + i,
      }),
    );

    if (!showRooms) {
      if (sel === i) g.appendChild(el("path", { d, fill: "none", stroke: "var(--hot)", "stroke-width": 2.2 }));
      return;
    }

    const cid = "cp" + i;
    const cp = el("clipPath", { id: cid });
    cp.appendChild(el("path", { d }));
    defs.appendChild(cp);
    const inner = el("g", { "clip-path": "url(#" + cid + ")" });
    g.appendChild(inner);

    const w = x1 - x0;
    const h = y1 - y0;
    const cells = stripRows(rooms(sy, LOC(doc, i)), w, h);
    cells.forEach((c) => {
      const idx = c.sm ? sy.indexOf(c.sm) : -1;
      const isSel = sel === i && c.sm != null && selSym === idx;
      const r = el("rect", {
        x: (x0 + c.x).toFixed(1),
        y: (y0 + c.y).toFixed(1),
        width: Math.max(0.6, c.w - 1).toFixed(1),
        height: Math.max(0.6, c.h - 1).toFixed(1),
        fill: c.sm ? KCOL[c.sm[1]] || "#5E626A" : dcol,
        "fill-opacity": faded ? 0.14 : c.sm ? (isSel ? 1 : 0.6) : 0.09,
        class: c.sm ? "hit" : "",
        "pointer-events": c.sm ? "all" : "none",
        ...(c.sm ? { "data-k": "s:" + i + ":" + idx } : {}),
      });
      if (c.sm) {
        const t = el("title", {});
        t.textContent = `${c.sm[0]}  (${KIND[c.sm[1]]})\n${doc.F[i]}:${c.sm[2]}-${c.sm[3]}`;
        r.appendChild(t);
      }
      inner.appendChild(r);
      if (isSel)
        inner.appendChild(
          el("rect", {
            x: (x0 + c.x).toFixed(1),
            y: (y0 + c.y).toFixed(1),
            width: Math.max(0.6, c.w - 1).toFixed(1),
            height: Math.max(0.6, c.h - 1).toFixed(1),
            fill: "none",
            stroke: "var(--hot)",
            "stroke-width": 1.8,
          }),
        );
      if (c.sm && !faded && c.w > 36 && c.h > 12) {
        const short = c.sm[0].split(".").pop()!;
        const fits = Math.floor((c.w - 6) / 5.4);
        if (fits >= 3) {
          const t2 = el("text", {
            x: (x0 + c.x + c.w / 2).toFixed(1),
            y: (y0 + c.y + c.h / 2 + 3.2).toFixed(1),
            "font-size": 8.5,
            "text-anchor": "middle",
            fill: "#fff",
            "fill-opacity": 0.95,
            "font-family": "IBM Plex Mono, monospace",
            "pointer-events": "none",
          });
          t2.textContent = short.length > fits ? short.slice(0, fits - 1) + "…" : short;
          inner.appendChild(t2);
        }
      }
    });
    g.appendChild(
      el("path", {
        d,
        fill: "none",
        stroke: sel === i ? "var(--hot)" : dcol,
        "stroke-width": sel === i ? 2.4 : 1.8,
        "stroke-opacity": sel === i ? 1 : 0.92,
        "stroke-linejoin": "round",
        "pointer-events": "none",
      }),
    );
    if (w > 44 && h > 26) {
      const lbl = el("text", {
        x: ((x0 + x1) / 2).toFixed(1),
        y: (y0 + 10).toFixed(1),
        "font-size": 9,
        "text-anchor": "middle",
        fill: "var(--ink)",
        "fill-opacity": faded ? 0.3 : 0.9,
        "font-family": "IBM Plex Mono, monospace",
        "paint-order": "stroke",
        stroke: "var(--canvas)",
        "stroke-width": 3.4,
        "stroke-linejoin": "round",
        "pointer-events": "none",
        "font-weight": 600,
      });
      lbl.textContent = doc.F[i].split("/").pop()!;
      g.appendChild(lbl);
    }
  }

  // ---------- transform during gestures, redraw on settle (issue #51) ----------
  // paint() rebuilds the whole SVG, and a drag or pinch calls it on every
  // pointermove. On the corpus maps that's not a rounding error:
  // web/scripts/perf-bench.mjs measured draw() itself past a second on dify
  // (6.3k files) and into the tens of seconds on aws-sdk-go-v2 (26.5k files
  // -- see the PR description for the full numbers). Instead, a gesture
  // moves the already-drawn <g> with an SVG `transform` -- a single
  // attribute write the browser's own compositor handles, not a DOM rebuild
  // -- and only pays for a real paint() once the gesture settles. This is
  // the standard "transform for the interactive frame, re-layout on idle"
  // trick map libraries use; the subtlety here is entirely about not
  // breaking the touch bug fixes above, which is why every call site below
  // goes through gestureFrame() rather than calling preview() or draw()
  // directly.

  /** Move `rootG` to reflect the CURRENT (k, tx, ty) relative to what was
   * actually baked into it at (drawnK, drawnTx, drawnTy): a point already
   * placed at screen position p0 = v*drawnK + drawnTx needs to land at
   * v*k + tx, i.e. p0*r + c with r = k/drawnK and c = tx - drawnTx*r (and
   * the same for ty) -- one translate+scale, applied to the group rather
   * than recomputed per element. Coalesced to at most one write per
   * animation frame: a pinch alone can deliver several pointermove events
   * inside one frame, and only the last matters for what actually paints. */
  private preview() {
    if (this.previewRaf != null) return;
    this.previewRaf = requestAnimationFrame(() => {
      this.previewRaf = null;
      if (!this.rootG) return;
      const r = this.k / this.drawnK;
      const tx = this.tx - this.drawnTx * r;
      const ty = this.ty - this.drawnTy * r;
      this.rootG.setAttribute("transform", `translate(${tx.toFixed(2)} ${ty.toFixed(2)}) scale(${r.toFixed(4)})`);
      // Strokes and text scale with the group during a preview frame --
      // acceptable for something on screen for at most SETTLE_MS.
      // non-scaling-stroke (index.css, scoped to this class) keeps stroke
      // WIDTH constant in device space while that's true, which reads as
      // noticeably less "rubbery" on the thin district/road strokes -- one
      // class toggle here, not a per-element attribute, so it never touches
      // what paint() itself puts in the DOM.
      this.svg.classList.add("previewing");
    });
  }

  /** True once the live (k, tx, ty) has drifted far enough from what's
   * actually painted that a person could notice stale content: blank map at
   * the edge of a pan or a zoom-out (#49's culling drew only near the
   * viewport at paint time), or a zoom-in outrunning the dot density (#48)
   * a fresh paint() would show. See the DRIFT_* fields above for the actual
   * numbers and why the zoom bound is asymmetric. */
  private driftExceeded(): boolean {
    if (!this.rootG) return true;
    const r = this.k / this.drawnK;
    if (r < MapRenderer.DRIFT_ZOOM_LO || r > MapRenderer.DRIFT_ZOOM_HI) return true;
    const dx = Math.abs(this.tx - this.drawnTx);
    const dy = Math.abs(this.ty - this.drawnTy);
    return dx > this.VW * MapRenderer.DRIFT_PAN_FRACTION || dy > this.VH * MapRenderer.DRIFT_PAN_FRACTION;
  }

  /** The one call site every gesture handler (drag, pinch, wheel, and the
   * intermediate frames of glide()) uses instead of draw(). Chooses a
   * transform-only preview() unless the drift bound above says the painted
   * content is going stale, in which case it forces a real paint() -- rate
   * limited to DRIFT_REDRAW_MS so a fast continuous pan pays for at most
   * one rebuild every quarter second instead of one per frame -- and always
   * (re)starts the settle timer that eventually bakes a final paint(). */
  private gestureFrame() {
    const now = performance.now();
    if (this.driftExceeded() && now - this.lastDriftRedraw > MapRenderer.DRIFT_REDRAW_MS) {
      this.lastDriftRedraw = now;
      if (this.previewRaf != null) {
        cancelAnimationFrame(this.previewRaf);
        this.previewRaf = null;
      }
      this.draw();
    } else {
      this.preview();
    }
    this.scheduleSettle();
  }

  private scheduleSettle() {
    if (this.settleTimer != null) clearTimeout(this.settleTimer);
    this.settleTimer = setTimeout(() => {
      this.settleTimer = null;
      this.draw();
    }, MapRenderer.SETTLE_MS);
  }

  /** Cancels whatever gesture-settling work is pending without drawing --
   * used right before a caller is about to draw() itself anyway (glide's
   * final frame, render() on a real state change), so that work never fires
   * a moment later against geometry that a fresh paint() already replaced.
   * destroy() also calls this, for the same reason cancelAnimationFrame is
   * already called there: nothing pending should outlive the renderer. */
  private cancelPendingGestureWork() {
    if (this.settleTimer != null) {
      clearTimeout(this.settleTimer);
      this.settleTimer = null;
    }
    if (this.previewRaf != null) {
      cancelAnimationFrame(this.previewRaf);
      this.previewRaf = null;
    }
  }

  // ---------- pan / zoom / tap (the subtle part) ----------
  private toSvg(ev: PointerEvent | WheelEvent): [number, number] {
    const r = this.svg.getBoundingClientRect();
    return [((ev.clientX - r.left) / r.width) * this.VW, ((ev.clientY - r.top) / r.height) * this.VH];
  }
  private mid(): [number, number] {
    const a = [...this.pts.values()];
    return [(a[0][0] + a[1][0]) / 2, (a[0][1] + a[1][1]) / 2];
  }
  private dist(): number {
    const a = [...this.pts.values()];
    return Math.hypot(a[0][0] - a[1][0], a[0][1] - a[1][1]) || 1;
  }

  private pointerDown(e: PointerEvent) {
    // Issue #51 follow-up: clear a settle timer inherited from a PREVIOUS,
    // already-finished gesture the instant a new pointer sequence starts.
    // Touch click dispatch is not guaranteed to land in the same task as
    // its pointerup, so a stale timer left ticking from before could
    // otherwise fire in the gap between THIS sequence's own pointerup and
    // its click -- deleting the click's target, Bug fix #2 again with a
    // timer as the culprit instead of a synchronous redraw. See endPointer()
    // for the other half: re-arming a fresh one if a real paint is still
    // owed once this sequence ends.
    if (this.settleTimer != null) {
      clearTimeout(this.settleTimer);
      this.settleTimer = null;
    }
    this.pts.set(e.pointerId, this.toSvg(e));
    if (this.pts.size === 2) {
      this.dragging = false;
      this.pinch = { d: this.dist(), k: this.k, m: this.mid() };
      return;
    }
    // Bug fix #1 (see docs/ARCHITECTURE.md / HANDOFF.md): deliberately NOT
    // capturing the pointer here. A captured pointer retargets the
    // subsequent click to the <svg> element itself, so taps would never
    // reach the shape under the finger — capture is taken only once a drag
    // is confirmed, in pointermove below.
    this.dragging = true;
    this.moved = 0;
    [this.lx, this.ly] = this.toSvg(e);
  }

  private pointerMove(e: PointerEvent) {
    if (!this.pts.has(e.pointerId)) return;
    this.pts.set(e.pointerId, this.toSvg(e));
    if (this.pts.size === 2 && this.pinch) {
      const nk = this.clampK(this.pinch.k * (this.dist() / this.pinch.d));
      const m = this.mid();
      this.tx = m[0] - (this.pinch.m[0] - this.tx) * (nk / this.pinch.k);
      this.ty = m[1] - (this.pinch.m[1] - this.ty) * (nk / this.pinch.k);
      this.k = nk;
      // Issue #51: a pinch frame previews (transform) rather than repaints.
      this.gestureFrame();
      return;
    }
    if (!this.dragging) return;
    const [x, y] = this.toSvg(e);
    this.moved += Math.abs(x - this.lx) + Math.abs(y - this.ly);
    if (this.moved < 5) {
      this.lx = x;
      this.ly = y;
      return; // below this it is still a tap
    }
    if (!this.svg.classList.contains("dragging")) {
      this.svg.classList.add("dragging");
      this.callbacks.onDragStart?.();
      try {
        this.svg.setPointerCapture(e.pointerId);
      } catch {
        /* ignore: pointer already released */
      }
    }
    this.tx += x - this.lx;
    this.ty += y - this.ly;
    this.lx = x;
    this.ly = y;
    this.gestureFrame();
  }

  private endPointer(e: PointerEvent) {
    this.pts.delete(e.pointerId);
    if (this.pts.size < 2) this.pinch = null;
    if (this.pts.size === 0) {
      this.dragging = false;
      this.svg.classList.remove("dragging");
      // Bug fix #2: no draw() call here. Redrawing on pointerup would delete
      // the very SVG element the upcoming click event is about to be
      // dispatched to, so the click's `data-k` walk (below) would find
      // nothing — taps would silently stop selecting anything on touch.
      //
      // A settle timer left armed by THIS gesture's own last pointermove is
      // safe for the same reason: it fires later, on its own setTimeout
      // callback, and a timer callback cannot preempt the synchronous
      // pointerup -> click sequence the browser dispatches for this event.
      // A timer inherited from a PREVIOUS, unrelated gesture is not safe --
      // touch click dispatch is not guaranteed to land in that same task,
      // so a stale one could fire in the gap and delete the click's target.
      // pointerDown() clears any inherited timer the moment a new sequence
      // starts, closing that window; if this sequence turns out to be a
      // genuine tap (nothing moved, so no gestureFrame() ever ran to re-arm
      // one), re-arm it here instead, with a fresh SETTLE_MS -- comfortably
      // longer than the gap between this tap's own pointerup and its click,
      // so it can't race that click either, and the map still ends up
      // painting for real whatever preview transform (this gesture's own,
      // or an inherited one) is still outstanding.
      if (this.rootG?.hasAttribute("transform")) this.scheduleSettle();
      if (this.moved < 6 && this.TOUCH) {
        const now = performance.now();
        if (now - this.lastTap < 300) {
          this.zoomBy(2);
          this.lastTap = 0;
          this.tapped = true;
        } else this.lastTap = now;
      }
    }
  }

  private wheel(e: WheelEvent) {
    e.preventDefault();
    const [x, y] = this.toSvg(e);
    const f = Math.exp(-e.deltaY * 0.0016);
    const nk = this.clampK(this.k * f);
    this.tx = x - (x - this.tx) * (nk / this.k);
    this.ty = y - (y - this.ty) * (nk / this.k);
    this.k = nk;
    // Issue #51: a wheel step is a gesture too -- most scroll wheels/trackpads
    // deliver a burst of small deltaY events, so this is the same
    // preview-then-settle treatment as drag and pinch, not a repaint per tick.
    this.gestureFrame();
  }

  // Bug fix #3: ONE delegated click listener reading a `data-k` attribute,
  // rather than a handler bound to each shape. Per-element handlers die on
  // every redraw (draw() clears svg.textContent every frame) and, on a
  // captured pointer, may never fire at all — a data attribute survives
  // both, because the browser resolves the click target itself and we just
  // read it back off whatever element (or its ancestor) is still there.
  private click(e: MouseEvent) {
    if (this.moved >= 6) return; // that was a drag
    if (this.tapped) {
      this.tapped = false;
      return; // that was a double-tap zoom
    }
    let t: Element | null = e.target as Element;
    while (t && t !== this.svg && !t.getAttribute?.("data-k")) t = t.parentNode as Element | null;
    const kk = t && t !== this.svg ? t.getAttribute?.("data-k") : null;
    if (!kk) {
      this.callbacks.onClearSelection();
      return;
    }
    const parts = kk.split(":");
    if (parts[0] === "d") this.callbacks.onSelectDistrict(+parts[1]);
    else if (parts[0] === "f") this.callbacks.onSelectFile(+parts[1]);
    else if (parts[0] === "s") this.callbacks.onSelectSymbol(+parts[1], +parts[2]);
    else if (parts[0] === "sd") this.callbacks.onSelectSubdistrict(+parts[1], +parts[2]);
    else if (parts[0] === "p") this.callbacks.onSelectParcel(+parts[1], +parts[2]);
  }
}
