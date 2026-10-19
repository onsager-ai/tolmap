import { useEffect, useState } from "react";
import foundation from "../src/index.css?inline";
import localStyle from "../storybook-public/local-pages/local.css?inline";
import { applyThemeAttribute, type ThemeChoice } from "../src/lib/theme";

export function NativeLocal({ name, theme }: { name: string; theme: ThemeChoice }) {
  const [html, setHtml] = useState<string>();
  useEffect(() => {
    const controller = new AbortController();
    void fetch(new URL(`local-pages/${name}.html`, location.href), { signal: controller.signal }).then(response => {
      if (!response.ok) throw new Error("Native local fixture missing");
      return response.text();
    }).then(text => setHtml(text
      // Freeze production auto-refresh for deterministic offline exploration.
      .replace('<meta http-equiv=refresh content=2>', '')
      .replace("<link rel=stylesheet href='/local-foundation.css'>", `<style>${foundation}</style>`)
      .replace('<link rel=stylesheet href=/local.css>', `<style>${localStyle}</style>`)
    ));
    return () => controller.abort();
  }, [name]);
  applyThemeAttribute(theme);
  return <div style={{ height: "100dvh", display: "flex", flexDirection: "column" }}>
    <div style={{ padding: "6px 12px", fontSize: 12, color: "var(--dim)", background: "var(--chrome2)" }}>Design fixture · synthetic local checkout · candidate reference</div>
    {html && <iframe title="Tolmap native local fixture" srcDoc={html} style={{ border: 0, width: "100%", flex: 1 }} onLoad={event => {
      const doc = event.currentTarget.contentDocument!;
      if (theme !== "system") doc.documentElement.dataset.theme = theme;
      doc.addEventListener("submit", event => event.preventDefault());
      doc.addEventListener("click", event => { if ((event.target as Element).closest("a")) event.preventDefault(); });
      doc.documentElement.dataset.fixtureReady = name;
    }} />}
  </div>;
}
