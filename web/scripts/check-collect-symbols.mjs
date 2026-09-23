#!/usr/bin/env node
// Exercise the static district copy and stale-directory removal with one
// tiny fixture. The real repository size audit runs on remote-build runners.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = fileURLToPath(new URL("..", import.meta.url));
const source = mkdtempSync(path.join(tmpdir(), "tolmap-symbol-copy-"));
const target = path.join(webRoot, "public", "maps", "django", "django.symbols");
const district = { district: 0, files: [0], symbols: [], symbol_indices: [], edges: [], module_code_lines: {} };
try {
  await writeFile(path.join(source, "django.json"), JSON.stringify({ F: ["mod.py"], districts: { 0: {} } }));
  const symbols = path.join(source, "django.symbols");
  await mkdir(symbols);
  await writeFile(path.join(symbols, "0.json"), JSON.stringify(district));
  const collect = () => execFileSync("node", [path.join(webRoot, "scripts", "collect-maps.mjs")], {
    env: { ...process.env, TOLMAP_MAPS_DIR: source }, stdio: "ignore",
  });
  collect();
  assert.deepEqual(JSON.parse(await readFile(path.join(target, "0.json"), "utf8")), district);
  await rm(symbols, { recursive: true });
  collect();
  await assert.rejects(readFile(path.join(target, "0.json")));
} finally {
  await rm(source, { recursive: true, force: true });
}
