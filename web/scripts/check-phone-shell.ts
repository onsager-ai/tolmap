#!/usr/bin/env -S npx tsx
// Unit checks for the phone shell's pure parts (docs/UX.md §3, phase 2):
//   - map/backStack.ts: the back-stack reducer (§3.4's pop order, dead
//     entries, reloads);
//   - map/phoneShell.ts: sheet detents (§3.1), snap from position and
//     velocity, the safe rectangle from the chrome's insets (§3.3), and who
//     owns a drag on the sheet (dragOwner: the sheet or its content);
//   - map/geometry.ts: fitViewport/scaleToFit framing into real insets.
// No browser: the same standalone-script pattern as check-gestures.ts (no
// test runner exists in this project -- web/README.md). CI runs it in the
// `web build and lint` job; the browser side (a real back button, a real
// drag) is check-view-stability.mjs's.
//
// Run: npx tsx web/scripts/check-phone-shell.ts

import { closeOverlay, hasLiveOverlay, initBack, isOverlayKind, openOverlay, popTo, type BackState, type OverlayKind } from "../src/map/backStack";
import {
  DETENT_ORDER,
  PEEK_BASE_PX,
  SHEET_FLICK_PX_PER_MS,
  SHEET_UNDERSHOOT_PX,
  detentHeights,
  dragHeight,
  dragOwner,
  nextDetent,
  pillBottom,
  releaseVelocity,
  safeInsets,
  safeRect,
  snapDetent,
  type DragStart,
  type PhoneMetrics,
} from "../src/map/phoneShell";
import { DESKTOP_INSETS, defaultInsets, fitCentreY, fitViewport, scaleToFit } from "../src/map/geometry";

let failures = 0;
let checks = 0;

