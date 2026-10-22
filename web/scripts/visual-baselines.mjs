import fs from 'node:fs/promises';
import path from 'node:path';
import { isDeepStrictEqual } from 'node:util';

const REGISTRY = 'approved.json';
const REPORT = 'visual-regression.json';
const PNG = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const identity = row => ({ id: row.id, host: row.host, viewport: row.viewport, theme: row.theme });
const key = row => JSON.stringify(identity(row));
const within = (root, name) => {
  const full = path.resolve(root, name);
  if (!full.startsWith(`${path.resolve(root)}${path.sep}`)) throw new Error(`Path escapes its evidence directory: ${name}`);
  return full;
};
const readPng = async (root, file) => {
  const full = within(root, file);
  const bytes = await fs.readFile(full);
  if (bytes.length < PNG.length || !bytes.subarray(0, PNG.length).equals(PNG)) throw new Error(`Invalid PNG: ${file}`);
  return `data:image/png;base64,${bytes.toString('base64')}`;
};
const writeDataUrl = async (file, dataUrl) => {
  const match = /^data:image\/png;base64,([A-Za-z0-9+/]+=*)$/.exec(dataUrl);
  if (!match) throw new Error(`Browser did not produce a PNG for ${file}`);
  await fs.writeFile(file, Buffer.from(match[1], 'base64'));
};

async function compare(browser, actualUrl, referenceUrl) {
  const page = await browser.newPage();
  try {
    return await page.evaluate(async ({ actualUrl, referenceUrl }) => {
      const decode = async url => {
        const image = new Image(); image.src = url;
        await image.decode();
        const canvas = document.createElement('canvas');
        canvas.width = image.naturalWidth; canvas.height = image.naturalHeight;
        const context = canvas.getContext('2d', { willReadFrequently: true });
        if (!context || !canvas.width || !canvas.height) throw new Error('Invalid PNG dimensions');
        context.drawImage(image, 0, 0);
        return { canvas, context, pixels: context.getImageData(0, 0, canvas.width, canvas.height).data };
      };
      const actual = await decode(actualUrl), reference = await decode(referenceUrl);
      if (actual.canvas.width !== reference.canvas.width || actual.canvas.height !== reference.canvas.height) {
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(actual.canvas.width, reference.canvas.width); canvas.height = Math.max(actual.canvas.height, reference.canvas.height);
        const context = canvas.getContext('2d'); context.fillStyle = '#dc2020'; context.fillRect(0, 0, canvas.width, canvas.height);
        return { equal: false, reason: 'dimensions-changed', actual: { width: actual.canvas.width, height: actual.canvas.height }, reference: { width: reference.canvas.width, height: reference.canvas.height }, diff: canvas.toDataURL('image/png') };
      }
      const diff = actual.context.createImageData(actual.canvas.width, actual.canvas.height);
      let differingPixels = 0;
      for (let i = 0; i < actual.pixels.length; i += 4) {
        const changed = actual.pixels[i] !== reference.pixels[i] || actual.pixels[i + 1] !== reference.pixels[i + 1] || actual.pixels[i + 2] !== reference.pixels[i + 2] || actual.pixels[i + 3] !== reference.pixels[i + 3];
        if (changed) {
          differingPixels++;
          diff.data[i] = 220; diff.data[i + 1] = 32; diff.data[i + 2] = 32; diff.data[i + 3] = 255;
        } else diff.data[i + 3] = 0;
      }
      const canvas = document.createElement('canvas'); canvas.width = actual.canvas.width; canvas.height = actual.canvas.height;
      const context = canvas.getContext('2d'); context.putImageData(diff, 0, 0);
      return { equal: differingPixels === 0, reason: differingPixels ? 'pixels-changed' : null, differingPixels, width: canvas.width, height: canvas.height, diff: canvas.toDataURL('image/png') };
    }, { actualUrl, referenceUrl });
  } finally { await page.close(); }
}

