export { hostPath } from "../src/host";
import type { HostProfile } from "../src/host";
export type { HostProfile };

// Build-only substitution. No preview state or fixture branch enters production.
export let hostProfile: HostProfile | undefined;
export function setHost(host: string) {
  hostProfile = host === "hub" ? {
    version: 1, private: true, workspace: "Acme’s workspace",
    repositories: [{ value: "onsager-ai/tolmap", label: "acme/platform" }],
    actions: [{ label: "Connections", path: "/workspace" }, { label: "Settings", path: "/settings" }, { label: "Sign out", path: "/logout", method: "post" }],
    onboarding: {
      offered_repositories_path: "/fixture/offered", batch_index_path: "/fixture/batch", batch_progress_path: "/fixture/batches/{batch_id}", open_sheet_parameter: "add",
      install: null, manage_access: { label: "Manage GitHub access", path: "/fixture/access" },
    },
  } : undefined;
}

export function setOnboardingFixture(state: string) {
  if (hostProfile?.onboarding && state === "hosted-empty-connect") hostProfile.onboarding.install = { label: "Connect GitHub", path: "/fixture/install" };
}
