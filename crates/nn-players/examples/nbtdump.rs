//! Prints a gzip NBT file as an indented tree: `cargo run --example nbtdump -- <file.dat>`.

use nn_players::nbt::{self, Tag};

fn print(tag: &Tag, indent: usize) {
    let pad = "  ".repeat(indent);
    match tag {
        Tag::Compound(entries) => {
            for (k, v) in entries {
                match v {
                    Tag::Compound(_) | Tag::List(_) => {
                        println!("{pad}{k}:");
                        print(v, indent + 1);
                    }
                    other => println!("{pad}{k}: {}", short(other)),
                }
            }
        }
        Tag::List(items) => {
            for (i, v) in items.iter().enumerate().take(12) {
                println!("{pad}[{i}]");
                print(v, indent + 1);
            }
        }
        other => println!("{pad}{}", short(other)),
    }
}

fn short(tag: &Tag) -> String {
    match tag {
        Tag::ByteArray(v) => format!("bytes[{}]", v.len()),
        Tag::IntArray(v) => format!("ints{v:?}"),
        Tag::LongArray(v) => format!("longs[{}]", v.len()),
        other => format!("{other:?}"),
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: nbtdump <file>");
    let bytes = std::fs::read(path).expect("read");
    let (_, root) = nbt::read_gzip(&bytes).expect("parse");
    print(&root, 0);
}
