import type { OrganizationMemberMap } from "@bindings/OrganizationMemberMap";
import type { PinSkew } from "@bindings/PinSkew";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { ApiRequestError } from "@/api/client";
import { ThemeToggle } from "@/components/ThemeToggle";
import { TolmapLogo } from "@/components/TolmapLogo";
import { useOrganizationDocument, useOrganizationMemberMaps } from "@/data/queries";
import { OrganizationCanvas } from "@/org/OrganizationCanvas";
import type { OrganizationSearch } from "@/routes/search";

function shortCommit(commit: string) {
  return commit.slice(0, 12);
}

function relatedSkew(skew: readonly PinSkew[], repo: string) {
  return skew.filter(item => item.consumer === repo || item.provider === repo);
}

function evidenceLabel(count: number) {
  return `${count} dependency ${count === 1 ? "edge" : "edges"}`;
}

function RepositoryCard({ member, pinSkew, edges, onOpen }: { member: OrganizationMemberMap; pinSkew: readonly PinSkew[]; edges: readonly { consumer: string; provider: string; evidence: unknown[] }[]; onOpen(): void }) {
  const incoming = edges.filter(edge => edge.provider === member.repo);
  const outgoing = edges.filter(edge => edge.consumer === member.repo);
  const skew = relatedSkew(pinSkew, member.repo);
  return (
    <aside className="absolute bottom-4 left-4 right-4 z-10 max-h-[calc(100%-2rem)] overflow-y-auto rounded-[16px] border border-[var(--rule)] bg-[var(--chrome)] p-4 shadow-xl sm:bottom-6 sm:left-6 sm:right-auto sm:w-[390px]" data-organization-card>
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="font-mono text-title">{member.repo}</p>
          <p className="mt-1 text-meta text-[var(--dim)]">Pinned commit <code>{shortCommit(member.commit)}</code></p>
          <p className="mt-1 break-all font-mono text-meta text-[var(--dim)]" data-organization-full-commit>{member.commit}</p>
        </div>
        <span className="rounded-full border border-[var(--rule)] px-2 py-1 font-mono text-meta">{member.map}</span>
      </div>
      <dl className="mt-4 grid grid-cols-2 gap-x-4 gap-y-2 text-small">
        <div><dt className="text-[var(--dim)]">Files</dt><dd className="font-mono">{member.files.toLocaleString()}</dd></div>
        <div><dt className="text-[var(--dim)]">Districts</dt><dd className="font-mono">{member.districts.toLocaleString()}</dd></div>
      </dl>
      <section className="mt-4">
        <h2 className="text-meta font-semibold">Provided packages</h2>
        <ul className="mt-1 space-y-1 text-small">{member.packages.length ? member.packages.map(pkg => <li key={`${pkg.ecosystem}:${pkg.name}`}><code>{pkg.name}</code>{pkg.version ? ` · ${pkg.version}` : ""}</li>) : <li className="text-[var(--dim)]">No declared packages</li>}</ul>
      </section>
      <section className="mt-4 grid grid-cols-2 gap-3 text-small">
        <div><h2 className="text-meta font-semibold">Depends on</h2><ul className="mt-1 space-y-1">{outgoing.length ? outgoing.map(edge => <li key={edge.provider}><code>{edge.provider}</code> · {evidenceLabel(edge.evidence.length)}</li>) : <li className="text-[var(--dim)]">None found</li>}</ul></div>
        <div><h2 className="text-meta font-semibold">Used by</h2><ul className="mt-1 space-y-1">{incoming.length ? incoming.map(edge => <li key={edge.consumer}><code>{edge.consumer}</code> · {evidenceLabel(edge.evidence.length)}</li>) : <li className="text-[var(--dim)]">None found</li>}</ul></div>
      </section>
      {skew.length > 0 && <section className="mt-4 rounded-[10px] border border-[var(--link-out)]/40 bg-[var(--chrome2)] p-3 text-small" data-pin-skew><h2 className="font-semibold">Pin skew</h2><ul className="mt-1 space-y-1">{skew.map(item => <li key={`${item.consumer}:${item.file}:${item.provider}`}><code>{item.consumer}</code> pins <code>{item.provider}</code> to {item.pin.value} ({shortCommit(item.member_commit)} is the member commit).</li>)}</ul></section>}
      <button type="button" onClick={onOpen} className="mt-5 flex h-11 w-full items-center justify-center rounded-[12px] bg-[var(--on)] px-4 text-small font-semibold text-[var(--canvas)]" data-organization-open-map>Open map at {shortCommit(member.commit)}</button>
    </aside>
  );
}

