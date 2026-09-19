import { createRootRoute, createRoute, createRouter, notFound } from "@tanstack/react-router";
import { RootLayout } from "@/routes/RootLayout";
import { CatalogueIndex } from "@/routes/CatalogueIndex";
import { ReservedPage } from "@/routes/ReservedPage";
import { MapView } from "@/routes/MapView";
import { IndexJobView } from "@/routes/IndexJobView";
import { RESERVED_NAMES } from "@/routes/reserved";
import { validateMapSearch, validateJobSearch } from "@/routes/search";

const rootRoute = createRootRoute({ component: RootLayout });

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: CatalogueIndex,
});

// One static route per reserved name (see routes/reserved.ts), except "new"
// which gets its own component below — the progress view, not the generic
// placeholder. TanStack Router prefers a static path segment over a dynamic
// one at the same depth, so these win the match before $owner/$repo ever
// sees a request for e.g. /settings — which matters the day a
// single-segment /$owner route (a profile page, say) gets added, not just
// today.
const reservedRoutes = [...RESERVED_NAMES]
  .filter((name) => name !== "new")
  .map((name) =>
    createRoute({
      getParentRoute: () => rootRoute,
      path: `/${name}`,
      component: ReservedPage,
    }),
  );

// The submit flow's progress view (milestone brief, "A progress view"):
// /new?job=<id>&slug=<owner/name>, watching one indexing job via SSE/poll
// and routing to /:owner/:repo on completion. "new" is already reserved
// above's set, so this can never collide with a real GitHub owner.
const newRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/new",
  validateSearch: validateJobSearch,
  component: IndexJobView,
});

const ownerRepoRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/$owner/$repo",
  // Defensive belt-and-braces for the two-segment case a reserved name COULD
  // collide with (e.g. a future /settings/general): refuse to resolve a
  // reserved word as a repository owner even though no current GitHub owner
  // literally spells "settings".
  beforeLoad: ({ params }) => {
    if (RESERVED_NAMES.has(params.owner)) throw notFound();
  },
  validateSearch: validateMapSearch,
  component: MapView,
});

const routeTree = rootRoute.addChildren([indexRoute, newRoute, ...reservedRoutes, ownerRepoRoute]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
