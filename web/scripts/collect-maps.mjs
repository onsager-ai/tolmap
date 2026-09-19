#!/usr/bin/env node
// Copies map JSON into web/public/maps/ and writes an index.json catalogue
// the app fetches at startup. Runs before `dev` and `build` (see
// package.json predev/prebuild) so the app never ships without data.
//
// web/public/maps/ is generated and gitignored — this script is the only
// thing that writes to it.
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WEB_ROOT = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const REPO_ROOT = path.resolve(WEB_ROOT, "..");
const OUT_DIR = path.join(WEB_ROOT, "public", "maps");
const CONFIG_PATH = path.join(WEB_ROOT, "maps.config.json");

// The nine baseline fixtures are named after the repo only (django.json, not
// django-django.json) — HANDOFF.md's "nine known-correct answers" list — and
// the real GitHub owner has to be supplied by hand. tolmap's own self-map
// joins the set once it exists.
const STEM_OWNERS = {
  scrapy: "scrapy",
  django: "django",
  flask: "pallets",
  sqlalchemy: "sqlalchemy",
  celery: "celery",
  rich: "Textualize",
  httpx: "encode",
  prometheus: "prometheus",
  vue: "vuejs",
  tolmap: "onsager-ai",
};
// vue.json's repo is vuejs/core, not vuejs/vue — the fixture predates the
// monorepo rename and the filename was never updated.
const REPO_NAME_OVERRIDE = { vue: "core" };

/**
 * Where to read <stem>.json map files from, in order:
 *   1. $TOLMAP_MAPS_DIR, for a one-off override without editing tracked config.
 *   2. maps.config.json's `mapsDir`, if the file exists and the directory is present.
 *   3. ../data (the repo's own committed fixtures) — the fallback for a fresh
 *      clone with nothing else generated. This is the acceptance corpus, not
 *      a complete demo set: only two of the nine committed fixtures carry the
 *      `P` parcel block, so the "plots" geometry mode will be empty for the
 *      rest unless a fuller `mapsDir` is configured or exported.
 */
async function resolveSourceDir() {
  const fromEnv = process.env.TOLMAP_MAPS_DIR;
  if (fromEnv && existsSync(fromEnv)) return fromEnv;

  if (existsSync(CONFIG_PATH)) {
    const raw = JSON.parse(await readFile(CONFIG_PATH, "utf8"));
    if (raw.mapsDir && existsSync(raw.mapsDir)) return raw.mapsDir;
    if (raw.mapsDir) {
      console.warn(`[collect-maps] maps.config.json's mapsDir does not exist: ${raw.mapsDir} — falling back`);
    }
  }

  return path.join(REPO_ROOT, "data");
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });
  const sourceDir = await resolveSourceDir();
  console.log(`[collect-maps] reading maps from ${sourceDir}`);

  const index = [];
  for (const [stem, owner] of Object.entries(STEM_OWNERS)) {
    const file = path.join(sourceDir, `${stem}.json`);
    if (!existsSync(file)) {
      // Expected for "tolmap" against a fresh clone (no self-map generated
      // yet) and is not a build failure — the milestone brief requires
      // exactly this: skip, don't fail.
      console.warn(`[collect-maps] skipping ${stem}: ${file} does not exist`);
      continue;
    }
    const repo = REPO_NAME_OVERRIDE[stem] ?? stem;
    let doc;
    try {
      doc = JSON.parse(await readFile(file, "utf8"));
    } catch (err) {
      console.warn(`[collect-maps] skipping ${owner}/${repo}: failed to parse ${file}: ${err.message}`);
      continue;
    }
    const destDir = path.join(OUT_DIR, owner);
    await mkdir(destDir, { recursive: true });
    await writeFile(path.join(destDir, `${repo}.json`), JSON.stringify(doc));
    index.push({
      slug: `${owner}/${repo}`,
      owner,
      repo,
      file: `/maps/${owner}/${repo}.json`,
      files: doc.F?.length ?? 0,
      districts: Object.keys(doc.districts ?? {}).length,
      modularity: doc.q ?? 0,
      lang: doc.lang ?? "unknown",
    });
  }
  index.sort((a, b) => a.slug.localeCompare(b.slug));
  await writeFile(path.join(OUT_DIR, "index.json"), JSON.stringify(index, null, 2));
  console.log(`[collect-maps] wrote ${index.length} map(s) to ${path.relative(REPO_ROOT, OUT_DIR)}/`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
