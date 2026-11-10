import type { OrganizationDocument } from "@bindings/OrganizationDocument";
import { useEffect, useRef } from "react";
import { districtClass, worldBounds } from "@/map/geometry";
import type { MapDocument } from "@/types";

type MemberMap = { repo: string; document: MapDocument };

interface OrganizationCanvasProps {
  organization: OrganizationDocument;
  maps: readonly MemberMap[];
  selectedRepo?: string;
  onSelectRepository(repo: string): void;
  onClearSelection(): void;
}

const NS = "http://www.w3.org/2000/svg";

function element<K extends keyof SVGElementTagNameMap>(name: K, attrs: Record<string, string | number> = {}): SVGElementTagNameMap[K] {
  const node = document.createElementNS(NS, name) as SVGElementTagNameMap[K];
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
  return node;
}

/** Imperative organization surface. This deliberately uses the same district
 * blob geometry and bounds helpers as MapRenderer, but stops before its file,
 * symbol, road, and LOD passes: the organization contract is outlines plus
 * declared inter-repository roads only. React owns the selected card; this
 * classless SVG composition only paints and reports repository taps. */
export function OrganizationCanvas({ organization, maps, selectedRepo, onSelectRepository, onClearSelection }: OrganizationCanvasProps) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const selectRef = useRef(onSelectRepository);
  const clearRef = useRef(onClearSelection);
  selectRef.current = onSelectRepository;
  clearRef.current = onClearSelection;

  useEffect(() => {
    const wrap = wrapRef.current;
    const svg = svgRef.current;
    if (!wrap || !svg) return;
    const draw = () => {
      const width = Math.max(1, wrap.clientWidth);
      const height = Math.max(1, wrap.clientHeight);
      svg.replaceChildren();
      svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
      svg.onclick = event => {
        if (!(event.target as Element).closest("[data-organization-repository]")) clearRef.current();
      };
      const regions = new Map(organization.placement.regions.map(region => [region.repo, region]));
      const extent: [number, number, number, number] = [Infinity, Infinity, -Infinity, -Infinity];
      for (const region of organization.placement.regions) {
        extent[0] = Math.min(extent[0], region.translation[0] - region.scale / 2);
        extent[1] = Math.min(extent[1], region.translation[1] - region.scale / 2);
        extent[2] = Math.max(extent[2], region.translation[0] + region.scale / 2);
        extent[3] = Math.max(extent[3], region.translation[1] + region.scale / 2);
      }
      for (const road of organization.placement.roads) {
        extent[0] = Math.min(extent[0], road.from[0], road.to[0]);
        extent[1] = Math.min(extent[1], road.from[1], road.to[1]);
        extent[2] = Math.max(extent[2], road.from[0], road.to[0]);
        extent[3] = Math.max(extent[3], road.from[1], road.to[1]);
      }
      if (!Number.isFinite(extent[0])) return;
      const pad = 34;
      const scale = Math.min((width - pad * 2) / Math.max(1, extent[2] - extent[0]), (height - pad * 2) / Math.max(1, extent[3] - extent[1]));
      const x = (value: number) => (value - extent[0]) * scale + (width - (extent[2] - extent[0]) * scale) / 2;
      const y = (value: number) => (value - extent[1]) * scale + (height - (extent[3] - extent[1]) * scale) / 2;

      const roads = element("g", { "data-organization-roads": "" });
      for (const road of organization.placement.roads) {
        roads.appendChild(element("path", {
          d: `M${x(road.from[0]).toFixed(1)} ${y(road.from[1]).toFixed(1)} L${x(road.to[0]).toFixed(1)} ${y(road.to[1]).toFixed(1)}`,
          fill: "none", stroke: "var(--ink)", "stroke-width": 2.5, "stroke-opacity": 0.5, "stroke-linecap": "round",
          "data-organization-road": `${road.consumer}:${road.provider}`,
        }));
      }
      svg.appendChild(roads);

      for (const { repo, document } of maps) {
        const region = regions.get(repo);
        if (!region) continue;
        const bounds = worldBounds(document, "r");
        const centerX = (bounds[0] + bounds[2]) / 2;
        const centerY = (bounds[1] + bounds[3]) / 2;
        const memberScale = region.scale / Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1], 1);
        const toX = (value: number) => x(region.translation[0] + (value - centerX) * memberScale);
        const toY = (value: number) => y(region.translation[1] + (value - centerY) * memberScale);
        const selected = selectedRepo === repo;
        const group = element("g", { "data-organization-repository": repo, role: "button", tabindex: 0, "aria-label": `Open ${repo} repository card` });
        const outline = element("rect", {
          x: x(region.translation[0] - region.scale / 2), y: y(region.translation[1] - region.scale / 2),
          width: region.scale * scale, height: region.scale * scale, rx: 10,
          fill: selected ? "var(--chrome2)" : "transparent", stroke: selected ? "var(--hot)" : "var(--rule)",
          "stroke-width": selected ? 2.4 : 1.5, "stroke-dasharray": selected ? "" : "5 4", "data-organization-repository-outline": repo,
        });
        group.appendChild(outline);
        for (const [districtId, district] of Object.entries(document.districts)) {
          if (districtClass(district) === "unconnected") continue;
          for (const polygon of district.blob) {
            group.appendChild(element("path", {
              d: `M${polygon.map(point => `${toX(point[0]).toFixed(1)} ${toY(point[1]).toFixed(1)}`).join("L")}Z`,
              fill: "var(--chrome2)", "fill-opacity": 0.35, stroke: "var(--ink)", "stroke-opacity": 0.56,
              "stroke-width": 1.1, "stroke-linejoin": "round", "pointer-events": "none", "data-organization-district": `${repo}:${districtId}`,
            }));
          }
        }
        const label = element("text", { x: x(region.translation[0]), y: y(region.translation[1] - region.scale / 2) - 9, "text-anchor": "middle", fill: "var(--on)", "font-size": 12, "font-family": "var(--font-mono, monospace)", "pointer-events": "none" });
        label.textContent = repo;
        group.appendChild(label);
        const activate = () => selectRef.current(repo);
        group.addEventListener("click", activate);
        group.addEventListener("keydown", event => {
          if (event.key === "Enter" || event.key === " ") { event.preventDefault(); activate(); }
        });
        svg.appendChild(group);
      }
    };
    draw();
    const observer = new ResizeObserver(draw);
    observer.observe(wrap);
    return () => observer.disconnect();
  }, [organization, maps, selectedRepo]);

  return <div ref={wrapRef} className="absolute inset-0" data-organization-canvas><svg ref={svgRef} className="h-full w-full touch-none" /></div>;
}
