import type { LensComparison } from "@bindings/LensComparison";
import type { LensReport } from "@bindings/LensReport";
import type { MapDocument } from "@/types";
import { WHY_COLOR } from "@/lib/cardText";
import { KIND } from "./constants";
import { MOSTLY_SHARE_THRESHOLD } from "./districtIndex";
import { symbolsOf } from "./geometry";
import type { AdjMap } from "./graph";
import { formatDirectory } from "./packageLayout";
import type { RoadPlan } from "./roads";
import type { CrossFlow, StreetFlow } from "./streets";
import { KIND_NAMES, rollReferences, rowKind, rowName, type DecodedDistrictSymbols } from "./symbolCards";

/** One run of a hover card's subline: plain text, mono data, or the file
 * card's landmark badge (`badge` is its colour). `field` names the value
 * for check:view. */
type HoverSpan = string | { text: string; mono?: boolean; badge?: string; field?: string };
/** docs/UX.md §5 linked hover, as the prototype draws it: a title line
 * (mono when it is a path or a symbol) over quiet sans sublines. */
export interface HoverCardContent {
  title: string;
  titleMono?: boolean;
  rows: HoverSpan[][];
}
/** The card as plain lines, title first: the phone sheet's structure card
 * (onTapStructure) shows the same words as text. */
export function cardLines(content: HoverCardContent): string[] {
  return [content.title, ...content.rows.map((row) => row.map((span) => (typeof span === "string" ? span : span.text)).join(""))];
}
/** A card that is only lines of text: the first is the title. */
function plainCard(lines: string[], titleMono = false): HoverCardContent {
  return { title: lines[0], titleMono, rows: lines.slice(1).map((line) => [line]) };
}
export interface HoverContentContext {
  readonly doc: MapDocument;
  readonly lens?: LensReport;
  readonly lensComparison?: LensComparison;
  readonly districtPaths: ReadonlyMap<number, readonly { path: string | null; share: number; other: boolean }[]>;
  readonly roadPlan: readonly RoadPlan[];
  readonly streetCache: { readonly intra: readonly StreetFlow[]; readonly cross: readonly CrossFlow[] } | null;
  readonly inAdj: Readonly<AdjMap>;
  readonly symLocalByGlobal: ReadonlyMap<number, { decoded: DecodedDistrictSymbols; local: number }>;
  readonly symVisible: ReadonlySet<number>;
}

/** Content for the hover card, by data-k prefix -- the same five kinds
 * click() dispatches on. Returns null for a key whose target this paint
 * doesn't actually have data for (e.g. a stale key from just before a
 * document swap); callers treat that as "nothing to show." */
