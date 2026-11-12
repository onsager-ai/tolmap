import { hostPath } from "@/host";

export class HostRequestError extends Error {
  status: number;
  constructor(status: number) {
    super(status === 401 || status === 403 ? "Your session has expired. Sign in again to add repositories." : "GitHub repositories are unavailable right now. Try again shortly.");
    this.status = status;
  }
}

/** Host endpoints stay on the browser origin, independent of the OSS API base. */
export async function hostRequest<T>(path: string, body?: unknown): Promise<T> {
  const response = await fetch(hostPath(path), { credentials: "same-origin", headers: body ? { "Content-Type": "application/json", Accept: "application/json" } : { Accept: "application/json" }, ...(body ? { method: "POST", body: JSON.stringify(body) } : {}) });
  if (!response.ok) throw new HostRequestError(response.status);
  return response.json() as Promise<T>;
}
