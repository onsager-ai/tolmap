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
      if (id === 'home') {
        await waitFor(() => { expect(root.querySelector('[data-catalogue-row]')).not.toBeNull(); expect(root.querySelector('[data-catalogue-error], [data-catalogue-empty]')).toBeNull(); });
      } else if (id === 'home-empty') {
        await waitFor(() => expect(root.querySelector('[data-catalogue-empty]')?.textContent?.trim()).toBe('No repositories mapped yet — paste one above.'));
      } else if (id === 'home-loading') {
        await waitFor(() => { expect(root.body.textContent).toContain('Mapped repositories'); expect(root.body.textContent?.toLowerCase()).toContain('loading…'); });
      } else if (id === 'home-unavailable') {
        await waitFor(() => expect(root.querySelector('[data-catalogue-error]')?.textContent).toContain("tolmap's index couldn't be reached."));
      } else if (id === 'job-missing') {
        await waitFor(() => expect(canvas.getByText('No mapping job to show.')).toBeVisible());
      } else if (id.startsWith('local-')) {
        const frame = await waitElement('iframe') as HTMLIFrameElement;
        await waitFor(() => expect(frame.contentDocument?.documentElement.dataset.fixtureReady).toBe(id.slice(6)));
        await frame.contentDocument!.fonts.ready;
      } else if (id.startsWith("map-") || id.startsWith("source-")) {
        if (id === 'map-loading') await waitElement('[data-load-progress="idle"], [data-load-progress="downloading"], [data-load-progress="parsing"]');
        else if (id === 'map-unavailable') await waitFor(() => expect(root.body.textContent).toContain("couldn't load onsager-ai/tolmap."));
        else await waitElement(".map-svg path");
      } else await waitFor(() => expect(canvas.getAllByRole("heading").length).toBeGreaterThan(0));
      if (id.startsWith('job-') && id !== 'job-missing') {
        const state = id === 'job-queued' ? 'queued' : id === 'job-fetch' ? 'cloning' : 'indexing';
        await waitFor(() => expect(root.querySelector('[data-job-state]')?.getAttribute('data-job-state')).toBe(state));
        const phase = ({ queued: null, fetch: 'fetch', read: 'read', map: 'map', detail: 'detail' } as Record<string, string | null>)[id.slice(4)];
        await waitFor(() => expect(phase ? root.querySelector(`[data-phase-id="${phase}"]`)?.getAttribute('data-phase-state') : root.querySelector('[data-phase-row]')?.getAttribute('data-phase-state')).toBe(phase ? 'running' : 'pending'));
      }
      if (id.startsWith('failed-')) await waitFor(() => {
        expect(root.querySelector('[data-job-failure]')?.getAttribute('data-job-failure-code')).toBe(id.slice('failed-'.length).replaceAll('-', '_'));
      });
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
          await waitFor(() => expect(root.querySelector('.cm-content')?.getAttribute('aria-readonly')).toBe('true'));
          if (id === 'source-long-line') await waitFor(() => expect(Array.from(root.querySelectorAll('.source-provenance')).some(node => node.textContent?.includes('long line protection'))).toBe(true));
          if (id === "source-search" || id === "source-search-empty") {
            await userEvent.click(canvas.getByRole("button", { name: "Find in file" }));
            const input = await waitElement('.cm-search input[name="search"]');
            // CodeMirror seeds Find with the selected committed source range.
            await userEvent.clear(input);
            await userEvent.type(input, id === "source-search" ? "return" : "absent-needle");
            await userEvent.click(await waitElement('.cm-search button[name="next"]'));
            await waitFor(() => expect(root.querySelector("[data-source-search-status]")?.textContent).toBe(id === "source-search" ? "Matches found" : "No results"));
          }
        } else if (id === "source-unavailable") await waitFor(() => expect(root.querySelector('[data-source-reader] [role="alert"]')?.textContent).toContain('No current-branch fallback was used.'));
        else if (id === "source-loading") await waitFor(() => expect(root.querySelector('[data-source-reader] [role="status"]')?.textContent).toBe('Loading committed source…'));
        else {
          const expectedStatus: Record<string, string> = {
            'source-missing': 'This file is missing from this snapshot.',
            'source-expired': 'Source for this snapshot has expired. No newer source was substituted.',
            'source-binary': 'Binary or non-UTF-8 source cannot be displayed.',
            'source-large': 'This file exceeds the 1 MiB reading limit.',
            'source-unsupported': 'Symlinks, submodules and non-regular files cannot be displayed.',
          };
          await waitFor(() => expect(root.querySelector('[data-source-reader] [role="status"]')?.textContent).toBe(expectedStatus[id]));
        }
      }
      if (id.startsWith('reserved-')) await waitFor(() => {
        const name = id.slice('reserved-'.length);
        const product = root.querySelector('[data-product-page]');
        expect(product?.getAttribute('data-preview-route')).toBe(`/${name}`);
        expect(product?.querySelector('h1')?.textContent).toBe('/');
        expect(product?.textContent).toContain('This name is reserved for tolmap itself and will never route to a GitHub owner called "".');
        expect(product?.querySelector('a[href="/"]')?.textContent).toContain('back to the map index');
      });
      if (['map-district', 'map-file', 'map-symbol'].includes(id) && root.querySelector('[data-panel-tab]')) await click('[data-panel-tab]');
      if (id === 'map-district') await waitFor(() => expect(root.querySelector('[data-sheet-card="district"]')).not.toBeNull());
      if (id === 'map-file') await waitFor(() => expect(root.querySelector('[data-sheet-card="file"]')).not.toBeNull());
      if (id === 'map-symbol') await waitFor(() => expect(root.querySelector('[data-sheet-card="symbol"]')).not.toBeNull());
      if (id === 'map-folder') await waitFor(() => expect(root.querySelector('[data-folder-path][aria-pressed="true"]')).not.toBeNull());
      if (id.startsWith('map-layer-')) {
        const layer = id.slice('map-layer-'.length);
        await waitFor(() => expect(root.querySelector(`[data-overview-layer="${layer}"], [data-layer="${layer}"][aria-pressed="true"]`)).not.toBeNull());
      }
      if (id === 'map-detail-pending') await waitFor(() => expect(root.querySelector('[data-detail-status="pending"]')?.textContent).toContain('Classes and functions are still being read.'));
      await root.fonts.ready;
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
      root.documentElement.dataset.visualReady = id;
    },
  };
}