export function hoverContent(key: string, context: HoverContentContext): HoverCardContent | null {
  const { doc } = context;
  const parts = key.split(":");
  if (parts[0] === "lc") {
    const change = context.lensComparison?.changes[+parts[1]];
    return change ? plainCard([`${change.status} ${change.kind}`, change.before.join(" → "), change.after.join(" → "), "Open base stop card"], true) : null;
  }
  if (parts[0] === "lh") {
    const path = context.lens?.paths[+parts[1]];
    const hop = path?.hops[+parts[2]];
    return hop && path ? plainCard([path.name, hop.source === "declared" ? "Declared hop" : `Computed · ${hop.source} · ${hop.edge_kind}`, hop.reason ?? "Lower bound from available map edges"]) : null;
  }
  if (parts[0] === "ln") {
    const note = context.lens?.notes[+parts[1]];
    return note ? plainCard([note.anchor, note.text]) : null;
  }
  if (parts[0] === "ls") {
    const stop = context.lens?.paths[+parts[1]]?.stops[+parts[2]];
    return stop ? plainCard([stop.spec, "Open stop card · Read selected source"], true) : null;
  }
  if (parts[0] === "d") {
    const d = +parts[1];
    const district = doc.districts[String(d)];
    if (!district) return null;
    const largestFolder = context.districtPaths.get(d)?.find((row) => !row.other);
    const mostly = largestFolder && largestFolder.share >= MOSTLY_SHARE_THRESHOLD
      ? formatDirectory(largestFolder.path ?? ".")
      : null;
    // §5 linked hover: name, files, mostly-folder -- the prototype's title
    // line and one quiet subline, mono only for the data in it.
    return {
      title: doc.names[String(d)] ?? `District ${d}`,
      rows: [[
        { text: String(district.size), mono: true, field: "files" },
        ` ${district.size === 1 ? "file" : "files"} · `,
        ...(mostly ? ["mostly ", { text: mostly, mono: true, field: "mostly" }] : [{ text: "mixed folders", field: "mostly" }]),
      ]],
    };
  }
  if (parts[0] === "dir") return plainCard([key.slice(4) + "/", "folder highlight"], true);
  if (parts[0] === "r") {
    // A3 (road hover/tap card, issue #82): "A -> B: n imports · B -> A: m
    // imports" -- read straight off this.roadPlan (built once per
    // document, loadDocument) rather than re-aggregating doc.E, so this
    // can never disagree with the ribbon's own width/chevron-direction
    // math (drawRoads), which reads the exact same plan entries.
    const a = +parts[1];
    const b = +parts[2];
    const plan = context.roadPlan.find((p) => p.a === a && p.b === b);
    if (!plan) return null;
    const nameA = doc.names[String(a)] ?? `district ${a}`;
    const nameB = doc.names[String(b)] ?? `district ${b}`;
    return plainCard([`${nameA} ↔ ${nameB}`, `${nameA} → ${nameB}: ${plan.ab} imports · ${nameB} → ${nameA}: ${plan.ba} imports`]);
  }
  // B4 scope item 4: streets. Read straight off the currently-cached
  // street plan (getStreetPlan) rather than re-aggregating doc.E, for the
  // same reason the "r" branch above reads roadPlan -- can't disagree with
  // what drawStreets actually drew.
  if (parts[0] === "st" && parts[1] === "n") {
    const a = parts[2];
    const b = parts[3];
    const plan = context.streetCache?.intra.find((p) => (p.a === a && p.b === b) || (p.a === b && p.b === a));
    if (!plan) return null;
    const nameA = doc.neighbourhoods?.[a]?.label ?? a;
    const nameB = doc.neighbourhoods?.[b]?.label ?? b;
    return plainCard([`${nameA} ↔ ${nameB}`, `${nameA} → ${nameB}: ${plan.ab} imports · ${nameB} → ${nameA}: ${plan.ba} imports`]);
  }
  if (parts[0] === "st" && parts[1] === "c") {
    const n = parts[2];
    const d = +parts[3];
    const plan = context.streetCache?.cross.find((p) => p.n === n && p.d === d);
    if (!plan) return null;
    const nameN = doc.neighbourhoods?.[n]?.label ?? n;
    const nameD = doc.names[String(d)] ?? `district ${d}`;
    return plainCard([`${nameN} ↔ ${nameD}`, `${nameN} → ${nameD}: ${plan.out} imports · ${nameD} → ${nameN}: ${plan.in} imports`]);
  }
  if (parts[0] === "n") {
    const n = doc.neighbourhoods?.[parts[1]];
    if (!n) return null;
    return plainCard([n.label, `${n.size} files`]);
  }
  if (parts[0] === "f") {
    const i = +parts[1];
    const filePath = doc.F[i];
    if (!filePath) return null;
    const inDeg = context.inAdj.get(i)?.length ?? 0;
    const slash = filePath.lastIndexOf("/");
    const name = slash < 0 ? filePath : filePath.slice(slash + 1);
    const path = slash < 0 ? "(repo root)" : filePath.slice(0, slash);
    const landmark = doc.L.find(([file]) => file === i)?.[1];
    // §5 linked hover: name, path, imported-by, landmark. The landmark is
    // the file card's own badge; a file that is not one shows none.
    return {
      title: name,
      titleMono: true,
      rows: [
        [{ text: path, mono: true, field: "path" }],
        [
          "imported by ",
          { text: String(inDeg), mono: true, field: "imported-by" },
          ` ${inDeg === 1 ? "file" : "files"}`,
          ...(landmark ? [{ text: landmark, badge: WHY_COLOR[landmark] ?? "var(--dim)", field: "landmark" }] : []),
        ],
      ],
    };
  }
  if (parts[0] === "s") {
    const i = +parts[1];
    const s = +parts[2];
    const sm = symbolsOf(doc, i)[s];
    if (!sm) return null;
    return plainCard([sm[0], KIND[sm[1]] ?? "symbol", `lines ${sm[2]}-${sm[3]}`], true);
  }
  // Issue #82 C2: a symbol card. `global` is the DistrictSymbols'
  // "symbol_indices" space (docs/API.md), not the "s:" branch's file-local
  // parity-constrained `S` index just above -- two different index spaces
  // sharing the map's data-k convention by an unrelated prefix ("hs" vs
  // "s") for exactly that reason.
  if (parts[0] === "hs") {
    const global = +parts[1];
    const entry = context.symLocalByGlobal.get(global);
    if (!entry) return null;
    const { decoded, local } = entry;
    const row = decoded.raw.symbols[local];
    const memberCount = decoded.children[local].length;
    const { out, in: inn } = rollReferences(decoded, local, context.symVisible);
    const outTotal = [...out.values()].reduce((a, b) => a + b, 0);
    const inTotal = [...inn.values()].reduce((a, b) => a + b, 0);
    return plainCard([
      rowName(row),
      `${KIND_NAMES[rowKind(row)] ?? "symbol"}${memberCount ? ` · ${memberCount} members` : ""}`,
      `lines ${row[3]}-${row[4]}`,
      `refs out ${outTotal} (${out.size} places) · in ${inTotal} (${inn.size} places)`,
    ], true);
  }
  return null;
}

