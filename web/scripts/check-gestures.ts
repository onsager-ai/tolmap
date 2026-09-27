#!/usr/bin/env -S npx tsx
// Unit checks for map/gestures.ts (docs/UX.md §7.1 rules 1-3) and
// map/renderGate.ts (rule 4's pure queue). Replaces CDP-driven pinches and
// double-taps as the check for gesture logic: check-view-stability.mjs's
// zoomIn() comment records why scripted touch input can't exercise these
// rules in headless Chromium (a double-tap's second pointerdown arrived over
// a second late; a two-finger move arrived as two half-updated dispatches).
//
// Run: npx tsx web/scripts/check-gestures.ts
// (no test runner exists in this project -- web/README.md -- so this is a
// standalone script, the same pattern as check-pinch-math.ts and
// check-reference-coverage.ts; CI runs it in the `web build and lint` job.)

import {
  DOUBLE_TAP_MS,
  DOUBLE_TAP_PX,
  GestureRecognizer,
  TAP_MAX_MS,
  TAP_SLOP_MOUSE_PX,
  TAP_SLOP_TOUCH_PX,
  type GestureEvent,
  type GestureIntent,
} from "../src/map/gestures";
import { RenderGate } from "../src/map/renderGate";

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

type Kind = GestureIntent["kind"];

/** A scripted pointer driver: every call feeds one event and returns the
 * intents it produced; `all` accumulates every intent of the script. */
function driver() {
  const g = new GestureRecognizer();
  const all: GestureIntent[] = [];
  let t = 1000;
  const feed = (type: GestureEvent["type"], id: number, x: number, y: number, opts: { pointerType?: string; isPrimary?: boolean; dt?: number } = {}) => {
    t += opts.dt ?? 16;
    const out = g.handle({ type, id, x, y, t, pointerType: opts.pointerType ?? "touch", isPrimary: opts.isPrimary ?? id === 1 });
    all.push(...out);
    return out;
  };
  return {
    g,
    all,
    down: (id: number, x: number, y: number, opts?: { pointerType?: string; isPrimary?: boolean; dt?: number }) => feed("down", id, x, y, opts),
    move: (id: number, x: number, y: number, opts?: { pointerType?: string; isPrimary?: boolean; dt?: number }) => feed("move", id, x, y, opts),
    up: (id: number, x: number, y: number, opts?: { pointerType?: string; isPrimary?: boolean; dt?: number }) => feed("up", id, x, y, opts),
    cancel: (id: number, opts?: { pointerType?: string; dt?: number }) => feed("cancel", id, 0, 0, opts),
    lost: (id: number, opts?: { pointerType?: string; dt?: number }) => feed("lostcapture", id, 0, 0, opts),
    kinds: (list: GestureIntent[] = all) => list.map((i) => i.kind),
  };
}

const has = (list: GestureIntent[], kind: Kind) => list.some((i) => i.kind === kind);
const fmt = (list: GestureIntent[]) => JSON.stringify(list);

// ---------------------------------------------------------------- rule 1: tap slop

