import { Link } from "@tanstack/react-router";
import { useCatalogue } from "@/data/queries";
import { SubmitRepoForm } from "@/components/SubmitRepoForm";
import { ThemeToggle } from "@/components/ThemeToggle";
import { ChevronIcon } from "@/components/phone/icons";
import type { CatalogueEntry } from "@/types";

/** docs/UX.md §4.9's "Mapped repositories" row: 64px, name then files ·
 * districts · language, from whatever the catalogue actually returns --
 * `CatalogueEntry` (src/types/index.ts) never carries a field that isn't in
 * both the static index and GET /api/maps, so nothing here is invented.
 * Numbers are mono (§8.1); the rest of the meta line is Archivo, matching
 * the approved artboard exactly (only "6,347" and "19" are mono there, not
 * "files" or "districts"). */
function RepoRow({ m }: { m: CatalogueEntry }) {
  return (
    <Link
      to="/$owner/$repo"
      params={{ owner: m.owner, repo: m.repo }}
      search={{ geo: "r", layer: "d" }}
      data-catalogue-row
      className="flex min-h-[64px] w-full items-center gap-3 border-b border-[var(--rule)] px-4 py-2.5 text-left last:border-b-0 hover:bg-[var(--chrome)]"
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate font-mono text-row">{m.slug}</span>
        <span className="truncate text-meta text-[var(--dim)]">
          <span className="font-mono">{m.files.toLocaleString()}</span> files ·{" "}
          <span className="font-mono">{m.districts.toLocaleString()}</span> district{m.districts === 1 ? "" : "s"} ·{" "}
          {m.lang}
        </span>
      </span>
      <span className="flex-none text-[var(--dim)]" aria-hidden="true">
        <ChevronIcon />
      </span>
    </Link>
  );
}

/** docs/UX.md §4.9 (phase 6): a plain one-screen page -- wordmark and theme
 * button, "Map a codebase" with one sentence, the repo field and its
 * button, then "Mapped repositories". The same content sits in a 640px
 * column on desktop, which is just this container's max-width -- Home has
 * no chrome that differs by breakpoint the way the map view's rail/sheet
 * do, so no useIsNarrow branch is needed here. */
export function CatalogueIndex() {
  const { data, isLoading, isError, serviceAvailable, refetch } = useCatalogue();

  return (
    <div className="h-full overflow-y-auto bg-[var(--chrome)] text-[var(--on)]">
      <div
        className="mx-auto flex w-full max-w-[640px] flex-col gap-8 px-5 pb-10"
        style={{ paddingTop: "calc(28px + env(safe-area-inset-top, 0px))" }}
      >
        <div className="flex items-center justify-between">
          <span className="text-title">tolmap</span>
          <ThemeToggle className="h-11 w-11 rounded-full" />
        </div>

        <div className="flex flex-col gap-3">
          <h1 className="text-title">Map a codebase</h1>
          <p className="text-body text-[var(--dim)]">
            tolmap groups a repository's files into districts by how they import each other and change together, so
            you can see its structure and find what to optimize.
          </p>
          <SubmitRepoForm />
        </div>

        <div className="flex flex-col gap-2.5">
          <h2 className="text-section-title">Mapped repositories</h2>

          {isLoading && <p className="text-small text-[var(--dim)]">loading…</p>}

          {isError && (
            <div
              data-catalogue-error
              className="flex flex-col items-start gap-3 rounded-[14px] border border-[var(--rule)] bg-[var(--chrome2)] p-4"
            >
              <p className="text-small text-[var(--link-out)]">
                tolmap's index couldn't be reached. This is usually temporary.
              </p>
              <button
                type="button"
                data-catalogue-retry
                onClick={() => refetch()}
                className="flex h-11 items-center rounded-[12px] border border-[var(--rule)] bg-[var(--chrome)] px-4 text-small font-semibold text-[var(--on)]"
              >
                Retry
              </button>
            </div>
          )}

          {!isLoading && !isError && data.length === 0 && (
            <p data-catalogue-empty className="text-small text-[var(--dim)]">
              No repositories mapped yet — paste one above.
            </p>
          )}

          {!isLoading && !isError && data.length > 0 && (
            <div className="flex flex-col overflow-hidden rounded-[14px] border border-[var(--rule)]">
              {data.map((m) => (
                <RepoRow key={m.slug} m={m} />
              ))}
            </div>
          )}

          {!isError && serviceAvailable === false && data.length > 0 && (
            <p className="text-meta text-[var(--dim)]">
              showing the bundled catalogue only — the indexing service isn't reachable.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
