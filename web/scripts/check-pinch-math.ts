#!/usr/bin/env -S npx tsx
// Regression check for pinch.ts's pinchTransform(): the pinch-to-zoom math
// factored out of MapRenderer.ts's pointerMove. The bug it guards against
// (flagged in review of #56): the reference's formula --
// `tx=m[0]-(pinch.m[0]-tx)*(nk/pinch.k)` (viewer/template.html, frozen, not
// touched) -- and this port's PREVIOUS version of the same line both read
// the LIVE this.tx as the anchor, so every pointermove during a pinch
// rewrites tx relative to whatever the last frame already wrote, which
// compounds: held-still input still drifts, geometrically, frame over
// frame. pinchTransform() instead anchors to the pinch's own start state
// (tx0/ty0, captured once when the second finger goes down), which makes it
// idempotent -- see pinch.ts's doc comment for the full derivation.
//
// Exercises the pure function directly with scripted sequences of
// midpoints/spreads rather than driving an actual two-finger gesture: CDP's
// synthesized touch events proved unreliable for that in this environment
// (see check-view-stability.mjs's zoomIn() comment -- a scripted pinch there
// compounded into a large, unrealistic pan for the very reason this file
// exists to fix).
//
// Run: npx tsx web/scripts/check-pinch-math.ts
// (no test runner exists in this project yet -- see web/README.md -- so this
// is a standalone script, the same pattern as no-change-proof.ts)

import { pinchTransform, type PinchAnchor } from "../src/map/pinch";

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

function approx(a: number, b: number, eps = 1e-9) {
  return Math.abs(a - b) <= eps;
}

// 1. No movement yet: midpoint and k both still at the pinch's own start
// values. The transform must be the identity -- returns the anchor's own
// tx0/ty0 unchanged, not some other value derived from state the pure
// function doesn't have.
{
  const anchor: PinchAnchor = { m: [140, 260], k: 1.5, tx0: 12, ty0: -34 };
  const { tx, ty } = pinchTransform(anchor, anchor.m, anchor.k);
  report(
    approx(tx, anchor.tx0) && approx(ty, anchor.ty0),
    "no movement yet: transform equals the anchor's own start tx0/ty0",
    `got tx=${tx} ty=${ty}, expected tx0=${anchor.tx0} ty0=${anchor.ty0}`,
  );
}

// 2. Held still after a spread: the fingers stopped moving, so every
// subsequent pointermove (a real device coalesces these; a scripted one can
// deliver near-duplicates) reports the SAME midpoint and the SAME spread
// (hence the same clamped nk). Calling pinchTransform() again and again with
// unchanged inputs must return the SAME (tx, ty) every time -- the bug this
// guards against instead walks away from (or toward, direction depends on
// the sign of nk/anchor.k - 1) a fixed point on every call, because it read
// its OWN previous output as part of its input.
{
  const anchor: PinchAnchor = { m: [200, 300], k: 2, tx0: -100, ty0: -50 };
  const midpoint: [number, number] = [220, 280]; // fingers moved during the spread, then stopped
  const nk = 3.4; // zoomed-to k once the spread stopped, unchanging thereafter
  const first = pinchTransform(anchor, midpoint, nk);
  let last = first;
  let drifted = false;
  for (let i = 0; i < 50; i++) {
    const next = pinchTransform(anchor, midpoint, nk);
    if (!approx(next.tx, last.tx) || !approx(next.ty, last.ty)) drifted = true;
    last = next;
  }
  report(
    !drifted && approx(last.tx, first.tx) && approx(last.ty, first.ty),
    "held-still after a spread: tx/ty stay constant across 50 repeated calls",
    `first=${JSON.stringify(first)} last=${JSON.stringify(last)}`,
  );
}

// 3. Fixed-midpoint spread: fingers spread or pinch symmetrically about a
// midpoint that never itself moves, only k changes (in and out, repeatedly,
// not monotonically -- a real pinch overshoots and corrects). The world
// point under that midpoint -- the actual definition of "zoom about the
// point between your fingers" -- must stay exactly fixed throughout: the
// round trip from screen space back to world space using whatever (tx, ty,
// k) came out of pinchTransform() must always recover the SAME world
// coordinate the anchor itself started with.
{
  const anchor: PinchAnchor = { m: [150, 400], k: 1, tx0: 40, ty0: 60 };
  const midpoint = anchor.m; // fingers never move off the start midpoint
  const worldAtStart: [number, number] = [(anchor.m[0] - anchor.tx0) / anchor.k, (anchor.m[1] - anchor.ty0) / anchor.k];
  let ok = true;
  let worst = 0;
  for (const nk of [1, 1.3, 1.8, 2.5, 4, 3, 1.9, 1, 0.8, 1.5]) {
    const { tx, ty } = pinchTransform(anchor, midpoint, nk);
    const world: [number, number] = [(midpoint[0] - tx) / nk, (midpoint[1] - ty) / nk];
    const err = Math.hypot(world[0] - worldAtStart[0], world[1] - worldAtStart[1]);
    worst = Math.max(worst, err);
    if (err > 1e-9) ok = false;
  }
  report(ok, "fixed-midpoint spread: the world point under the fingers stays fixed as k changes", `worst error ${worst}`);
}

// 4. A drifting midpoint (the fingers pan while pinching, the normal case)
// still keeps the world point under the pinch's START midpoint mapped
// consistently -- i.e. the transform composes a pan and a zoom in one
// coherent step, not two that fight each other. Checked by re-deriving tx
// algebraically at one arbitrary (midpoint, nk) and comparing to the
// function's own output.
{
  const anchor: PinchAnchor = { m: [80, 90], k: 1.2, tx0: 5, ty0: -8 };
  const midpoint: [number, number] = [130, 60];
  const nk = 2.1;
  const r = nk / anchor.k;
  const expectedTx = midpoint[0] - (anchor.m[0] - anchor.tx0) * r;
  const expectedTy = midpoint[1] - (anchor.m[1] - anchor.ty0) * r;
  const { tx, ty } = pinchTransform(anchor, midpoint, nk);
  report(approx(tx, expectedTx) && approx(ty, expectedTy), "panning pinch: matches the closed-form (anchor, midpoint, nk) formula");
}

console.log(`\n${checks - failures}/${checks} checks passed`);
if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
