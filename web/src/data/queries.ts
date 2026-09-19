import { useQuery } from "@tanstack/react-query";
import type { CatalogueEntry, MapDocument } from "@/types";
import {
  getServiceCatalogue,
  getServiceMapDocument,
  pingService,
  type ServiceCatalogueEntry,
} from "@/api/client";

async function fetchJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status} ${res.statusText}`);
  return res.json() as Promise<T>;
}

function fetchStaticCatalogue(): Promise<CatalogueEntry[]> {
  return fetchJson<CatalogueEntry[]>("/maps/index.json");
}

function fetchStaticMap(owner: string, repo: string): Promise<MapDocument> {
  return fetchJson<MapDocument>(`/maps/${owner}/${repo}.json`);
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
