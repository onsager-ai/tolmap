import { Outlet, useLocation } from "@tanstack/react-router";
import { AddRepositoryProvider, AppShell } from "@/components/AppShell";
import { hostProfile } from "@/host";

export function RootLayout() {
  const pathname = useLocation({ select: location => location.pathname });
  const map = /^\/[^/]+\/[^/]+$/.test(pathname) && !pathname.startsWith("/org/");
  return <AddRepositoryProvider>{hostProfile && map ? <div className="flex h-full flex-col bg-[var(--chrome)] text-[var(--on)]"><div className="border-b border-[var(--rule)] px-4 py-2"><AppShell /></div><div className="relative min-h-0 flex-1"><Outlet /></div></div> : <Outlet />}</AddRepositoryProvider>;
}
