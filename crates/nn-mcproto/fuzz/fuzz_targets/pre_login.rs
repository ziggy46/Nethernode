//! Feeds arbitrary bytes through the same parsing path the proxy uses before
//! login: legacy detection, bounded frames, Handshake, then Login Start or
//! the status exchange. Run: `cargo +nightly fuzz run pre_login` from
//! `crates/nn-mcproto`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use nn_mcproto::{
    Handshake, LegacyDetection, LoginStart, MAX_HANDSHAKE_FRAME, MAX_LOGIN_START_FRAME, MAX_STATUS_FRAME,
    legacy, normalize_hostname, parse_frame, status,
};

fuzz_target!(|data: &[u8]| {
    for complete in [false, true] {
        let _ = legacy::detect(data, complete);
    }
    if !matches!(legacy::detect(data, false), LegacyDetection::Modern) {
        return;
    }

    let Ok(Some(frame)) = parse_frame(data, MAX_HANDSHAKE_FRAME) else { return };
    assert!(frame.total <= data.len());
    assert!(frame.payload.len() <= MAX_HANDSHAKE_FRAME);
    let Ok(handshake) = Handshake::decode(&data[frame.payload.clone()]) else { return };

    // Invariants: encoding round-trips, and normalization is idempotent.
    assert_eq!(Handshake::decode(&handshake.encode()).as_ref(), Ok(&handshake));
    let host = handshake.hostname();
    assert_eq!(normalize_hostname(&host), host);

    let rest = &data[frame.total..];
    if handshake.next_state.is_login() {
        if let Ok(Some(next)) = parse_frame(rest, MAX_LOGIN_START_FRAME) {
            let _ = LoginStart::decode(&rest[next.payload], handshake.protocol_version);
        }
    } else if let Ok(Some(next)) = parse_frame(rest, MAX_STATUS_FRAME) {
        let payload = &rest[next.payload.clone()];
        let _ = status::decode_status_request(payload);
        let _ = status::decode_ping(payload);
    }
});
