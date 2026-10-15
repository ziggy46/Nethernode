//! Machines that run servers, and the codes agents join with.

use crate::{Result, Store, new_id};

/// The ID of the panel's own machine.
pub const LOCAL_NODE: &str = "local";

/// A machine that runs servers.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Node {
    /// ULID, or [`LOCAL_NODE`].
    pub id: String,
    /// Display name, unique ignoring case.
    pub name: String,
    /// The agent's public key (hex); `None` for this machine and for a node
    /// whose agent hasn't joined yet.
    pub public_key: Option<String>,
    /// The agent's version, as it last said.
    pub version: Option<String>,
    /// What the agent last reported about its machine (JSON).
    pub host_json: Option<String>,
    /// Unix milliseconds the agent was last heard from.
    pub last_seen_at: Option<i64>,
    /// Unix milliseconds.
    pub created_at: i64,
}

impl Node {
    /// Whether this is the panel's own machine.
    pub fn is_local(&self) -> bool {
        self.id == LOCAL_NODE
    }
}

impl Store {
    /// Every node, this machine first, then oldest first.
    pub async fn list_nodes(&self) -> Result<Vec<Node>> {
        Ok(sqlx::query_as!(
            Node,
            r#"SELECT id AS "id!", name, public_key, version, host_json, last_seen_at, created_at
               FROM nodes ORDER BY id <> 'local', created_at, id"#
        )
        .fetch_all(self.pool())
        .await?)
    }

    /// One node.
    pub async fn find_node(&self, id: &str) -> Result<Option<Node>> {
        Ok(sqlx::query_as!(
            Node,
            r#"SELECT id AS "id!", name, public_key, version, host_json, last_seen_at, created_at
               FROM nodes WHERE id = ?1"#,
            id
        )
        .fetch_optional(self.pool())
        .await?)
    }

    /// The node whose agent holds `public_key` (hex).
    pub async fn find_node_by_key(&self, public_key: &str) -> Result<Option<Node>> {
        Ok(sqlx::query_as!(
            Node,
            r#"SELECT id AS "id!", name, public_key, version, host_json, last_seen_at, created_at
               FROM nodes WHERE public_key = ?1"#,
            public_key
        )
        .fetch_optional(self.pool())
        .await?)
    }

    /// Adds a node waiting for its agent. Fails with a unique-constraint
    /// error if the name is taken.
    pub async fn create_node(&self, name: &str) -> Result<Node> {
        let id = new_id();
        let now = crate::now_ms();
        sqlx::query!(
            "INSERT INTO nodes (id, name, created_at) VALUES (?1, ?2, ?3)",
            id,
            name,
            now
        )
        .execute(self.pool())
        .await?;
        Ok(Node {
            id,
            name: name.to_owned(),
            public_key: None,
            version: None,
            host_json: None,
            last_seen_at: None,
            created_at: now,
        })
    }

