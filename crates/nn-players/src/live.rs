//! An online player's data, read from the running server over RCON.
//!
//! On vanilla and Fabric one `data get entity <uuid>` prints everything.
//! Paper cuts long command output short ("{AbsorptionAmount: 0.0f, …..."),
//! so there the data is read a field and an item at a time instead: the
//! stats shown, then each stack (`Inventory[0]`, `Inventory[1]`, …), and
//! for a stack that is itself too long (enchantments, a custom name) the
//! parts of it that are shown. The number of commands is capped; what
//! didn't fit is reported as incomplete rather than left out silently.
//!
//! The pieces are put back together into the same shape as the saved file,
//! so [`crate::playerdata::parse`] reads either.

use std::time::Instant;

use crate::nbt::Tag;
use crate::snbt::{DataGet, data_get};

/// How much one read may cost.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Commands, counting the first `data get entity`.
    pub commands: usize,
    /// No more commands are sent after this.
    pub until: Instant,
}

/// What was read.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveData {
    /// The player's data, as in their saved file.
    pub root: Tag,
    /// False when something couldn't be read: the command limit ran out,
    /// or a value was too long even on its own (dozens of enchantments).
    pub complete: bool,
    /// Commands sent.
    pub commands: usize,
}

/// The stats shown, read one by one.
const FIELDS: [&str; 8] = [
    "Pos",
    "Dimension",
    "Health",
    "foodLevel",
    "XpLevel",
    "XpP",
    "playerGameType",
    "SelectedItemSlot",
];

/// 1.21.5+ keeps armor and the off hand here.
const EQUIPMENT: [&str; 5] = ["head", "chest", "legs", "feet", "offhand"];

/// Stacks read per list at most (an inventory has 41 slots, an ender
/// chest 27).
const MAX_STACKS: usize = 64;

/// The item components shown.
const COMPONENTS: [&str; 3] = [
    "minecraft:enchantments",
    "minecraft:custom_name",
    "minecraft:damage",
];

/// Where commands go: the server's console, over RCON.
pub trait Console: Send {
    /// Sends one command and returns its output, or `None` if it couldn't
    /// be sent.
    fn command(&mut self, command: String) -> impl Future<Output = Option<String>> + Send;
}

/// Reads `uuid`'s data. `None` means the player can't be read live
/// (they're offline, or the output is unknown), so their saved file is
/// what there is.
pub async fn read<C: Console>(uuid: &str, limits: Limits, console: &mut C) -> Option<LiveData> {
    let mut reader = Reader {
        uuid,
        console,
        limits,
        sent: 1,
        complete: true,
        entries: Vec::new(),
    };
    let first = reader
        .console
        .command(format!("data get entity {uuid}"))
        .await?;
    let printed = match data_get(&first) {
        DataGet::Value(root) => {
            return Some(LiveData {
                root,
                complete: true,
                commands: 1,
            });
        }
        DataGet::Truncated(printed) => printed,
        DataGet::Unreadable(e) => {
            tracing::warn!(error = %e, "a running player's data couldn't be read");
            return None;
        }
        DataGet::NoEntity | DataGet::Missing => return None,
    };
    // Fields printed before the cut needn't be asked for again.
    reader.entries = printed
        .into_iter()
        .filter(|(key, _)| FIELDS.contains(&key.as_str()))
        .collect();
    match reader.fill().await {
        Ok(()) => {}
        Err(Stop::Limit) => reader.complete = false,
        Err(Stop::Gone) => return None,
    }
    Some(LiveData {
        root: Tag::Compound(reader.entries),
        complete: reader.complete,
        commands: reader.sent,
    })
}

/// Why reading stopped early.
enum Stop {
    /// The command limit ran out: keep what was read.
    Limit,
    /// The player left, or RCON stopped answering: use the saved file.
    Gone,
}

/// One path's value.
enum Step {
    Value(Tag),
    /// Cut short; the compound entries printed whole.
    Truncated(Vec<(String, Tag)>),
    Absent,
}

fn has(entries: &[(String, Tag)], key: &str) -> bool {
    entries.iter().any(|(k, _)| k == key)
}

struct Reader<'a, C> {
    uuid: &'a str,
    console: &'a mut C,
    limits: Limits,
    sent: usize,
    complete: bool,
    entries: Vec<(String, Tag)>,
}

