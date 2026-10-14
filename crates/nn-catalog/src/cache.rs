//! Upstream answers kept on disk, so the panel stays fast and keeps working
//! (with older data) when an upstream is slow or down.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Error;

/// A folder of cached JSON answers, each fresh for `ttl`.
pub(crate) struct Cache {
    dir: Option<PathBuf>,
    ttl: Duration,
    /// One fetch per key at a time; others wait and read its result.
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Cache {
    pub(crate) fn new(dir: Option<PathBuf>, ttl: Duration) -> Self {
        Self {
            dir,
            ttl,
            inflight: Mutex::default(),
        }
    }

    fn path(&self, key: &str) -> Option<PathBuf> {
        let name: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.as_ref().map(|d| d.join(format!("{name}.json")))
    }

    async fn read<T: DeserializeOwned>(&self, key: &str) -> Option<(T, bool)> {
        let path = self.path(key)?;
        let meta = tokio::fs::metadata(&path).await.ok()?;
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age < self.ttl);
        let bytes = tokio::fs::read(&path).await.ok()?;
        let value = serde_json::from_slice(&bytes).ok()?;
        Some((value, fresh))
    }

    async fn write<T: Serialize>(&self, key: &str, value: &T) {
        let Some(path) = self.path(key) else { return };
        let Ok(json) = serde_json::to_vec(value) else {
            return;
        };
        if let Some(dir) = path.parent()
            && tokio::fs::create_dir_all(dir).await.is_err()
        {
            return;
        }
        let tmp = path.with_extension("json.tmp");
        if tokio::fs::write(&tmp, json).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &path).await;
        }
    }

    /// The cached value for `key` if fresh, else `fetch`'s result (cached).
    /// If fetching fails, an expired value is better than nothing.
    pub(crate) async fn get<T, F, Fut>(&self, key: &str, fetch: F) -> Result<T, Error>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        if let Some((value, true)) = self.read::<T>(key).await {
            return Ok(value);
        }
        let lock = self
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key.to_owned())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        // Someone else may have fetched it while we waited.
        let stale = match self.read::<T>(key).await {
            Some((value, true)) => return Ok(value),
            other => other.map(|(v, _)| v),
        };
        match fetch().await {
            Ok(value) => {
                self.write(key, &value).await;
                Ok(value)
            }
            Err(e) => match stale {
                Some(value) => {
                    tracing::warn!(key, error = %e, "using expired catalog data");
                    Ok(value)
                }
                None => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_values_skip_the_fetch_and_stale_ones_cover_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(Some(dir.path().to_owned()), Duration::from_secs(3600));
        let v: Vec<u32> = cache.get("k/1", || async { Ok(vec![1, 2]) }).await.unwrap();
        assert_eq!(v, [1, 2]);
        let again: Vec<u32> = cache
            .get("k/1", || async { panic!("fresh: no fetch") })
            .await
            .unwrap();
        assert_eq!(again, [1, 2]);

        let expired = Cache::new(Some(dir.path().to_owned()), Duration::ZERO);
        let fallback: Vec<u32> = expired
            .get("k/1", || async { Err(Error::Upstream("down".into())) })
            .await
            .unwrap();
        assert_eq!(fallback, [1, 2], "an expired value beats an error");
        let missing: Result<Vec<u32>, _> = expired
            .get("other", || async { Err(Error::Upstream("down".into())) })
            .await;
        assert!(missing.is_err());
    }
}
