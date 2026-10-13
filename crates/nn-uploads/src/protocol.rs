//! tus 1.0 header parsing.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// The protocol version spoken (`Tus-Resumable`, `Tus-Version`).
pub const TUS_VERSION: &str = "1.0.0";

/// Extensions supported (`Tus-Extension`).
pub const TUS_EXTENSIONS: &str = "creation,termination,checksum,expiration";

/// Checksum algorithms supported (`Tus-Checksum-Algorithm`).
pub const CHECKSUM_ALGORITHMS: &str = "sha256";

/// Content type of a `PATCH` body.
pub const PATCH_CONTENT_TYPE: &str = "application/offset+octet-stream";

/// A malformed tus header. Messages are shown to the uploader.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    /// `Upload-Metadata` isn't `key base64,key base64`.
    #[error("Upload-Metadata is malformed: {0}")]
    Metadata(&'static str),
    /// A checksum algorithm other than SHA-256.
    #[error("only sha256 checksums are supported")]
    UnsupportedAlgorithm,
    /// `Upload-Checksum` isn't `sha256 <base64 of 32 bytes>`.
    #[error("Upload-Checksum is malformed")]
    Checksum,
}

/// Parses `Upload-Metadata`: comma-separated `key base64value` pairs, where
/// the value may be missing. Values must be UTF-8. Keys must be unique.
pub fn parse_metadata(header: &str) -> Result<HashMap<String, Option<String>>, HeaderError> {
    let mut out = HashMap::new();
    for pair in header.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let mut parts = pair.split(' ').filter(|p| !p.is_empty());
        let key = parts.next().ok_or(HeaderError::Metadata("empty pair"))?;
        let value = parts
            .next()
            .map(|v| {
                let bytes = STANDARD
                    .decode(v)
                    .map_err(|_| HeaderError::Metadata("a value isn't base64"))?;
                String::from_utf8(bytes).map_err(|_| HeaderError::Metadata("a value isn't UTF-8"))
            })
            .transpose()?;
        if parts.next().is_some() {
            return Err(HeaderError::Metadata("a pair has extra parts"));
        }
        if !key.chars().all(|c| c.is_ascii_graphic() && c != ',') {
            return Err(HeaderError::Metadata("a key has invalid characters"));
        }
        if out.insert(key.to_owned(), value).is_some() {
            return Err(HeaderError::Metadata("a key appears twice"));
        }
    }
    Ok(out)
}

/// Parses `Upload-Checksum: sha256 <base64>`.
pub fn parse_checksum(header: &str) -> Result<[u8; 32], HeaderError> {
    let (algorithm, value) = header.trim().split_once(' ').ok_or(HeaderError::Checksum)?;
    if !algorithm.eq_ignore_ascii_case("sha256") {
        return Err(HeaderError::UnsupportedAlgorithm);
    }
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|_| HeaderError::Checksum)?;
    bytes.try_into().map_err(|_| HeaderError::Checksum)
}

/// Parses a full-file SHA-256 given as 64 hex digits.
pub fn parse_sha256_hex(value: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(value.trim()).ok()?;
    bytes.try_into().ok()
}

/// Parses a non-negative integer header (`Upload-Length`, `Upload-Offset`).
pub fn parse_u64(value: &str) -> Option<u64> {
    let value = value.trim();
    // Digits only: no sign, no whitespace inside.
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Formats a Unix-millisecond time for `Upload-Expires` (RFC 9110 date).
pub fn http_date(unix_ms: i64) -> String {
    let Ok(ts) = jiff::Timestamp::from_millisecond(unix_ms) else {
        return String::new();
    };
    ts.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_as_tus_js_client_sends_it() {
        // filename "world.zip", filetype "application/zip", a key with no value.
        let parsed =
            parse_metadata("filename d29ybGQuemlw,filetype YXBwbGljYXRpb24vemlw,extract").unwrap();
        assert_eq!(parsed["filename"].as_deref(), Some("world.zip"));
        assert_eq!(parsed["filetype"].as_deref(), Some("application/zip"));
        assert_eq!(parsed["extract"], None);
        assert!(parse_metadata("").unwrap().is_empty());
        // UTF-8 names survive.
        let name = STANDARD.encode("wörld.zip");
        assert_eq!(
            parse_metadata(&format!("filename {name}")).unwrap()["filename"].as_deref(),
            Some("wörld.zip")
        );
    }

    #[test]
    fn bad_metadata_is_refused() {
        assert!(parse_metadata("filename !!!").is_err());
        assert!(parse_metadata("a YQ==,a YQ==").is_err());
        assert!(parse_metadata("a YQ== YQ==").is_err());
        let not_utf8 = STANDARD.encode([0xff, 0xfe]);
        assert!(parse_metadata(&format!("a {not_utf8}")).is_err());
    }

    #[test]
    fn checksums() {
        let digest = [7u8; 32];
        let header = format!("sha256 {}", STANDARD.encode(digest));
        assert_eq!(parse_checksum(&header), Ok(digest));
        assert_eq!(
            parse_checksum("md5 AAAA"),
            Err(HeaderError::UnsupportedAlgorithm)
        );
        assert_eq!(parse_checksum("sha256 AAAA"), Err(HeaderError::Checksum));
        assert_eq!(parse_checksum("sha256"), Err(HeaderError::Checksum));
    }

    #[test]
    fn integers() {
        assert_eq!(parse_u64("0"), Some(0));
        assert_eq!(parse_u64(" 5368709120 "), Some(5_368_709_120));
        assert_eq!(parse_u64("-1"), None);
        assert_eq!(parse_u64("+1"), None);
        assert_eq!(parse_u64("1e3"), None);
        assert_eq!(parse_u64(""), None);
    }

    #[test]
    fn dates() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }
}
