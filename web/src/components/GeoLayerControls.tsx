import type { Layer } from "@/map/constants";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useIsNarrow } from "@/hooks/useIsNarrow";

const LAYER_LABEL: Record<Layer, string> = { d: "district", c: "churn", x: "complexity", p: "package" };
const LAYER_ORDER: Layer[] = ["d", "c", "x", "p"];

interface Props {
  layer: Layer;
  onLayer(l: Layer): void;
}

/** Desktop: one labelled segmented group (matches #lD/#lC/#lX in the
 * reference). Phone: one compact cycle-button (#mLayer) — there isn't room
 * for a labelled group in the compact bar, so tapping steps to the next
 * value, same as the reference's mLayer onclick handler.
 *
 * The geometry toggle (regions/plots/treemap) that used to live here is
 * gone (owner decision, 2026-09-22): plots needs `P` parcel data only 2 of
 * the 9 acceptance fixtures carry, and treemap trades away the map's own
 * silhouette, which is the thing this whole project is about. See
 * map/constants.ts's GEO_ORDER/GEO_LABEL comment for how to bring it back
 * (untouched, not deleted -- just moved out of this component file). */
export function GeoLayerControls({ layer, onLayer }: Props) {
  const narrow = useIsNarrow();

  if (narrow) {
    return (
      <button
        className="whitespace-nowrap rounded-md border border-[var(--rule)] bg-[var(--chrome2)] px-2.5 py-1.5 text-[11px] text-[var(--on)]"
        onClick={() => onLayer(LAYER_ORDER[(LAYER_ORDER.indexOf(layer) + 1) % LAYER_ORDER.length])}
        aria-label="Cycle layer"
      >
        {LAYER_LABEL[layer]}
      </button>
    );
  }

  return (
    <>
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
