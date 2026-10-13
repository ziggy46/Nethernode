//! Arbitrary bytes as a zip archive: must never panic or escape.
#![no_main]

mod common;

libfuzzer_sys::fuzz_target!(|data: &[u8]| common::extract_and_check(data));
