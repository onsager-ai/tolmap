#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const os = require('node:os');
const path = require('node:path');
const fs = require('node:fs');

const supportedGlibc = { major: 2, minor: 35 };

function parseVersion(value) {
  const match = /^(\d+)\.(\d+)(?:\.(\d+))?/.exec(value || '');
  return match ? { major: Number(match[1]), minor: Number(match[2]) } : null;
}

function glibcVersion() {
  if (process.env.TOLMAP_TEST_GLIBC_VERSION !== undefined) {
    return process.env.TOLMAP_TEST_GLIBC_VERSION;
  }
  try {
    return process.report?.getReport()?.header?.glibcVersionRuntime;
  } catch {
    return undefined;
  }
}

const platform = process.env.TOLMAP_TEST_PLATFORM || process.platform;
const arch = process.env.TOLMAP_TEST_ARCH || process.arch;
const libc = glibcVersion();
const libcVersion = parseVersion(libc);
const meetsGlibcFloor = libcVersion &&
  (libcVersion.major > supportedGlibc.major ||
    (libcVersion.major === supportedGlibc.major && libcVersion.minor >= supportedGlibc.minor));

if (platform !== 'linux' || arch !== 'x64' || !meetsGlibcFloor) {
  const detected = libc ? `glibc ${libc}` : 'musl or an unidentified C library';
  process.stderr.write(
    `tolmap 0.1.0 supports linux-x64 with glibc >= 2.35; detected ${platform}-${arch} (${detected}).\n` +
    'Install tolmap on a supported Linux x64 system or build from source.\n',
  );
  process.exit(1);
}

const binary = path.resolve(__dirname, '..', 'lib', 'tolmap');
const args = process.argv.slice(2);
if (args[0] === 'local' &&
    !args.some((arg) => arg === '--static-dir' || arg.startsWith('--static-dir='))) {
  args.push('--static-dir', path.resolve(__dirname, '..', 'web', 'dist'));
}

const child = spawn(binary, args, { stdio: 'inherit' });
const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'];
// Local workers have their own process groups. Forward to the whole descendant
// tree as well as the server, so terminating npx cannot leave an engine behind.
function descendants(pid, seen = new Set()) {
  try {
    for (const task of fs.readdirSync(`/proc/${pid}/task`)) {
      const children = fs.readFileSync(`/proc/${pid}/task/${task}/children`, 'utf8').trim();
      for (const value of children.split(/\s+/).filter(Boolean)) {
        const next = Number(value);
        if (!seen.has(next)) { seen.add(next); descendants(next, seen); }
      }
    }
  } catch (error) {
    if (error.code !== 'ENOENT' && error.code !== 'ESRCH') throw error;
  }
  return seen;
}
let stopping = false;
for (const signal of signals) {
  process.on(signal, () => {
    if (stopping || child.exitCode !== null || child.signalCode !== null) return;
    stopping = true;
    for (const pid of [...descendants(child.pid)].reverse()) {
      try {
        process.kill(pid, signal);
        process.kill(pid, 'SIGCONT');
      } catch (error) {
        if (error.code !== 'ESRCH') throw error;
      }
    }
    child.kill(signal);
  });
}

child.on('error', (error) => {
  process.stderr.write(`tolmap could not start the packaged binary: ${error.message}\n`);
  process.exitCode = 1;
});

child.on('exit', (code, signal) => {
  if (signal) {
    for (const forwarded of signals) process.removeAllListeners(forwarded);
    process.kill(process.pid, signal);
    process.exitCode = 128 + (os.constants.signals[signal] || 1);
  } else {
    process.exitCode = code ?? 1;
  }
});
