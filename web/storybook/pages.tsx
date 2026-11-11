import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";
import { Page } from "./Page";
import { NativeLocal } from "./NativeLocal";
import { cases } from "./catalogue.mjs";
import { failedFreshness, installFixture, resolveRoute } from "./fixtures";
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
    : <Page key={`${args.visualCaseId}-${context.globals.host}-${context.globals.theme}`} route={context.loaded.route} freshness={args.visualCaseId === "map-freshness" ? failedFreshness : undefined} />,
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
      } else if (id === 'home-repositories') {
        await waitFor(() => expect(root.querySelectorAll('[data-catalogue-row]').length).toBe(3));
        for (const status of ['indexing', 'failed', 'reconnect_needed']) await waitElement(`[data-repository-status="${status}"]`);
        for (const row of root.querySelectorAll('[data-catalogue-row]')) {
          const links = row.querySelectorAll('a');
          expect(links.length).toBe(2);
          expect(links[0].getAttribute('aria-label')).toMatch(/^View repository /);
          expect(links[1].getAttribute('aria-label')).toMatch(/^Open latest map for /);
          for (const link of links) expect(link.getBoundingClientRect().height).toBeGreaterThanOrEqual(44);
        }
      } else if (id.startsWith('repository-')) {
        await waitElement('[data-repository-header]');
        if (id === 'repository-empty') await waitElement('[data-history-empty]');
        else await waitFor(() => expect(root.querySelectorAll('[data-snapshot-row]').length).toBe(9));
        if (id === 'repository-running') {
          const progress = await waitElement('[data-job-state="indexing"]');
          expect(progress.textContent).toContain('acme/platform');
          expect(progress.textContent).not.toContain('onsager-ai/tolmap');
          expect(progress.getBoundingClientRect().height).toBeLessThan(180);
        }
        if (id === 'repository-no-capabilities') {
          expect(canvas.queryByRole('button', { name: 'Re-index' })).toBeNull();
          expect(canvas.queryByRole('link', { name: 'Open latest map' })).toBeNull();
          expect(canvas.queryByRole('button', { name: /Delete snapshot/ })).toBeNull();
          expect(canvas.queryByLabelText('Repository actions')).toBeNull();
        }
        if (id === 'repository-history') {
          expect(root.querySelector('[data-snapshot-row]')?.textContent).toContain('Latest');
          expect(root.querySelector('[data-snapshot-row] a')?.getAttribute('href')).toContain('commit=11111111');
          expect(root.querySelectorAll('[data-snapshot-row]')[2].textContent).toContain('different indexer');
        }
        if (id === 'repository-delete-snapshot') {
          await userEvent.click(canvas.getAllByRole('button', { name: /Delete snapshot/ })[0]);
          const dialog = await waitElement('dialog[open]');
          expect(dialog.textContent).toContain('Delete snapshot 11111111?');
          expect(dialog.textContent).toContain('The previous snapshot becomes the latest.');
        }
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
      } else if (id.startsWith("organization-")) {
        if (id === "organization-loading") await waitElement("[data-organization-loading]");
        else if (id === "organization-error") await waitElement("[data-organization-error]");
        else {
          await waitElement("[data-organization-canvas] [data-organization-repository]");
          if (id === "organization-card") await waitElement("[data-organization-card]");
        }
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
      if (action === "lens-walk") {
        // Issue #248: a deep link restores the step, its card and its frame.
        const state = id.slice("map-lens-walk-".length);
        if (!root.querySelector("[data-phone-shell]") && root.querySelector("[data-panel-tab]")) await click("[data-panel-tab]");
        await waitElement(`[data-lens-walkthrough][data-lens-walk-state="${state}"]`);
        if (state !== "unresolved") await waitElement('[data-sheet-card="file"], [data-sheet-card="symbol"]');
        await waitElement("[data-lens-overlay]");
      }
      if (action === "lens-comparison") {
        const state = id.endsWith("loading") ? "loading" : id.endsWith("stale") || id.endsWith("error") ? "error" : "loaded";
        await waitElement(`[data-lens-comparison-state="${state}"]`);
        if (id.endsWith("stale")) expect(root.querySelector("[data-lens-comparison]")?.textContent).toContain("Stale or mismatched");
      }
      if (action === "lenses") {
        const phone = !!root.querySelector("[data-phone-shell]");
        if (phone && id === "map-lens-empty") { await click('[aria-label="Map layers"]'); await waitElement("[data-layers-sheet]"); }
        else if (root.querySelector("[data-panel-tab]")) await click("[data-panel-tab]");
        const panel = await waitElement(phone && id !== "map-lens-empty" ? "[data-lens-sheet]" : "[data-lens-panel]");
        if (panel instanceof HTMLDetailsElement && !panel.open) await userEvent.click(panel.querySelector("summary")!);
        await waitElement(`[data-lens-state="${id.slice('map-lens-'.length)}"]`);
      }
      if (action === "debt") {
        // The built-in debt lens opens the same panel and sheet as a repository lens.
        const phone = !!root.querySelector("[data-phone-shell]");
        if (!phone && root.querySelector("[data-panel-tab]")) await click("[data-panel-tab]");
        const panel = await waitElement(phone ? "[data-lens-sheet]" : "[data-lens-panel]");
        if (panel instanceof HTMLDetailsElement && !panel.open) await userEvent.click(panel.querySelector("summary")!);
        await waitElement(`[data-lens-state="${id.slice('map-debt-'.length)}"]`);
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
      if (id === 'map-freshness') await waitFor(() => {
        expect(root.querySelector('[data-snapshot-freshness-note] [data-snapshot-state="failed"]')).not.toBeNull();
        expect(root.querySelector('[data-detail-status="pending"]')).not.toBeNull();
      });
      await root.fonts.ready;
      await new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
      root.documentElement.dataset.visualReady = id;
    },
  };
}