function report(ok: boolean, label: string, detail?: string) {
  checks++;
  if (ok) console.log(`  ok    ${label}`);
  else {
    failures++;
    console.log(`  FAIL  ${label}${detail ? " -- " + detail : ""}`);
  }
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// ---------------------------------------------------------------- detents
console.log("\nsheet detents (§3.1)");
const phone: PhoneMetrics = { width: 390, height: 844, safeTop: 0, safeBottom: 0 };
const notched: PhoneMetrics = { width: 390, height: 844, safeTop: 47, safeBottom: 34 };
const small: PhoneMetrics = { width: 320, height: 568, safeTop: 0, safeBottom: 0 };
const mid: PhoneMetrics = { width: 360, height: 640, safeTop: 0, safeBottom: 0 };

{
  const h = detentHeights(phone);
  report(eq(h, { peek: 156, half: 480, full: 776 }), "390x844: peek 156, half 480, full = H - (pill bottom 60 + 8)", JSON.stringify(h));
  const n = detentHeights(notched);
  report(n.peek === PEEK_BASE_PX + 34 && n.peek === 190, "peek adds the bottom safe inset (190 on a notched 390x844, the spec's own number)", JSON.stringify(n));
  report(n.full === 844 - (47 + 12 + 48 + 8), "full stops 8 px under the pill, which sits below the top safe inset", JSON.stringify(n));
  const s = detentHeights(small);
  report(s.half === Math.round(568 * 0.57) && s.half < 480, "320x568: half is 57% of the visible height when that is under 480", JSON.stringify(s));
  report(s.peek < s.half && s.half < s.full, "320x568: peek < half < full", JSON.stringify(s));
  const m = detentHeights(mid);
  report(m.half === Math.round(640 * 0.57) && m.full === 640 - 68, "360x640: half and full from the visible height", JSON.stringify(m));
  const tiny = detentHeights({ width: 320, height: 250, safeTop: 0, safeBottom: 0 });
  report(tiny.peek <= tiny.half && tiny.half <= tiny.full, "a very short viewport never inverts the detent order", JSON.stringify(tiny));
  const keyboard = detentHeights({ ...phone, height: 500 });
  report(keyboard.full === 500 - 68 && keyboard.peek === 156, "heights follow the visible height (the keyboard's visualViewport), not the layout viewport", JSON.stringify(keyboard));
}

console.log("\ngrabber cycle (§3.1)");
report(nextDetent("peek") === "half" && nextDetent("half") === "full" && nextDetent("full") === "peek", "grabber tap cycles peek -> half -> full -> peek");

// ---------------------------------------------------------------- snap
console.log("\nsnap from position and velocity");
{
  const h = detentHeights(phone); // 156 / 480 / 776
  const slow = SHEET_FLICK_PX_PER_MS / 2;
  const fast = SHEET_FLICK_PX_PER_MS * 2;
  report(snapDetent(300, 0, h) === "peek", "a slow release nearer Peek settles at Peek");
  report(snapDetent(340, 0, h) === "half", "a slow release nearer Half settles at Half");
  report(snapDetent(700, slow, h) === "full", "a slow release (below flick speed) goes to the nearest detent");
  report(snapDetent(200, -fast, h) === "half", "a flick up from just above Peek goes to Half, not back to Peek");
  report(snapDetent(470, -fast, h) === "half", "a flick up from just below Half lands on Half (the nearest detent above)");
  report(snapDetent(500, -fast, h) === "full", "a flick up from just above Half goes to Full");
  report(snapDetent(760, fast, h) === "half", "a flick down from just below Full goes to Half");
  report(snapDetent(460, fast, h) === "peek", "a flick down from just below Half goes to Peek");
  report(snapDetent(776, -fast, h) === "full" && snapDetent(156, fast, h) === "peek", "a flick past the ends stays at Full / Peek");
  report(snapDetent(130, 0, h) === "peek" && snapDetent(130, fast, h) === "peek", "dragged below Peek springs back to Peek (§3.5)");
  report(dragHeight(156, 200, h) === 156 - SHEET_UNDERSHOOT_PX, "a drag down clamps at Peek minus the undershoot");
  report(dragHeight(480, -1000, h) === 776, "a drag up clamps at Full");
  report(dragHeight(480, -100, h) === 580, "a drag follows the finger (up = taller)");
  const v = releaseVelocity([
    [0, 500],
    [10, 490],
    [20, 470],
  ]);
  report(Math.abs(v - -1.5) < 1e-9, "release velocity is px/ms over the kept samples, negative when moving up", String(v));
  report(releaseVelocity([[0, 1]]) === 0, "one sample has no velocity");
}

// ---------------------------------------------------------------- safe rect
console.log("\nsafe rectangle from the insets (§3.3)");
{
  const h = detentHeights(phone);
  report(pillBottom(phone) === 60 && pillBottom(notched) === 107, "pill bottom = safe top + 12 + 48");
  const peek = safeRect(phone, h.peek);
  report(eq(peek, { left: 12, top: 68, right: 390 - 12 - 44 - 8, bottom: 844 - 156 - 8 }), "390x844 at Peek: [12, 68, W - 64, H - 164]", JSON.stringify(peek));
  const half = safeRect(phone, h.half);
  report(half.bottom === 844 - 480 - 8 && half.top === peek.top && half.left === peek.left && half.right === peek.right, "raising the sheet moves only the bottom edge", JSON.stringify(half));
  const ni = safeInsets(notched, detentHeights(notched).peek);
  report(eq(ni, { left: 12, top: 47 + 60 + 8, right: 64, bottom: 190 + 8 }), "a notched phone's insets include both safe areas", JSON.stringify(ni));
  // fitViewport frames into exactly those insets.
  const rect = fitViewport(390, 844, safeInsets(phone, h.peek));
  report(eq(rect, [12, 68, 326, 680]), "fitViewport(insets) is the safe rect, not a width-keyed constant", JSON.stringify(rect));
  report(fitCentreY(390, 844, 100, safeInsets(phone, h.peek)) === (68 + 680) / 2, "a fit centres vertically in the safe rect");
  const b: [number, number, number, number] = [0, 0, 1, 1];
  const sPeek = scaleToFit(b, 390, 844, safeInsets(phone, h.peek));
  const sHalf = scaleToFit(b, 390, 844, safeInsets(phone, h.half));
  report(sPeek === Math.min(326 - 12, 680 - 68) && sHalf <= sPeek, "the fit scale shrinks when the sheet rises (the map stays above it)", `${sPeek} ${sHalf}`);
  report(eq(fitViewport(1200, 800, DESKTOP_INSETS), [24, 12, 1176, 762]) && eq(defaultInsets(1200), DESKTOP_INSETS), "desktop keeps its margins (phase 5 realigns them)");
}

// ---------------------------------------------------------------- back stack
console.log("\nback-stack reducer (§3.4)");
{
  // One scripted session: a history of entries, a `go(-1)` that pops.
  function session(startIndex = 0, marker: unknown = null, hasSel = false) {
    let state: BackState = initBack(startIndex, marker, hasSel);
    let index = startIndex;
    const markers = new Map<number, OverlayKind>();
    const log: string[] = [];
    return {
      get state() {
        return state;
      },
      get index() {
        return index;
      },
      open(kind: OverlayKind) {
        const r = openOverlay(state, kind, index);
        state = r.state;
        if (r.push) {
          index++;
          markers.set(index, kind);
          log.push(`push:${kind}`);
        }
        return r.push;
      },
      close(kind: OverlayKind) {
        state = closeOverlay(state, kind);
      },
      /** Browser back; follows skipBack like MapView does. Returns what was closed. */
      back(hasSel: boolean): OverlayKind[] {
        const closed: OverlayKind[] = [];
        for (;;) {
          index--;
          const r = popTo(state, index, markers.get(index), hasSel);
          state = r.state;
          closed.push(...r.undo);
          if (!r.skipBack) break;
        }
        return closed;
      },
    };
  }

  const s = session();
  s.open("sel");
  s.open("sheet");
  s.open("layers");
  report(s.index === 3 && hasLiveOverlay(s.state), "selection, raised sheet and layers each push one entry");
  report(s.open("layers") === false && s.index === 3, "re-opening an open overlay pushes nothing");
  report(eq(s.back(true), ["layers"]), "back 1: closes the layers sheet");
  report(eq(s.back(true), ["sheet"]), "back 2: returns the sheet to Peek");
  report(eq(s.back(true), ["sel"]), "back 3: clears the selection");
  report(s.index === 0 && !hasLiveOverlay(s.state), "back 4 would leave the map: nothing is left open", JSON.stringify(s.state));

  const t = session();
  t.open("sel");
  t.open("sheet");
  t.open("search");
  report(eq(t.back(true), ["search"]) && eq(t.back(true), ["sheet"]) && eq(t.back(true), ["sel"]), "search, then sheet height, then selection");

  // The repository sheet (owner, 2026-09-28): one entry, closed first.
  const rp = session();
  rp.open("sel");
  rp.open("sheet");
  report(rp.open("repos") === true && rp.index === 3, "opening the repository sheet pushes one entry");
  report(rp.open("repos") === false && rp.index === 3, "re-opening it pushes nothing");
  report(eq(rp.back(true), ["repos"]), "back 1: closes the repository sheet only");
  report(eq(rp.back(true), ["sheet"]) && eq(rp.back(true), ["sel"]), "then the sheet height, then the selection");
  report(rp.index === 0 && !hasLiveOverlay(rp.state), "then back leaves the map");
  const rl = session();
  rl.open("layers");
  rl.close("layers");
  rl.open("repos");
  report(eq(rl.back(false), ["repos"]) && rl.index === 1, "back closes the repository sheet before any other layer, and stops there", `index=${rl.index}`);
  const rc = session();
  rc.open("sel");
  rc.open("repos");
  rc.close("repos"); // closed from its button or the scrim
  report(eq(rc.back(true), ["sel"]) && rc.index === 0, "a repository sheet closed from the UI costs no back press", `index=${rc.index}`);
  const rr = popTo(initBack(3, "repos", true), 2, "sel", true);
  report(rr.undo.length === 0 && rr.skipBack, "a reload on a repository-sheet entry does not reopen it; back carries on past it");
  report(isOverlayKind("repos"), "\"repos\" is a marker the page recognises after a reload");

  // Closed from the UI: the entry stays, dead, and back skips it.
  const u = session();
  u.open("sel");
  u.open("search");
  u.close("search"); // picked a result
  report(!u.state.entries.find((e) => e.kind === "search")!.live, "an overlay closed from the UI leaves a dead entry, not a history move");
  report(eq(u.back(true), ["sel"]) && u.index === 0, "back passes over the dead search entry and clears the selection in the same press", `index=${u.index}`);
  const u2 = session();
  u2.open("sel");
  u2.open("sheet");
  u2.close("sheet"); // dragged back to Peek
  const closed = u2.back(true);
  report(eq(closed, ["sel"]) && u2.index === 0, "back landing on a dead entry goes back again: one press clears the selection", `${JSON.stringify(closed)} index=${u2.index}`);
  const u3 = session();
  u3.open("sel");
  u3.open("sheet");
  u3.close("sheet");
  u3.close("sel"); // tapped empty map
  const nothing = u3.back(false);
  report(nothing.length === 0 && u3.index === -1, "every overlay closed from the UI: one back skips them all and leaves the map", `index=${u3.index}`);

  // Opening after a back discards the forward entries.
  const v = session();
  v.open("sel");
  v.open("sheet");
  v.back(true);
  v.open("layers");
  report(eq(v.state.entries.map((e) => `${e.kind}@${e.index}`), ["sel@1", "layers@2"]), "a push after a back drops the forward entry it replaces", JSON.stringify(v.state.entries));

  // Reloads and deep links: overlays never survive a reload.
  const r1 = initBack(4, "sel", true);
  report(r1.entries.length === 1 && r1.entries[0].live, "a reload on a selection entry adopts it as live (back will clear the selection)");
  const r2 = initBack(4, "layers", true);
  report(r2.entries.length === 1 && !r2.entries[0].live, "a reload on a layers entry does not reopen layers: the entry is dead");
  const r3 = initBack(4, null, true);
  report(r3.entries.length === 0, "a fresh deep link starts with an empty stack (MapView then pushes its selection entry)");
  const r4 = popTo(initBack(5, "sheet", true), 4, "sel", true);
  report(r4.undo.length === 0 && r4.skipBack && r4.state.entries.some((e) => e.kind === "sel" && e.live), "back from a reloaded overlay entry adopts the selection entry below it and carries on to it");
  const r4b = popTo(r4.state, 3, undefined, true);
  report(eq(r4b.undo, ["sel"]), "...so the same press clears the selection");
  const r5 = popTo(initBack(5, null, false), 4, "search", false);
  report(r5.skipBack, "back onto an untracked overlay entry (from before a reload) skips it");
  const r6 = popTo(initBack(3, null, false), 4, "layers", false);
  report(!r6.skipBack && r6.undo.length === 0, "forward onto an overlay entry does not reopen it and does not bounce back");
  const r7 = popTo(initBack(1, "sel", true), 0, undefined, true);
  report(eq(r7.undo, ["sel"]) && !r7.skipBack, "back off the selection entry closes it and stays (the base entry is not ours)");
}

// ---------------------------------------------------------------- drag owner
// Owner, 2026-09-28: "when details opened, unable to drag down to collapse
// because of scrolling". Every combination of detent, scroll position,
// direction and start zone, against the standard bottom-sheet rule.
console.log("\nwho owns a drag on the sheet (dragOwner)");
{
  const expected = (s: DragStart): "sheet" | "content" => {
    if (s.zone === "handle") return "sheet"; // the grabber and header never scroll
    if (s.detent === "peek") return "sheet"; // Peek never scrolls (§3.1)
    if (!s.canScroll) return "sheet"; // nothing to scroll
    if (s.direction === "down") return s.scrollTop <= 0 ? "sheet" : "content";
    return s.detent === "full" ? "content" : "sheet"; // up: raise the sheet first
  };
  let all = 0;
  let agree = 0;
  const mismatches: string[] = [];
  for (const detent of DETENT_ORDER)
    for (const scrollTop of [-12, 0, 1, 240])
      for (const canScroll of [false, true])
        for (const direction of ["up", "down"] as const)
          for (const zone of ["handle", "content"] as const) {
            if (!canScroll && scrollTop > 0) continue; // cannot be scrolled if it cannot scroll
            const s: DragStart = { detent, scrollTop, canScroll, direction, zone };
            all++;
            if (dragOwner(s) === expected(s)) agree++;
            else mismatches.push(JSON.stringify(s));
          }
  report(agree === all && all === 3 * (2 * 2 * 2 + 4 * 2 * 2), `every combination (${all}) follows the rule`, mismatches.join(" "));

  const at = (detent: DragStart["detent"], scrollTop: number, direction: DragStart["direction"], zone: DragStart["zone"] = "content", canScroll = true) =>
    dragOwner({ detent, scrollTop, canScroll, direction, zone });
  report(at("full", 0, "down") === "sheet", "Full, content at its top, drag down: the sheet lowers (the owner's report)");
  report(at("full", -8, "down") === "sheet", "Full, content bouncing past its top (iOS negative scrollTop), drag down: the sheet");
  report(at("full", 1, "down") === "content", "Full, content scrolled even 1 px, drag down: the content scrolls toward its top");
  report(at("full", 400, "down") === "content", "Full, content mid-scroll, drag down: the content scrolls (the next drag moves the sheet)");
  report(at("full", 0, "up") === "content" && at("full", 400, "up") === "content", "Full, drag up: the content scrolls, from the top or mid-scroll");
  report(at("half", 0, "up") === "sheet" && at("half", 120, "up") === "sheet", "below Full, drag up: the sheet rises first, whatever the scroll");
  report(at("half", 0, "down") === "sheet", "Half, content at its top, drag down: the sheet lowers");
  report(at("half", 120, "down") === "content", "Half, a scroller under the finger mid-scroll, drag down: it scrolls");
  report(at("peek", 120, "up") === "sheet" && at("peek", 120, "down") === "sheet", "Peek: every drag moves the sheet");
  report(
    at("full", 400, "down", "handle") === "sheet" && at("full", 400, "up", "handle") === "sheet" && at("half", 0, "up", "handle") === "sheet",
    "the grabber and the card header always move the sheet, never scroll",
  );
  report(at("full", 0, "up", "content", false) === "sheet" && at("full", 0, "down", "content", false) === "sheet", "content that cannot scroll: the sheet owns every drag");
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exitCode = 1;
}
