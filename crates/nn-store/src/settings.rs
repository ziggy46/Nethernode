//! Key/value settings stored as JSON.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{Result, Store};

impl Store {
    /// Reads a setting, returning `None` if it has never been set.
    pub async fn get_setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw = sqlx::query_scalar!("SELECT value_json FROM settings WHERE key = ?1", key)
            .fetch_optional(self.pool())
            .await?;
        raw.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    /// Writes a setting, replacing any previous value.
    pub async fn set_setting<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        sqlx::query!(
            "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
            key,
            json,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Removes a setting. Returns `false` if it wasn't set.
    pub async fn delete_setting(&self, key: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM settings WHERE key = ?1", key)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_store;

    #[tokio::test]
    async fn settings_round_trip() {
        let store = test_store().await;
        assert_eq!(store.get_setting::<u32>("answer").await.unwrap(), None);
        store.set_setting("answer", &41u32).await.unwrap();
        store.set_setting("answer", &42u32).await.unwrap();
        assert_eq!(store.get_setting::<u32>("answer").await.unwrap(), Some(42));
        assert!(store.delete_setting("answer").await.unwrap());
        assert!(!store.delete_setting("answer").await.unwrap());
    }
}
