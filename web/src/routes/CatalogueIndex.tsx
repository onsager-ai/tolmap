import { Link } from "@tanstack/react-router";
import { useCatalogue } from "@/data/queries";
import { Card, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";

export function CatalogueIndex() {
  const { data, isLoading, isError, error } = useCatalogue();

  return (
    <div className="h-full overflow-y-auto bg-[var(--chrome)] px-5 py-8 text-[var(--on)]">
      <div className="mx-auto max-w-3xl">
        <h1 className="font-sans text-xl font-semibold">tolmap</h1>
        <p className="mt-1.5 text-[12px] text-[var(--dim)]">
          A map of nine reference repositories, districts computed from imports,
          co-change and structural proximity. Pick one.
        </p>

        {isLoading && <p className="mt-8 text-sm text-[var(--dim)]">loading catalogue…</p>}
        {isError && (
          <p className="mt-8 text-sm text-[var(--hot)]">
            couldn't load /maps/index.json: {(error as Error).message}. Run `pnpm dev` (or
            `pnpm build`) so scripts/collect-maps.mjs has generated it.
          </p>
        )}

        {data && (
          <div className="mt-6 grid grid-cols-1 gap-3 sm:grid-cols-2">
            {data.map((m) => (
              <Link
                key={m.slug}
                to="/$owner/$repo"
                params={{ owner: m.owner, repo: m.repo }}
                search={{ geo: "r", layer: "d" }}
              >
                <Card className="cursor-pointer bg-[var(--chrome2)] transition-colors hover:border-[#6FB39F]">
                  <CardHeader>
                    <CardTitle className="text-[var(--on)]">{m.slug}</CardTitle>
                    <CardDescription>
                      {m.files} files · {m.districts} districts · modularity {m.modularity.toFixed(3)} · {m.lang}
                    </CardDescription>
                  </CardHeader>
                </Card>
              </Link>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
