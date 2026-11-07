//! Pure production native-local template, shared with the offline workbench.
pub const CSS: &str = r#"
html:has([data-local-page]), body:has([data-local-page]) { height: auto; min-height: 100%; overflow: auto; }
body:has([data-local-page]) { margin: 0; background: var(--chrome); color: var(--on); font-family: Archivo, sans-serif; font-size: 16px; line-height: 1.5; }
[data-local-page] { box-sizing: border-box; max-width: 760px; margin: 40px auto; padding: 20px; overflow-wrap: anywhere; }
[data-local-page] h1 { margin: 0 0 24px; font-size: 30px; font-weight: 700; line-height: 1.2; }
[data-local-page] h1 svg { display: inline-block; width: 28px; height: 28px; vertical-align: -3px; margin-right: 4px; }
[data-local-page] h2 { margin: 32px 0 12px; font-size: 20px; font-weight: 600; line-height: 1.3; }
[data-local-page] p { margin: 0 0 16px; }
[data-local-page] p:empty { display: none; }
[data-local-page] code { font-family: 'IBM Plex Mono', monospace; overflow-wrap: anywhere; }
[data-local-page] button { min-height: 44px; max-width: 100%; margin: 0 0 8px; padding: 10px 16px; background: var(--chrome2); color: var(--on); border: 1px solid var(--rule); border-radius: 12px; font: inherit; font-weight: 600; text-align: left; cursor: pointer; }
[data-local-page] button:focus-visible, [data-local-page] a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
[data-local-page] a { display: inline-flex; align-items: center; min-height: 44px; color: var(--accent); }
@media (max-width: 600px) { [data-local-page] { margin: 12px auto; } }
"#;
pub struct Job {
    pub id: String,
    pub stage: String,
}
pub struct Prepared {
    pub id: String,
    pub commit: String,
    pub chosen: String,
    pub all: Vec<String>,
}
pub struct Snapshot {
    pub id: String,
    pub commit: String,
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn render(
    stylesheet: &str,
    storage: &str,
    notice: &str,
    job: Option<&Job>,
    prepared: Option<&Prepared>,
    snapshots: &[Snapshot],
) -> String {
    let mut content=format!("<!doctype html><html lang=en><meta name=viewport content='width=device-width,initial-scale=1'><title>Tolmap local</title><link rel=icon type=image/x-icon href=/favicon.ico sizes='16x16 32x32 48x48'><link rel=icon type=image/svg+xml href=/favicon.svg sizes=any><link rel=apple-touch-icon href=/assets/brand/tolmap-apple-touch-icon.png><meta name=theme-color media='(prefers-color-scheme: light)' content='#eef1ee'><meta name=theme-color media='(prefers-color-scheme: dark)' content='#151c21'><link rel=stylesheet href='{stylesheet}'><link rel=stylesheet href=/local.css><main data-local-page><h1><svg viewBox='0 0 32 32' width=28 height=28 fill=currentColor aria-hidden=true><rect x=2 y=2 width=19 height=4></rect><rect x=26 y=2 width=4 height=19></rect><rect x=11 y=26 width=19 height=4></rect><rect x=2 y=11 width=4 height=19></rect><rect x=11 y=11 width=10 height=10></rect></svg>Your local repository</h1><p>Processing and storage stay on this computer. No account, subscription, provider login or upload is required.</p><p>This page opens only the checkout tolmap local was started with. A browser or hosted tolmap URL cannot open a folder on your computer, and nothing is sent to Hub from here; Hub is an optional, separate choice.</p><p>Storage: <code>{}</code></p><p>Only committed HEAD is analyzed. Uncommitted and untracked files are excluded. Commit your changes before refreshing.</p><p role=status>{}</p>",escape(storage),escape(notice));
    if let Some(j) = job {
        content.push_str(&format!("<meta http-equiv=refresh content=2><h2>Indexing</h2><p>{}</p><form method=post action=/local/cancel?id={}><button>Cancel indexing</button></form>",escape(&j.stage),j.id));
    } else if let Some(p) = prepared {
        content.push_str(&format!("<h2>Confirm analysis scope</h2><p>Commit <code>{}</code></p><p>{}</p><form method=post action=/local/index?id={}><button>Index detected source</button></form>",p.commit,escape(&p.chosen),p.id));
        if !p.all.is_empty() {
            content.push_str(&format!("<p>All sources above the detection floor: {}</p><form method=post action='/local/index?id={}&amp;all=true'><button>Index all listed sources</button></form>",p.all.iter().map(|x|escape(x)).collect::<Vec<_>>().join("; "),p.id));
        }
        content.push_str("<form method=post action=/local/refresh><button>Refresh from checkout HEAD</button></form>");
    } else {
        content.push_str("<form method=post action=/local/refresh><button>Detect committed source</button></form>");
    }
    content.push_str("<h2>Saved snapshots</h2><p>Localhost links work only on this computer. Map exports contain structure and symbol names, but no source text. Treat exported maps as private.</p>");
    for p in snapshots {
        content.push_str(&format!("<p><a href=/snapshot/{}>Open <code>{}</code></a> · <a href=/api/maps/snapshot/{}/artifact>Export map JSON (no source)</a></p>",p.id,p.commit,p.id));
    }
    content.push_str("</main></html>");
    content
}
