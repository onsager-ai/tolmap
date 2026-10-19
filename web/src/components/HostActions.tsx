import { hostProfile } from "@/host";

/** Product identity lives in the authored Home frame; the map stays full bleed. */
export function HostActions() {
  if (!hostProfile) return null;
  return (
    <div data-host-workspace className="flex flex-wrap items-center gap-3 text-small">
      <span className="mr-auto text-[var(--dim)]">{hostProfile.workspace}</span>
      {hostProfile.actions.map((action) => action.method === "post" ? (
        <form key={action.path} method="post" action={action.path}>
          <button className="min-h-11 rounded-[12px] border border-[var(--rule)] px-3" type="submit">{action.label}</button>
        </form>
      ) : (
        <a key={action.path} className="inline-flex min-h-11 items-center underline underline-offset-2" href={action.path}>{action.label}</a>
      ))}
    </div>
  );
}
