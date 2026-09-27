import type { CatalogueEntry, MapDocument } from "@/types";

/** One option per mapped repository, its label carrying the file count.
 *
 * Issue #171 ("the desktop repository select shows '0 files' for every
 * map"): a native <select value={x}> whose value matches none of its
 * <option>s silently shows a stale one (the 09-21 bug), so the current repo
 * always gets an option -- and that option used to be a placeholder built
 * with `files: 0`, whose label then printed "· 0 files". It is needed
 * whenever the catalogue does not list the map on screen: CI serves dify
 * from check-fixtures/, unpacked after collect-maps wrote /maps/index.json,
 * so dify is never in the catalogue there (every CI frame said
 * "langgenius/dify · 0 files"); in production, a deep link to a repository
 * the service indexed after the catalogue was fetched, or one the static
 * index does not carry, is the same case. The current repository's count
 * now comes from the map document on screen, which is always loaded here
 * and is the authority for what the map shows, listed or not. */
export function repoOptions(catalogue: CatalogueEntry[] | undefined, doc: MapDocument, owner: string, repo: string) {
  const slug = `${owner}/${repo}`;
  const rows = (catalogue ?? []).map((m) => ({ slug: m.slug, files: m.files }));
  const current = { slug, files: doc.F.length };
  const i = rows.findIndex((m) => m.slug === slug);
  if (i >= 0) rows[i] = current;
  else rows.unshift(current);
  return rows.map((m) => ({ slug: m.slug, label: `${m.slug} · ${m.files.toLocaleString("en-US")} files` }));
}
