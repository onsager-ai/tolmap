// Issue #110 P1c: what the dependency-install sandbox can reach. Run inside
// the jail by `tolmap sandbox-exec --repo <dir> -- node egress_check.js`
// (ci.yml `scip-install` on a runner, scip-image-build.yml in the built
// image), after the sandbox's own self-test. The jail's network namespace
// has only loopback; its one way out is the egress proxy's bridge on
// 127.0.0.1:3128, which the package managers reach through HTTPS_PROXY.
//
// Expected: registry.npmjs.org answers a real request through the proxy,
// with TLS end to end; every other destination is refused by the proxy
// (another host, the metadata address, a suffix of the allowed name, a
// private IPv6 address, a port other than 443, plain HTTP); and nothing is
// reachable without the proxy (a direct connection, DNS). Prints one line
// per check and exits 1 if any check fails.
'use strict';
const dns = require('dns');
const net = require('net');
const tls = require('tls');

const PROXY = { host: '127.0.0.1', port: 3128 };
const results = [];

function record(name, ok, detail) {
  results.push({ name, ok });
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}: ${detail}`);
}

function withTimeout(promise, ms, fallback) {
  return Promise.race([promise, new Promise((resolve) => setTimeout(() => resolve(fallback), ms))]);
}

// Opens a CONNECT tunnel; resolves with the proxy's status line and, when
// it answered, the socket positioned after the response head.
function connectVia(target) {
  return withTimeout(
    new Promise((resolve) => {
      const socket = net.connect(PROXY, () => {
        socket.write(`CONNECT ${target} HTTP/1.1\r\nHost: ${target}\r\n\r\n`);
      });
      let head = Buffer.alloc(0);
      const onData = (chunk) => {
        head = Buffer.concat([head, chunk]);
        const end = head.indexOf('\r\n\r\n');
        if (end < 0) return;
        socket.removeListener('data', onData);
        resolve({ status: head.subarray(0, end).toString().split('\r\n')[0], socket });
      };
      socket.on('data', onData);
      socket.on('error', (error) => resolve({ status: `error ${error.code || error.message}` }));
      socket.on('end', () => resolve({ status: 'closed without a response' }));
    }),
    15000,
    { status: 'timeout' },
  );
}

async function registryAnswers() {
  const name = 'registry.npmjs.org through the proxy';
  const { status, socket } = await connectVia('registry.npmjs.org:443');
  if (!/^HTTP\/1\.[01] 200 /.test(status)) {
    if (socket) socket.destroy();
    return record(name, false, status);
  }
  const reply = await withTimeout(
    new Promise((resolve) => {
      const secure = tls.connect({ socket, servername: 'registry.npmjs.org' }, () => {
        secure.write(
          'GET /tiny-invariant/1.3.3 HTTP/1.1\r\nHost: registry.npmjs.org\r\n' +
            'Accept: application/json\r\nConnection: close\r\n\r\n',
        );
      });
      let text = '';
      secure.on('data', (chunk) => (text += chunk));
      secure.on('end', () => resolve(text));
      secure.on('error', (error) => resolve(`error ${error.code || error.message}`));
    }),
    30000,
    'timeout',
  );
  const line = reply.split('\r\n')[0];
  record(name, /^HTTP\/1\.1 200 /.test(line) && reply.includes('"tiny-invariant"'), `${status}; ${line}`);
}

async function proxyRefuses(target) {
  const { status, socket } = await connectVia(target);
  if (socket) socket.destroy();
  record(`the proxy refuses CONNECT ${target}`, /^HTTP\/1\.[01] 403 /.test(status), status);
}

async function proxyRefusesPlainHttp() {
  const status = await withTimeout(
    new Promise((resolve) => {
      const socket = net.connect(PROXY, () => {
        socket.write('GET http://169.254.169.254/latest/meta-data/ HTTP/1.1\r\nHost: 169.254.169.254\r\n\r\n');
      });
      let text = '';
      socket.on('data', (chunk) => {
        text += chunk;
        if (text.includes('\r\n')) {
          resolve(text.split('\r\n')[0]);
          socket.destroy();
        }
      });
      socket.on('error', (error) => resolve(`error ${error.code || error.message}`));
    }),
    10000,
    'timeout',
  );
  record('the proxy refuses plain HTTP to 169.254.169.254', /^HTTP\/1\.[01] 40[35] /.test(status), status);
}

function directBlocked(host, port) {
  return new Promise((resolve) => {
    const socket = net.connect({ host, port });
    const finish = (ok, detail) => {
      socket.destroy();
      record(`no direct connection to ${host}:${port}`, ok, detail);
      resolve();
    };
    socket.setTimeout(5000);
    socket.on('connect', () => finish(false, 'connected'));
    socket.on('timeout', () => finish(true, 'timed out'));
    socket.on('error', (error) => finish(true, error.code || error.message));
  });
}

function dnsUnavailable(name) {
  return withTimeout(
    new Promise((resolve) => {
      dns.lookup(name, (error, address) => {
        record(`no DNS for ${name}`, Boolean(error), error ? error.code || error.message : `resolved ${address}`);
        resolve();
      });
    }),
    15000,
    undefined,
  ).then((value) => {
    if (!results.some((result) => result.name === `no DNS for ${name}`)) {
      record(`no DNS for ${name}`, true, 'timed out');
    }
    return value;
  });
}

(async () => {
  await registryAnswers();
  for (const target of [
    'github.com:443',
    'codeload.github.com:443',
    'registry.yarnpkg.com:443',
    'registry.npmjs.org.evil.example:443',
    'registry.npmjs.org:80',
    '169.254.169.254:443',
    '169.254.169.254:80',
    '[fdaa::3]:443',
    '127.0.0.1:8787',
  ]) {
    await proxyRefuses(target);
  }
  await proxyRefusesPlainHttp();
  await directBlocked('169.254.169.254', 80);
  await directBlocked('140.82.112.3', 443);
  await directBlocked('1.1.1.1', 53);
  await directBlocked('127.0.0.1', 8787);
  await dnsUnavailable('github.com');
  await dnsUnavailable('registry.npmjs.org');
  const failed = results.filter((result) => !result.ok);
  console.log(`${results.length - failed.length}/${results.length} egress checks passed`);
  process.exit(failed.length === 0 ? 0 : 1);
})();
