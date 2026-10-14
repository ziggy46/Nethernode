//! Keeps secrets out of logs.
//!
//! Every value sealed or opened with the data key ([`crate::SecretBox`]) is
//! remembered here: RCON passwords, management-protocol secrets, API keys,
//! webhook URLs. [`redact`] replaces those values, and anything that looks
//! like a credential (`password=…`, webhook tokens, session cookies), in
//! text about to be logged.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{LazyLock, RwLock};

use regex::Regex;

/// What replaces a secret.
pub const MASK: &str = "[redacted]";
/// Shorter values aren't remembered: they'd match ordinary words.
const MIN_LEN: usize = 8;
/// A bound on what's remembered.
const MAX_KNOWN: usize = 10_000;

static KNOWN: LazyLock<RwLock<HashSet<String>>> = LazyLock::new(RwLock::default);

static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("ANSI pattern"));

static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        // Discord webhook URLs: the token is the last path segment.
        (
            Regex::new(r"(?i)(discord(?:app)?\.com/api/webhooks/\d+/)[A-Za-z0-9_\-]+")
                .expect("webhook pattern"),
            "${1}[redacted]",
        ),
        // `password=…`, `api_key: …`, `"token":"…"` and the like.
        (
            Regex::new(
                r#"(?i)\b(password|passwd|secret|token|api[_-]?key|authorization|cookie|rcon_password|cf_api_key)("?\s*[=:]\s*"?)[^\s",;&}]+"#,
            )
            .expect("credential pattern"),
            "${1}${2}[redacted]",
        ),
        // Session cookies wherever they appear.
        (
            Regex::new(r"(nn_session=)[^;\s\x22]+").expect("cookie pattern"),
            "${1}[redacted]",
        ),
    ]
});

/// Remembers `secret` so it's masked in logs from now on.
pub fn register(secret: &str) {
    let secret = secret.trim();
    if secret.len() < MIN_LEN {
        return;
    }
    let mut known = KNOWN.write().unwrap_or_else(|e| e.into_inner());
    if known.len() < MAX_KNOWN {
        known.insert(secret.to_owned());
    }
}

/// `text` with known secrets and credential-looking values masked.
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(text);
    {
        let known = KNOWN.read().unwrap_or_else(|e| e.into_inner());
        for secret in known.iter() {
            if out.contains(secret.as_str()) {
                out = Cow::Owned(out.replace(secret.as_str(), MASK));
            }
        }
    }
    for (pattern, replacement) in PATTERNS.iter() {
        if let Cow::Owned(replaced) = pattern.replace_all(&out, *replacement) {
            out = Cow::Owned(replaced);
        }
    }
    out
}

/// A formatted log line, ready to write. Colour codes can sit between a
/// field's name and value, so the check runs on an uncoloured copy; a line
/// with something to hide is written uncoloured, others unchanged.
pub fn redact_log_line(line: &str) -> Cow<'_, str> {
    let plain = ANSI.replace_all(line, "");
    match redact(&plain) {
        Cow::Borrowed(_) => Cow::Borrowed(line),
        Cow::Owned(redacted) => Cow::Owned(redacted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_known_secrets_and_credential_shapes() {
        register("correct-horse-battery-staple");
        register("short");
        let line = "rcon failed for correct-horse-battery-staple; short is fine";
        assert_eq!(redact(line), "rcon failed for [redacted]; short is fine");
        assert_eq!(
            redact("POST https://discord.com/api/webhooks/123456/AbC-dEf_123 failed"),
            "POST https://discord.com/api/webhooks/123456/[redacted] failed"
        );
        assert_eq!(
            redact(r#"password=hunter22 api_key: "k-123" {"token":"abc"} token_hash=keep"#),
            r#"password=[redacted] api_key: "[redacted]" {"token":"[redacted]"} token_hash=keep"#
        );
        assert_eq!(
            redact("cookie: nn_csrf=x; nn_session=s3cr3t"),
            "cookie: [redacted]; nn_session=[redacted]"
        );
        // Log colours between the name, `=`, and the value don't hide it.
        let colored = "\x1b[3mpassword\x1b[0m\x1b[2m=\x1b[0mhunter22 next";
        assert_eq!(redact_log_line(colored), "password=[redacted] next");
        let harmless = "\x1b[32m INFO\x1b[0m started";
        assert_eq!(redact_log_line(harmless), harmless);
        // Nothing to hide: no copy.
        assert!(matches!(redact("server started"), Cow::Borrowed(_)));
    }
}
