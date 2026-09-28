import { useMemo, type ReactNode } from "react";
import type { MapDocument } from "@/types";
import type { Layer } from "@/map/constants";
import { buildLayerOverview, displayLanguage, layerOverviewHeadline, type LayerOverview } from "@/map/layerOverview";
import type { PackageGrouping } from "@/map/packageLayout";
import { formatDirectory } from "@/map/packageLayout";
import { LAYER_SURFACE_MIX, RAMP_STOPS } from "@/map/geometry";

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
      <h2 data-overview-headline data-overview-layer={layer} className={className}><HeadlineText text={headline.primary} /></h2>
      {headline.secondary && (
        // Each " · " fact wraps as a unit, so a narrow panel breaks between
        // facts ("Merge commits excluded ·" / "most active district: …")
        // instead of stranding the tail of a district name on its own line.
        <p data-overview-subheadline className="mt-1.5 flex flex-wrap gap-x-1 text-meta leading-[1.45] text-[var(--dim)]">
          {headline.secondary.split(" · ").map((part, i, parts) => (
            <span key={i}>{part}{i < parts.length - 1 ? " ·" : ""}</span>
          ))}
        </p>
      )}
    </>
  );
}

/** A headline whose counts never split from their unit: "18 islands" and
 * "6,347 files" stay whole, so a wrap falls at " + " or " · " rather than
 * leaving "files" (or "islands · …") alone on the next line. The text
 * content is unchanged; only the break opportunities move. */
export function HeadlineText({ text }: { text: string }) {
  const parts = text.split(/(\d[\d,.]*\s\S+)/);
  return (
    <>
      {parts.map((part, i) => (i % 2 === 1 ? <span key={i} className="whitespace-nowrap">{part}</span> : part))}
    </>
  );
}

export interface LayerOverviewIndexProps {
  doc: MapDocument;
  overview: LayerOverview;
  layer: Exclude<Layer, "d">;
  touch: boolean;
  /** The phone sheet's full-bleed list (rows ruled like its District index)
   * rather than the desktop panel's inset, rounded rows. */
  sheet?: boolean;
  onSelectDistrict(district: number): void;
  onSelectFile(file: number): void;
  onHighlightDistricts?(districts: readonly number[] | null): void;
  onFrameDistricts(districts: readonly number[]): void;
}

function displayNumber(value: number): string {
  return value.toLocaleString("en-US", { maximumFractionDigits: 2 });
}

function smallDistrictSummary(districts: number, islands: number): string {
  const parts = [
    ...(districts > 0 ? [`${districts} small ${districts === 1 ? "district" : "districts"}`] : []),
    ...(islands > 0 ? [`${islands} small ${islands === 1 ? "island" : "islands"}`] : []),
  ];
  return `${parts.join(" + ")} not ranked`;
}

function packageSpanSummary(districts: number, islands: number): string {
  const parts = [
    ...(districts > 0 ? [`${districts} ${districts === 1 ? "district" : "districts"}`] : []),
    ...(islands > 0 ? [`${islands} ${islands === 1 ? "island" : "islands"}`] : []),
  ];
  return parts.length ? parts.join(" + ") : "No District index span";
}

// The map's own churn/complexity colour for a value: geometry.ts ramp()'s
// three stops, mixed toward --canvas by LAYER_SURFACE_MIX. The mix is done in
// CSS rather than with ramp() itself because ramp() reads --canvas once into
// a cache; a React swatch rendered before a theme switch would keep the old
// theme's mix, while color-mix() follows the live token.
const RAMP_RGB = RAMP_STOPS.map((hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)));
function rampSwatch(t: number): string {
  const v = Math.max(0, Math.min(1, Number.isFinite(t) ? t : 0));
  const a = v < 0.5 ? RAMP_RGB[0] : RAMP_RGB[1];
  const b = v < 0.5 ? RAMP_RGB[1] : RAMP_RGB[2];
  const u = v < 0.5 ? v * 2 : (v - 0.5) * 2;
  const rgb = a.map((c, i) => Math.round(c + (b[i] - c) * u));
  return `color-mix(in srgb, rgb(${rgb.join(",")}) ${Math.round(LAYER_SURFACE_MIX * 100)}%, var(--canvas))`;
}

/** Hard-edged stripes of each package colour: a district that mixes
 * packages is drawn in several of them on the Package layer. */
function stripeSwatch(colors: readonly string[]): string {
  const shown = colors.slice(0, 4);
  if (shown.length <= 1) return shown[0] ?? "var(--dim)";
  const step = 100 / shown.length;
  return `linear-gradient(90deg, ${shown.map((color, i) => `${color} ${(i * step).toFixed(2)}% ${((i + 1) * step).toFixed(2)}%`).join(", ")})`;
}

