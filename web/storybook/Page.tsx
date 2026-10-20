import { useMemo } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { routeTree } from "../src/router";

export function Page({ route }: { route: string }) {
  const state = useMemo(() => ({
    query: new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } }),
    router: createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [route] }) }),
  }), [route]);
  return <div style={{ height: "100dvh", display: "flex", flexDirection: "column" }}>
    <div data-design-fixture style={{ flex: "0 0 auto", padding: "6px 12px", font: "12px/20px Archivo, sans-serif", background: "var(--chrome2)", color: "var(--dim)", borderBottom: "1px solid var(--rule)" }}>
      Design fixture · synthetic data · candidate reference
    </div>
    <div data-product-page data-preview-route={state.router.state.location.pathname} style={{ flex: "1 1 auto", minHeight: 0, position: "relative" }} onSubmit={event => event.preventDefault()} onClickCapture={event => {
      const anchor = (event.target as Element).closest("a");
      // Router Links remain interactive. Native host/provider links stay here.
      if (anchor && (new URL(anchor.href).origin !== location.origin || ["/workspace", "/logout"].includes(anchor.getAttribute("href") ?? ""))) event.preventDefault();
    }}>
      <QueryClientProvider client={state.query}><RouterProvider router={state.router} /></QueryClientProvider>
    </div>
  </div>;
}
