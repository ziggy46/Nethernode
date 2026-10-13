//! Configuration: defaults, then `nethernode.toml`, then `NETHERNODE_*` env vars.
//!
//! Nested keys use a double underscore in env vars, e.g.
//! `NETHERNODE_HTTP__BIND=127.0.0.1:9000` or `NETHERNODE_DATA__DIR=/srv/nn`.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use nn_api::SecureCookies;
use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Web panel and API.
    pub http: HttpConfig,
    /// On-disk state.
    pub data: DataConfig,
    /// Docker connection.
    pub docker: DockerConfig,
    /// Smart Proxy.
    pub proxy: ProxyConfig,
    /// Resumable uploads.
    pub uploads: UploadsConfig,
}

/// `[http]`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// Listen address for the panel.
    pub bind: SocketAddr,
    /// `auto`, `always`, or `never`: when to mark cookies `Secure`.
    pub secure_cookies: SecureCookiesSetting,
    /// Trust `CF-Connecting-IP` / `X-Forwarded-For` for client IPs. Enable only
    /// behind a reverse proxy or Cloudflare Tunnel.
    pub trust_proxy_headers: bool,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            secure_cookies: SecureCookiesSetting::Auto,
            trust_proxy_headers: false,
        }
    }
}

/// Serializable mirror of [`SecureCookies`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SecureCookiesSetting {
    /// Secure when the request arrived over HTTPS.
    #[default]
    Auto,
    /// Always Secure.
    Always,
    /// Never Secure.
    Never,
}

impl From<SecureCookiesSetting> for SecureCookies {
    fn from(value: SecureCookiesSetting) -> Self {
        match value {
            SecureCookiesSetting::Auto => SecureCookies::Auto,
            SecureCookiesSetting::Always => SecureCookies::Always,
            SecureCookiesSetting::Never => SecureCookies::Never,
        }
    }
}

/// `[data]`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DataConfig {
    /// Directory for the database, secrets, and server files.
    pub dir: PathBuf,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("./data"),
        }
    }
}

impl DataConfig {
    /// Path of the SQLite database.
    pub fn database_path(&self) -> PathBuf {
        self.dir.join("nethernode.db")
    }

    /// Path of the at-rest encryption key.
    pub fn secret_key_path(&self) -> PathBuf {
        self.dir.join("secret.key")
    }
}

/// `[docker]`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DockerConfig {
    /// Docker Engine endpoint.
    pub socket: String,
    /// First host port (127.0.0.1) handed to servers for game and RCON.
    pub port_range_start: u16,
    /// Last host port handed to servers.
    pub port_range_end: u16,
    /// Container memory beyond each server's Java heap, in percent.
    pub memory_overhead_percent: u32,
    /// Container memory beyond each server's Java heap, at least, in MiB.
    pub memory_overhead_min_mb: u32,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: "unix:///var/run/docker.sock".into(),
            port_range_start: 30000,
            port_range_end: 30999,
            memory_overhead_percent: 25,
            memory_overhead_min_mb: 1024,
        }
    }
}

/// `[proxy]`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
    /// Whether the Smart Proxy runs.
    pub enabled: bool,
    /// Listener created on first run, when none exist yet. Listeners are
    /// managed in the UI after that.
    pub default_listener: SocketAddr,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_listener: SocketAddr::from(([0, 0, 0, 0], 25565)),
        }
    }
}

/// `[uploads]`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct UploadsConfig {
    /// Largest chunk (one request), in MiB. Keep it well under Cloudflare's
    /// 100 MB request limit; at most 64.
    pub chunk_limit_mb: u32,
    /// Largest file, in GiB.
    pub max_file_gb: u32,
    /// Most data one user may have in unfinished uploads at once (the sum of
    /// their sizes), in GiB.
    pub quota_gb: u32,
    /// Hours an unfinished upload may go without progress before it's
    /// discarded.
    pub expiry_hours: u32,
}

impl Default for UploadsConfig {
    fn default() -> Self {
        Self {
            chunk_limit_mb: 16,
            max_file_gb: 50,
            quota_gb: 100,
            expiry_hours: 24,
        }
    }
}

