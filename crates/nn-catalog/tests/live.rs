//! Against the real upstreams. Run with `cargo test -p nn-catalog -- --ignored`.

use nn_catalog::{Catalog, versions::LOADERS};

#[tokio::test]
#[ignore = "talks to the internet"]
async fn every_loader_lists_versions_and_builds() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Catalog::new(Some(dir.path().to_owned()), Default::default());
    for loader in LOADERS {
        let versions = catalog.mc_versions(loader, false).await.unwrap();
        assert!(!versions.is_empty(), "{loader}: no versions");
        let newest = &versions[0].id;
        let builds = catalog.loader_versions(loader, newest).await.unwrap();
        let older = catalog
            .loader_versions(loader, "1.20.1")
            .await
            .unwrap_or_default();
        println!(
            "{loader:9} {} versions, newest {newest}: {} builds (recommended {:?}); 1.20.1: {} builds",
            versions.len(),
            builds.len(),
            builds.iter().find(|b| b.recommended).map(|b| &b.version),
            older.len(),
        );
        if *loader != "vanilla" {
            assert!(!builds.is_empty(), "{loader} has no builds for {newest}");
        }
    }
}
