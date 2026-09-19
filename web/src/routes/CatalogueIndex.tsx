import { Link } from "@tanstack/react-router";
import { useCatalogue } from "@/data/queries";
import { Card, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { SubmitRepoForm } from "@/components/SubmitRepoForm";

export function CatalogueIndex() {
  const { data, isLoading, isError, error, serviceAvailable } = useCatalogue();

  return (
    <div className="h-full overflow-y-auto bg-[var(--chrome)] px-5 py-8 text-[var(--on)]">
      <div className="mx-auto max-w-3xl">
        <h1 className="font-sans text-xl font-semibold">tolmap</h1>
        <p className="mt-1.5 text-[12px] text-[var(--dim)]">
          Paste a public GitHub repository to map it, or pick one of the maps already indexed —
          districts computed from imports, co-change and structural proximity.
        </p>

        <SubmitRepoForm />

        {isLoading && <p className="mt-8 text-sm text-[var(--dim)]">loading catalogue…</p>}
        {isError && (
          <p className="mt-8 text-sm text-[var(--hot)]">
            couldn't load the catalogue: {(error as Error)?.message}. Run `pnpm dev` (or
            `pnpm build`) so scripts/collect-maps.mjs has generated the bundled set.
          </p>
        )}

        {data.length > 0 && (
          <div className="mt-8 grid grid-cols-1 gap-3 sm:grid-cols-2">
            {data.map((m) => (
              <Link
                key={m.slug}
                to="/$owner/$repo"
                params={{ owner: m.owner, repo: m.repo }}
                search={{ geo: "r", layer: "d" }}
              >
                <Card className="cursor-pointer bg-[var(--chrome2)] transition-colors hover:border-[#6FB39F]">
                  <CardHeader>
                    <div className="flex items-start justify-between gap-2">
                      <CardTitle className="text-[var(--on)]">{m.slug}</CardTitle>
                      {m.source === "service" && (
                        <Badge className="flex-none bg-[#6FB39F] text-[#0a1410]">indexed</Badge>
                      )}
                    </div>
                    <CardDescription>
                      {m.files} files · {m.districts} districts · modularity {m.modularity.toFixed(3)} · {m.lang}
                    </CardDescription>
                  </CardHeader>
                </Card>
              </Link>
            ))}
          </div>
        )}

        {!isLoading && !isError && data.length === 0 && (
          <p className="mt-8 text-sm text-[var(--dim)]">no maps yet — paste a repository above.</p>
        )}

        {serviceAvailable === false && (
          <p className="mt-8 text-[11px] text-[var(--dim)]">
            showing the bundled catalogue only — the indexing service isn't reachable.
          </p>
        )}
      </div>
    </div>
  );
}
