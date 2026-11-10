#!/usr/bin/env node
// CI-only browser journey for issue #255's organization overview. The mock
// service supplies a prebuilt OrganizationDocument and each member map at its
// exact pinned SHA; this verifies that composition and navigation contract,
// not a production service deployment.
import assert from "node:assert/strict";
import { chromium } from "playwright";

function parseArgs(argv) {
  const args = { base: "http://127.0.0.1:5176", profile: null };
  for (let index = 0; index < argv.length; index++) {
    if (argv[index] === "--base") args.base = argv[++index];
    else if (argv[index] === "--profile") args.profile = argv[++index];
    else throw new Error(`unknown arg: ${argv[index]}`);
  }
  if (args.profile != null && !["desktop", "phone"].includes(args.profile)) throw new Error("--profile must be desktop or phone");
  return args;
}

const ALL_PROFILES = [
  { name: "desktop", viewport: { width: 1200, height: 800 }, isMobile: false, hasTouch: false },
  { name: "phone", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
];

const args = parseArgs(process.argv.slice(2));
const profiles = args.profile ? ALL_PROFILES.filter(profile => profile.name === args.profile) : ALL_PROFILES;
const browser = await chromium.launch();
try {
  for (const profile of profiles) {
    const context = await browser.newContext({ ...profile, colorScheme: "light" });
    const page = await context.newPage();
    await page.goto(`${args.base}/org/acme-platform`);
    await page.locator('[data-organization-view]').waitFor();
    assert.equal(await page.locator('[data-organization-roads] [data-organization-road]').count(), 1, `${profile.name}: one declared road`);
    assert.ok(await page.locator('[data-organization-repository-outline]').count() >= 2, `${profile.name}: member repository outlines`);
    assert.ok(await page.locator('[data-organization-district]').count() > 0, `${profile.name}: member district outlines`);
    assert.equal(await page.locator('[data-organization-canvas] [data-k^="f:"], [data-organization-canvas] [data-k^="s:"]').count(), 0, `${profile.name}: no files or symbols at overview`);
    const repository = page.locator('[data-organization-repository="acme/app"]');
    await repository.click();
    await page.locator('[data-organization-card]').waitFor();
    assert.equal(await page.locator('[data-pin-skew]').count(), 1, `${profile.name}: repository card shows pin skew`);
    await page.locator('[data-organization-open-map]').click();
    await page.waitForURL(url => url.pathname === "/acme/app" && url.searchParams.get("commit") === "b".repeat(40));
    assert.equal(new URL(page.url()).searchParams.get("commit"), "b".repeat(40), `${profile.name}: member navigation pins its commit`);
    await page.goBack();
    await page.waitForURL(url => url.pathname === "/org/acme-platform");
    await page.locator('[data-organization-view]').waitFor();
    await context.close();
    console.log(`ok ${profile.name}: overview, repository card, pinned member navigation, back`);
  }
} finally {
  await browser.close();
}
