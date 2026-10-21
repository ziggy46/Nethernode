//! Working out which DNS records to add, change and remove in one zone so
//! that modules' addresses lead to the tunnel. Nothing here talks to
//! Cloudflare: [`plan`] takes the zone's records and returns changes.
//!
//! Rules:
//!
//! - NetherNode only ever changes or removes records it made, marked by a
//!   comment starting `nethernode:`. A record someone else made where a
//!   module's address should go is a conflict, left alone and reported.
//! - A module's address is a proxied CNAME to the tunnel.
//! - **Names above it keep answering.** Adding `map.mc1.example.com` makes
//!   `mc1.example.com` exist in DNS, so a wildcard (`*.example.com`) stops
//!   answering for it and players can't find the server. When such a name
//!   has no records of its own and a wildcard covered it, it gets a copy of
//!   the wildcard's records (same values, so nothing changes for players),
//!   kept up to date with the wildcard and removed with the last NetherNode
//!   name under it.

use std::collections::{BTreeMap, BTreeSet};

use super::api::Record;

/// Marks a module address NetherNode added.
pub(crate) const SITE: &str = "nethernode:site (a NetherNode module's address)";
/// Marks a copy of a wildcard NetherNode added.
pub(crate) const KEEP: &str = "nethernode:keep (copies the wildcard so this name keeps answering)";

fn is_site(r: &Record) -> bool {
    r.comment
        .as_deref()
        .is_some_and(|c| c.starts_with("nethernode:site"))
}

fn is_keep(r: &Record) -> bool {
    r.comment
        .as_deref()
        .is_some_and(|c| c.starts_with("nethernode:keep"))
}

fn ours(r: &Record) -> bool {
    is_site(r) || is_keep(r)
}

/// One change to a zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    Create(Record),
    Update { id: String, record: Record },
    Delete(Record),
}

impl Change {
    /// For people: what it does.
    pub(crate) fn describe(&self) -> String {
        let what = |r: &Record| {
            let cloud = if r.proxied { "proxied" } else { "DNS only" };
            let why = if is_keep(r) {
                " (keeps it answering like the wildcard)"
            } else {
                ""
            };
            format!("{} {} → {} ({cloud}){why}", r.kind, r.name, r.content)
        };
        match self {
            Change::Create(r) => format!("Add {}", what(r)),
            Change::Update { record, .. } => format!("Update {}", what(record)),
            Change::Delete(r) => format!("Remove {}", what(r)),
        }
    }
}

/// What to do in one zone.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ZonePlan {
    /// In order: copies first (names keep answering throughout), then
    /// addresses, then removals.
    pub changes: Vec<Change>,
    /// Addresses that will lead to the tunnel.
    pub routed: Vec<String>,
    /// Why some addresses can't.
    pub conflicts: Vec<String>,
}

/// The names between `host` and the zone's apex: for `map.mc1.example.com`
/// in `example.com`, `mc1.example.com`.
fn parents<'a>(host: &'a str, zone: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut name = host;
    while let Some((_, parent)) = name.split_once('.') {
        if parent == zone || !parent.ends_with(&format!(".{zone}")) {
            break;
        }
        out.push(parent);
        name = parent;
    }
    out
}

/// The wildcard that answers for `name` (`*.<name's parent>`): its A,
/// AAAA or CNAME records.
fn wildcard_for<'r>(records: &'r [Record], name: &str) -> Vec<&'r Record> {
    let Some((_, parent)) = name.split_once('.') else {
        return Vec::new();
    };
    let wildcard = format!("*.{parent}");
    records
        .iter()
        .filter(|r| r.name == wildcard && matches!(r.kind.as_str(), "A" | "AAAA" | "CNAME"))
        .collect()
}

