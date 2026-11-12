import { hostProfile, hostPath } from "@/host";
import type { HostAction } from "@bindings/HostAction";

export function HostActionLink({ action, children, className = "repo-control" }: { action: HostAction; children?: React.ReactNode; className?: string }) {
  return action.method === "post" ? <form method="post" action={hostPath(action.path)}><button className={className} type="submit">{children ?? action.label}</button></form>
    : <a className={className} href={hostPath(action.path)}>{children ?? action.label}</a>;
}

export function HostActions() {
  if (!hostProfile || hostProfile.actions.length === 0) return null;
  return <details className="relative"><summary aria-label="Account menu" className="repo-control repo-control-ghost repo-control-icon list-none">⋯</summary><div className="repo-menu">
    {hostProfile.actions.map(action => <HostActionLink key={action.path} action={action} className="repo-control repo-control-ghost" />)}
  </div></details>;
}
