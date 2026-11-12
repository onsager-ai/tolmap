/** Optional same-origin host composition. Servers own all authorization. */
export type { HostProfile } from "@bindings/HostProfile";
import type { HostProfile } from "@bindings/HostProfile";

export function hostPath(path: string) {
  if (!path.startsWith("/") || path.startsWith("//") || path.includes("\\") || [...path].some(character => character.charCodeAt(0) <= 32 || character.charCodeAt(0) === 127)) throw new Error("Host paths must stay on this origin");
  return path;
}

function readProfile(): HostProfile | undefined {
  const node = document.getElementById("tolmap-host");
  if (!node) return undefined;
  const value = JSON.parse(node.textContent || "null") as HostProfile | null;
  if (value?.version !== 1 || !Array.isArray(value.repositories) || !Array.isArray(value.actions)) throw new Error("Unsupported host profile");
  for (const action of value.actions) hostPath(action.path);
  if (value.onboarding) {
    const onboarding = value.onboarding;
    hostPath(onboarding.offered_repositories_path);
    hostPath(onboarding.batch_index_path);
    hostPath(onboarding.batch_progress_path);
    if (onboarding.batch_progress_path.split("{batch_id}").length !== 2) throw new Error("Batch progress path must contain {batch_id} once");
    if (!onboarding.open_sheet_parameter) throw new Error("Missing open sheet parameter");
    if (onboarding.install) hostPath(onboarding.install.path);
    if (onboarding.manage_access) hostPath(onboarding.manage_access.path);
  }
  return value;
}

export const hostProfile = readProfile();