    /// Renames a node. Returns `false` if it doesn't exist; fails with a
    /// unique-constraint error if the name is taken.
    pub async fn rename_node(&self, id: &str, name: &str) -> Result<bool> {
        let done = sqlx::query!("UPDATE nodes SET name = ?2 WHERE id = ?1", id, name)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Deletes a node (never this machine) and its join codes. Returns
    /// `false` if there was nothing to delete.
    pub async fn delete_node(&self, id: &str) -> Result<bool> {
        let done = sqlx::query!("DELETE FROM nodes WHERE id = ?1 AND id <> 'local'", id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Replaces any join code for `node_id` with a new one.
    pub async fn set_join_code(
        &self,
        node_id: &str,
        code_id: &str,
        secret_hash: &str,
        expires_at: i64,
    ) -> Result<()> {
        let mut tx = self.pool().begin().await?;
        sqlx::query!("DELETE FROM node_join_codes WHERE node_id = ?1", node_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query!(
            "INSERT INTO node_join_codes (id, node_id, secret_hash, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
            code_id,
            node_id,
            secret_hash,
            expires_at
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Spends a join code: if `code_id` is unexpired at `now` and its secret
    /// hashes to `secret_hash`, deletes it and records `public_key` as the
    /// node's agent. Returns the node, or `None` if the code doesn't check
    /// out or the node already has an agent.
    pub async fn claim_join_code(
        &self,
        code_id: &str,
        secret_hash: &str,
        public_key: &str,
        now: i64,
    ) -> Result<Option<Node>> {
        let mut tx = self.pool().begin().await?;
        let node_id = sqlx::query_scalar!(
            "DELETE FROM node_join_codes
             WHERE id = ?1 AND secret_hash = ?2 AND expires_at > ?3
             RETURNING node_id",
            code_id,
            secret_hash,
            now
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(node_id) = node_id else {
            return Ok(None);
        };
        let set = sqlx::query!(
            "UPDATE nodes SET public_key = ?2, last_seen_at = ?3
             WHERE id = ?1 AND public_key IS NULL AND id <> 'local'",
            node_id,
            public_key,
            now
        )
        .execute(&mut *tx)
        .await?;
        if set.rows_affected() == 0 {
            return Ok(None);
        }
        tx.commit().await?;
        self.find_node(&node_id).await
    }

    /// Whether the node has an unexpired join code.
    pub async fn join_code_pending(&self, node_id: &str, now: i64) -> Result<bool> {
        let n = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM node_join_codes WHERE node_id = ?1 AND expires_at > ?2",
            node_id,
            now
        )
        .fetch_one(self.pool())
        .await?;
        Ok(n > 0)
    }

    /// Records that a node's agent was heard from.
    pub async fn node_seen(
        &self,
        id: &str,
        version: &str,
        host_json: Option<&str>,
        at: i64,
    ) -> Result<()> {
        sqlx::query!(
            "UPDATE nodes SET version = ?2, host_json = COALESCE(?3, host_json), last_seen_at = ?4
             WHERE id = ?1",
            id,
            version,
            host_json,
            at
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Deletes expired join codes. Returns how many.
    pub async fn prune_join_codes(&self, now: i64) -> Result<u64> {
        let done = sqlx::query!("DELETE FROM node_join_codes WHERE expires_at <= ?1", now)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_store;

    use super::*;

    #[tokio::test]
    async fn this_machine_is_always_there_and_first() {
        let store = test_store().await;
        let first = store.create_node("Basement PC").await.unwrap();
        let nodes = store.list_nodes().await.unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_local());
        assert_eq!(nodes[1].id, first.id);
        assert!(!store.delete_node(LOCAL_NODE).await.unwrap());
        assert!(
            store
                .create_node("basement pc")
                .await
                .unwrap_err()
                .is_unique_violation()
        );
        assert!(store.rename_node(LOCAL_NODE, "Panel box").await.unwrap());
    }

    #[tokio::test]
    async fn a_join_code_works_once_before_it_expires() {
        let store = test_store().await;
        let node = store.create_node("Attic").await.unwrap();
        store
            .set_join_code(&node.id, "code1", "hash1", 1_000)
            .await
            .unwrap();
        assert!(store.join_code_pending(&node.id, 999).await.unwrap());
        // Wrong secret, then expired: nothing claimed, the code survives
        // the wrong guess.
        assert!(
            store
                .claim_join_code("code1", "nope", "k1", 500)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .claim_join_code("code1", "hash1", "k1", 1_000)
                .await
                .unwrap()
                .is_none()
        );
        store
            .set_join_code(&node.id, "code2", "hash2", 2_000)
            .await
            .unwrap();
        // A new code replaces the old.
        assert!(
            store
                .claim_join_code("code1", "hash1", "k1", 500)
                .await
                .unwrap()
                .is_none()
        );
        let joined = store
            .claim_join_code("code2", "hash2", "k1", 1_500)
            .await
            .unwrap()
            .expect("joined");
        assert_eq!(joined.public_key.as_deref(), Some("k1"));
        assert_eq!(
            store.find_node_by_key("k1").await.unwrap().unwrap().id,
            node.id
        );
        assert!(
            store
                .claim_join_code("code2", "hash2", "k2", 1_500)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!store.join_code_pending(&node.id, 0).await.unwrap());

        store
            .node_seen(&node.id, "0.2.0", Some(r#"{"cpus":2}"#), 3_000)
            .await
            .unwrap();
        store
            .node_seen(&node.id, "0.2.0", None, 4_000)
            .await
            .unwrap();
        let seen = store.find_node(&node.id).await.unwrap().unwrap();
        assert_eq!(seen.host_json.as_deref(), Some(r#"{"cpus":2}"#));
        assert_eq!(seen.last_seen_at, Some(4_000));

        assert!(store.delete_node(&node.id).await.unwrap());
        assert!(store.find_node_by_key("k1").await.unwrap().is_none());
    }
}
