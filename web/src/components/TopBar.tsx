import type { ReactNode } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import type { CatalogueEntry, MapDocument } from "@/types";
import { repoOptions } from "@/lib/repoOptions";
import type { Layer } from "@/map/constants";
import { TOP_BAR_HEIGHT_PX } from "@/map/layoutProfile";
import { ThemeToggle } from "./ThemeToggle";
import { ChevronDownIcon, PanelIcon } from "./phone/icons";

interface Props {
  catalogue: CatalogueEntry[] | undefined;
  doc: MapDocument;
  owner: string;
  repo: string;
  layer: Layer;
  onLayer(l: Layer): void;
  /** The search box, laid out by the caller (MapView owns picking). */
  search: ReactNode;
  /** Tablet (docs/UX.md §9): the rail is collapsible from here. Absent on
   * desktop, where the rail always shows. */
  rail?: { open: boolean; onToggle(): void };
  /** 44 px targets (a tablet is touch, §9) instead of desktop's 36-40. */
  touch: boolean;
}

const LAYERS: { id: Layer; label: string }[] = [
  { id: "d", label: "District" },
  { id: "c", label: "Churn" },
  { id: "x", label: "Complexity" },
  { id: "p", label: "Package" },
];

/** docs/UX.md §5: the 56 px desktop top bar -- wordmark, repository switcher
 * (mono), search (460 px; `/` focuses it), flexible space, the layer
 * segmented control and the theme button. On a tablet (§9) a rail toggle
 * leads it and every control is 44 px. The top safe-area inset is padded
 * above it, and the side insets beside it (§9). */
export function TopBar({ catalogue, doc, owner, repo, layer, onLayer, search, rail, touch }: Props) {
  const navigate = useNavigate();
  const slug = `${owner}/${repo}`;
  const options = repoOptions(catalogue, doc, owner, repo);
  const h = touch ? "h-11" : "h-9";

  return (
    <header
      data-top-bar
      className="relative z-40 flex shrink-0 items-center gap-2 border-b border-[var(--rule)] bg-[var(--chrome)] text-[var(--on)] min-[900px]:gap-3"
      style={{
        height: `calc(${TOP_BAR_HEIGHT_PX}px + env(safe-area-inset-top, 0px))`,
        paddingTop: "env(safe-area-inset-top, 0px)",
        paddingLeft: "calc(12px + env(safe-area-inset-left, 0px))",
        paddingRight: "calc(12px + env(safe-area-inset-right, 0px))",
      }}
    >
      {rail && (
        <button
          type="button"
          data-rail-toggle
          aria-label={rail.open ? "Hide the district index" : "Show the district index"}
          aria-expanded={rail.open}
          onClick={rail.onToggle}
          className={`flex w-11 shrink-0 items-center justify-center rounded-[10px] ${h} ${rail.open ? "bg-[var(--chrome2)]" : ""}`}
        >
          <PanelIcon />
        </button>
      )}
      {/* The wordmark leads Home (owner, 2026-09-28: "no way to go back"). */}
      <Link to="/" data-wordmark aria-label="tolmap home" className="mr-1 shrink-0 text-[18px] font-bold tracking-[-0.02em] max-[899px]:hidden">
        tolmap
      </Link>
      {/* A native select laid under a styled face: the browser's own picker
          opens (a tablet's too), with nothing of ours to dismiss. The face
          shows the slug alone; each option also carries its file count
          (issue #171). */}
      <span data-repo-switcher className={`relative flex min-w-[120px] max-w-[260px] shrink items-center gap-2 rounded-[10px] border border-[var(--rule)] bg-[var(--chrome2)] pl-2.5 pr-2 ${h}`}>
        <span className="min-w-0 truncate font-mono text-small">{slug}</span>
        <span className="shrink-0 text-[var(--dim)]">
          <ChevronDownIcon />
        </span>
        <select
          aria-label="Repository"
          value={slug}
          onChange={(e) => {
            const [o, r] = e.target.value.split("/");
            navigate({ to: "/$owner/$repo", params: { owner: o, repo: r }, search: { geo: "r", layer: "d" } });
          }}
          className="absolute inset-0 h-full w-full cursor-pointer appearance-none opacity-0"
        >
          {options.map((m) => (
            <option key={m.slug} value={m.slug}>
              {m.label}
            </option>
          ))}
        </select>
      </span>
      {/* Up to §5's 460 px, and shrinking before anything else does (a
          tablet's top bar has less room than the design's 1440 px), so the
          layer control and the theme button always fit. Grows three times
          faster than the spacer so it reaches 460 first. */}
      <div className="relative min-w-[140px] max-w-[460px] flex-[3_1_0%] min-[900px]:ml-4">{search}</div>
      <span className="min-w-0 flex-1" />
      <div role="radiogroup" aria-label="Map layer" data-layer-segmented className="flex shrink-0 rounded-[10px] border border-[var(--rule)] bg-[var(--canvas)] p-0.5">
        {LAYERS.map((l) => {
          const on = layer === l.id;
          return (
            <button
              key={l.id}
              type="button"
              role="radio"
              aria-checked={on}
              data-layer={l.id}
              onClick={() => onLayer(l.id)}
              className={`rounded-[8px] px-2.5 text-small min-[900px]:px-3 ${touch ? "h-11" : "h-[30px]"} ${on ? "bg-[var(--rule)] font-semibold text-[var(--on)]" : "text-[var(--dim)] hover:text-[var(--on)]"}`}
            >
              {l.label}
            </button>
          );
        })}
      </div>
      <ThemeToggle className={`${touch ? "h-11 w-11" : "h-9 w-9"} rounded-[10px]`} />
    </header>
  );
}
