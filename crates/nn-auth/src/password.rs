//! Argon2id password hashing.

use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

/// Minimum accepted password length, in characters.
pub const MIN_PASSWORD_LEN: usize = 10;
/// Maximum accepted password length, in bytes. Bounds hashing cost.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Hashes `password` with Argon2id (OWASP-recommended default parameters)
/// and a random salt, returning a PHC string.
///
/// This is CPU- and memory-intensive; call it from a blocking task.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    Ok(Argon2::default().hash_password(password.as_bytes())?.to_string())
}

/// Checks `password` against a PHC string produced by [`hash_password`].
///
/// Returns `false` for a wrong password or a malformed hash. Parameters are
/// read from the hash, so hashes stay verifiable if defaults change.
pub fn verify_password(password: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

/// Spends the same time as a real verification without matching anything.
///
/// Used when a username doesn't exist, so response timing doesn't reveal
/// which usernames are registered.
pub fn verify_dummy(password: &str) {
    static DUMMY: LazyLock<String> = LazyLock::new(|| {
        hash_password("nethernode-dummy-password").expect("hashing a constant cannot fail")
    });
    let _ = verify_password(password, &DUMMY);
}

/// Checks password length rules. Returns a user-facing message on failure.
pub fn validate_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!("Password must be at least {MIN_PASSWORD_LEN} characters."));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(format!("Password must be at most {MAX_PASSWORD_BYTES} bytes."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_argon2id_and_verify() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong horse battery", &hash));
    }

    #[test]
    fn salts_differ() {
        assert_ne!(hash_password("same").unwrap(), hash_password("same").unwrap());
    }

    #[test]
    fn malformed_hash_does_not_verify() {
        assert!(!verify_password("x", "not-a-phc-string"));
    }

    #[test]
    fn length_rules() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("long enough!").is_ok());
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).is_err());
    }
}
