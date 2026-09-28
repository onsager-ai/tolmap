// docs/UX.md §6 and §12 (phase 4): the indexing, queue and failure pages, and
// the early handover to the map, driven against scripts/mock-api-server.mjs
// (viewer-check.yml starts it before Vite; there is no Rust `tolmap serve` in
// that job). Called from check-view-stability.mjs's main() with its own
// `report`, so the counts land in one total.
//
// Held states come from the mock's scripted jobs (`mockstate/<state>`: a job
// frozen in one state that takes no running slot). The live flows -- cancel,
// the early handover while the Detail phase runs, the redirect when a job is
// done -- use ordinary mock jobs, each slug timestamped so a re-run never
// meets an old job of the same name.

const DESKTOP = { name: "desktop", viewport: { width: 1200, height: 800 }, isMobile: false, hasTouch: false };
const PHONE = { name: "phone", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 };

async function submit(base, repo) {
  const res = await fetch(`${base}/api/index`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ repo }),
  });
  return res.json();
}

async function cancel(base, jobId) {
  await fetch(`${base}/api/jobs/${jobId}/cancel`, { method: "POST" }).catch(() => {});
}

async function snapshotOf(base, jobId) {
  return (await fetch(`${base}/api/jobs/${jobId}`)).json();
}

async function openJob(browser, base, profile, repo, colorScheme = "light") {
  const accepted = await submit(base, repo);
  const context = await browser.newContext({ ...profile, colorScheme });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (err) => errors.push(String(err)));
  await page.goto(`${base}/new?job=${accepted.job_id}&slug=${encodeURIComponent(repo)}`, { waitUntil: "domcontentloaded" });
  return { accepted, context, page, errors };
}

async function noSideScroll(page) {
  return page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, viewport: window.innerWidth }));
}

async function heightOf(locator) {
  const box = await locator.boundingBox().catch(() => null);
  return box ? box.height : 0;
}

