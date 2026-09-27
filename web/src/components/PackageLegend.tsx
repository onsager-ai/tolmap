import { useState } from "react";
import type { PackageGrouping } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";
import { useIsNarrow } from "@/hooks/useIsNarrow";

interface Props {
  grouping: PackageGrouping;
  auto: boolean;
  minDepth: number;
  maxDepth: number;
  onDepth(depth: number): void;
  /** "sheet": inside the phone's Layers sheet (docs/UX.md §4.6) -- static,
   * always expanded, 44 px depth buttons. "float" (default): the desktop
   * legend over the map. */
  variant?: "float" | "sheet";
}

/** Package colour key and its only control. It is React chrome over the map,
 * not SVG content: changing depth swaps one precomputed file-colour array in
 * MapRenderer state and leaves the current view transform untouched. */
export function PackageLegend({ grouping, auto, minDepth, maxDepth, onDepth, variant = "float" }: Props) {
  const narrow = useIsNarrow();
  const [phoneOpen, setPhoneOpen] = useState(false);
  if (variant === "sheet") {
    return (
      <section aria-label="Package legend" data-package-legend data-package-expanded="true" className="mt-2 text-meta text-[var(--dim)]">
        <div className="flex items-center gap-2">
          <span className="mr-auto text-small text-[var(--on)]">Folder depth</span>
          <button
            type="button"
            aria-label="Decrease package depth"
            disabled={grouping.depth <= minDepth}
            onClick={() => onDepth(grouping.depth - 1)}
            className="h-11 w-11 rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] text-body text-[var(--on)] disabled:opacity-30"
          >
            −
          </button>
          <span className="min-w-[72px] text-center font-mono text-small" data-package-depth>
            {grouping.depth}{auto ? " · auto" : ""}
          </span>
          <button
            type="button"
            aria-label="Increase package depth"
            disabled={grouping.depth >= maxDepth}
            onClick={() => onDepth(grouping.depth + 1)}
            className="h-11 w-11 rounded-[12px] border border-[var(--rule)] bg-[var(--chrome2)] text-body text-[var(--on)] disabled:opacity-30"
          >
            +
          </button>
        </div>
        <div className="mt-2" data-package-groups>
          {grouping.groups.map((group) => (
            <div
              key={group.path ?? "other"}
              className="grid min-h-[36px] grid-cols-[12px_1fr_auto] items-center gap-2 border-t border-[var(--rule)] first:border-t-0"
            >
              <i className="h-3 w-3 rounded-sm" style={{ background: group.color }} />
              <span className="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[var(--on)]">
                {group.other ? "other" : formatDirectory(group.path!)}
              </span>
              <span className="font-mono">{group.count}</span>
            </div>
          ))}
        </div>
      </section>
    );
  }
  const expanded = !narrow || phoneOpen;

  return (
    <aside
      aria-label="Package legend"
      data-package-legend
      data-package-expanded={expanded ? "true" : "false"}
      className={`absolute left-2.5 top-[62px] z-10 rounded-md border border-[var(--rule)] bg-[rgba(var(--chrome-float-rgb),0.94)] text-meta text-[var(--dim)] shadow-lg min-[821px]:bottom-2.5 min-[821px]:top-auto ${expanded ? "w-[240px] px-2.5 py-2" : "w-auto p-0"}`}
    >
      {!expanded ? (
        <button
          type="button"
          aria-label="Expand package legend"
          aria-expanded="false"
          onClick={() => setPhoneOpen(true)}
          className="whitespace-nowrap px-2.5 py-1.5 text-label uppercase text-[var(--on)]"
        >
          packages · depth {grouping.depth} ▾
        </button>
      ) : (
        <>
          <div className="mb-1.5 flex items-center gap-1.5">
            {narrow ? (
              <button
                type="button"
                aria-label="Collapse package legend"
                aria-expanded="true"
                onClick={() => setPhoneOpen(false)}
                className="mr-auto text-label uppercase text-[var(--on)]"
              >
                packages ▴
              </button>
            ) : (
              <b className="mr-auto text-label uppercase text-[var(--on)]">packages</b>
            )}
            <button
              type="button"
              aria-label="Decrease package depth"
              disabled={grouping.depth <= minDepth}
              onClick={() => onDepth(grouping.depth - 1)}
              className="h-5 w-5 rounded border border-[var(--rule)] text-[12px] text-[var(--on)] disabled:opacity-30"
            >
              −
            </button>
            <span className="min-w-[50px] text-center" data-package-depth>
              depth {grouping.depth}{auto ? " · auto" : ""}
            </span>
            <button
              type="button"
              aria-label="Increase package depth"
              disabled={grouping.depth >= maxDepth}
              onClick={() => onDepth(grouping.depth + 1)}
              className="h-5 w-5 rounded border border-[var(--rule)] text-[12px] text-[var(--on)] disabled:opacity-30"
            >
              +
            </button>
          </div>
          <div className="max-h-[190px] overflow-y-auto" data-package-groups>
            {grouping.groups.map((group) => (
              <div
                key={group.path ?? "other"}
                className="grid grid-cols-[8px_1fr_auto] items-center gap-1.5 border-t border-[var(--rule)] py-0.5 first:border-t-0"
              >
                <i className="h-2 w-2 rounded-sm" style={{ background: group.color }} />
                <span className="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[var(--on)]">
                  {group.other ? "other" : formatDirectory(group.path!)}
                </span>
                <span className="font-mono">{group.count}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </aside>
  );
}