/** The organization route composes immutable member documents. It never
 * invokes a layout or calculates a combined modularity: the service-supplied
 * regions and roads are the only cross-repository geometry/evidence here. */
export function OrganizationView() {
  const { name } = useParams({ strict: false }) as { name: string };
  const search = useSearch({ strict: false }) as OrganizationSearch;
  const navigate = useNavigate();
  const document = useOrganizationDocument(name);
  const members = useOrganizationMemberMaps(document.data?.members ?? []);
  const selected = document.data?.members.find(member => member.repo === search.repo);

  function selectRepository(repo?: string) {
    navigate({ to: ".", search: { repo }, replace: true, state: true });
  }

  if (document.isLoading) return <div className="flex h-full items-center justify-center bg-[var(--canvas)] text-small text-[var(--dim)]" data-organization-loading>Loading organization…</div>;
  if (document.isError || !document.data) {
    const error = document.error;
    const missingMap = error instanceof ApiRequestError && error.status === 404;
    return <div className="flex h-full flex-col items-center justify-center gap-2 bg-[var(--chrome)] p-6 text-center text-small"><p className="text-[var(--link-out)]" data-organization-error>{missingMap ? "A pinned member map is unavailable (404)." : `Couldn't load organization ${name}.`}</p><p className="text-[var(--dim)]">{error instanceof Error ? error.message : "The organization is unavailable."}</p><Link to="/" className="mt-2 text-[var(--link)]">Back to the map index</Link></div>;
  }
  if (members.isLoading) return <div className="flex h-full items-center justify-center bg-[var(--canvas)] text-small text-[var(--dim)]" data-organization-loading>Loading pinned member maps…</div>;
  if (members.error || members.maps.length !== document.data.members.length) return <div className="flex h-full flex-col items-center justify-center gap-2 bg-[var(--chrome)] p-6 text-center text-small"><p className="text-[var(--link-out)]" data-organization-error>A pinned member map is unavailable (404).</p><p className="text-[var(--dim)]">{members.error instanceof Error ? members.error.message : "The organization cannot be composed without every pinned member map."}</p></div>;

  return <div className="relative h-full overflow-hidden bg-[var(--canvas)]" data-organization-view>
    <OrganizationCanvas organization={document.data} maps={members.maps} selectedRepo={selected?.repo} onSelectRepository={selectRepository} />
    <header className="pointer-events-none absolute left-4 right-4 top-4 z-10 flex items-center justify-between sm:left-6 sm:right-6 sm:top-6">
      <div className="pointer-events-auto rounded-[14px] border border-[var(--rule)] bg-[var(--chrome)] px-3 py-2 shadow-sm"><Link to="/" className="flex items-center gap-2"><TolmapLogo className="h-7 w-auto" /><span className="font-mono text-small">{document.data.name}</span></Link><p className="mt-1 text-meta text-[var(--dim)]">Repository outlines · district outlines · dependency roads</p></div>
      <ThemeToggle className="pointer-events-auto h-11 w-11 rounded-full border border-[var(--rule)] bg-[var(--chrome)]" />
    </header>
    {selected && <RepositoryCard member={selected} pinSkew={document.data.pin_skew} edges={document.data.edges} onOpen={() => {
      const [owner, repo] = selected.repo.split("/", 2);
      navigate({ to: "/$owner/$repo", params: { owner, repo }, search: { geo: "r", layer: "d", commit: selected.commit } });
    }} />}
  </div>;
}
