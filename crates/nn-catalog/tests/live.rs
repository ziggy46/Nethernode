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

#[tokio::test]
#[ignore = "talks to the internet"]
async fn modrinth_search_versions_and_hashes() {
    use nn_catalog::{ProjectKind, SearchQuery};
    let catalog = Catalog::default();
    let page = catalog
        .modrinth_search(
            ProjectKind::Mod,
            &SearchQuery {
                text: "lithium".into(),
                mc_version: Some("1.20.1".into()),
                loader: Some("fabric".into()),
                limit: 5,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let lithium = page
        .projects
        .iter()
        .find(|p| p.slug == "lithium")
        .expect("lithium");
    let versions = catalog
        .modrinth_versions(&lithium.id, Some("1.20.1"), Some("fabric"))
        .await
        .unwrap();
    let file = versions[0].primary_file().unwrap();
    let sha1 = file.sha1.clone().unwrap();
    let found = catalog
        .modrinth_identify(std::slice::from_ref(&sha1))
        .await
        .unwrap();
    assert_eq!(found[&sha1].project_id, lithium.id);
    let updates = catalog
        .modrinth_updates(std::slice::from_ref(&sha1), "1.20.1", "fabric")
        .await
        .unwrap();
    assert!(updates.contains_key(&sha1));
    let packs = catalog
        .modrinth_search(
            ProjectKind::Modpack,
            &SearchQuery {
                limit: 3,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    println!(
        "lithium {} versions; top packs: {:?}",
        versions.len(),
        packs.projects.iter().map(|p| &p.slug).collect::<Vec<_>>()
    );
    let project = catalog.modrinth_project("lithium").await.unwrap();
    assert_eq!(project.id, lithium.id);
}
