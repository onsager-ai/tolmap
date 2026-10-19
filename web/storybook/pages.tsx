import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";
import { Page } from "./Page";
import { NativeLocal } from "./NativeLocal";
import { cases } from "./catalogue.mjs";
import { installFixture, resolveRoute } from "./fixtures";
import { setHost } from "./host";
import { applyThemeChoice } from "../src/lib/theme";

type Args = { visualCaseId: string };
export const meta = {
  title: "Tolmap/Pages",
  loaders: [async ({ args, globals }) => {
    delete document.documentElement.dataset.visualReady;
    setHost(globals.host);
    applyThemeChoice(globals.theme);
    const item = cases.find(item => item.id === args.visualCaseId)!;
    const doc = await installFixture(item.fixture);
    return { route: resolveRoute(item.route, doc) };
  }],
  render: (args, context) => context.loaded.route.startsWith('local:')
    ? <NativeLocal key={`${args.visualCaseId}-${context.globals.theme}`} name={context.loaded.route.slice(6)} theme={context.globals.theme} />
    : <Page key={`${args.visualCaseId}-${context.globals.host}-${context.globals.theme}`} route={context.loaded.route} />,
} satisfies Meta<Args>;

export function page(id: string): StoryObj<Args> {
  const item = cases.find(item => item.id === id)!;
  return {
    name: item.name, args: { visualCaseId: id },
    play: async ({ canvasElement }) => {
      const canvas = within(canvasElement);
      const root = canvasElement.ownerDocument;
      const waitElement = async (selector: string) => {
        let element: HTMLElement | null = null;
        await waitFor(() => { element = root.querySelector(selector); expect(element).not.toBeNull(); }, { timeout: 15000 });
        return element!;
      };
      const click = async (selector: string) => userEvent.click(await waitElement(selector));
      if (id.startsWith('local-')) {
        const frame = await waitElement('iframe') as HTMLIFrameElement;
        await waitFor(() => expect(frame.contentDocument?.documentElement.dataset.fixtureReady).toBe(id.slice(6)));
        await frame.contentDocument!.fonts.ready;
      } else if (id.startsWith("map-") || id.startsWith("source-")) {
        if (!["map-loading", "map-unavailable"].includes(id)) await waitElement(".map-svg path");
        else await waitFor(() => expect(root.querySelector("[data-product-page]")?.textContent?.trim()).toBeTruthy());
      } else await waitFor(() => expect(canvas.getAllByRole("heading").length).toBeGreaterThan(0));
      if (id === 'home') await waitElement('[data-catalogue-row]');
      if (id === 'home-empty') await waitElement('[data-catalogue-empty]');
      if (id === 'home-unavailable') await waitElement('[data-catalogue-error]');
      if (id.startsWith('job-') && id !== 'job-missing') await waitElement('[data-job-state]');
      if (id.startsWith('failed-')) await waitElement('[data-job-failure]');
      const action = item.action;
      if (action?.startsWith("search")) {
        await click("[data-open-desktop-search], [data-open-search]");
        const input = await waitElement("[data-search-input]");
        await userEvent.type(input, action === "search-empty" ? "zzzz-no-match" : "src");
        await waitElement(action === "search-empty" ? '[data-search-state="no-results"]' : "[data-search-option]");
      }
      if (action === "layers") {
        if (root.querySelector("[data-phone-shell]")) { await click('[aria-label="Map layers"]'); await waitElement("[data-layers-sheet]"); }
        else await waitElement("[data-layer-segmented]");
      }
      if (action === "repositories") { await click("[data-repo-switcher], [data-switch-repo]"); await waitElement("[data-repository-menu], [data-repo-sheet]"); }
      if (action === "quality") {
        // Tablet starts with the panel collapsed.
        if (root.querySelector("[data-panel-tab]")) await click("[data-panel-tab]");
        await click("[data-open-map-quality], [data-map-quality]"); await waitElement('[data-sheet-card="quality"]');
      }
      if (action === "keyboard" && root.querySelector("[data-open-keyboard]")) { await click("[data-open-keyboard]"); await waitElement("[data-keyboard-dialog]"); }
      if (action === "path") {
        if (root.querySelector("[data-panel-tab]")) await click("[data-panel-tab]");
        // Expand the phone sheet through its production detail control.
        const target = await waitElement('[data-path-start="from"]');
        target.scrollIntoView({ block: "center" }); await userEvent.click(target); await waitElement('[data-path-state="picking"]');
      }
      if (id.startsWith("source-")) {
        await waitElement("[data-source-reader]");
        if (["source-ready", "source-long-line", "source-search", "source-search-empty"].includes(id)) {
          await waitElement(".cm-content");
          if (id === "source-search" || id === "source-search-empty") {
            await userEvent.click(canvas.getByRole("button", { name: "Find in file" }));
            await userEvent.type(await waitElement('.cm-search input[name="search"]'), id === "source-search" ? "return" : "absent-needle");
            await waitFor(() => expect(root.querySelector("[data-source-search-status]")?.textContent).toBe(id === "source-search" ? "Matches found" : "No results"));
          }
        } else if (id === "source-unavailable") await waitElement('[data-source-reader] [role="alert"]');
        else if (id !== "source-loading") await waitFor(() => {
          const status = root.querySelector('[data-source-reader] [role="status"]')?.textContent;
          expect(status).toBeTruthy(); expect(status).not.toContain("Loading");
        });
      }
      await root.fonts.ready;
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
      root.documentElement.dataset.visualReady = id;
    },
  };
}