{
  const d = driver();
  d.down(1, 100, 100);
  d.move(1, 106, 108); // 10 px straight-line: still inside the touch slop
  const out = d.up(1, 106, 108);
  report(has(out, "tap") && !has(d.all, "pan"), `touch: ${TAP_SLOP_TOUCH_PX} px from pointerdown is still a tap`, fmt(d.all));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.move(1, 110.5, 100);
  const out = d.up(1, 110.5, 100);
  report(!has(out, "tap") && has(d.all, "pan"), "touch: 10.5 px from pointerdown is a pan, not a tap", fmt(d.all));
}
{
  const d = driver();
  d.down(1, 100, 100, { pointerType: "mouse" });
  d.move(1, 104, 100, { pointerType: "mouse" });
  const out = d.up(1, 104, 100, { pointerType: "mouse" });
  report(has(out, "tap") && !has(d.all, "pan"), `mouse: ${TAP_SLOP_MOUSE_PX} px from pointerdown is still a tap`, fmt(d.all));
}
{
  const d = driver();
  d.down(1, 100, 100, { pointerType: "mouse" });
  d.move(1, 104.5, 100, { pointerType: "mouse" });
  const out = d.up(1, 104.5, 100, { pointerType: "mouse" });
  report(!has(out, "tap") && has(d.all, "pan"), "mouse: 4.5 px from pointerdown is a pan, not a tap", fmt(d.all));
}
// Regression: the old rule summed per-move distances, so jitter that never
// strays 2 px from the start added up past 4 px and became a drag.
for (const pointerType of ["mouse", "touch"]) {
  const d = driver();
  d.down(1, 200, 200, { pointerType });
  const jitter: Array<[number, number]> = [[202, 200], [200, 201], [201, 202], [199, 200], [201, 199], [200, 200]];
  for (const [x, y] of jitter) d.move(1, x, y, { pointerType });
  const out = d.up(1, 200, 200, { pointerType });
  report(has(out, "tap") && !has(d.all, "pan"), `${pointerType}: jitter of 1-2 px per move (about 12 px cumulative, never 3 px from the start) is still a tap`, fmt(d.all));
}
// A drag that doubles back near its start is still a drag: once panning, a
// sequence never becomes a tap again.
{
  const d = driver();
  d.down(1, 100, 100, { pointerType: "mouse" });
  d.move(1, 130, 100, { pointerType: "mouse" });
  d.move(1, 101, 100, { pointerType: "mouse" });
  const out = d.up(1, 101, 100, { pointerType: "mouse" });
  report(!has(out, "tap"), "a drag that returns to within the slop of its start is not a tap", fmt(d.all));
}
{
  const d = driver();
  d.down(1, 100, 100);
  const out = d.up(1, 100, 100, { dt: TAP_MAX_MS - 1 });
  report(has(out, "tap"), `a press of ${TAP_MAX_MS - 1} ms is a tap`, fmt(out));
}
{
  const d = driver();
  d.down(1, 100, 100);
  const out = d.up(1, 100, 100, { dt: TAP_MAX_MS });
  report(!has(out, "tap") && d.kinds(out).join() === "end", `a press of ${TAP_MAX_MS} ms is not a tap (long press)`, fmt(out));
}
// Pan deltas: the first one carries the whole displacement since pointerdown
// (the grabbed point stays under the finger), later ones are incremental, so
// they always sum to the pointer's total travel.
{
  const d = driver();
  d.down(1, 50, 50);
  d.move(1, 55, 50); // inside slop: no pan yet
  d.move(1, 65, 58); // leaves slop
  d.move(1, 80, 70);
  d.move(1, 78, 90);
  d.up(1, 78, 90);
  const pans = d.all.filter((i): i is Extract<GestureIntent, { kind: "pan" }> => i.kind === "pan");
  const sx = pans.reduce((a, p) => a + p.dx, 0);
  const sy = pans.reduce((a, p) => a + p.dy, 0);
  report(pans.length === 3 && pans[0].start && !pans[1].start && !pans[2].start, "pan: first intent is marked start, later ones are not", fmt(pans));
  report(sx === 28 && sy === 40, "pan: deltas sum to the displacement since pointerdown", `sum=(${sx}, ${sy})`);
}

// ------------------------------------------------------------- rule 2: double-tap

