import { useQuery } from "@tanstack/react-query";
import type { CatalogueEntry, MapDocument } from "@/types";

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);
  return res.json() as Promise<T>;
}

/** The catalogue is written by scripts/collect-maps.mjs at dev/build time —
 * see web/README or the script itself. Static JSON, no backend: a later
 * milestone adds a job service, this one only serves what's already on disk. */
export function useCatalogue() {
  return useQuery({
    queryKey: ["catalogue"],
    queryFn: () => fetchJson<CatalogueEntry[]>("/maps/index.json"),
    staleTime: Infinity,
  });
}

export function useMapDocument(owner: string, repo: string) {
  return useQuery({
    queryKey: ["map", owner, repo],
    queryFn: () => fetchJson<MapDocument>(`/maps/${owner}/${repo}.json`),
    staleTime: Infinity,
  });
}
