import { useNavigate } from "@tanstack/react-router";
import type { CatalogueEntry } from "@/types";
import type { Layer } from "@/map/constants";
import { GeoLayerControls } from "./GeoLayerControls";
import { useIsNarrow } from "@/hooks/useIsNarrow";

interface Props {
  catalogue: CatalogueEntry[] | undefined;
  owner: string;
  repo: string;
  layer: Layer;
  onLayer(l: Layer): void;
}

/** The reference's <select id="repo"> switched between repos inlined in one
 * HTML page; here each repo is its own route, so the same dropdown just
 * navigates. It's the one piece of chrome that reaches outside this map's
 * own state. */
export function TopBar({ catalogue, owner, repo, layer, onLayer }: Props) {
  const navigate = useNavigate();
  const narrow = useIsNarrow();
  const slug = `${owner}/${repo}`;

  // Bug (reported 09-21, unverified until now): the dropdown showed a
  // PREVIOUS repo's slug while a different one was actually on screen.
  // Reproduced by loading a deep link to a repo the catalogue hasn't
  // caught up to yet (a slow/partial static or service fetch, or -- as
  // found while reproducing this -- a build whose bundled /maps/index.json
  // doesn't include every repo the app can still be linked to). Root cause:
  // a native <select value={x}> where `x` matches none of its <option>s
  // does NOT clear the visible selection -- it silently leaves whatever was
  // selected before (or index 0 on first load), so the currently-viewed
  // repo can render correctly while the dropdown keeps showing a stale one.
  // The fix is to guarantee the current repo always HAS a matching option,
  // synthesized the same way the catalogue-still-undefined fallback below
  // already does for that case, rather than only when `catalogue` itself is
  // wholly missing.
  const options = catalogue ?? [];
  const withCurrent = options.some((m) => m.slug === slug)
    ? options
    : [{ slug, owner, repo, file: "", files: 0, districts: 0, modularity: 0, lang: "" }, ...options];

  return (
    <div
      className="flex flex-wrap items-center gap-2.5 border-b border-[var(--rule)] bg-[var(--chrome)] px-3 py-2 text-[var(--on)]"
      style={{ paddingTop: "calc(8px + env(safe-area-inset-top, 0px))" }}
    >
      {!narrow && <span className="font-sans text-[15px] font-semibold">tolmap</span>}
      <select
        aria-label="Repository"
        value={slug}
        onChange={(e) => {
          const [o, r] = e.target.value.split("/");
          navigate({ to: "/$owner/$repo", params: { owner: o, repo: r }, search: { geo: "r", layer: "d" } });
        }}
        className={`rounded-md border border-[var(--rule)] bg-[var(--chrome2)] px-2 py-1.5 text-[11.5px] text-[var(--on)] ${narrow ? "min-w-0 flex-1 py-1.5 text-xs" : ""}`}
      >
        {withCurrent.map((m) => (
          <option key={m.slug} value={m.slug}>
            {narrow ? m.slug : `${m.slug} · ${m.files} files`}
          </option>
        ))}
      </select>
      <span className="flex-1" />
      <GeoLayerControls layer={layer} onLayer={onLayer} />
    </div>
  );
}
