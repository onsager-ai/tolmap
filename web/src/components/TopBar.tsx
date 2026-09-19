import { useNavigate } from "@tanstack/react-router";
import type { CatalogueEntry } from "@/types";
import type { Geo, Layer } from "@/map/constants";
import { GeoLayerControls } from "./GeoLayerControls";
import { useIsNarrow } from "@/hooks/useIsNarrow";

interface Props {
  catalogue: CatalogueEntry[] | undefined;
  owner: string;
  repo: string;
  geo: Geo;
  layer: Layer;
  onGeo(g: Geo): void;
  onLayer(l: Layer): void;
}

/** The reference's <select id="repo"> switched between repos inlined in one
 * HTML page; here each repo is its own route, so the same dropdown just
 * navigates. It's the one piece of chrome that reaches outside this map's
 * own state. */
export function TopBar({ catalogue, owner, repo, geo, layer, onGeo, onLayer }: Props) {
  const navigate = useNavigate();
  const narrow = useIsNarrow();
  const slug = `${owner}/${repo}`;

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
        {(catalogue ?? [{ slug, owner, repo, file: "", files: 0, districts: 0, modularity: 0, lang: "" }]).map((m) => (
          <option key={m.slug} value={m.slug}>
            {narrow ? m.slug : `${m.slug} · ${m.files} files`}
          </option>
        ))}
      </select>
      <span className="flex-1" />
      <GeoLayerControls geo={geo} layer={layer} onGeo={onGeo} onLayer={onLayer} />
    </div>
  );
}
