//! Player data and statistics, read-only.
//!
//! Where they live and how items are stored changed over the years, so
//! both shapes are read:
//!
//! - 26.x: `world/players/data/<uuid>.dat` and `world/players/stats/`;
//!   items are `{id, count, components}`; armor and offhand live in an
//!   `equipment` compound.
//! - Earlier: `world/playerdata/<uuid>.dat` and `world/stats/`; before
//!   1.20.5 items are `{id, Count, tag}` and armor/offhand are inventory
//!   slots 100–103 and -106.
//!
//! The server writes these when a player leaves and on autosave, so for an
//! online player they show the state as of the last save.

use crate::nbt::Tag;

/// Where a player's data file may be, newest layout first, relative to
/// the server folder.
pub fn data_paths(level_name: &str, uuid: &str) -> [String; 2] {
    [
        format!("{level_name}/players/data/{uuid}.dat"),
        format!("{level_name}/playerdata/{uuid}.dat"),
    ]
}

/// Where a player's statistics may be, newest layout first.
pub fn stats_paths(level_name: &str, uuid: &str) -> [String; 2] {
    [
        format!("{level_name}/players/stats/{uuid}.json"),
        format!("{level_name}/stats/{uuid}.json"),
    ]
}

/// One stack of items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Inventory slot (0–8 hotbar, 9–35 main), or ender chest slot.
    pub slot: i64,
    /// `minecraft:diamond`.
    pub id: String,
    /// How many.
    pub count: i64,
    /// A name given in an anvil, as plain text.
    pub custom_name: Option<String>,
    /// `(enchantment, level)`.
    pub enchantments: Vec<(String, i64)>,
    /// Durability used.
    pub damage: Option<i64>,
}

/// What a player is wearing and holding in their off hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Equipment {
    /// Helmet.
    pub head: Option<Item>,
    /// Chestplate or elytra.
    pub chest: Option<Item>,
    /// Leggings.
    pub legs: Option<Item>,
    /// Boots.
    pub feet: Option<Item>,
    /// Off hand.
    pub offhand: Option<Item>,
}

/// Everything shown about a player's saved state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerData {
    /// Minecraft's data version of the file.
    pub data_version: Option<i64>,
    /// `[x, y, z]`.
    pub position: Option<[f64; 3]>,
    /// `minecraft:overworld`.
    pub dimension: Option<String>,
    /// Half-hearts, 0–20 normally.
    pub health: Option<f64>,
    /// 0–20.
    pub food: Option<i64>,
    /// Experience level.
    pub xp_level: Option<i64>,
    /// Progress to the next level, 0–1.
    pub xp_progress: Option<f64>,
    /// 0 survival, 1 creative, 2 adventure, 3 spectator.
    pub game_mode: Option<i64>,
    /// Selected hotbar slot.
    pub selected_slot: Option<i64>,
    /// Main inventory and hotbar.
    pub inventory: Vec<Item>,
    /// Armor and off hand.
    pub equipment: Equipment,
    /// Ender chest.
    pub ender_chest: Vec<Item>,
}

/// Maps a parsed `.dat` root onto [`PlayerData`]. Missing fields stay empty
/// rather than failing, so modded or unusual files still show what they can.
pub fn parse(root: &Tag) -> PlayerData {
    let num = |key: &str| root.get(key).and_then(Tag::as_i64);
    let mut data = PlayerData {
        data_version: num("DataVersion"),
        position: root.get("Pos").and_then(Tag::as_list).and_then(|p| {
            let v: Vec<f64> = p.iter().filter_map(Tag::as_f64).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        }),
        dimension: root.get("Dimension").and_then(|d| match d {
            Tag::String(s) => Some(s.clone()),
            // Before 1.16: -1 nether, 0 overworld, 1 end.
            other => other.as_i64().map(|n| match n {
                -1 => "minecraft:the_nether".to_owned(),
                1 => "minecraft:the_end".to_owned(),
                _ => "minecraft:overworld".to_owned(),
            }),
        }),
        health: root.get("Health").and_then(Tag::as_f64),
        food: num("foodLevel"),
        xp_level: num("XpLevel"),
        xp_progress: root.get("XpP").and_then(Tag::as_f64),
        game_mode: num("playerGameType"),
        selected_slot: num("SelectedItemSlot"),
        ..Default::default()
    };

    for tag in root
        .get("Inventory")
        .and_then(Tag::as_list)
        .unwrap_or_default()
    {
        let Some(item) = item(tag) else { continue };
        match item.slot {
            100 => data.equipment.feet = Some(item),
            101 => data.equipment.legs = Some(item),
            102 => data.equipment.chest = Some(item),
            103 => data.equipment.head = Some(item),
            -106 => data.equipment.offhand = Some(item),
            _ => data.inventory.push(item),
        }
    }
    if let Some(equipment) = root.get("equipment") {
        let slot = |key: &str| equipment.get(key).and_then(item);
        let e = &mut data.equipment;
        e.head = slot("head").or(e.head.take());
        e.chest = slot("chest").or(e.chest.take());
        e.legs = slot("legs").or(e.legs.take());
        e.feet = slot("feet").or(e.feet.take());
        e.offhand = slot("offhand").or(e.offhand.take());
    }
    data.ender_chest = root
        .get("EnderItems")
        .and_then(Tag::as_list)
        .unwrap_or_default()
        .iter()
        .filter_map(item)
        .collect();
    data.inventory.sort_by_key(|i| i.slot);
    data.ender_chest.sort_by_key(|i| i.slot);
    data
}