const RAMP_GRADIENT = "linear-gradient(90deg, var(--chrome-ramp-low), var(--chrome-ramp-mid), var(--chrome-ramp-high))";

/** A short bar that reveals the legend's ramp up to its length, so the
 * longest bar ends warm and a short one stays cool -- the same scale the
 * legend and the map use, not a flat accent fill. */
function RampBar({ value, sheet }: { value: number; sheet: boolean }) {
  const t = Math.max(0.04, Math.min(1, Number.isFinite(value) ? value : 0));
  return (
    <span aria-hidden="true" className={`${sheet ? "w-12" : "w-10"} h-1 shrink-0 overflow-hidden rounded-full bg-[var(--rule)]`}>
      <span className="block h-full rounded-full" style={{ width: `${t * 100}%`, backgroundImage: RAMP_GRADIENT, backgroundSize: `${100 / t}% 100%` }} />
    </span>
  );
}

/** One row anatomy for every layer's index, the District index's own:
 * swatch, name, one quiet meta line, then the value right-aligned in
 * tabular numerals (the unit lives in the section heading, not on every
 * row). */
function OverviewRow({
  sheet,
  touch,
  swatch,
  name,
  nameMono = false,
  meta,
  metaMono = false,
  value,
  valueTitle,
  bar,
  districtRow,
  fileRow,
  packageRow,
  packageDistricts,
  onHighlight,
  onSelect,
}: {
  sheet: boolean;
  touch: boolean;
  swatch: string;
  name: string;
  nameMono?: boolean;
  meta: string;
  metaMono?: boolean;
  value?: string;
  valueTitle?: string;
  bar?: number;
  districtRow?: number;
  fileRow?: number;
  packageRow?: string;
  packageDistricts?: string;
  onHighlight?(on: boolean): void;
  onSelect(): void;
}) {
  const frame = sheet
    ? "min-h-[64px] gap-3 border-b border-[var(--rule)] px-5 py-2.5 active:bg-[var(--chrome-hover)]"
    : `${touch ? "min-h-[64px]" : "h-[50px] min-h-[50px]"} gap-2.5 rounded-[10px] px-2.5 py-1.5 hover:bg-[var(--chrome-hover)]`;
  return (
    <button
      type="button"
      data-overview-district-row={districtRow}
      data-overview-file={fileRow}
      data-layer-overview-file-row={fileRow}
      data-package-overview-row={packageRow}
      data-overview-districts={packageDistricts}
      onMouseEnter={onHighlight ? () => onHighlight(true) : undefined}
      onMouseLeave={onHighlight ? () => onHighlight(false) : undefined}
      onFocus={onHighlight ? () => onHighlight(true) : undefined}
      onBlur={onHighlight ? () => onHighlight(false) : undefined}
      onClick={onSelect}
      className={`flex w-full shrink-0 items-center text-left ${frame}`}
    >
      <i aria-hidden="true" data-package-overview-swatch={packageRow} className="h-2.5 w-2.5 shrink-0 rounded-[3px]" style={{ background: swatch }} />
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className={`truncate ${nameMono ? `font-mono font-medium ${sheet ? "text-[15px] leading-[22px]" : "text-[13px] leading-5"}` : sheet ? "text-row" : "text-small font-semibold"}`}>{name}</span>
        <span className={`truncate text-meta text-[var(--dim)] ${metaMono ? "font-mono" : ""}`}>{meta}</span>
      </span>
      {bar != null && <RampBar value={bar} sheet={sheet} />}
      {value != null && (
        <span title={valueTitle} className={`min-w-[4ch] shrink-0 text-right font-mono tabular-nums text-[var(--dim)] ${sheet ? "text-small" : "text-meta"}`}>{value}</span>
      )}
    </button>
  );
}

function IndexSection({ title, sheet, children, first = false }: { title: string; sheet: boolean; children: ReactNode; first?: boolean }) {
  // Desktop: the same heading row as the District layer's "Index" (32 px,
  // then rows inset by -8 px so their swatches sit on the text edge).
  // Phone: the same heading and ruled list as the sheet's District index,
  // so headings and rows share the sheet's 20 px edge.
  return (
    <section className={sheet ? "" : first ? "" : "mt-4"} data-layer-overview-section={title.toLowerCase().replaceAll(" ", "-")}>
      {sheet ? (
        <h3 className="px-5 pb-2.5 pt-5 text-[15px] font-semibold">{title}</h3>
      ) : (
        <div className="flex min-h-8 items-center"><h3 className="text-small font-semibold">{title}</h3></div>
      )}
      <div className={sheet ? "flex flex-col border-t border-[var(--rule)]" : "-mx-2 mt-1 flex flex-col gap-px"}>{children}</div>
    </section>
  );
}