export class HoverCard {
  private hoverCard: HTMLDivElement | null = null;

  private readonly svg: SVGSVGElement;
  private readonly HOVER: boolean;

  constructor(svg: SVGSVGElement, HOVER: boolean) {
    this.svg = svg;
    this.HOVER = HOVER;
  }

  destroy() {
    this.hoverCard?.remove();
    this.hoverCard = null;
  }

  private ensureCard(): HTMLDivElement {
    if (this.hoverCard) return this.hoverCard;
    const card = document.createElement("div");
    card.className = "tolmap-hover-card glass";
    (this.svg.parentElement ?? this.svg.ownerDocument!.body).appendChild(card);
    this.hoverCard = card;
    return card;
  }

  /** Replaces the SVG's native <title> tooltip on fine pointers (spec).
   * `<title>` elements stay in the DOM either way -- removing them would
   * cost the aria-label a screen reader / keyboard user gets from the same
   * markup on a device this class never activates on -- this card is
   * purely an additional, visual, mouse-only affordance layered on top. */
  showCard(content: HoverCardContent) {
    if (!this.HOVER) return;
    const card = this.ensureCard();
    const title = document.createElement("div");
    title.className = content.titleMono ? "tolmap-hover-card-title mono" : "tolmap-hover-card-title";
    title.textContent = content.title;
    card.replaceChildren(
      title,
      ...content.rows.map((spans) => {
        const row = document.createElement("div");
        row.className = "tolmap-hover-card-sub";
        for (const span of spans) {
          if (typeof span === "string") {
            row.appendChild(document.createTextNode(span));
            continue;
          }
          const node = document.createElement("span");
          node.textContent = span.text;
          if (span.field) node.dataset.hoverField = span.field;
          if (span.badge) {
            // The file card's landmark badge (SheetCards' file facts): the
            // landmark's colour on a 16% tint of itself.
            node.className = "tolmap-hover-card-badge";
            node.style.color = span.badge;
            node.style.background = `color-mix(in srgb, ${span.badge} 16%, transparent)`;
          } else if (span.mono) {
            node.className = "mono";
          }
          row.appendChild(node);
        }
        return row;
      }),
    );
    card.style.display = "block";
  }

  hideCard() {
    if (this.hoverCard) this.hoverCard.style.display = "none";
  }