fn item(tag: &Tag) -> Option<Item> {
    let id = tag.get("id")?.as_str()?.to_owned();
    let count = tag
        .get("count")
        .or_else(|| tag.get("Count"))
        .and_then(Tag::as_i64)
        .unwrap_or(1);
    let slot = tag.get("Slot").and_then(Tag::as_i64).unwrap_or(-1);
    let components = tag.get("components");
    let legacy = tag.get("tag");

    let enchantments = components
        .and_then(|c| c.get("minecraft:enchantments"))
        .map(|e| {
            // 1.20.5–1.21.4 wrap levels in `levels`; 1.21.5+ don't.
            let levels = e.get("levels").unwrap_or(e);
            match levels {
                Tag::Compound(entries) => entries
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_i64()?)))
                    .collect(),
                _ => Vec::new(),
            }
        })
        .or_else(|| {
            legacy
                .and_then(|t| t.get("Enchantments"))
                .and_then(Tag::as_list)
                .map(|list| {
                    list.iter()
                        .filter_map(|e| {
                            let id = e.get("id")?.as_str()?.to_owned();
                            Some((id, e.get("lvl")?.as_i64()?))
                        })
                        .collect()
                })
        })
        .unwrap_or_default();

    let custom_name = components
        .and_then(|c| c.get("minecraft:custom_name"))
        .or_else(|| {
            legacy
                .and_then(|t| t.get("display"))
                .and_then(|d| d.get("Name"))
        })
        .and_then(text_of);

    let damage = components
        .and_then(|c| c.get("minecraft:damage"))
        .or_else(|| legacy.and_then(|t| t.get("Damage")))
        .and_then(Tag::as_i64)
        .filter(|d| *d > 0);

    Some(Item {
        slot,
        id,
        count,
        custom_name,
        enchantments,
        damage,
    })
}

/// Plain text from a text component: a JSON string (`'"Bob"'`,
/// `{"text":"Bob"}`) or, since 1.21.5, an NBT compound.
fn text_of(tag: &Tag) -> Option<String> {
    match tag {
        Tag::String(raw) => match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::String(s)) => Some(s),
            Ok(serde_json::Value::Object(map)) => {
                map.get("text").and_then(|t| t.as_str()).map(str::to_owned)
            }
            _ => Some(raw.clone()),
        },
        compound @ Tag::Compound(_) => compound
            .get("text")
            .and_then(Tag::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

/// One statistic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    /// `minecraft:custom`, `minecraft:mined`, …
    pub category: String,
    /// `minecraft:play_time`, `minecraft:stone`, …
    pub key: String,
    /// The count (ticks, centimeters, or times, depending on the stat).
    pub value: i64,
}

/// Reads a stats file. Pre-1.13 files (`"stat.playOneMinute": n`) map to
/// the `legacy` category.
pub fn parse_stats(bytes: &[u8]) -> Vec<Stat> {
    let Ok(serde_json::Value::Object(root)) = serde_json::from_slice::<serde_json::Value>(bytes)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match root.get("stats") {
        Some(serde_json::Value::Object(categories)) => {
            for (category, values) in categories {
                let Some(values) = values.as_object() else {
                    continue;
                };
                for (key, value) in values {
                    if let Some(value) = value.as_i64() {
                        out.push(Stat {
                            category: category.clone(),
                            key: key.clone(),
                            value,
                        });
                    }
                }
            }
        }
        _ => {
            for (key, value) in &root {
                if let Some(value) = value.as_i64() {
                    out.push(Stat {
                        category: "legacy".into(),
                        key: key.clone(),
                        value,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.category.cmp(&b.category).then(b.value.cmp(&a.value)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_formats() {
        let modern = parse_stats(br#"{"stats":{"minecraft:custom":{"minecraft:play_time":504,"minecraft:jump":3}},"DataVersion":4786}"#);
        assert_eq!(
            modern[0].key, "minecraft:play_time",
            "largest first within a category"
        );
        let legacy = parse_stats(br#"{"stat.playOneMinute":1200,"achievement.openInventory":1}"#);
        assert_eq!(legacy.len(), 2);
        assert!(parse_stats(b"nope").is_empty());
    }

    #[test]
    fn names_from_text_components() {
        assert_eq!(
            text_of(&Tag::String("\"Bob\"".into())).as_deref(),
            Some("Bob")
        );
        assert_eq!(
            text_of(&Tag::String(r#"{"text":"Bob"}"#.into())).as_deref(),
            Some("Bob")
        );
        assert_eq!(
            text_of(&Tag::Compound(vec![(
                "text".into(),
                Tag::String("Bob".into())
            )]))
            .as_deref(),
            Some("Bob")
        );
    }

    #[test]
    fn paths() {
        assert_eq!(
            data_paths("world", "u"),
            [
                "world/players/data/u.dat".to_owned(),
                "world/playerdata/u.dat".to_owned()
            ]
        );
    }
}