function IndexNote({ sheet, children }: { sheet: boolean; children: ReactNode }) {
  return <p className={`text-meta text-[var(--dim)] ${sheet ? "px-5 py-3" : "px-2.5 pb-1 pt-2"}`}>{children}</p>;
}

function fileParts(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? { name: path, dir: "(repo root)" } : { name: path.slice(slash + 1), dir: path.slice(0, slash) };
}

export function LayerOverviewIndex({
  doc,
  overview,
  layer,
  touch,
  sheet = false,
  onSelectDistrict,
  onSelectFile,
  onHighlightDistricts,
  onFrameDistricts,
}: LayerOverviewIndexProps) {
  const highlighted = (districts: readonly number[] | null) => onHighlightDistricts?.(districts);
  const hover = (districts: readonly number[]) => (onHighlightDistricts ? (on: boolean) => highlighted(on ? districts : null) : undefined);

  // Swatch colours are the map's own: MapRenderer colours each file by
  // value / the document's maximum, and a district row takes its median
  // file's colour (as the District index does on these layers).
  const scale = useMemo(() => {
    let maxCh = 1;
    let maxCx = 1;
    const members = new Map<number, number[]>();
    doc.N.forEach((row, file) => {
      if (typeof row[5] === "number" && row[5] > maxCh) maxCh = row[5];
      if (typeof row[4] === "number" && row[4] > maxCx) maxCx = row[4];
      const list = members.get(row[0]);
      if (list) list.push(file);
      else members.set(row[0], [file]);
    });
    return { maxCh, maxCx, members };
  }, [doc]);
  const fileSwatch = (file: number, column: 4 | 5) => {
    const value = doc.N[file]?.[column];
    return rampSwatch(typeof value === "number" ? value / (column === 5 ? scale.maxCh : scale.maxCx) : 0);
  };
  const districtSwatch = (district: number, column: 4 | 5) => {
    const values = (scale.members.get(district) ?? [])
      .map((file) => doc.N[file]?.[column])
      .filter((value): value is number => typeof value === "number" && Number.isFinite(value))
      .sort((a, b) => a - b);
    if (values.length === 0) return "var(--dim)";
    return rampSwatch(values[Math.floor((values.length - 1) / 2)] / (column === 5 ? scale.maxCh : scale.maxCx));
  };
  const packageColors = new Map(overview.package.groups.map((row) => [row.path, row.color] as const));

  return (
    <div data-layer-overview-index={layer}>
      {layer === "c" && (
        <>
          <IndexSection first sheet={sheet} title="Districts by commits">
            {overview.churn.districts.map((row) => (
              <OverviewRow
                key={row.district}
                sheet={sheet}
                touch={touch}
                districtRow={row.district}
                swatch={districtSwatch(row.district, 5)}
                name={row.name}
                meta={`${row.island ? "Island · " : ""}${displayNumber(row.commitsPerFile)} per file · ${row.knownFiles.toLocaleString("en-US")} files`}
                value={displayNumber(row.commits)}
                valueTitle={`${displayNumber(row.commits)} file commits`}
                bar={row.bar}
                onHighlight={hover([row.district])}
                onSelect={() => { highlighted(null); onSelectDistrict(row.district); }}
              />
            ))}
            {(overview.churn.smallDistrictCount > 0 || overview.churn.smallIslandCount > 0) && <IndexNote sheet={sheet}>{smallDistrictSummary(overview.churn.smallDistrictCount, overview.churn.smallIslandCount)}</IndexNote>}
            {overview.churn.districts.length === 0 && overview.churn.smallDistrictCount === 0 && overview.churn.smallIslandCount === 0 && <IndexNote sheet={sheet}>Churn values are not present in this map.</IndexNote>}
          </IndexSection>
          <IndexSection sheet={sheet} title="Most changed files">
            {overview.churn.files.map((row) => {
              const { name, dir } = fileParts(doc.F[row.file]);
              return (
                <OverviewRow
                  key={row.file}
                  sheet={sheet}
                  touch={touch}
                  fileRow={row.file}
                  swatch={fileSwatch(row.file, 5)}
                  name={name}
                  nameMono
                  meta={dir}
                  metaMono
                  value={displayNumber(row.value)}
                  valueTitle={`${displayNumber(row.value)} commits`}
                  onHighlight={hover([row.district])}
                  onSelect={() => { highlighted(null); onSelectFile(row.file); }}
                />
              );
            })}
            {overview.churn.files.length === 0 && <IndexNote sheet={sheet}>Churn values are not present in this map.</IndexNote>}
          </IndexSection>
        </>
      )}

      {layer === "x" && (
        <>
          <IndexSection first sheet={sheet} title="Districts by median complexity">
            {overview.complexity.districts.map((row) => (
              <OverviewRow
                key={row.district}
                sheet={sheet}
                touch={touch}
                districtRow={row.district}
                swatch={districtSwatch(row.district, 4)}
                name={row.name}
                meta={`${row.island ? "Island · " : ""}${row.knownFiles.toLocaleString("en-US")} files`}
                value={displayNumber(row.median)}
                valueTitle={`Median complexity ${displayNumber(row.median)}`}
                bar={overview.complexity.districts[0]?.median ? row.median / overview.complexity.districts[0].median : 0}
                onHighlight={hover([row.district])}
                onSelect={() => { highlighted(null); onSelectDistrict(row.district); }}
              />
            ))}
            {(overview.complexity.smallDistrictCount > 0 || overview.complexity.smallIslandCount > 0) && <IndexNote sheet={sheet}>{smallDistrictSummary(overview.complexity.smallDistrictCount, overview.complexity.smallIslandCount)}</IndexNote>}
            {overview.complexity.districts.length === 0 && overview.complexity.smallDistrictCount === 0 && overview.complexity.smallIslandCount === 0 && <IndexNote sheet={sheet}>Complexity values are not present in this map.</IndexNote>}
          </IndexSection>
          <IndexSection sheet={sheet} title="Most complex files">
            {overview.complexity.files.map((row) => {
              const { name, dir } = fileParts(doc.F[row.file]);
              return (
                <OverviewRow
                  key={row.file}
                  sheet={sheet}
                  touch={touch}
                  fileRow={row.file}
                  swatch={fileSwatch(row.file, 4)}
                  name={name}
                  nameMono
                  meta={dir}
                  metaMono
                  value={displayNumber(row.value)}
                  valueTitle={`Complexity ${displayNumber(row.value)}`}
                  onHighlight={hover([row.district])}
                  onSelect={() => { highlighted(null); onSelectFile(row.file); }}
                />
              );
            })}
            {overview.complexity.files.length === 0 && <IndexNote sheet={sheet}>Complexity values are not present in this map.</IndexNote>}
          </IndexSection>
        </>
      )}

      {layer === "p" && (
        <>
          <IndexSection first sheet={sheet} title="Packages">
            {overview.package.groups.map((row) => (
              <OverviewRow
                key={row.path ?? "other"}
                sheet={sheet}
                touch={touch}
                packageRow={row.path ?? "other"}
                packageDistricts={row.districtIds.join(",")}
                swatch={row.color}
                name={row.other ? "Other packages" : formatDirectory(row.path ?? ".")}
                nameMono={!row.other}
                meta={`${row.language ? `${displayLanguage(row.language)} · ` : ""}${packageSpanSummary(row.districtCount, row.islandCount)}`}
                value={row.files.toLocaleString("en-US")}
                valueTitle={`${row.files.toLocaleString("en-US")} files`}
                onHighlight={touch ? undefined : hover(row.districtIds)}
                onSelect={() => { highlighted(null); onFrameDistricts(row.districtIds); }}
              />
            ))}
            {overview.package.groups.length === 0 && <IndexNote sheet={sheet}>Package paths are not present in this map.</IndexNote>}
          </IndexSection>
          <IndexSection sheet={sheet} title="Districts and islands that mix packages">
            {overview.package.mixedDistricts.map((row) => (
              <OverviewRow
                key={row.district}
                sheet={sheet}
                touch={touch}
                districtRow={row.district}
                swatch={stripeSwatch(row.packages.map((path) => packageColors.get(path) ?? "var(--dim)"))}
                name={row.name}
                meta={`${row.island ? "Island · " : ""}${row.packages.map((path) => path == null ? "Other packages" : formatDirectory(path)).join(" · ")}`}
                value={String(row.packages.length)}
                valueTitle={`${row.packages.length} packages`}
                onHighlight={hover([row.district])}
                onSelect={() => { highlighted(null); onSelectDistrict(row.district); }}
              />
            ))}
            {overview.package.mixedDistricts.length === 0 && <IndexNote sheet={sheet}>No district mixes packages.</IndexNote>}
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
