import { Outlet } from "@tanstack/react-router";

/** Chrome shared by every route. Currently just an outlet — the top bar and
 * canvas live inside MapView because they need repo-specific state (the
 * reserved-name and catalogue pages don't have a map to show controls for). */
export function RootLayout() {
  return <Outlet />;
}
