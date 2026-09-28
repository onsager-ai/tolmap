import { useMemo, type ReactNode } from "react";
import type { MapDocument } from "@/types";
import type { Layer } from "@/map/constants";
import { buildLayerOverview, displayLanguage, layerOverviewHeadline, type LayerOverview } from "@/map/layerOverview";
import type { PackageGrouping } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";

export function LayerOverviewHeadline({
  overview,
  layer,
  className,
}: {
  overview: LayerOverview;
  layer: Layer;
  className: string;
}) {
  const headline = layerOverviewHeadline(overview, layer);
  return (
    <>
      <h2 data-overview-headline data-overview-layer={layer} className={className}>{headline.primary}</h2>
      {headline.secondary && <p data-overview-subheadline className="mt-1.5 text-meta leading-[1.45] text-[var(--dim)]">{headline.secondary}</p>}
    </>
  );
}

export interface LayerOverviewIndexProps {
  doc: MapDocument;
  overview: LayerOverview;
  layer: Exclude<Layer, "d">;
  touch: boolean;
  onSelectDistrict(district: number): void;
  onSelectFile(file: number): void;
  onHighlightDistricts?(districts: readonly number[] | null): void;
  onFrameDistricts(districts: readonly number[]): void;
}

function displayNumber(value: number): string {
  return value.toLocaleString("en-US", { maximumFractionDigits: 2 });
}

function DistrictRow({
  district,
  name,
  detail,
  touch,
  onSelect,
  onHighlight,
  bar,
  total,
}: {
  district: number;
  name: string;
  detail: string;
  touch: boolean;
  onSelect(): void;
  onHighlight?(districts: readonly number[] | null): void;
  bar?: number;
  total?: string;
}) {
  const height = touch ? "min-h-[64px]" : "h-[50px] min-h-[50px]";
  return (
    <button
      type="button"
      data-overview-district-row={district}
      onMouseEnter={onHighlight ? () => onHighlight([district]) : undefined}
      onMouseLeave={onHighlight ? () => onHighlight(null) : undefined}
      onFocus={onHighlight ? () => onHighlight([district]) : undefined}
      onBlur={onHighlight ? () => onHighlight(null) : undefined}
      onClick={onSelect}
      className={`flex ${height} w-full shrink-0 items-center gap-2.5 rounded-[10px] px-2.5 py-1.5 text-left hover:bg-[var(--chrome-hover)]`}
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-center justify-between gap-2">
          <span className="truncate text-small font-semibold">{name}</span>
          {total && <span className="shrink-0 font-mono text-meta text-[var(--dim)]">{total}</span>}
        </span>
        <span className="truncate text-meta text-[var(--dim)]">{detail}</span>
        {bar != null && (
          <span aria-hidden="true" className="h-[3px] w-full overflow-hidden rounded-full bg-[var(--rule)]">
            <span className="block h-full rounded-full bg-[var(--accent)]" style={{ width: `${Math.max(0, Math.min(100, bar * 100))}%` }} />
          </span>
        )}
      </span>
    </button>
  );
}

function FileRow({
  doc,
  file,
  detail,
  touch,
  onSelect,
  onHighlight,
}: {
  doc: MapDocument;
  file: number;
  detail: string;
  touch: boolean;
  onSelect(): void;
  onHighlight?(districts: readonly number[] | null): void;
}) {
  const path = doc.F[file];
  const slash = path.lastIndexOf("/");
  return (
    <button
      type="button"
      data-overview-file={file}
      data-layer-overview-file-row={file}
      onMouseEnter={onHighlight ? () => onHighlight([doc.N[file][0]]) : undefined}
      onMouseLeave={onHighlight ? () => onHighlight(null) : undefined}
      onFocus={onHighlight ? () => onHighlight([doc.N[file][0]]) : undefined}
      onBlur={onHighlight ? () => onHighlight(null) : undefined}
      onClick={onSelect}
      className={`flex ${touch ? "min-h-[64px]" : "h-[50px] min-h-[50px]"} w-full shrink-0 items-center justify-between gap-2 rounded-[10px] px-2.5 py-1.5 text-left hover:bg-[var(--chrome-hover)]`}
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate font-mono text-small">{slash < 0 ? path : path.slice(slash + 1)}</span>
        <span className="truncate font-mono text-meta text-[var(--dim)]">{slash < 0 ? "(repo root)" : path.slice(0, slash)}</span>
      </span>
      <span className="shrink-0 font-mono text-meta text-[var(--dim)]">{detail}</span>
    </button>
  );
}

function IndexSection({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mt-4" data-layer-overview-section={title.toLowerCase().replaceAll(" ", "-")}>
      <h3 className="mb-1 text-small font-semibold">{title}</h3>
      <div className="-mx-2 flex flex-col gap-px">{children}</div>
    </section>
  );
}