{
  const d = driver();
  d.down(1, 100, 100);
  const first = d.up(1, 100, 100);
  d.down(1, 110, 108, { dt: DOUBLE_TAP_MS });
  const second = d.up(1, 110, 108);
  const dt = second.find((i) => i.kind === "double-tap") as Extract<GestureIntent, { kind: "double-tap" }> | undefined;
  report(has(first, "tap"), "double-tap: the first tap is reported as a tap (its selection stands)", fmt(first));
  report(!!dt && !has(second, "tap") && dt.x === 110 && dt.y === 108,
    `double-tap: a second tap within ${DOUBLE_TAP_MS} ms and ${DOUBLE_TAP_PX} px zooms about the tapped point, and is not also a tap`, fmt(second));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.up(1, 100, 100);
  d.down(1, 100, 100, { dt: DOUBLE_TAP_MS + 1 });
  const second = d.up(1, 100, 100);
  report(has(second, "tap") && !has(second, "double-tap"), `double-tap: ${DOUBLE_TAP_MS + 1} ms apart is two taps`, fmt(second));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.up(1, 100, 100);
  d.down(1, 100 + DOUBLE_TAP_PX + 1, 100);
  const second = d.up(1, 100 + DOUBLE_TAP_PX + 1, 100);
  report(has(second, "tap") && !has(second, "double-tap"), `double-tap: ${DOUBLE_TAP_PX + 1} px apart is two taps`, fmt(second));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.up(1, 100, 100);
  d.down(1, 100, 100);
  d.up(1, 100, 100);
  d.down(1, 100, 100);
  const third = d.up(1, 100, 100);
  report(has(third, "tap") && !has(third, "double-tap"), "double-tap: a third quick tap starts over (tap, double-tap, tap)", d.kinds().join());
}
{
  const d = driver();
  d.down(1, 100, 100, { pointerType: "mouse" });
  d.up(1, 100, 100, { pointerType: "mouse" });
  d.down(1, 100, 100, { pointerType: "mouse" });
  const second = d.up(1, 100, 100, { pointerType: "mouse" });
  report(has(second, "tap") && !has(second, "double-tap"), "double-tap: a mouse double-click stays two clicks (desktop never zoomed on it)", fmt(second));
}
// Regression: a pinch's end looked like a motionless tap, so a tap right
// after a pinch zoomed.
{
  const d = driver();
  d.down(1, 100, 100);
  d.down(2, 200, 100, { isPrimary: false });
  d.move(2, 220, 100, { isPrimary: false });
  d.up(2, 220, 100, { isPrimary: false });
  d.up(1, 100, 100);
  d.down(1, 105, 100, { dt: 50 });
  const out = d.up(1, 105, 100);
  report(has(out, "tap") && !has(out, "double-tap"), "a tap right after a pinch is a tap, not a double-tap", d.kinds().join());
}
// A pinch where neither finger moves is still no tap, and not a first tap.
{
  const d = driver();
  d.down(1, 100, 100);
  d.down(2, 140, 100, { isPrimary: false });
  d.up(2, 140, 100, { isPrimary: false });
  const out = d.up(1, 100, 100);
  report(!has(out, "tap") && !has(out, "double-tap"), "a motionless two-finger touch is not a tap", fmt(out));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.up(1, 100, 100);
  d.down(1, 100, 100);
  d.cancel(1);
  d.down(1, 100, 100, { dt: 20 });
  const out = d.up(1, 100, 100);
  report(has(out, "tap") && !has(out, "double-tap"), "a pointercancel between two taps breaks the double-tap", d.kinds().join());
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.up(1, 100, 100);
  d.down(1, 100, 100);
  d.move(1, 140, 100);
  d.up(1, 140, 100);
  d.down(1, 140, 100, { dt: 20 });
  const out = d.up(1, 140, 100);
  report(has(out, "tap") && !has(out, "double-tap"), "a pan between two taps breaks the double-tap", d.kinds().join());
}

// --------------------------------------------------------------- rule 3: pinch

