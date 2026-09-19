/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Overrides src/api/client.ts's default same-origin `/api` base. */
  readonly VITE_API_BASE?: string;
}