// ---------------------------------------------------------------------------
// §6.1-§6.4: the running page, held in the Read phase.
async function checkRunningPage(browser, base, report, profile) {
  const label = `indexing page, Read running (${profile.name})`;
  console.log(`\n${label}`);
  const { accepted, context, page, errors } = await openJob(browser, base, profile, "mockstate/read");
  await page.waitForSelector("[data-phase-row]", { timeout: 15_000 });
  await page.waitForSelector("[data-found-so-far]", { timeout: 5_000 }).catch(() => {});

  const title = (await page.locator("[data-eta-range]").innerText()).trim();
  report(title === "About 3–5 min left", `${label}: the ETA leads, big`, JSON.stringify(title));
  const titleSize = await page.locator("[data-eta-range]").evaluate((el) => parseFloat(getComputedStyle(el).fontSize));
  report(titleSize >= 30, `${label}: the ETA is the page title size (30 px)`, String(titleSize));
  const meta = await page.locator("[data-running-text]").innerText();
  report(/^Running \d+m \d+s · started .+ · estimate$/.test(meta.trim()), `${label}: "Running … · started … · estimate"`, JSON.stringify(meta));

  const states = await page.locator("[data-phase-row]").evaluateAll((rows) => rows.map((r) => `${r.dataset.phaseId}:${r.dataset.phaseState}`));
  report(
    JSON.stringify(states) === JSON.stringify(["fetch:done", "read:running", "map:pending", "detail:pending"]),
    `${label}: four phases -- Fetch done, Read running, Map and Detail waiting`,
    JSON.stringify(states),
  );
  const fetchRow = await page.locator('[data-phase-row][data-phase-id="fetch"]').innerText();
  report(/Download the repository/.test(fetchRow) && /15s/.test(fetchRow), `${label}: a finished phase shows its meaning and duration`, JSON.stringify(fetchRow));
  const step = await page.locator("[data-current-step]").innerText();
  report(/Parsing files/.test(step) && /3,912 \/ 6,347 files/.test(step) && /410 files\/s/.test(step), `${label}: the current step in plain words, with count and rate`, JSON.stringify(step));
  const pct = Number(await page.locator("[data-current-step] [data-progress-fill]").getAttribute("data-progress-pct"));
  report(Math.abs(pct - 61.6) < 0.2, `${label}: the step's bar is determinate (3,912 of 6,347)`, String(pct));
  const found = await page.locator("[data-found-so-far]").innerText().catch(() => "");
  report(/Found so far:.*6,347 source files/.test(found), `${label}: found so far, from the parse stage's total`, JSON.stringify(found));

  report((await page.locator("[data-copy-link]").count()) === 1, `${label}: Copy link is there`);
  const note = await page.locator("[data-copy-note]").innerText().catch(() => "");
  report(/You can close this page\. The link keeps working, and the map opens here when it's ready\./.test(note), `${label}: with its note`, JSON.stringify(note));
  const details = page.locator("details[data-tech-details]");
  const open = await details.evaluate((el) => el.open).catch(() => null);
  report(open === !profile.isMobile, `${label}: Technical details ${profile.isMobile ? "closed on a phone" : "open on desktop"}`, String(open));
  const position = await page.locator("[data-stage-position]").innerText().catch(() => "");
  report(position.trim() === "stage 6 of 22", `${label}: "stage 6 of 22"`, JSON.stringify(position));
  if (profile.isMobile) await details.locator("summary").click();
  const rows = await page.locator("[data-stage-row]").count();
  report(rows === 19, `${label}: the full stage list behind the disclosure (22 less 3 unused index stages)`, String(rows));
  const cancelText = await page.locator("[data-cancel-button]").innerText().catch(() => "");
  report(cancelText.trim() === "Cancel mapping", `${label}: Cancel mapping is a quiet text button`, JSON.stringify(cancelText));

  if (profile.isMobile) {
    const widths = await noSideScroll(page);
    report(widths.scroll <= widths.viewport, `${label}: no horizontal scroll`, JSON.stringify(widths));
    const heights = {
      back: await heightOf(page.getByRole("link", { name: "tolmap home" })),
      copy: await heightOf(page.locator("[data-copy-link]")),
      cancel: await heightOf(page.locator("[data-cancel-button]")),
      details: await heightOf(details.locator("summary")),
    };
    report(Object.values(heights).every((h) => h >= 44), `${label}: touch targets are at least 44 px`, JSON.stringify(heights));
  }
  report(errors.length === 0, `${label}: no uncaught page errors`, errors.join(" | "));
  await context.close();
  await cancel(base, accepted.job_id);
}

// §6.1: the queued page.
async function checkQueuedPage(browser, base, report, profile) {
  const label = `indexing page, queued (${profile.name})`;
  console.log(`\n${label}`);
  const { accepted, context, page } = await openJob(browser, base, profile, "mockstate/queued");
  await page.waitForSelector("[data-queued-text]", { timeout: 15_000 });
  const title = (await page.locator("[data-queued-title]").innerText()).trim();
  report(title === "Starts in about 2 min", `${label}: "Starts in about N min" leads`, JSON.stringify(title));
  const meta = await page.locator("[data-queued-text]").innerText();
  report(/^2nd in line · waiting \d+m \d+s · (rough )?estimate$/.test(meta.trim()), `${label}: position and waiting time`, JSON.stringify(meta));
  const states = await page.locator("[data-phase-row]").evaluateAll((rows) => rows.map((r) => r.dataset.phaseState));
  report(states.length === 4 && states.every((s) => s === "pending"), `${label}: four phases, none started`, JSON.stringify(states));
  const cancelText = await page.locator("[data-cancel-button]").innerText().catch(() => "");
  report(cancelText.trim() === "Leave the queue", `${label}: "Leave the queue"`, JSON.stringify(cancelText));
  report((await page.locator("[data-copy-link]").count()) === 1, `${label}: Copy link is there`);
  await context.close();
  await cancel(base, accepted.job_id);
}

// §6.2: each phase running.
async function checkEachPhase(browser, base, report) {
  const cases = [
    { state: "fetch", running: "fetch", step: /Downloading/, indeterminate: false },
    { state: "map", running: "map", step: /Finding districts/, indeterminate: true, found: /6,347 source files · 4,000 commits read/ },
    { state: "detail", running: "detail", step: /Reading classes and functions/, indeterminate: false, found: /19 districts/ },
  ];
  for (const c of cases) {
    const label = `indexing page, ${c.state} running (desktop)`;
    console.log(`\n${label}`);
    const { accepted, context, page } = await openJob(browser, base, DESKTOP, `mockstate/${c.state}`);
    await page.waitForSelector(`[data-phase-row][data-phase-id="${c.running}"][data-phase-state="running"]`, { timeout: 15_000 }).catch(() => {});
    const running = await page.locator('[data-phase-row][data-phase-state="running"]').evaluateAll((rows) => rows.map((r) => r.dataset.phaseId));
    report(JSON.stringify(running) === JSON.stringify([c.running]), `${label}: exactly ${c.running} runs`, JSON.stringify(running));
    const step = await page.locator("[data-current-step]").innerText().catch(() => "");
    report(c.step.test(step), `${label}: the step reads ${c.step}`, JSON.stringify(step));
    const pct = await page.locator("[data-current-step] [data-progress-fill]").getAttribute("data-progress-pct").catch(() => null);
    report(c.indeterminate ? pct == null : pct != null, `${label}: the step's bar is ${c.indeterminate ? "indeterminate" : "determinate"}`, String(pct));
    if (c.found) {
      await page.waitForFunction((re) => new RegExp(re).test(document.querySelector("[data-found-so-far]")?.textContent ?? ""), c.found.source, { timeout: 5_000 }).catch(() => {});
      const found = await page.locator("[data-found-so-far]").innerText().catch(() => "");
      report(c.found.test(found), `${label}: found so far keeps earlier stages' facts`, JSON.stringify(found));
    }
    await context.close();
    await cancel(base, accepted.job_id);
  }
}

// §6.4: cancel keeps its confirm step and ends on the cancelled page.
async function checkCancel(browser, base, report) {
  const label = "cancel mapping (desktop)";
  console.log(`\n${label}`);
  const { context, page } = await openJob(browser, base, DESKTOP, "mockstate/read");
  await page.locator("[data-cancel-button]").click({ timeout: 15_000 });
  report((await page.locator("[data-cancel-confirm]").count()) === 1, `${label}: asks before cancelling`);
  await page.getByRole("button", { name: "Yes, cancel" }).click();
  await page.waitForSelector('[data-job-failure][data-job-failure-code="cancelled"]', { timeout: 15_000 }).catch(() => {});
  const title = await page.locator("[data-failure-title]").innerText().catch(() => "");
  report(title === "Mapping was cancelled", `${label}: ends on "Mapping was cancelled"`, JSON.stringify(title));
  const actions = await page.locator("[data-failure-action]").evaluateAll((els) => els.map((e) => e.dataset.failureAction));
  report(JSON.stringify(actions) === JSON.stringify(["again", "home"]), `${label}: Map it again; Home`, JSON.stringify(actions));
  await context.close();
}

// §6.5: every error_code row.
const FAILURE_ROWS = [
  ["detection_uncertain", "tolmap couldn't tell where this repository's code lives", ["another", "report"]],
  ["detection_failed", "tolmap found no Python, Go, TypeScript or Rust source here", ["another"]],
  ["cancelled", "Mapping was cancelled", ["again", "home"]],
  ["clone_failed", "tolmap couldn't download this repository", ["retry", "check-name"]],
  ["worker_crashed", "The mapping job stopped during Read", ["retry"]],
  ["busy", "tolmap is busy right now", ["retry"]],
  ["server_stopping", "tolmap is busy right now", ["retry"]],
  ["rate_limited", "tolmap is busy right now", ["retry"]],
  ["index_failed", "Something went wrong while mapping", ["retry", "report"]],
  ["internal_error", "Something went wrong while mapping", ["retry", "report"]],
  ["mystery_code", "Something went wrong while mapping", ["retry", "report"]],
];

async function checkFailurePages(browser, base, report) {
  for (const [profile, colorScheme] of [
    [DESKTOP, "light"],
    [PHONE, "dark"],
  ]) {
    for (const [code, title, actions] of FAILURE_ROWS) {
      const label = `failure page ${code} (${profile.name}, ${colorScheme})`;
      const { context, page, errors } = await openJob(browser, base, profile, `mockstate/failed-${code}`, colorScheme);
      await page.waitForSelector(`[data-job-failure][data-job-failure-code="${code}"]`, { timeout: 15_000 }).catch(() => {});
      const got = await page.locator("[data-failure-title]").innerText().catch(() => "");
      const gotActions = await page.locator("[data-failure-action]").evaluateAll((els) => els.map((e) => e.dataset.failureAction));
      const evidence = await page.locator("[data-job-failure-evidence]").innerText().catch(() => "");
      const problems = [];
      if (got !== title) problems.push(`title ${JSON.stringify(got)}`);
      if (JSON.stringify(gotActions) !== JSON.stringify(actions)) problems.push(`actions ${JSON.stringify(gotActions)}`);
      if (!/What tolmap saw/.test(evidence) || evidence.split("\n").length < 2) problems.push(`evidence ${JSON.stringify(evidence)}`);
      const deterministic = code.startsWith("detection_");
      if (deterministic) {
        const retries = await page.locator('[data-failure-action="retry"], [data-failure-action="again"]').count();
        if (retries !== 0) problems.push("a deterministic refusal offers trying again");
        const copy = await page.locator("[data-job-failure-explanation]").innerText();
        if (!/Trying again won't change the result\./.test(copy)) problems.push("no \"Trying again won't change the result.\"");
      }
      if (gotActions.includes("report")) {
        const href = (await page.locator('[data-failure-action="report"]').getAttribute("href")) ?? "";
        const url = new URL(href);
        if (url.origin + url.pathname !== "https://github.com/onsager-ai/tolmap/issues/new") problems.push(`report href ${href}`);
        if (!(url.searchParams.get("title") ?? "").includes(`mockstate/failed-${code}`)) problems.push("the issue title lacks the slug");
        if (!(url.searchParams.get("body") ?? "").includes(code)) problems.push("the issue body lacks the error");
        if ((await page.locator('[data-failure-action="report"]').evaluate((el) => el.tagName)) !== "A") problems.push("report is not a plain link");
      }
      if (profile.isMobile) {
        const widths = await noSideScroll(page);
        if (widths.scroll > widths.viewport) problems.push(`horizontal scroll ${JSON.stringify(widths)}`);
        for (const kind of gotActions) {
          const h = await heightOf(page.locator(`[data-failure-action="${kind}"]`));
          if (h < 44) problems.push(`${kind} is ${h} px tall`);
        }
      }
      if (errors.length) problems.push(errors.join(" | "));
      report(problems.length === 0, `${label}: "${title}", ${actions.join(" + ")}`, problems.join("; "));
      await context.close();
    }
  }
}

// §12: the page hands over to the map when the Map phase finishes; the map
// view pins the job's commit, says the details are still coming, and drops
// its `job` param when the job is done.
async function checkEarlyHandover(browser, base, report, profile) {
  const label = `early handover at Map (${profile.name})`;
  console.log(`\n${label}`);
  const repo = `checkorg/slowdetail-${profile.name}-${Date.now()}`;
  const { accepted, context, page, errors } = await openJob(browser, base, profile, repo);
  const mapRequests = [];
  page.on("request", (req) => {
    const url = new URL(req.url());
    if (url.pathname === `/api/maps/${repo}`) mapRequests.push(url.search);
  });
  const handedOver = await page
    .waitForURL((url) => url.pathname === `/${repo}` && url.searchParams.get("job") === accepted.job_id, { timeout: 60_000 })
    .then(() => true)
    .catch(() => false);
  report(handedOver, `${label}: the page moves to the map with ?job=`, page.url());
  if (!handedOver) {
    await context.close();
    await cancel(base, accepted.job_id);
    return;
  }
  const status = (await snapshotOf(base, accepted.job_id)).status;
  report(status !== "done" && status !== "failed", `${label}: ...while the job is still running`, status);
  await page.waitForSelector("svg [data-k]", { timeout: 30_000 }).catch(() => {});
  report((await page.locator("svg [data-k]").count()) > 0, `${label}: the map draws before the job is done`);
  report((await page.locator('[data-detail-status="pending"]').count()) === 1, `${label}: it says classes and functions are still coming`);
  const commit = (await snapshotOf(base, accepted.job_id)).commit;
  report(mapRequests.length > 0 && mapRequests.every((q) => q.includes(`commit=${commit}`)), `${label}: the map is fetched at the job's commit`, JSON.stringify(mapRequests));
  const settled = await page
    .waitForURL((url) => url.pathname === `/${repo}` && !url.searchParams.has("job"), { timeout: 60_000 })
    .then(() => true)
    .catch(() => false);
  report(settled, `${label}: the job param goes once the job is done`, page.url());
  await page.waitForTimeout(300);
  report((await page.locator("[data-detail-status]").count()) === 0, `${label}: and the note with it`);
  report((await page.locator("svg [data-k]").count()) > 0, `${label}: the map stays drawn`);
  report(errors.length === 0, `${label}: no uncaught page errors`, errors.join(" | "));
  await context.close();
}

// The plain redirect: a job whose map is not opened early goes to the map
// when it is done, with no job param.
async function checkDoneRedirect(browser, base, report) {
  const label = "done redirect without an early map (desktop)";
  console.log(`\n${label}`);
  const repo = `checkorg/noearly-${Date.now()}`;
  const { accepted, context, page } = await openJob(browser, base, DESKTOP, repo);
  const sawJobParam = [];
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame() && new URL(frame.url()).pathname === `/${repo}`) sawJobParam.push(new URL(frame.url()).searchParams.has("job"));
  });
  const arrived = await page
    .waitForURL((url) => url.pathname === `/${repo}`, { timeout: 60_000 })
    .then(() => true)
    .catch(() => false);
  report(arrived, `${label}: the page moves to the map`, page.url());
  const status = (await snapshotOf(base, accepted.job_id)).status;
  report(status === "done", `${label}: only once the job is done`, status);
  report(!sawJobParam.includes(true), `${label}: with no job param`, JSON.stringify(sawJobParam));
  await page.waitForSelector("svg [data-k]", { timeout: 30_000 }).catch(() => {});
  report((await page.locator("svg [data-k]").count()) > 0, `${label}: the map draws`);
  await context.close();
}

export async function runJobPageChecks(browser, base, report) {
  for (const profile of [DESKTOP, PHONE]) await checkRunningPage(browser, base, report, profile);
  for (const profile of [DESKTOP, PHONE]) await checkQueuedPage(browser, base, report, profile);
  await checkEachPhase(browser, base, report);
  await checkCancel(browser, base, report);
  await checkFailurePages(browser, base, report);
  // The live flows last: the mock runs one real job at a time.
  await checkEarlyHandover(browser, base, report, DESKTOP);
  await checkEarlyHandover(browser, base, report, PHONE);
  await checkDoneRedirect(browser, base, report);
}
