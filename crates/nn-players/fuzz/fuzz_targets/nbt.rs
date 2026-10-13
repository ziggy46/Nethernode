//! Player data files come from disk and can be uploaded, so the NBT reader
//! and the player-data mapping must never panic or blow up on any input.
#![no_main]

use nn_players::{nbt, playerdata};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    for parsed in [nbt::read(data), nbt::read_gzip(data)] {
        if let Ok((_, root)) = parsed {
            let _ = playerdata::parse(&root);
        }
    }
    let _ = playerdata::parse_stats(data);
});