async function verifyVisualBaselinesImpl({ browser, captures, evidenceDir, baselineDir, provenance, sourceRevision }) {
  await fs.mkdir(evidenceDir, { recursive: true });
  const manifestPath = path.join(baselineDir, REGISTRY);
  let registry;
  try { registry = JSON.parse(await fs.readFile(manifestPath, 'utf8')); }
  catch (error) { throw new Error(`Cannot read ${manifestPath}: ${error.message}`); }
  if (registry.schemaVersion !== 1 || !Array.isArray(registry.images)) throw new Error('Unsupported visual baseline registry schema');
  const report = { schemaVersion: 1, status: 'failed', sourceRevision: sourceRevision ?? null, referenceSourceRevision: registry.sourceRevision ?? null, provenance, results: [] };
  const seen = new Set();
  for (const row of captures) {
    const id = key(row);
    if (seen.has(id)) throw new Error(`Duplicate capture identity: ${id}`);
    seen.add(id);
  }

  if (registry.status === 'pending-review' && registry.sourceRevision === null && registry.review === null && registry.provenance === null && registry.images.length === 0) {
    report.status = 'pending-review';
    report.note = 'No approved references exist; candidate pixels were not compared.';
    report.results = captures.map(row => ({ ...identity(row), status: row.status ?? 'not-run', file: row.file }));
    await fs.writeFile(path.join(evidenceDir, REPORT), JSON.stringify(report, null, 2));
    return report;
  }
  if (registry.status !== 'approved' || !registry.sourceRevision || !registry.review || !registry.provenance || registry.images.length === 0) throw new Error('Registry must be pending-review with empty approval fields or approved with sourceRevision, review, provenance, and images');
  const references = new Map();
  for (const image of registry.images) {
    const id = key(image);
    if (references.has(id)) throw new Error(`Duplicate approved reference identity: ${id}`);
    references.set(id, image);
  }
  const missing = [...seen].filter(id => !references.has(id));
  const extra = [...references.keys()].filter(id => !seen.has(id));
  if (missing.length || extra.length) throw new Error(`Approved reference matrix mismatch: missing=[${missing.join(', ')}], extra=[${extra.join(', ')}]`);
  if (!isDeepStrictEqual(provenance, registry.provenance)) throw new Error('Capture provenance does not match approved references');

  let failed = false;
  for (const row of captures) {
    const result = { ...identity(row), status: 'failed', file: row.file };
    try {
      if (row.status !== 'passed' || row.result !== 'passed') throw new Error(`Capture did not complete successfully (status=${row.status ?? 'missing'}, result=${row.result ?? 'missing'})`);
      const reference = references.get(key(row));
      const actualUrl = await readPng(evidenceDir, row.file);
      const referenceUrl = await readPng(baselineDir, reference.file);
      const comparison = await compare(browser, actualUrl, referenceUrl);
      Object.assign(result, comparison);
      result.referenceFile = reference.file;
      if (!comparison.equal) {
        failed = true;
        const stem = `${row.host}-${row.id}-${row.viewport}-${row.theme}`;
        await fs.copyFile(within(evidenceDir, row.file), path.join(evidenceDir, `${stem}.actual.png`));
        await fs.copyFile(within(baselineDir, reference.file), path.join(evidenceDir, `${stem}.reference.png`));
        await writeDataUrl(path.join(evidenceDir, `${stem}.diff.png`), comparison.diff);
        result.actualEvidence = `${stem}.actual.png`; result.referenceEvidence = `${stem}.reference.png`; result.diffEvidence = `${stem}.diff.png`;
      } else result.status = 'passed';
    } catch (error) { failed = true; result.error = error.message; }
    report.results.push(result);
  }
  report.status = failed ? 'failed' : 'passed';
  await fs.writeFile(path.join(evidenceDir, REPORT), JSON.stringify(report, null, 2));
  if (failed) throw new Error(`Visual baselines failed; see ${REPORT}`);
  return report;
}

export async function verifyVisualBaselines(options) {
  try { return await verifyVisualBaselinesImpl(options); }
  catch (error) {
    if (error.message !== `Visual baselines failed; see ${REPORT}`) {
      await fs.mkdir(options.evidenceDir, { recursive: true });
      await fs.writeFile(path.join(options.evidenceDir, REPORT), JSON.stringify({ schemaVersion: 1, status: 'failed', sourceRevision: options.sourceRevision ?? null, provenance: options.provenance ?? null, results: [], error: error.message }, null, 2));
    }
    throw error;
  }
}
