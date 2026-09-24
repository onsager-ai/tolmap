import { useMemo } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import type { CatalogueEntry, DistrictSymbols, MapDocument } from "@/types";
import {
  getServiceCatalogue,
  getServiceDistrictSymbols,
  getServiceMapDocument,
  pingService,
  type ServiceCatalogueEntry,
} from "@/api/client";
import { fetchJsonTracked } from "@/api/streaming";

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);
  return res.json() as Promise<T>;
}

// The catalogue index is small (one row per repo) -- no streaming/worker
// parse needed. The map document and district symbols below are the two
// documents that can be large enough for that to matter, so only they go
// through fetchJsonTracked (src/api/streaming.ts), for both the service and
// the static path -- a static build serves the exact same JSON shape, and
// the reader doesn't know or care which source produced the bytes it's
// waiting on.
function fetchStaticCatalogue(): Promise<CatalogueEntry[]> {
  return fetchJson<CatalogueEntry[]>("/maps/index.json");
}

function fetchStaticMap(owner: string, repo: string): Promise<MapDocument> {
  return fetchJsonTracked<MapDocument>(`/maps/${owner}/${repo}.json`, `map:${owner}/${repo}`, (res) => {
    if (!res.ok) throw new Error(`/maps/${owner}/${repo}.json: ${res.status} ${res.statusText}`);
  });
}

/** docs/API.md: "Static map collection copies this directory to
 * /maps/<owner>/<repo>.symbols/, so a static client can fetch one district
 * at /maps/<owner>/<repo>.symbols/<district>.json." */
function fetchStaticDistrictSymbols(owner: string, repo: string, district: number): Promise<DistrictSymbols> {
  return fetchJsonTracked<DistrictSymbols>(
    `/maps/${owner}/${repo}.symbols/${district}.json`,
    `symbols:${owner}/${repo}:${district}`,
    (res) => {
      if (!res.ok) throw new Error(`/maps/${owner}/${repo}.symbols/${district}.json: ${res.status} ${res.statusText}`);
    },
  );
}

function toCatalogueEntry(m: ServiceCatalogueEntry): CatalogueEntry {
  return {
    slug: m.slug,
    owner: m.owner,
    repo: m.repo,
    file: "",
    files: m.files,
    districts: m.districts,
    modularity: m.modularity,
    lang: m.lang,
    source: "service",
    commit: m.commit,
  };
}

/** Data-source resolution (milestone brief, "Data source resolution"): the
 * app discovers whether the job/index service is reachable, with no build
 * config beyond VITE_API_BASE, and picks per-query below. staleTime is
 * short — a service that comes up mid-session (or the reverse) should be
 * noticed on the next navigation, not require a reload. */
export function useServiceAvailable() {
  return useQuery({
    queryKey: ["service-available"],
    queryFn: () => pingService(),
    staleTime: 15_000,
    retry: false,
    refetchOnWindowFocus: true,
  });
}

/** Merges the two catalogues, service entries winning on a slug collision
 * because the service copy is the fresher one (it's keyed by (repo,
 * commit_sha) and re-indexes on demand; the static set is fixed at build
 * time). Sorted the way scripts/collect-maps.mjs sorts its own output. */
function mergeCatalogues(
  staticEntries: CatalogueEntry[] | undefined,
  serviceEntries: ServiceCatalogueEntry[] | undefined,
): CatalogueEntry[] {
  const bySlug = new Map<string, CatalogueEntry>();
  for (const e of staticEntries ?? []) bySlug.set(e.slug, e);
  for (const e of serviceEntries ?? []) bySlug.set(e.slug, toCatalogueEntry(e));
  return [...bySlug.values()].sort((a, b) => a.slug.localeCompare(b.slug));
}

export function useCatalogue() {
  const { data: available, isLoading: checkingService } = useServiceAvailable();

  const staticQuery = useQuery({
    queryKey: ["catalogue", "static"],
    queryFn: fetchStaticCatalogue,
    staleTime: Infinity,
    retry: false,
  });
  const serviceQuery = useQuery({
    queryKey: ["catalogue", "service"],
    queryFn: getServiceCatalogue,
    enabled: available === true,
    staleTime: 15_000,
    retry: false,
  });

  const data = mergeCatalogues(staticQuery.data, serviceQuery.data);
  const haveAnyData = staticQuery.data !== undefined || serviceQuery.data !== undefined;
  // Loading only while we have nothing at all to show yet — a slow or
  // absent service must never block the bundled catalogue from rendering.
  const isLoading = !haveAnyData && (checkingService || staticQuery.isLoading || (available === true && serviceQuery.isFetching));
  // Error only when every source that could have answered has failed.
  const isError = !haveAnyData && staticQuery.isError && (available === false || serviceQuery.isError);

  return { data, isLoading, isError, error: staticQuery.error ?? serviceQuery.error, serviceAvailable: available === true };
}

