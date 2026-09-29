// Issue #205: waiting for the map camera to stop moving, shared by
// check-view-stability.mjs and screenshots.mjs so the frames they capture are
// framed the same way every run.
//
// Why a fixed sleep is not enough: MapRenderer.zoomAbout() derives each
// step's target from the CURRENT camera, and glide() takes 240 ms. A click
// that lands mid-glide compounds from wherever the previous glide happens to
// be, so four back-to-back Playwright clicks end at a camera that depends on
// how long each click took. That is deterministic for a given timing, and the
// renderer is right to do it (a person mashing the button wants to keep
// zooming), but it makes light and dark contexts in one run disagree by
// 10-100 px. Settling between clicks makes every step start from the exact
// end of the last one.

/** What the camera puts on screen: every district polygon's box. The camera
 * itself is not in the DOM once glide() has painted -- paint() bakes it into
 * the geometry and drops the <g> transform -- so this is the readable proxy.
 * To a tenth of a pixel because paint() rounds path coordinates to a tenth. */
export function cameraBoxes(page) {
  return page.evaluate(() => [...document.querySelectorAll('svg.map-svg path.hit[data-k^="d:"]')].map((el) => {
    const r = el.getBoundingClientRect();
    return [el.getAttribute("data-k"), +r.left.toFixed(1), +r.top.toFixed(1), +r.width.toFixed(1), +r.height.toFixed(1)];
  }));
}

/** Resolves once two reads of the camera, 100 ms apart, match. The first
 * 300 ms is unconditional: glide()'s ease-in-out cubic moves under a pixel
 * in its first frames, so an early pair of reads can agree while the glide
 * has not really begun. 300 ms is the 240 ms glide plus a frame of slack;
 * the poll then only has to catch a slow runner. */
export async function settleCamera(page, { timeoutMs = 5000 } = {}) {
  await page.waitForTimeout(300);
  const deadline = Date.now() + timeoutMs;
  let last = JSON.stringify(await cameraBoxes(page));
  while (Date.now() < deadline) {
    await page.waitForTimeout(100);
    const now = JSON.stringify(await cameraBoxes(page));
    if (now === last) return;
    last = now;
  }
  throw new Error(`map camera did not settle within ${timeoutMs} ms`);
}

/** Presses the zoom-in button `clicks` times, letting the camera settle after
 * each press so no click reads a mid-glide camera. */
export async function zoomInSettled(page, clicks) {
  for (let i = 0; i < clicks; i++) {
    await page.locator('button[aria-label="Zoom in"]').click();
    await settleCamera(page);
  }
}
