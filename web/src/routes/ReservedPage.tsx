import { Link, useParams } from "@tanstack/react-router";

/** Placeholder for a reserved top-level name (see reserved.ts). Real content
 * for /about, /docs etc. is out of scope for this milestone — what matters
 * here is that the route exists and wins the match, not what it renders. */
export function ReservedPage() {
  const params = useParams({ strict: false });
  const name = (params as { name?: string }).name ?? "";
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 bg-[var(--chrome)] p-8 text-center text-[var(--on)]">
      <h1 className="font-sans text-lg font-semibold">/{name}</h1>
      <p className="max-w-sm text-sm text-[var(--dim)]">
        This name is reserved for tolmap itself and will never route to a GitHub
        owner called "{name}".
      </p>
      <Link to="/" className="text-sm text-[#6FB39F] underline underline-offset-2">
        back to the map index
      </Link>
    </div>
  );
}