{
  const d = driver();
  d.down(1, 100, 100);
  const start = d.down(2, 200, 100, { isPrimary: false });
  const ps = start.find((i) => i.kind === "pinch-start") as Extract<GestureIntent, { kind: "pinch-start" }> | undefined;
  report(!!ps && ps.ids.includes(1) && ps.ids.includes(2) && ps.mid[0] === 150 && ps.mid[1] === 100,
    "pinch-start names both pointer ids (the renderer captures both on the map element)", fmt(start));
  const spread = d.move(2, 300, 100, { isPrimary: false });
  const p = spread.find((i) => i.kind === "pinch") as Extract<GestureIntent, { kind: "pinch" }> | undefined;
  report(!!p && p.scale === 2 && p.mid[0] === 200 && p.mid[1] === 100, "pinch: scale is current spread over start spread, mid is the live midpoint", fmt(spread));
}
// A pinch that starts mid-pan takes over from the pan.
{
  const d = driver();
  d.down(1, 100, 100);
  d.move(1, 130, 100);
  d.down(2, 230, 100, { isPrimary: false });
  const out = d.move(1, 120, 100);
  report(has(out, "pinch") && !has(out, "pan"), "a second finger during a pan turns it into a pinch", fmt(out));
}
// Leftover finger after a pinch does nothing until it lifts.
{
  const d = driver();
  d.down(1, 100, 100);
  d.down(2, 200, 100, { isPrimary: false });
  d.up(2, 200, 100, { isPrimary: false });
  const out = d.move(1, 160, 140);
  report(out.length === 0, "the finger left over after a pinch neither pans nor pinches", fmt(out));
}
// Regression: a lost pointerup left a ghost pointer, and the next one-finger
// drag became a pinch.
{
  const d = driver();
  d.down(1, 100, 100);
  d.down(2, 200, 100, { isPrimary: false });
  d.move(2, 240, 100, { isPrimary: false });
  // pointer 2's pointerup never arrives
  d.up(1, 100, 100);
  const before = d.all.length;
  d.down(3, 300, 300, { isPrimary: true, dt: 400 });
  d.move(3, 330, 300, { isPrimary: true });
  d.move(3, 360, 300, { isPrimary: true });
  const after = d.all.slice(before);
  report(has(after, "cancel") && has(after, "pan") && !has(after, "pinch") && !has(after, "pinch-start") && d.g.pointerCount === 1,
    "a lost pointerup followed by a one-finger drag pans rather than pinches", fmt(after));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.down(2, 200, 100, { isPrimary: false });
  const out = d.cancel(2);
  report(d.kinds(out).join() === "cancel" && !d.g.active, "pointercancel resets the gesture: no pointer survives", fmt(out));
  const after = d.move(1, 150, 150);
  report(after.length === 0, "after pointercancel, the other finger's moves are ignored until a new pointerdown", fmt(after));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.move(1, 140, 100);
  const out = d.lost(1);
  report(d.kinds(out).join() === "cancel" && !d.g.active, "lostpointercapture for a tracked pointer resets the gesture", fmt(out));
}
{
  const d = driver();
  d.down(1, 100, 100);
  d.move(1, 140, 100);
  d.up(1, 140, 100);
  const out = d.lost(1);
  report(out.length === 0, "the lostpointercapture that follows a normal pointerup is a no-op", fmt(out));
}
{
  // A mouse whose pointerup was lost (released outside the window): the
  // next mouse pointerdown is primary, so the stale press is dropped and
  // the new one starts clean.
  const d = driver();
  d.down(1, 100, 100, { pointerType: "mouse" });
  const out = d.down(1, 300, 300, { pointerType: "mouse", dt: 900 });
  const up = d.up(1, 300, 300, { pointerType: "mouse" });
  report(has(out, "cancel") && has(up, "tap"), "an isPrimary pointerdown with a stale pointer tracked resets, then tracks the new press", d.kinds().join());
}
// Regression: a third finger was added to the pointer map and the view jumped.
{
  const d = driver();
  d.down(1, 100, 100);
  d.down(2, 200, 100, { isPrimary: false });
  d.move(2, 220, 100, { isPrimary: false });
  const before = d.all.length;
  const third = [
    ...d.down(3, 600, 600, { isPrimary: false }),
    ...d.move(3, 700, 700, { isPrimary: false }),
    ...d.up(3, 700, 700, { isPrimary: false }),
    ...d.cancel(3),
  ];
  report(third.length === 0 && d.g.pointerCount === 2, "a third finger's down, move, up and cancel produce nothing", fmt(third));
  const next = d.move(2, 240, 100, { isPrimary: false });
  const p = next.find((i) => i.kind === "pinch") as Extract<GestureIntent, { kind: "pinch" }> | undefined;
  report(!!p && p.mid[0] === 170 && p.mid[1] === 100 && Math.abs(p.scale - 1.4) < 1e-9,
    "a third finger doesn't jump the view: the pinch keeps its own two pointers' midpoint and spread", fmt(d.all.slice(before)));
}

// --------------------------------------------------------------- end / active

{
  const d = driver();
  d.down(1, 100, 100);
  const midway = d.g.active;
  const out = d.up(1, 100, 100);
  report(midway && !d.g.active && d.kinds(out).join() === "tap,end", "a tap ends with [tap, end], and active tracks the pointer", fmt(out));
}

// ---------------------------------------------------------- rule 4: render gate

{
  const gate = new RenderGate<string>();
  report(gate.offer("a") && gate.latest === null, "render gate: an offer with no hold applies immediately");
  gate.hold();
  const queued = [gate.offer("b"), gate.offer("c"), gate.offer("d")];
  report(queued.every((q) => q === false) && gate.latest === "d", "render gate: offers while held are queued, newest wins", JSON.stringify({ queued, latest: gate.latest }));
  const released = gate.release();
  report(released === "d" && gate.latest === null && !gate.held, "render gate: release hands back the newest state exactly once", String(released));
  report(gate.release() === null, "render gate: a second release has nothing to apply");
  report(gate.offer("e"), "render gate: after release, offers apply immediately again");
  gate.hold();
  gate.offer("f");
  gate.drop();
  report(gate.held && gate.latest === null && gate.release() === null, "render gate: drop forgets a queued state (a new document supersedes it) without ending the hold");
  gate.hold();
  report(gate.release() === null, "render gate: a hold with no offers releases nothing (no redundant paint)");
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
