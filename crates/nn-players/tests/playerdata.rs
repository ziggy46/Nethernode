//! Player files saved by real servers: vanilla 26.1 and 1.20.1, after a
//! test player received items, armor, an off-hand shield, an ender chest
//! item, creative mode, and five levels.

use nn_players::nbt;
use nn_players::playerdata::{self, PlayerData};

fn load(name: &str) -> PlayerData {
    let bytes = std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let (_, root) = nbt::read_gzip(&bytes).unwrap();
    playerdata::parse(&root)
}

#[test]
fn modern_layout() {
    let data = load("player-26.1.dat");
    assert_eq!(data.data_version, Some(4786));
    assert_eq!(data.dimension.as_deref(), Some("minecraft:overworld"));
    assert_eq!(
        (data.game_mode, data.xp_level, data.food),
        (Some(1), Some(5), Some(20))
    );
    assert_eq!(data.health, Some(20.0));
    assert!(data.position.is_some());
    let diamonds = &data.inventory[0];
    assert_eq!(
        (diamonds.id.as_str(), diamonds.count, diamonds.slot),
        ("minecraft:diamond", 64, 0)
    );
    let sword = &data.inventory[1];
    assert_eq!(sword.id, "minecraft:diamond_sword");
    assert_eq!(
        sword.enchantments,
        vec![("minecraft:sharpness".to_owned(), 3)]
    );
    assert_eq!(
        data.equipment.head.as_ref().unwrap().id,
        "minecraft:iron_helmet"
    );
    assert_eq!(
        data.equipment.offhand.as_ref().unwrap().id,
        "minecraft:shield"
    );
    assert_eq!(data.ender_chest[0].id, "minecraft:emerald");
    assert_eq!(data.ender_chest[0].count, 5);
}

#[test]
fn legacy_layout() {
    let data = load("player-1.20.1.dat");
    assert_eq!(data.data_version, Some(3465));
    assert_eq!(data.position, Some([10.5, 75.0, -19.5]));
    assert_eq!(data.dimension.as_deref(), Some("minecraft:overworld"));
    assert_eq!(
        data.inventory.len(),
        1,
        "armor and off hand aren't inventory slots"
    );
    assert_eq!(
        (data.inventory[0].id.as_str(), data.inventory[0].count),
        ("minecraft:diamond", 64)
    );
    assert_eq!(
        data.equipment.head.as_ref().unwrap().id,
        "minecraft:iron_helmet"
    );
    assert_eq!(
        data.equipment.offhand.as_ref().unwrap().id,
        "minecraft:shield"
    );
    assert_eq!(data.ender_chest[0].count, 5);
    assert_eq!(data.xp_level, Some(5));
}

#[test]
fn stats_file() {
    let bytes = std::fs::read(format!(
        "{}/tests/fixtures/stats-26.1.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let stats = playerdata::parse_stats(&bytes);
    let play = stats
        .iter()
        .find(|s| s.key == "minecraft:play_time")
        .unwrap();
    assert_eq!(play.value, 504);
}
