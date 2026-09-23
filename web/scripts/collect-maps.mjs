#!/usr/bin/env node
// Copies map JSON into web/public/maps/ and writes an index.json catalogue
// the app fetches at startup. Runs before `dev` and `build` (see
// package.json predev/prebuild) so the app never ships without data.
//
// web/public/maps/ is generated and gitignored — this script is the only
// thing that writes to it.
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
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
    // A relative mapsDir is resolved against the repository root, not the
    // process's cwd: the config is checked in and has to mean the same thing
    // wherever the script is invoked from.
    const configured = raw.mapsDir
      ? path.resolve(REPO_ROOT, raw.mapsDir)
      : null;
    if (configured && existsSync(configured)) return configured;
    if (configured) {
      console.warn(`[collect-maps] maps.config.json's mapsDir does not exist: ${configured} — falling back`);
    }
  }

  return path.join(REPO_ROOT, "data");
}

async function resolveCorpus() {
  const rootFromEnv = process.env.TOLMAP_CORPUS_DIR;
  const manifestFromEnv = process.env.TOLMAP_CORPUS_MANIFEST;
  // Do not auto-discover ~/.cache here. With neither variable set the
  // catalogue must be byte-for-byte the baseline catalogue a fresh clone
  // has always generated.
  if (!rootFromEnv && !manifestFromEnv) return null;

  const root = rootFromEnv
    ? path.resolve(rootFromEnv)
    : path.join(homedir(), ".cache", "tolmap-corpus");
  const manifest = manifestFromEnv
    ? path.resolve(manifestFromEnv)
    : path.join(REPO_ROOT, "eval", "corpus.toml");
  const mapsDir = path.join(root, "maps");
  if (!existsSync(manifest)) {
    console.warn(`[collect-maps] corpus manifest does not exist: ${manifest}`);
    return null;
  }
  if (!existsSync(mapsDir)) {
    console.warn(`[collect-maps] corpus maps directory does not exist: ${mapsDir}`);
    return null;
  }
  return { manifest, mapsDir };
}

function corpusEntries(toml) {
  // corpus.toml has a deliberately narrow stdlib-readable shape. Pulling in
  // a JavaScript TOML dependency only for owner/repo strings would make the
  // viewer build depend on eval tooling; blocks still let us honor recorded
  // failures without mistaking their absent maps for catalogue corruption.
  return toml.split(/^\s*\[\[repo\]\]\s*$/m).slice(1).flatMap((block) => {
    const slug = block.match(/^\s*slug\s*=\s*"([^"]+)"\s*$/m)?.[1];
    const status = block.match(/^\s*status\s*=\s*"([^"]+)"\s*$/m)?.[1];
    return slug && status !== "failed" ? [{ slug }] : [];
  });
}

async function collectMap(indexBySlug, slug, file) {
  const [owner, repo, ...rest] = slug.split("/");
  if (!owner || !repo || rest.length) {
    console.warn(`[collect-maps] skipping invalid slug ${slug}`);
    return;
  }
  let doc;
  try {
    doc = JSON.parse(await readFile(file, "utf8"));
  } catch (err) {
    console.warn(`[collect-maps] skipping ${slug}: failed to parse ${file}: ${err.message}`);
    return;
  }
  const destDir = path.join(OUT_DIR, owner);
  await mkdir(destDir, { recursive: true });
  await writeFile(path.join(destDir, `${repo}.json`), JSON.stringify(doc));
  const symbolsFile = file.replace(/\.json$/, ".symbols.json");
  if (existsSync(symbolsFile)) {
    await writeFile(path.join(destDir, `${repo}.symbols.json`), await readFile(symbolsFile));
  } else {
    await rm(path.join(destDir, `${repo}.symbols.json`), { force: true });
  }
  const symbolsDir = file.replace(/\.json$/, ".symbols");
  const destSymbolsDir = path.join(destDir, `${repo}.symbols`);
  await rm(destSymbolsDir, { recursive: true, force: true });
  if (existsSync(symbolsDir)) {
    await cp(symbolsDir, destSymbolsDir, { recursive: true });
  }
  indexBySlug.set(slug, {
    slug,
    owner,
    repo,
    file: `/maps/${owner}/${repo}.json`,
    files: doc.F?.length ?? 0,
    districts: Object.keys(doc.districts ?? {}).length,
    modularity: doc.q ?? 0,
    lang: doc.lang ?? "unknown",
    source: "static",
  });
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });
  const sourceDir = await resolveSourceDir();
  console.log(`[collect-maps] reading maps from ${sourceDir}`);

  const indexBySlug = new Map();
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
    await collectMap(indexBySlug, `${owner}/${repo}`, file);
  }

  const corpus = await resolveCorpus();
  if (corpus) {
    console.log(`[collect-maps] adding corpus maps from ${corpus.mapsDir}`);
    const entries = corpusEntries(await readFile(corpus.manifest, "utf8"));
    for (const { slug } of entries) {
      const file = path.join(corpus.mapsDir, `${slug.replace("/", "__")}.json`);
      if (!existsSync(file)) {
        console.warn(`[collect-maps] skipping corpus ${slug}: ${file} does not exist`);
        continue;
      }
      // A corpus map at its explicit pin wins a duplicate fixture slug. The
      // baseline source and its precedence are untouched when no corpus is
      // requested, which is the normal production-build path.
      await collectMap(indexBySlug, slug, file);
    }
  }
  const index = [...indexBySlug.values()];
  index.sort((a, b) => a.slug.localeCompare(b.slug));
  await writeFile(path.join(OUT_DIR, "index.json"), JSON.stringify(index, null, 2));
  console.log(`[collect-maps] wrote ${index.length} map(s) to ${path.relative(REPO_ROOT, OUT_DIR)}/`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