  positionCard(clientX: number, clientY: number, target: Element, keyElements: ReadonlyMap<string, Element[]>) {
    if (!this.hoverCard || this.hoverCard.style.display === "none") return;
    const host = this.svg.parentElement;
    const origin = host ? host.getBoundingClientRect() : this.svg.getBoundingClientRect();
    const width = host?.clientWidth ?? origin.width;
    const height = host?.clientHeight ?? origin.height;
    const inspector = host?.closest("[data-desktop-shell]")?.querySelector<HTMLElement>('[data-desktop-panel][aria-hidden="false"]');
    const inspectorBox = inspector?.getBoundingClientRect();
    const leftEdge = inspectorBox ? Math.max(8, inspectorBox.right - origin.left + 8) : 8;
    this.hoverCard.style.maxWidth = `${Math.min(300, width - leftEdge - 8)}px`;
    const cardW = this.hoverCard.offsetWidth;
    const cardH = this.hoverCard.offsetHeight;
    const px = clientX - origin.left;
    const py = clientY - origin.top;
    const gap = 16;
    const margin = 8;
    // A district's name and its "N files" subtitle are one placed unit
    // (7d); hovering either keeps the card clear of both, not only of the
    // line under the pointer (the card used to clip the subtitle's end).
    let targetBox = target.getBoundingClientRect();
    const key = target.closest("[data-k]")?.getAttribute("data-k");
    if (target instanceof SVGTextElement && key?.startsWith("d:")) {
      let [l, t, r, b] = [targetBox.left, targetBox.top, targetBox.right, targetBox.bottom];
      for (const element of keyElements.get(key) ?? []) {
        if (!(element instanceof SVGTextElement)) continue;
        const box = element.getBoundingClientRect();
        if (box.width === 0 && box.height === 0) continue;
        [l, t, r, b] = [Math.min(l, box.left), Math.min(t, box.top), Math.max(r, box.right), Math.max(b, box.bottom)];
      }
      targetBox = new DOMRect(l, t, r - l, b - t);
    }
    const tx = targetBox.left - origin.left;
    const ty = targetBox.top - origin.top;
    const tr = tx + targetBox.width;
    const tb = ty + targetBox.height;
    const candidates: [number, number][] = [
      [tr + gap, Math.max(margin, Math.min(height - cardH - margin, py - cardH / 2))],
      [tx - gap - cardW, Math.max(margin, Math.min(height - cardH - margin, py - cardH / 2))],
      [Math.max(leftEdge, Math.min(width - cardW - margin, px - cardW / 2)), tb + gap],
      [Math.max(leftEdge, Math.min(width - cardW - margin, px - cardW / 2)), ty - gap - cardH],
      [px + gap, py + gap],
      [px - gap - cardW, py + gap],
      [px + gap, py - gap - cardH],
      [px - gap - cardW, py - gap - cardH],
    ];
    const fits = (x: number, y: number) => x >= leftEdge && y >= margin && x + cardW <= width - margin && y + cardH <= height - margin;
    const clearOfPointer = (x: number, y: number) => !(px >= x && px <= x + cardW && py >= y && py <= y + cardH);
    const clearOfTarget = (x: number, y: number) => x + cardW <= tx || x >= tr || y + cardH <= ty || y >= tb;
    let [x, y] = candidates.find(([cx, cy]) => fits(cx, cy) && clearOfPointer(cx, cy) && clearOfTarget(cx, cy))
      ?? candidates.find(([cx, cy]) => fits(cx, cy) && clearOfPointer(cx, cy))
      ?? candidates[0];
    x = Math.max(leftEdge, Math.min(width - cardW - margin, x));
    y = Math.max(margin, Math.min(height - cardH - margin, y));
    if (!clearOfPointer(x, y)) {
      const left = px - gap - cardW;
      const right = px + gap;
      if (left >= leftEdge) x = left;
      else if (right + cardW <= width - margin) x = right;
      const above = py - gap - cardH;
      const below = py + gap;
      if (above >= margin) y = above;
      else if (below + cardH <= height - margin) y = below;
    }
    this.hoverCard.style.left = `${Math.round(x)}px`;
    this.hoverCard.style.top = `${Math.round(y)}px`;
  }

}