export function LayerOverviewIndex({
  doc,
  overview,
  layer,
  touch,
  onSelectDistrict,
  onSelectFile,
  onHighlightDistricts,
  onFrameDistricts,
}: LayerOverviewIndexProps) {
  const highlighted = (districts: readonly number[] | null) => onHighlightDistricts?.(districts);
  return (
    <div data-layer-overview-index={layer}>
      {layer === "c" && (
        <>
          <IndexSection title="Districts by commits">
            {overview.churn.districts.map((row) => (
              <DistrictRow
                key={row.district}
                district={row.district}
                name={row.name}
                detail={`${displayNumber(row.commitsPerFile)} commits per file · ${row.knownFiles.toLocaleString("en-US")} files`}
                total={`${displayNumber(row.commits)} file commits`}
                bar={row.bar}
                touch={touch}
                onHighlight={highlighted}
                onSelect={() => { highlighted(null); onSelectDistrict(row.district); }}
              />
            ))}
            {overview.churn.districts.length === 0 && <p className="px-2 py-2 text-meta text-[var(--dim)]">Churn values are not present in this map.</p>}
          </IndexSection>
          <IndexSection title="Most changed files">
            {overview.churn.files.map((row) => (
              <FileRow key={row.file} doc={doc} file={row.file} touch={touch} detail={`${displayNumber(row.value)} commits`} onHighlight={highlighted} onSelect={() => { highlighted(null); onSelectFile(row.file); }} />
            ))}
            {overview.churn.files.length === 0 && <p className="px-2 py-2 text-meta text-[var(--dim)]">Churn values are not present in this map.</p>}
          </IndexSection>
        </>
      )}

      {layer === "x" && (
        <>
          <IndexSection title="Districts by median complexity">
            {overview.complexity.districts.map((row) => (
              <DistrictRow
                key={row.district}
                district={row.district}
                name={row.name}
                detail={`Median ${displayNumber(row.median)} · ${row.knownFiles.toLocaleString("en-US")} files`}
                touch={touch}
                onHighlight={highlighted}
                onSelect={() => { highlighted(null); onSelectDistrict(row.district); }}
              />
            ))}
            {overview.complexity.districts.length === 0 && <p className="px-2 py-2 text-meta text-[var(--dim)]">Complexity values are not present in this map.</p>}
          </IndexSection>
          <IndexSection title="Most complex files">
            {overview.complexity.files.map((row) => (
              <FileRow key={row.file} doc={doc} file={row.file} touch={touch} detail={`Complexity ${displayNumber(row.value)}`} onHighlight={highlighted} onSelect={() => { highlighted(null); onSelectFile(row.file); }} />
            ))}
            {overview.complexity.files.length === 0 && <p className="px-2 py-2 text-meta text-[var(--dim)]">Complexity values are not present in this map.</p>}
          </IndexSection>
        </>
      )}

      {layer === "p" && (
        <>
          <IndexSection title="Packages">
            {overview.package.groups.map((row) => {
              const label = row.other ? "Other packages" : formatDirectory(row.path ?? ".");
              return (
                <button
                  key={row.path ?? "other"}
                  type="button"
                  data-package-overview-row={row.path ?? "other"}
                  data-overview-districts={row.districtIds.join(",")}
                  onMouseEnter={!touch ? () => highlighted(row.districtIds) : undefined}
                  onMouseLeave={!touch ? () => highlighted(null) : undefined}
                  onFocus={!touch ? () => highlighted(row.districtIds) : undefined}
                  onBlur={!touch ? () => highlighted(null) : undefined}
                  onClick={() => { highlighted(null); onFrameDistricts(row.districtIds); }}
                  className={`flex ${touch ? "min-h-[64px]" : "h-[50px] min-h-[50px]"} w-full shrink-0 items-center justify-between gap-2 rounded-[10px] px-2.5 py-1.5 text-left hover:bg-[var(--chrome-hover)]`}
                >
                  <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                    <span className="truncate text-small font-semibold">{label}</span>
                    <span className="truncate text-meta text-[var(--dim)]">{overview.package.language ? `${displayLanguage(overview.package.language)} · ` : ""}{row.files.toLocaleString("en-US")} files</span>
                  </span>
                  <span className="shrink-0 font-mono text-meta text-[var(--dim)]">{row.districtIds.length} districts</span>
                </button>
              );
            })}
            {overview.package.groups.length === 0 && <p className="px-2 py-2 text-meta text-[var(--dim)]">Package paths are not present in this map.</p>}
          </IndexSection>
          <IndexSection title="Districts that mix packages">
            {overview.package.mixedDistricts.map((row) => (
              <DistrictRow
                key={row.district}
                district={row.district}
                name={row.name}
                detail={`${row.packages.length} packages · ${row.packages.map((path) => path == null ? "Other packages" : formatDirectory(path)).join(" · ")}`}
                touch={touch}
                onHighlight={highlighted}
                onSelect={() => { highlighted(null); onSelectDistrict(row.district); }}
              />
            ))}
            {overview.package.mixedDistricts.length === 0 && <p className="px-2 py-2 text-meta text-[var(--dim)]">No district mixes packages.</p>}
          </IndexSection>
        </>
      )}
    </div>
  );
}

export function useLayerOverview(doc: MapDocument, grouping: PackageGrouping): LayerOverview {
  // Kept as a component-facing helper so desktop and phone memoize the same
  // pure derivation without each caller rebuilding it on chrome state changes.
  return useMemo(() => buildLayerOverview(doc, grouping), [doc, grouping]);
}