/// Plans `zone` so that each of `wanted` (lowercase hostnames in it) is a
/// proxied CNAME to `target`.
pub(crate) fn plan(zone: &str, records: &[Record], target: &str, wanted: &[String]) -> ZonePlan {
    let mut out = ZonePlan::default();
    let theirs = |name: &str| records.iter().any(|r| r.name == name && !ours(r));

    // Addresses.
    let mut site_changes = Vec::new();
    let mut routed: BTreeSet<&str> = BTreeSet::new();
    for host in wanted {
        if host == zone || host.starts_with('*') {
            out.conflicts
                .push(format!("{host} can't be a module's address."));
            continue;
        }
        if theirs(host) {
            out.conflicts.push(format!(
                "{host} already has DNS records NetherNode didn't make. Remove them in Cloudflare to let NetherNode manage it."
            ));
            continue;
        }
        routed.insert(host);
        let want = Record {
            id: String::new(),
            kind: "CNAME".into(),
            name: host.clone(),
            content: target.to_owned(),
            proxied: true,
            comment: Some(SITE.into()),
        };
        let mine: Vec<&Record> = records
            .iter()
            .filter(|r| r.name == *host && is_site(r))
            .collect();
        match mine.split_first() {
            None => site_changes.push(Change::Create(want)),
            Some((first, rest)) => {
                if first.kind != want.kind || first.content != want.content || !first.proxied {
                    site_changes.push(Change::Update {
                        id: first.id.clone(),
                        record: want,
                    });
                }
                site_changes.extend(rest.iter().map(|r| Change::Delete((*r).clone())));
            }
        }
    }
    let site_removals: Vec<Change> = records
        .iter()
        .filter(|r| is_site(r) && !routed.contains(r.name.as_str()))
        .map(|r| Change::Delete(r.clone()))
        .collect();

    // Copies that keep the names above the addresses answering.
    let mut copies: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for host in &routed {
        for parent in parents(host, zone) {
            if theirs(parent) {
                // It has records of its own; nothing above it is ours to fix.
                break;
            }
            let wildcard = wildcard_for(records, parent);
            if !wildcard.is_empty() {
                copies.insert(parent, wildcard);
            }
        }
    }
    let mut keep_changes = Vec::new();
    for (name, wildcard) in &copies {
        let mut mine: Vec<&Record> = records
            .iter()
            .filter(|r| r.name == *name && is_keep(r))
            .collect();
        for w in wildcard {
            let want = Record {
                id: String::new(),
                kind: w.kind.clone(),
                name: (*name).to_owned(),
                content: w.content.clone(),
                proxied: w.proxied,
                comment: Some(KEEP.into()),
            };
            match mine.iter().position(|r| r.kind == w.kind) {
                Some(i) => {
                    let r = mine.remove(i);
                    if r.content != want.content || r.proxied != want.proxied {
                        keep_changes.push(Change::Update {
                            id: r.id.clone(),
                            record: want,
                        });
                    }
                }
                None => keep_changes.push(Change::Create(want)),
            }
        }
        // Types the wildcard no longer has.
        keep_changes.extend(mine.into_iter().map(|r| Change::Delete(r.clone())));
    }
    let keep_removals: Vec<Change> = records
        .iter()
        .filter(|r| is_keep(r) && !copies.contains_key(r.name.as_str()))
        .map(|r| Change::Delete(r.clone()))
        .collect();

    out.changes = keep_changes;
    out.changes.extend(site_changes);
    out.changes.extend(site_removals);
    out.changes.extend(keep_removals);
    out.routed = routed.into_iter().map(str::to_owned).collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE: &str = "xtra-pickles.com";
    const TUNNEL: &str = "t1.cfargotunnel.com";

    fn rec(id: &str, kind: &str, name: &str, content: &str, comment: Option<&str>) -> Record {
        Record {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            content: content.into(),
            proxied: false,
            comment: comment.map(str::to_owned),
        }
    }

    fn wildcard() -> Record {
        rec("w", "A", "*.xtra-pickles.com", "203.0.113.5", None)
    }

    fn want(hosts: &[&str]) -> Vec<String> {
        hosts.iter().map(|h| (*h).to_owned()).collect()
    }

    fn summary(p: &ZonePlan) -> Vec<String> {
        p.changes.iter().map(Change::describe).collect()
    }

    #[test]
    fn a_name_covered_by_the_wildcard_gets_a_copy_first() {
        let p = plan(
            ZONE,
            &[wildcard()],
            TUNNEL,
            &want(&["map.mc1.xtra-pickles.com"]),
        );
        assert_eq!(
            summary(&p),
            [
                "Add A mc1.xtra-pickles.com → 203.0.113.5 (DNS only) (keeps it answering like the wildcard)",
                "Add CNAME map.mc1.xtra-pickles.com → t1.cfargotunnel.com (proxied)",
            ]
        );
        assert_eq!(p.routed, ["map.mc1.xtra-pickles.com"]);
    }

    #[test]
    fn an_explicit_name_needs_no_copy() {
        let records = [
            wildcard(),
            rec("m", "A", "mc1.xtra-pickles.com", "203.0.113.5", None),
        ];
        let p = plan(ZONE, &records, TUNNEL, &want(&["map.mc1.xtra-pickles.com"]));
        assert_eq!(
            summary(&p),
            ["Add CNAME map.mc1.xtra-pickles.com → t1.cfargotunnel.com (proxied)"]
        );
    }

    #[test]
    fn without_a_wildcard_nothing_is_copied() {
        let p = plan(ZONE, &[], TUNNEL, &want(&["map.mc1.xtra-pickles.com"]));
        assert_eq!(p.changes.len(), 1, "{:?}", summary(&p));
    }

    #[test]
    fn two_addresses_under_one_name_share_one_copy() {
        let p = plan(
            ZONE,
            &[wildcard()],
            TUNNEL,
            &want(&["map.mc1.xtra-pickles.com", "stats.mc1.xtra-pickles.com"]),
        );
        let copies = p
            .changes
            .iter()
            .filter(|c| matches!(c, Change::Create(r) if r.name == "mc1.xtra-pickles.com"))
            .count();
        assert_eq!(copies, 1);
        assert_eq!(p.changes.len(), 3);
    }

    #[test]
    fn copies_follow_the_wildcard_and_go_with_the_last_address() {
        let copy = Record {
            proxied: false,
            ..rec("k", "A", "mc1.xtra-pickles.com", "203.0.113.5", Some(KEEP))
        };
        let site = Record {
            proxied: true,
            ..rec("s", "CNAME", "map.mc1.xtra-pickles.com", TUNNEL, Some(SITE))
        };
        let other = Record {
            proxied: true,
            ..rec(
                "s2",
                "CNAME",
                "stats.mc1.xtra-pickles.com",
                TUNNEL,
                Some(SITE),
            )
        };
        // Up to date: nothing to do.
        let now = [wildcard(), copy.clone(), site.clone(), other.clone()];
        let both = want(&["map.mc1.xtra-pickles.com", "stats.mc1.xtra-pickles.com"]);
        assert!(plan(ZONE, &now, TUNNEL, &both).changes.is_empty());
        // The home IP changed: the copy follows.
        let moved = Record {
            content: "198.51.100.9".into(),
            ..wildcard()
        };
        let p = plan(
            ZONE,
            &[moved, copy.clone(), site.clone(), other.clone()],
            TUNNEL,
            &both,
        );
        assert_eq!(
            summary(&p),
            [
                "Update A mc1.xtra-pickles.com → 198.51.100.9 (DNS only) (keeps it answering like the wildcard)"
            ]
        );
        // One address goes: the copy stays for the other.
        let p = plan(ZONE, &now, TUNNEL, &want(&["map.mc1.xtra-pickles.com"]));
        assert_eq!(
            summary(&p),
            ["Remove CNAME stats.mc1.xtra-pickles.com → t1.cfargotunnel.com (proxied)"]
        );
        // The last one goes: so does the copy, after it.
        let p = plan(ZONE, &now, TUNNEL, &[]);
        let names: Vec<&str> = p
            .changes
            .iter()
            .map(|c| match c {
                Change::Delete(r) => r.name.as_str(),
                _ => panic!("only removals"),
            })
            .collect();
        assert_eq!(
            names,
            [
                "map.mc1.xtra-pickles.com",
                "stats.mc1.xtra-pickles.com",
                "mc1.xtra-pickles.com"
            ]
        );
    }

    #[test]
    fn records_someone_else_made_are_left_alone() {
        let records = [
            wildcard(),
            rec(
                "x",
                "CNAME",
                "map.mc1.xtra-pickles.com",
                "elsewhere.example",
                None,
            ),
        ];
        let p = plan(ZONE, &records, TUNNEL, &want(&["map.mc1.xtra-pickles.com"]));
        assert!(p.changes.is_empty(), "{:?}", summary(&p));
        assert!(p.routed.is_empty());
        assert!(p.conflicts[0].contains("didn't make"));
        // And never removed, even when nothing wants the name.
        assert!(plan(ZONE, &records, TUNNEL, &[]).changes.is_empty());
    }

    #[test]
    fn a_changed_tunnel_updates_the_address() {
        let old = Record {
            proxied: true,
            ..rec(
                "s",
                "CNAME",
                "map.xtra-pickles.com",
                "old.cfargotunnel.com",
                Some(SITE),
            )
        };
        let p = plan(
            ZONE,
            &[wildcard(), old],
            TUNNEL,
            &want(&["map.xtra-pickles.com"]),
        );
        assert_eq!(
            summary(&p),
            ["Update CNAME map.xtra-pickles.com → t1.cfargotunnel.com (proxied)"]
        );
        assert_eq!(parents("map.xtra-pickles.com", ZONE), Vec::<&str>::new());
        assert_eq!(
            parents("a.b.c.xtra-pickles.com", ZONE),
            ["b.c.xtra-pickles.com", "c.xtra-pickles.com"]
        );
    }
}
