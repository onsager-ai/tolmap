//! Pure production native-local template, shared with the offline workbench.
pub const CSS: &str = "html:has([data-local-page]),body:has([data-local-page]){height:auto;min-height:100%;overflow:auto}body{margin:0;background:var(--chrome);color:var(--on)}main{max-width:760px;margin:40px auto;padding:20px}button,a{min-height:44px}button{padding:12px;margin:6px 0;background:var(--chrome2);color:var(--on);border:1px solid var(--rule);border-radius:8px}code{overflow-wrap:anywhere}a{color:var(--accent)}";
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
    let mut content=format!("<!doctype html><html lang=en><meta name=viewport content='width=device-width,initial-scale=1'><title>Tolmap local</title><link rel=stylesheet href='{stylesheet}'><link rel=stylesheet href=/local.css><main data-local-page><h1>Your local repository</h1><p>Processing and storage stay on this computer. No account, subscription, provider login or upload is required.</p><p>Storage: <code>{}</code></p><p>Only committed HEAD is analyzed. Uncommitted and untracked files are excluded. Commit your changes before refreshing.</p><p role=status>{}</p>",escape(storage),escape(notice));
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
