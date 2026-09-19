import { fileURLToPath, URL } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The dev-only counterpart of src/api/client.ts's same-origin `/api`
// default: proxy it to the job/index service running locally. Loopback
// only, deliberately — "Nothing here may assume or enable a public origin"
// (milestone brief). $TOLMAP_API_PROXY_TARGET overrides the port for
// whatever the service (or scripts/mock-api-server.mjs) is actually
// listening on; the exact port isn't fixed by the contract yet.
const apiProxyTarget = process.env.TOLMAP_API_PROXY_TARGET ?? "http://127.0.0.1:8787";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
      "@bindings": fileURLToPath(new URL("../bindings", import.meta.url)),
    },
  },
  server: {
    proxy: {
      "/api": {
        target: apiProxyTarget,
        changeOrigin: true,
        // SSE (GET /api/jobs/{id}/events) is a plain streamed HTTP
        // response, not a WebSocket upgrade — no `ws: true` needed, and
        // http-proxy does not buffer it.
      },
    },
  },
});
