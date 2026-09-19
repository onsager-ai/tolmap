// Reserved top-level names, wired into the route tree before any of them can
// collide with a real GitHub owner. The map lives at /:owner/:repo with no
// forge prefix (docs/ARCHITECTURE.md, "MVP: a site that maps any public
// repository") — that scheme has no room for the app's own pages unless
// their names are carved out first. Each name gets a real static route (see
// router.tsx); TanStack Router matches a static segment before a dynamic
// one at the same depth, so /settings never falls through to $owner even if
// a single-segment owner route is added later.
export const RESERVED_NAMES = new Set([
  "about",
  "docs",
  "api",
  "new",
  "settings",
  "assets",
  "static",
  "health",
  "login",
  "admin",
  "search",
]);
