import type { HostProfile } from "../src/host";
export type { HostProfile };

// Build-only substitution. No preview state or fixture branch enters production.
export let hostProfile: HostProfile | undefined;
export function setHost(host: string) {
  hostProfile = host === "hub" ? {
    version: 1, private: true, workspace: "Acme’s workspace",
    repositories: [{ value: "onsager-ai/tolmap", label: "acme/platform" }],
    actions: [{ label: "Connections and history", path: "/workspace" }, { label: "Sign out", path: "/logout", method: "post" }],
  } : undefined;
}
