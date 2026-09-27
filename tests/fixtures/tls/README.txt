TEST ONLY. Throwaway TLS material for the worker listener's CI tests
(tests/service_remote.rs, src/service/workers.rs). Never use it anywhere else.

- test-only-ca.pem: a self-signed CA ("tolmap TEST-ONLY worker CA").
- test-only-server.pem / test-only-server.key: a server certificate it signed
  for IP 127.0.0.1 and DNS localhost, and its P-256 key (PKCS#8).

The private keys are public by being in this repository; nothing they sign
can be trusted. The CA's own key was discarded after signing. Git does not
keep file modes, so tests copy the key to a 0600 file before using it (the
master refuses a key file that group or others can read).
