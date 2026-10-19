// CI only: reuse the committed Rust viewer fixture and production fonts.
import fs from 'node:fs';
import { gunzipSync } from 'node:zlib';
import { execFileSync } from 'node:child_process';

fs.rmSync('storybook-public', { recursive: true, force: true });
fs.mkdirSync('storybook-public/fixtures', { recursive: true });
fs.cpSync('public/fonts', 'storybook-public/fonts', { recursive: true });
execFileSync('rustc', ['--edition=2021', 'scripts/export-local-storybook.rs', '-o', 'storybook-public/export-local'], { stdio: 'inherit' });
execFileSync('./storybook-public/export-local', ['storybook-public/local-pages'], { stdio: 'inherit' });
fs.rmSync('storybook-public/export-local');
fs.writeFileSync('storybook-public/fixtures/map.json', gunzipSync(fs.readFileSync('check-fixtures/onsager-ai__tolmap.json.gz')));
execFileSync('tar', ['-xzf', 'check-fixtures/onsager-ai__tolmap.symbols.tar.gz', '-C', 'storybook-public/fixtures']);
execFileSync('pnpm', ['exec', 'msw', 'init', 'storybook-public', '--no-save'], { stdio: 'inherit' });
