/** Optional same-origin host composition, not an authorization mechanism.
 * The default OSS service/static build needs no profile. Servers still own
 * identity, repository permission and every artifact/source request. */
export interface HostProfile {
  version: 1;
  private: boolean;
  workspace: string;
  repositories: { value: string; label: string }[];
  actions: { label: string; path: string; method?: "post" }[];
}

function readProfile(): HostProfile | undefined {
  const node = document.getElementById("tolmap-host");
  if (!node) return undefined;
  const value = JSON.parse(node.textContent || "null") as HostProfile | null;
  if (value?.version !== 1 || !Array.isArray(value.repositories) || !Array.isArray(value.actions)) {
    throw new Error("Unsupported host profile");
  }
  if (value.actions.some((a) => !a.path.startsWith("/") || a.path.startsWith("//"))) {
    throw new Error("Host actions must stay on this origin");
  }
  return value;
}

export const hostProfile = readProfile();
