import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const webRoot = path.resolve(process.argv[2] || path.join(repo, 'web'));
const output = path.resolve(process.argv[3] || path.join(repo, 'npm', 'stage', 'licenses', 'web'));
const rootPackage = JSON.parse(fs.readFileSync(path.join(webRoot, 'package.json'), 'utf8'));
const packages = new Map();

function packageDirectory(name, fromDirectory) {
  const parts = name.split('/');
  for (const candidate of [
    path.join(fromDirectory, 'node_modules', ...parts),
    path.join(path.dirname(fromDirectory), ...parts),
    path.join(webRoot, 'node_modules', ...parts),
  ]) {
    try {
      const real = fs.realpathSync(candidate);
      if (fs.statSync(path.join(real, 'package.json')).isFile()) return real;
    } catch {
      // A peer or optional dependency may not be installed in this tree.
    }
  }
  return null;
}

function licenseNames(packageJson) {
  const value = packageJson.license ?? packageJson.licenses;
  if (Array.isArray(value)) return value.map((item) => typeof item === 'string' ? item : item.type).filter(Boolean).join(' OR ');
  if (value && typeof value === 'object') return value.type || 'Declared in included license files';
  return typeof value === 'string' ? value : 'See included license files';
}

function walk(name, fromDirectory) {
  const directory = packageDirectory(name, fromDirectory);
  if (!directory) {
    throw new Error(`production dependency ${name} could not be resolved from ${fromDirectory}`);
  }
  const packageJson = JSON.parse(fs.readFileSync(path.join(directory, 'package.json'), 'utf8'));
  const key = `${packageJson.name}@${packageJson.version}`;
  if (packages.has(key)) return;

  const licenseDirectory = path.join(directory);
  const files = fs.readdirSync(licenseDirectory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && /^(licen[cs]es?|copying|copyright|notice|patents)(?:[._-].*)?$/i.test(entry.name))
    .map((entry) => entry.name)
    .sort();
  if (files.length === 0) {
    throw new Error(`${key} has no top-level license/copyright text to include`);
  }

  const id = `${packageJson.name.replace(/^@/, '').replaceAll('/', '__').replaceAll('/', '_')}@${packageJson.version}`;
  const targetDirectory = path.join(output, id);
  fs.mkdirSync(targetDirectory, { recursive: true });
  for (const filename of files) {
    fs.copyFileSync(path.join(licenseDirectory, filename), path.join(targetDirectory, filename));
  }
  const record = {
    name: packageJson.name,
    version: packageJson.version,
    license: licenseNames(packageJson),
    source: `https://registry.npmjs.org/${packageJson.name}/-/${packageJson.name.split('/').at(-1)}-${packageJson.version}.tgz`,
    files: files.map((filename) => path.posix.join('licenses', 'web', id, filename)),
  };
  packages.set(key, record);

  const dependencies = {
    ...packageJson.dependencies,
    ...packageJson.optionalDependencies,
  };
  for (const dependency of Object.keys(dependencies || {}).sort()) {
    if (packageDirectory(dependency, directory)) walk(dependency, directory);
  }
}

fs.rmSync(output, { recursive: true, force: true });
fs.mkdirSync(output, { recursive: true });
for (const dependency of Object.keys(rootPackage.dependencies || {}).sort()) {
  walk(dependency, webRoot);
}

const records = [...packages.values()].sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
const lines = [
  '# Web dependency notices',
  '',
  'The production viewer bundle includes these production dependencies. Exact package versions, declared licenses, package-source tarballs and copied license texts follow.',
  '',
  '| Package | Version | Declared license | Source | License text |',
  '| --- | --- | --- | --- | --- |',
];
for (const record of records) {
  const files = record.files.map((filename) => `\`${filename}\``).join('<br>');
  lines.push(`| \`${record.name}\` | \`${record.version}\` | ${record.license.replaceAll('|', '\\|')} | [npm source](${record.source}) | ${files} |`);
}
fs.writeFileSync(path.join(output, 'THIRD_PARTY_NOTICES.md'), `${lines.join('\n')}\n`);
console.log(`collected license texts for ${records.length} production web dependencies`);