impl UploadsConfig {
    /// The settings uploads run with, or why these values can't work.
    pub fn settings(&self) -> Result<nn_api::uploads::UploadSettings, String> {
        if !(1..=64).contains(&self.chunk_limit_mb) {
            return Err("uploads.chunk_limit_mb must be between 1 and 64".into());
        }
        if self.max_file_gb == 0 || self.quota_gb == 0 || self.expiry_hours == 0 {
            return Err("uploads.max_file_gb, quota_gb, and expiry_hours must be above 0".into());
        }
        const GIB: u64 = 1024 * 1024 * 1024;
        Ok(nn_api::uploads::UploadSettings {
            chunk_limit: u64::from(self.chunk_limit_mb) * 1024 * 1024,
            max_file: u64::from(self.max_file_gb) * GIB,
            quota: u64::from(self.quota_gb) * GIB,
            expiry: std::time::Duration::from_secs(u64::from(self.expiry_hours) * 3600),
        })
    }
}

impl Config {
    /// Loads configuration. A missing config file is fine; defaults apply.
    pub fn load(path: &Path) -> Result<Self, Box<figment::Error>> {
        Self::figment(path).extract().map_err(Box::new)
    }

    fn figment(path: &Path) -> Figment {
        Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::file(path))
            .merge(Env::prefixed("NETHERNODE_").split("__").ignore(&["config"]))
    }
}

#[cfg(test)]
// `Jail` closures return figment's large error type by design.
#[allow(clippy::result_large_err)]
mod tests {
    use figment::Jail;

    use super::*;

    #[test]
    fn defaults_match_the_build_guide() {
        let config = Config::default();
        assert_eq!(config.http.bind.to_string(), "0.0.0.0:8080");
        assert_eq!(config.data.dir, PathBuf::from("./data"));
        assert_eq!(config.docker.socket, "unix:///var/run/docker.sock");
        assert!(config.proxy.enabled);
        assert_eq!(config.proxy.default_listener.to_string(), "0.0.0.0:25565");
        let uploads = config.uploads.settings().unwrap();
        assert_eq!(uploads.chunk_limit, 16 * 1024 * 1024);
        assert_eq!(uploads.expiry.as_secs(), 24 * 3600);
    }

    #[test]
    fn upload_chunks_stay_under_cloudflare_limits() {
        for chunk_limit_mb in [0, 65, 100] {
            let uploads = UploadsConfig {
                chunk_limit_mb,
                ..UploadsConfig::default()
            };
            assert!(uploads.settings().is_err());
        }
    }

    #[test]
    fn file_then_env_overrides() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "nethernode.toml",
                r#"
                [http]
                bind = "127.0.0.1:9000"
                secure_cookies = "always"

                [proxy]
                enabled = false
                "#,
            )?;
            jail.set_env("NETHERNODE_HTTP__BIND", "127.0.0.1:9100");
            jail.set_env("NETHERNODE_DATA__DIR", "/srv/nn");
            let config = Config::load(Path::new("nethernode.toml")).map_err(|e| *e)?;
            assert_eq!(config.http.bind.to_string(), "127.0.0.1:9100");
            assert_eq!(config.http.secure_cookies, SecureCookiesSetting::Always);
            assert!(!config.proxy.enabled);
            assert_eq!(config.data.dir, PathBuf::from("/srv/nn"));
            Ok(())
        });
    }

    #[test]
    fn missing_file_uses_defaults() {
        Jail::expect_with(|_| {
            assert_eq!(
                Config::load(Path::new("absent.toml")).map_err(|e| *e)?,
                Config::default()
            );
            Ok(())
        });
    }

    #[test]
    fn unknown_keys_are_rejected() {
        Jail::expect_with(|jail| {
            jail.create_file("nethernode.toml", "[http]\nbnd = \"x\"\n")?;
            assert!(Config::load(Path::new("nethernode.toml")).is_err());
            Ok(())
        });
    }
}