impl<C: Console> Reader<'_, C> {
    async fn get(&mut self, path: &str) -> Result<Step, Stop> {
        if self.sent >= self.limits.commands || Instant::now() >= self.limits.until {
            return Err(Stop::Limit);
        }
        self.sent += 1;
        let output = self
            .console
            .command(format!("data get entity {} {path}", self.uuid))
            .await
            .ok_or(Stop::Gone)?;
        Ok(match data_get(&output) {
            DataGet::Value(tag) => Step::Value(tag),
            DataGet::Truncated(printed) => Step::Truncated(printed),
            DataGet::Unreadable(e) => {
                tracing::debug!(path, error = %e, "part of a running player's data couldn't be read");
                self.complete = false;
                Step::Absent
            }
            DataGet::NoEntity => return Err(Stop::Gone),
            DataGet::Missing => Step::Absent,
        })
    }

    /// A value that should fit on its own; one that doesn't is skipped and
    /// makes the read incomplete.
    async fn value(&mut self, path: &str) -> Result<Option<Tag>, Stop> {
        match self.get(path).await? {
            Step::Value(tag) => Ok(Some(tag)),
            Step::Truncated(_) => {
                self.complete = false;
                Ok(None)
            }
            Step::Absent => Ok(None),
        }
    }

    async fn fill(&mut self) -> Result<(), Stop> {
        for key in FIELDS {
            if has(&self.entries, key) {
                continue;
            }
            if let Some(tag) = self.value(key).await? {
                self.entries.push((key.to_owned(), tag));
            }
        }
        match self.get("equipment").await? {
            Step::Value(tag) => self.entries.push(("equipment".to_owned(), tag)),
            Step::Truncated(mut slots) => {
                let walked = self.equipment(&mut slots).await;
                self.entries
                    .push(("equipment".to_owned(), Tag::Compound(slots)));
                walked?;
            }
            Step::Absent => {}
        }
        // Before 1.21.5 armor and the off hand are inventory slots, so the
        // inventory comes before the ender chest.
        for list in ["Inventory", "EnderItems"] {
            let mut stacks = Vec::new();
            let walked = self.stacks(list, &mut stacks).await;
            self.entries.push((list.to_owned(), Tag::List(stacks)));
            walked?;
        }
        Ok(())
    }

    async fn equipment(&mut self, slots: &mut Vec<(String, Tag)>) -> Result<(), Stop> {
        for slot in EQUIPMENT {
            if has(slots, slot) {
                continue;
            }
            if let Some(item) = self.item(&format!("equipment.{slot}")).await? {
                slots.push((slot.to_owned(), item));
            }
        }
        Ok(())
    }

    /// Walks a list by index until "Found no elements matching": the list
    /// holds only the slots in use, each with its `Slot`.
    async fn stacks(&mut self, list: &str, stacks: &mut Vec<Tag>) -> Result<(), Stop> {
        for i in 0..MAX_STACKS {
            match self.item(&format!("{list}[{i}]")).await? {
                Some(item) => stacks.push(item),
                None => return Ok(()),
            }
        }
        Ok(())
    }

    async fn item(&mut self, path: &str) -> Result<Option<Tag>, Stop> {
        match self.get(path).await? {
            Step::Value(tag) => Ok(Some(tag)),
            Step::Absent => Ok(None),
            Step::Truncated(printed) => self.item_parts(path, printed).await.map(Some),
        }
    }

    /// A stack too long to print whole: what was printed, and the rest
    /// part by part.
    async fn item_parts(&mut self, path: &str, mut parts: Vec<(String, Tag)>) -> Result<Tag, Stop> {
        for key in ["Slot", "id"] {
            if has(&parts, key) {
                continue;
            }
            if let Some(tag) = self.value(&format!("{path}.{key}")).await? {
                parts.push((key.to_owned(), tag));
            }
        }
        // 1.20.5+ stacks have `count` and `components`; older ones `Count`
        // and `tag`.
        let modern = if has(&parts, "count") {
            true
        } else if has(&parts, "Count") {
            false
        } else if let Some(count) = self.value(&format!("{path}.count")).await? {
            parts.push(("count".to_owned(), count));
            true
        } else if let Some(count) = self.value(&format!("{path}.Count")).await? {
            parts.push(("Count".to_owned(), count));
            false
        } else {
            return Ok(Tag::Compound(parts));
        };
        if modern {
            if !has(&parts, "components")
                && let Some(components) = self.components(path).await?
            {
                parts.push(("components".to_owned(), components));
            }
        } else if !has(&parts, "tag") {
            let mut tag = Vec::new();
            for key in ["Enchantments", "Damage"] {
                if let Some(value) = self.value(&format!("{path}.tag.{key}")).await? {
                    tag.push((key.to_owned(), value));
                }
            }
            if let Some(name) = self.value(&format!("{path}.tag.display.Name")).await? {
                tag.push((
                    "display".to_owned(),
                    Tag::Compound(vec![("Name".to_owned(), name)]),
                ));
            }
            parts.push(("tag".to_owned(), Tag::Compound(tag)));
        }
        Ok(Tag::Compound(parts))
    }

    /// A stack's components: whole if they fit, otherwise the ones shown.
    async fn components(&mut self, item: &str) -> Result<Option<Tag>, Stop> {
        let path = format!("{item}.components");
        let mut components = match self.get(&path).await? {
            Step::Value(tag) => return Ok(Some(tag)),
            Step::Absent => return Ok(None),
            Step::Truncated(printed) => printed,
        };
        for key in COMPONENTS {
            if has(&components, key) {
                continue;
            }
            let at = format!("{path}.\"{key}\"");
            match self.get(&at).await? {
                Step::Value(tag) => components.push((key.to_owned(), tag)),
                Step::Absent => {}
                Step::Truncated(printed) if key == "minecraft:enchantments" => {
                    let enchantments = self.enchantments(&at, printed).await?;
                    components.push((key.to_owned(), enchantments));
                }
                Step::Truncated(_) => self.complete = false,
            }
        }
        Ok(Some(Tag::Compound(components)))
    }

    /// Enchantments too many to print with the rest. 1.20.5–1.21.4 keep
    /// them under `levels`, which fits more on its own; 1.21.5+ print them
    /// directly, so only the ones before the cut are known.
    async fn enchantments(&mut self, path: &str, printed: Vec<(String, Tag)>) -> Result<Tag, Stop> {
        if has(&printed, "levels") {
            return Ok(Tag::Compound(printed));
        }
        let levels = match self.get(&format!("{path}.levels")).await? {
            Step::Value(levels) => levels,
            Step::Truncated(some) => {
                self.complete = false;
                Tag::Compound(some)
            }
            Step::Absent => {
                self.complete = false;
                return Ok(Tag::Compound(printed));
            }
        };
        Ok(Tag::Compound(vec![("levels".to_owned(), levels)]))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::*;
    use crate::playerdata;

    const UUID: &str = "5627dd98-e6be-3c21-b8a8-e92344183641";

    /// A server answering from a table of `data get entity` outputs, the
    /// way Paper 1.21.4 does: missing paths are "Found no elements".
    struct Server {
        answers: HashMap<String, String>,
        sent: Vec<String>,
    }

    impl Server {
        fn new(answers: &[(&str, &str)]) -> Self {
            Self {
                answers: answers
                    .iter()
                    .map(|(path, value)| {
                        let command = if path.is_empty() {
                            format!("data get entity {UUID}")
                        } else {
                            format!("data get entity {UUID} {path}")
                        };
                        let output = if value.starts_with("No entity") {
                            (*value).to_owned()
                        } else {
                            format!("Steve has the following entity data: {value}")
                        };
                        (command, output)
                    })
                    .collect(),
                sent: Vec::new(),
            }
        }

        async fn read(&mut self, commands: usize) -> Option<LiveData> {
            let limits = Limits {
                commands,
                until: Instant::now() + Duration::from_secs(60),
            };
            read(UUID, limits, self).await
        }
    }

    impl Console for Server {
        async fn command(&mut self, command: String) -> Option<String> {
            self.sent.push(command.clone());
            let path = command
                .strip_prefix(&format!("data get entity {UUID} "))
                .unwrap_or("");
            Some(
                self.answers
                    .get(&command)
                    .cloned()
                    .unwrap_or_else(|| format!("Found no elements matching {path}")),
            )
        }
    }

    // Real Paper 1.21.4 output: it prints the first few tags of a long
    // value and then "...", wherever that falls.
    const ROOT: &str = "{Bukkit.updateLevel: 2, foodTickTimer: 0, AbsorptionAmount: 0.0f, XpTotal: 0, playerGameType: 1, SelectedItemSlot: 0, Invulnerable: 0b, ...";

    fn paper_steve() -> Server {
        Server::new(&[
            ("", ROOT),
            ("Pos", "[5.437960069226508d, 76.0d, 5.078934157015606d]"),
            ("Dimension", "\"minecraft:overworld\""),
            ("Health", "19.333332f"),
            ("foodLevel", "17"),
            ("XpLevel", "5"),
            ("XpP", "0.25f"),
            (
                "Inventory[0]",
                "{count: 1, Slot: 0b, components: {\"minecraft:custom_name\": '\"Excalibur\"', \"minecraft:enchantments\": {levels: {\"minecraft:mending...",
            ),
            ("Inventory[0].id", "\"minecraft:diamond_sword\""),
            (
                "Inventory[0].components",
                "{\"minecraft:custom_name\": '\"Excalibur\"', \"minecraft:enchantments\": {levels: {\"minecraft:mending\": 1, \"minecraft:fire_aspect\": 2, \"minecraft:looting...",
            ),
            (
                "Inventory[0].components.\"minecraft:enchantments\"",
                "{levels: {\"minecraft:mending\": 1, \"minecraft:fire_aspect\": 2, \"minecraft:looting\": 3, \"minecraft:sharpness\": 5, \"minecraft:unbreaking\"...",
            ),
            (
                "Inventory[0].components.\"minecraft:enchantments\".levels",
                "{\"minecraft:mending\": 1, \"minecraft:fire_aspect\": 2, \"minecraft:looting\": 3, \"minecraft:sharpness\": 5, \"minecraft:unbreaking\": 3}",
            ),
            ("Inventory[0].components.\"minecraft:damage\"", "12"),
            (
                "Inventory[1]",
                "{count: 64, Slot: 1b, id: \"minecraft:oak_planks\"}",
            ),
            (
                "Inventory[2]",
                "{count: 1, Slot: 3b, components: {\"minecraft:enchantments\": {levels: {\"minecraft:power\": 5}}}, id...",
            ),
            ("Inventory[2].id", "\"minecraft:bow\""),
            (
                "Inventory[3]",
                "{count: 1, Slot: 103b, id: \"minecraft:iron_helmet\"}",
            ),
            (
                "EnderItems[0]",
                "{count: 16, Slot: 3b, id: \"minecraft:ender_pearl\"}",
            ),
        ])
    }

    #[tokio::test]
    async fn vanilla_needs_one_command() {
        let mut server = Server::new(&[(
            "",
            "{Health: 20.0f, Inventory: [{Slot: 0b, count: 1, id: \"minecraft:stone\"}]}",
        )]);
        let live = server.read(200).await.unwrap();
        assert!(live.complete);
        assert_eq!(live.commands, 1);
        assert_eq!(server.sent.len(), 1);
        assert_eq!(
            playerdata::parse(&live.root).inventory[0].id,
            "minecraft:stone"
        );
    }

    #[tokio::test]
    async fn cut_short_output_is_read_piece_by_piece() {
        let mut server = paper_steve();
        let live = server.read(200).await.unwrap();
        assert!(live.complete, "everything fit: {:?}", server.sent);
        assert_eq!(live.commands, server.sent.len());
        let data = playerdata::parse(&live.root);
        assert_eq!(data.health, Some(19.333332f32.into()));
        assert_eq!(data.food, Some(17));
        assert_eq!(data.xp_level, Some(5));
        assert_eq!(data.game_mode, Some(1));
        assert_eq!(data.selected_slot, Some(0));
        assert_eq!(data.dimension.as_deref(), Some("minecraft:overworld"));
        assert_eq!(data.inventory.len(), 3);
        let sword = &data.inventory[0];
        assert_eq!(
            (sword.slot, sword.id.as_str(), sword.count),
            (0, "minecraft:diamond_sword", 1)
        );
        assert_eq!(sword.custom_name.as_deref(), Some("Excalibur"));
        assert_eq!(sword.enchantments.len(), 5);
        assert!(
            sword
                .enchantments
                .contains(&("minecraft:sharpness".to_owned(), 5))
        );
        assert_eq!(sword.damage, Some(12));
        assert_eq!(data.inventory[1].count, 64);
        let bow = &data.inventory[2];
        assert_eq!((bow.slot, bow.id.as_str()), (3, "minecraft:bow"));
        assert_eq!(bow.enchantments, [("minecraft:power".to_owned(), 5)]);
        assert_eq!(
            data.equipment.head.as_ref().map(|i| i.id.as_str()),
            Some("minecraft:iron_helmet"),
            "armor is an inventory slot before 1.21.5"
        );
        assert_eq!(data.ender_chest[0].count, 16);
        // What was printed before a cut isn't asked for again: the game
        // mode and slot from the first read, the bow's components.
        let asked = |path: &str| server.sent.iter().any(|c| c.ends_with(&format!(" {path}")));
        assert!(!asked("playerGameType") && !asked("SelectedItemSlot"));
        assert!(!asked("Inventory[2].components") && !asked("Inventory[2].Slot"));
        assert!(!server.sent.iter().any(|c| c.contains("Inventory[1].")));
    }

    #[tokio::test]
    async fn the_command_limit_keeps_what_was_read() {
        let mut server = paper_steve();
        let live = server.read(12).await.unwrap();
        assert!(!live.complete);
        assert_eq!(server.sent.len(), 12);
        let data = playerdata::parse(&live.root);
        assert_eq!(data.health, Some(19.333332f32.into()));
        assert!(data.ender_chest.is_empty());
    }

    #[tokio::test]
    async fn a_value_too_long_on_its_own_is_incomplete() {
        let mut server = paper_steve();
        server.answers.insert(
            format!("data get entity {UUID} Inventory[0].components.\"minecraft:enchantments\".levels"),
            "Steve has the following entity data: {\"minecraft:mending\": 1, \"minecraft:fire_aspect\": 2, \"minecraft:looting\": 3, \"minecraft:sharpness\": 5, \"minecraft:unbreaking\": 3, \"minecraft:knockback\"...".to_owned(),
        );
        let live = server.read(200).await.unwrap();
        assert!(!live.complete);
        let data = playerdata::parse(&live.root);
        assert_eq!(data.inventory[0].id, "minecraft:diamond_sword");
        assert_eq!(
            data.inventory[0].enchantments.len(),
            5,
            "the ones printed are kept"
        );
    }

    #[tokio::test]
    async fn a_player_who_leaves_midway_falls_back_to_the_file() {
        let mut server = paper_steve();
        server.answers.insert(
            format!("data get entity {UUID} XpLevel"),
            "No entity was found".to_owned(),
        );
        assert_eq!(server.read(200).await, None);
        let mut offline = Server::new(&[("", "No entity was found")]);
        assert_eq!(offline.read(200).await, None);
    }

    #[tokio::test]
    async fn newer_equipment_too_long_to_print_is_read_by_slot() {
        let mut server = paper_steve();
        server.answers.extend(
            [
                ("equipment", "{chest: {components: {\"minecraft:enchantments\": {\"minecraft:protection\": 4}}, cou..."),
                ("equipment.head", "{count: 1, id: \"minecraft:turtle_helmet\"}"),
                ("equipment.chest", "{components: {\"minecraft:enchantments\": {\"minecraft:protection\": 4}}, cou..."),
                ("equipment.chest.id", "\"minecraft:netherite_chestplate\""),
                ("equipment.chest.count", "1"),
                ("equipment.chest.components.\"minecraft:enchantments\"", "{\"minecraft:protection\": 4}"),
            ]
            .map(|(path, value)| {
                (
                    format!("data get entity {UUID} {path}"),
                    format!("Steve has the following entity data: {value}"),
                )
            }),
        );
        let live = server.read(200).await.unwrap();
        assert!(live.complete);
        let e = playerdata::parse(&live.root).equipment;
        assert_eq!(e.head.unwrap().id, "minecraft:turtle_helmet");
        let chest = e.chest.unwrap();
        assert_eq!(chest.id, "minecraft:netherite_chestplate");
        assert_eq!(chest.enchantments, [("minecraft:protection".to_owned(), 4)]);
    }
}
