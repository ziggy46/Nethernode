//! Modpacks, mods, and versions: the catalog, the CurseForge API key, and
//! project icons.

pub(crate) mod icons;
pub(crate) mod manual;
pub(crate) mod modpacks;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use connectrpc::ConnectError;
use nn_auth::SecretBox;
use nn_catalog::Catalog;
use nn_store::Store;

/// Where the sealed CurseForge API key is kept.
const CURSEFORGE_KEY_SETTING: &str = "curseforge.api_key";

/// Catalog access shared by the RPC services.
pub struct Content {
    /// Versions and Modrinth/CurseForge content.
    pub catalog: Arc<Catalog>,
    /// Seals the CurseForge key at rest; `None` means it can't be saved.
    secrets: Option<Arc<SecretBox>>,
    curseforge_key: RwLock<Option<String>>,
    /// `<data>/cache`, for project icons.
    cache_dir: Option<PathBuf>,
    pub(crate) icon_fetches: tokio::sync::Semaphore,
    /// For icons: never follows redirects, so an allowed host can't send
    /// NetherNode somewhere else.
    pub(crate) icon_http: reqwest::Client,
}

impl Default for Content {
    fn default() -> Self {
        Self {
            catalog: Arc::new(Catalog::default()),
            secrets: None,
            curseforge_key: RwLock::new(None),
            cache_dir: None,
            icon_fetches: tokio::sync::Semaphore::new(4),
            icon_http: reqwest::Client::builder()
                .user_agent(nn_catalog::USER_AGENT)
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }
}

impl Content {
    /// Content caching under `cache_dir` (`<data>/cache`).
    pub fn new(
        catalog: Catalog,
        secrets: Option<Arc<SecretBox>>,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            catalog: Arc::new(catalog),
            secrets,
            cache_dir,
            ..Default::default()
        }
    }

    pub(crate) fn icon_dir(&self) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|d| d.join("icons"))
    }

    /// Where player heads are cached (`<data>/cache/heads`).
    pub(crate) fn heads_dir(&self) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|d| d.join("heads"))
    }

    /// The data key, for sealing other secrets.
    pub(crate) fn secrets(&self) -> Option<&nn_auth::SecretBox> {
        self.secrets.as_deref()
    }

    /// The CurseForge API key, if one is set.
    pub fn curseforge_key(&self) -> Option<String> {
        self.curseforge_key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Reads the saved key at startup. Returns it for the orchestrator.
    pub async fn load_curseforge_key(&self, store: &Store) -> Option<String> {
        let secrets = self.secrets.as_ref()?;
        let sealed: String = store.get_setting(CURSEFORGE_KEY_SETTING).await.ok()??;
        match secrets.open(&sealed) {
            Ok(key) => {
                *self
                    .curseforge_key
                    .write()
                    .unwrap_or_else(|e| e.into_inner()) = Some(key.clone());
                Some(key)
            }
            Err(e) => {
                tracing::warn!(error = %e, "couldn't read the saved CurseForge API key");
                None
            }
        }
    }

    /// Saves (or with `None`, removes) the key.
    pub(crate) async fn save_curseforge_key(
        &self,
        store: &Store,
        key: Option<&str>,
    ) -> Result<(), ConnectError> {
        match key {
            Some(key) => {
                let secrets = self.secrets.as_ref().ok_or_else(|| {
                    ConnectError::unavailable(
                        "the data key isn't available, so secrets can't be saved",
                    )
                })?;
                store
                    .set_setting(CURSEFORGE_KEY_SETTING, &secrets.seal(key))
                    .await
                    .map_err(crate::rpc::store_error)?;
            }
            None => {
                store
                    .delete_setting(CURSEFORGE_KEY_SETTING)
                    .await
                    .map_err(crate::rpc::store_error)?;
            }
        }
        *self
            .curseforge_key
            .write()
            .unwrap_or_else(|e| e.into_inner()) = key.map(str::to_owned);
        Ok(())
    }
}

/// Maps catalog errors to Connect codes.
pub(crate) fn catalog_error(e: nn_catalog::Error) -> ConnectError {
    use nn_catalog::Error;
    match e {
        Error::Upstream(m) => ConnectError::unavailable(m),
        Error::Unsupported(m) => ConnectError::invalid_argument(m),
        Error::NotConfigured(m) => ConnectError::failed_precondition(m),
        Error::NotFound(m) => ConnectError::not_found(m),
    }
}
