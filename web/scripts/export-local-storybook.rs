//! Compile this tiny std-only exporter on CI. It shares production markup,
//! without starting a local service, reading a checkout or building the engine.
#[path = "../../src/local_pages.rs"]
mod pages;
fn main() -> std::io::Result<()> {
    let output = std::path::PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    std::fs::write(output.join("local.css"), pages::CSS)?;
    let prepared = pages::Prepared {
        id: "00000000-0000-0000-0000-000000000001".into(),
        commit: "1111111111111111111111111111111111111111".into(),
        chosen: "src · Rust · 120 files · confidence 0.96 (synthetic fixture)".into(),
        all: vec!["src · Rust".into(), "web/src · TypeScript".into()],
    };
    let job = pages::Job { id: prepared.id.clone(), stage: "Parsing committed source".into() };
    let snapshots = vec![pages::Snapshot { id: prepared.id.clone(), commit: prepared.commit.clone() }];
    for (name, notice, running, selected, history) in [
        ("start", "", None, None, &[][..]),
        ("confirm", "", None, Some(&prepared), &[][..]),
        ("running", "", Some(&job), Some(&prepared), &[][..]),
        ("cancelling", "Cancellation requested; waiting for the worker to stop.", Some(&job), Some(&prepared), &[][..]),
        ("detection-failed", "Source detection failed. Confirm this checkout has a supported committed source and retry.", None, None, &[][..]),
        ("index-failed", "Indexing failed or was cancelled. The committed source selection is retained; retry indexing or refresh from HEAD.", None, Some(&prepared), &[][..]),
        ("saved", "Snapshot saved. Open it from history below.", None, None, &snapshots[..]),
    ] {
        std::fs::write(output.join(format!("{name}.html")), pages::render("/local-foundation.css", "/private/synthetic-tolmap-store", notice, running, selected, history))?;
    }
    Ok(())
}
