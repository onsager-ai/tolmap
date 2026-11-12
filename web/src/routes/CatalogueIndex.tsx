import { Link } from "@tanstack/react-router";
import { useCatalogue } from "@/data/queries";
import { SubmitRepoForm } from "@/components/SubmitRepoForm";
import { ChevronIcon } from "@/components/phone/icons";
import type { CatalogueEntry } from "@/types";
import { RepositoryRowActions } from "@/components/RepositoryRowActions";
import { AppShell, FirstRepositoryAction } from "@/components/AppShell";
import { hostProfile } from "@/host";
import { IndexedTime, RepositoryState } from "@/components/RepositoryMeta";

/** Stable host identities get repository navigation and a direct-map action.
 * Legacy static/service entries preserve the approved 64px direct-map row. */
function RepoRow({ m }: { m: CatalogueEntry }) {
  if (m.repository_id) return (
    <div data-catalogue-row className="relative flex min-h-[64px] items-center border-b border-[var(--rule)] last:border-b-0">
      <Link to="/repositories/$repositoryId/history" params={{ repositoryId: m.repository_id }} aria-label={`View repository ${m.label ?? m.slug}`} className="flex min-h-16 min-w-0 flex-1 flex-col justify-center gap-0.5 px-4 py-2 hover:bg-[var(--chrome)]">
        <span className="flex flex-wrap items-center gap-x-2 gap-y-1"><span className="truncate font-mono text-row">{m.label ?? m.slug}</span><RepositoryState status={m.status} /></span>
        <span className="text-meta text-[var(--dim)]"><span className="font-mono">{m.files.toLocaleString()}</span> files · <span className="font-mono">{m.districts.toLocaleString()}</span> districts · {m.lang}{m.indexed_at && <> · <IndexedTime value={m.indexed_at} /></>}</span>
      </Link>
      {m.latest_snapshot_id && <Link to="/$owner/$repo" params={{ owner: m.owner, repo: m.repo }} search={{ geo: "r", layer: "d" }} className="repo-control repo-control-ghost mr-2" aria-label={`Open latest map for ${m.label ?? m.slug}`}>Map <ChevronIcon /></Link>}
      {hostProfile && <RepositoryRowActions repositoryId={m.repository_id} name={m.label ?? m.slug} />}
    </div>
  );
  return (
    <Link
      to="/$owner/$repo"
      params={{ owner: m.owner, repo: m.repo }}
      search={{ geo: "r", layer: "d" }}
      data-catalogue-row
      className="flex min-h-[64px] w-full items-center gap-3 border-b border-[var(--rule)] px-4 py-2.5 text-left last:border-b-0 hover:bg-[var(--chrome)]"
    >
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate font-mono text-row">{m.label ?? m.slug}</span>
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

/** Home uses the shared shell. Hosted workspaces show their catalogue;
 * OSS also offers the public repository form (docs/UX.md §4.9). */
export function CatalogueIndex() {
  const { data, isLoading, isError, serviceAvailable, refetch } = useCatalogue();

  return (
    <div className="h-full overflow-y-auto bg-[var(--chrome)] text-[var(--on)]">
      <div
        className="mx-auto flex w-full max-w-[640px] flex-col gap-8 px-5 pb-10"
        style={{ paddingTop: "calc(28px + env(safe-area-inset-top, 0px))" }}
      >
        <AppShell />

        {!hostProfile && <div className="flex flex-col gap-3">
          <h1 className="text-title">Map a codebase</h1>
          <p className="text-body text-[var(--dim)]">
            tolmap groups a repository's files into districts by how they import each other and change together, so
            you can see its structure and find what to optimize.
          </p>
          <SubmitRepoForm />
        </div>}

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
                className="repo-control"
              >
                Retry
              </button>
            </div>
          )}

          {!isLoading && !isError && data.length === 0 && (
            hostProfile ? <div data-catalogue-empty className="flex flex-col items-start gap-3 rounded-[14px] border border-[var(--rule)] p-5"><h1 className="text-title">Map your first repository</h1><p className="text-small text-[var(--dim)]">Choose the repositories you want to explore.</p><FirstRepositoryAction /></div> : <p data-catalogue-empty className="text-small text-[var(--dim)]">No repositories mapped yet — paste one above.</p>
          )}

          {!isLoading && !isError && data.length > 0 && (
            <div className="flex flex-col rounded-[14px] border border-[var(--rule)]">
              {data.map((m) => (
                <RepoRow key={m.repository_id ?? m.slug} m={m} />
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
