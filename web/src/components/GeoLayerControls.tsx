import type { Geo, Layer } from "@/map/constants";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useIsNarrow } from "@/hooks/useIsNarrow";

const GEO_LABEL: Record<Geo, string> = { r: "regions", p: "plots", t: "treemap" };
const LAYER_LABEL: Record<Layer, string> = { d: "district", c: "churn", x: "complexity" };
const GEO_ORDER: Geo[] = ["r", "p", "t"];
const LAYER_ORDER: Layer[] = ["d", "c", "x"];

interface Props {
  geo: Geo;
  layer: Layer;
  onGeo(g: Geo): void;
  onLayer(l: Layer): void;
}

/** Desktop: two labelled segmented groups (matches #gR/#gP/#gT and
 * #lD/#lC/#lX in the reference). Phone: two compact cycle-buttons (#mGeo /
 * #mLayer) — there isn't room for six labelled buttons in the compact bar,
 * so tapping steps to the next value, same as the reference's mGeo/mLayer
 * onclick handlers. */
export function GeoLayerControls({ geo, layer, onGeo, onLayer }: Props) {
  const narrow = useIsNarrow();

  if (narrow) {
    return (
      <>
        <button
          className="whitespace-nowrap rounded-md border border-[var(--rule)] bg-[var(--chrome2)] px-2.5 py-1.5 text-[11px] text-[var(--on)]"
          onClick={() => onGeo(GEO_ORDER[(GEO_ORDER.indexOf(geo) + 1) % 3])}
          aria-label="Toggle geometry"
        >
          {GEO_LABEL[geo]}
        </button>
        <button
          className="whitespace-nowrap rounded-md border border-[var(--rule)] bg-[var(--chrome2)] px-2.5 py-1.5 text-[11px] text-[var(--on)]"
          onClick={() => onLayer(LAYER_ORDER[(LAYER_ORDER.indexOf(layer) + 1) % 3])}
          aria-label="Cycle layer"
        >
          {LAYER_LABEL[layer]}
        </button>
      </>
    );
  }

  return (
    <>
      <span className="text-[9.5px] uppercase tracking-[0.12em] text-[var(--dim)]">geometry</span>
      <ToggleGroup type="single" value={geo} onValueChange={(v) => v && onGeo(v as Geo)} aria-label="Geometry">
        {GEO_ORDER.map((g) => (
          <ToggleGroupItem key={g} value={g}>
            {GEO_LABEL[g]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      <span className="text-[9.5px] uppercase tracking-[0.12em] text-[var(--dim)]">layer</span>
      <ToggleGroup type="single" value={layer} onValueChange={(v) => v && onLayer(v as Layer)} aria-label="Layer">
        {LAYER_ORDER.map((l) => (
          <ToggleGroupItem key={l} value={l}>
            {LAYER_LABEL[l]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    </>
  );
}