export function useMapDocument(owner: string, repo: string) {
  const { data: available, isLoading: checkingService } = useServiceAvailable();

  const serviceQuery = useQuery({
    queryKey: ["map", "service", owner, repo],
    queryFn: () => getServiceMapDocument(owner, repo),
    enabled: available === true && !!owner && !!repo,
    staleTime: 15_000,
    // A 404 from the service just means this repo hasn't been indexed there
    // yet — fall through to the static copy, don't retry a request that
    // will fail the same way three more times.
    retry: false,
  });

  // Any service failure (404 - not indexed there yet, or a real error) falls
  // through to the static copy rather than surfacing as this map's error.
  const serviceMiss = available === false || (available === true && serviceQuery.isError);

  const staticQuery = useQuery({
    queryKey: ["map", "static", owner, repo],
    queryFn: () => fetchStaticMap(owner, repo),
    enabled: serviceMiss && !!owner && !!repo,
    staleTime: Infinity,
    retry: false,
  });

  const doc = serviceQuery.data ?? staticQuery.data;
  const isLoading = !doc && (checkingService || (available === true && serviceQuery.isFetching) || (serviceMiss && staticQuery.isFetching));
  const isError = !doc && serviceMiss && staticQuery.isError;

  return {
    data: doc,
    isLoading,
    isError,
    error: staticQuery.error ?? serviceQuery.error,
    source: serviceQuery.data ? ("service" as const) : staticQuery.data ? ("static" as const) : undefined,
  };
}

/** Issue #82 C2 scope item 1: "Fetch a district only when one of its files
 * crosses the symbol gate, or when a file in it is selected." `districts` is
 * the caller's (MapView's) `wantedDistricts` set -- MapRenderer decides WHEN
 * a district becomes wanted (it alone knows the current zoom/viewport) and
 * reports it up through `onNeedSymbols`; this hook only fetches and caches
 * whatever it's told. `useQueries` (a dynamic-length array, unlike
 * `useQuery`) is the one hook in this file that can do that without calling
 * a hook conditionally.
 *
 * `staleTime: Infinity` + `retry: false`: a district's symbols never change
 * within one loaded map (same doc, same commit), and a map with NO symbols
 * sibling at all (any repo built before #85, or one still without the
 * static `.symbols/` collection) 404s once per requested district and
 * degrades silently rather than retrying a request that will 404 the same
 * way three more times (same reasoning as useMapDocument's own retry:false). */
export interface DistrictSymbolsMap {
  map: Map<number, DistrictSymbols>;
  /** Districts whose symbols fetch is currently in flight (query enabled,
   * no data or error settled yet). Issue #82 C2 follow-up: SelectionPanel's
   * file card uses this to show a single "loading symbols…" line instead of
   * flashing its old flat list while the hierarchical outline is still on
   * the way. */
  loading: Set<number>;
}

export function useDistrictSymbolsMap(
  owner: string,
  repo: string,
  source: "static" | "service" | undefined,
  districts: readonly number[],
): DistrictSymbolsMap {
  const queries = useQueries({
    queries: districts.map((d) => ({
      queryKey: ["symbols", source, owner, repo, d],
      queryFn: () =>
        source === "service" ? getServiceDistrictSymbols(owner, repo, d) : fetchStaticDistrictSymbols(owner, repo, d),
      enabled: !!source && !!owner && !!repo,
      staleTime: Infinity,
      retry: false,
    })),
  });
  // Recomputed only when what's actually LOADED or its LOADING state
  // changes (not on every unrelated re-render, which would hand MapCanvas a
  // new Map identity and trigger a full repaint for nothing -- see
  // MapCanvas.tsx's state-render effect, keyed on prop identity). `isLoading`
  // is folded into the signature too, so a fetch settling into a 404 (no
  // `dataUpdatedAt` change) still produces a fresh loading Set.
  const signature = districts
    .map((d, i) => `${d}:${queries[i]?.dataUpdatedAt ?? 0}:${queries[i]?.isLoading ? 1 : 0}`)
    .join(",");
  return useMemo(() => {
    const map = new Map<number, DistrictSymbols>();
    const loading = new Set<number>();
    districts.forEach((d, i) => {
      const data = queries[i]?.data;
      if (data) map.set(d, data);
      else if (queries[i]?.isLoading) loading.add(d);
    });
    return { map, loading };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);
}
