import "./repository.css";
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { Link, useLocation } from "@tanstack/react-router";
import { hostProfile } from "@/host";
import { TolmapLogo } from "@/components/TolmapLogo";
import { ThemeToggle } from "@/components/ThemeToggle";
import { HostActions, HostActionLink } from "@/components/HostActions";
import { AddRepositorySheet } from "@/components/AddRepositorySheet";

const AddRepositoryContext = createContext<() => void>(() => {});
export function AddRepositoryProvider({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const href = useLocation({ select: location => location.href });
  const marker = hostProfile?.onboarding?.open_sheet_parameter;
  useEffect(() => {
    if (marker && new URL(href, window.location.origin).searchParams.get(marker) === "1") setOpen(true);
  }, [href, marker]);
  function close() {
    setOpen(false);
    if (marker) {
      const url = new URL(window.location.href);
      url.searchParams.delete(marker);
      window.history.replaceState(window.history.state, "", url);
    }
  }
  return <AddRepositoryContext.Provider value={() => setOpen(true)}>{children}{open && hostProfile?.onboarding && <AddRepositorySheet onClose={close} />}</AddRepositoryContext.Provider>;
}

export function useAddRepository() { return useContext(AddRepositoryContext); }

export function AddRepositoryButton() {
  const open = useAddRepository();
  if (!hostProfile?.onboarding) return null;
  return <button type="button" className="repo-control" onClick={open}>Add repository</button>;
}

export function FirstRepositoryAction() {
  const install = hostProfile?.onboarding?.install;
  return install ? <HostActionLink action={install} className="repo-control repo-control-primary">Connect GitHub</HostActionLink> : <AddRepositoryButton />;
}

/** Shared identity and account controls, including the map command bar. */
export function AppShell() {
  return <header data-app-shell className="app-shell">
    <Link to="/" aria-label="tolmap home" className="shrink-0"><TolmapLogo className="h-7 w-auto" /></Link>
    {hostProfile && <span data-host-workspace className="app-workspace text-small text-[var(--dim)]">{hostProfile.workspace}</span>}
    <div className="ml-auto flex items-center gap-2 py-1"><AddRepositoryButton /><HostActions /><ThemeToggle className="repo-control repo-control-icon" /></div>
  </header>;
}
