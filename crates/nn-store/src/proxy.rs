//! Smart Proxy listeners and hostname routes.

use crate::{Result, Store, new_id};

/// A TCP listener the Smart Proxy accepts players on.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProxyListener {
    /// ULID.
    pub id: String,
    /// Bind address, e.g. `0.0.0.0` or `::`.
    pub bind_addr: String,
    /// TCP port.
    pub port: i64,
    /// Whether the listener is active.
    pub enabled: bool,
    /// Send a PROXY protocol v2 header to backends.
    pub proxy_protocol: bool,
}

/// Fields for creating or updating a listener.
#[derive(Debug, Clone)]
pub struct ListenerInput<'a> {
    /// Bind address.
    pub bind_addr: &'a str,
    /// TCP port.
    pub port: i64,
    /// Whether the listener is active.
    pub enabled: bool,
    /// Send PROXY protocol v2 to backends.
    pub proxy_protocol: bool,
}

/// A hostname route, joined with its destination server.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Route {
    /// ULID.
    pub id: String,
    /// Destination server (NULL for slug-based wildcards).
    pub server_id: Option<String>,
    /// Hostname pattern: exact, `*.suffix`, or `*`.
    pub hostname: String,
    /// Listener this route is limited to (NULL = all).
    pub listener_id: Option<String>,
    /// Tie-breaker among routes of the same kind; higher wins.
    pub priority: i64,
    /// Destination server's name.
    pub server_name: Option<String>,
    /// Destination server's slug.
    pub server_slug: Option<String>,
}

/// Fields for creating or updating a route.
#[derive(Debug, Clone)]
pub struct RouteInput<'a> {
    /// Destination server.
    pub server_id: Option<&'a str>,
    /// Hostname pattern (already normalized).
    pub hostname: &'a str,
    /// Listener restriction.
    pub listener_id: Option<&'a str>,
    /// Priority.
    pub priority: i64,
}

impl Store {
    /// All listeners, by port.
    pub async fn list_listeners(&self) -> Result<Vec<ProxyListener>> {
        let rows = sqlx::query_as!(
            ProxyListener,
            r#"SELECT id AS "id!", bind_addr, port, enabled AS "enabled: bool",
                      proxy_protocol AS "proxy_protocol: bool"
               FROM proxy_listeners ORDER BY port, bind_addr"#,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Creates a listener, or updates the one with `id`. Returns its ID, or
    /// `None` if `id` was given but doesn't exist.
    pub async fn upsert_listener(
        &self,
        id: Option<&str>,
        input: ListenerInput<'_>,
    ) -> Result<Option<String>> {
        match id {
            Some(id) => {
                let result = sqlx::query!(
                    r#"UPDATE proxy_listeners SET bind_addr = ?1, port = ?2, enabled = ?3, proxy_protocol = ?4
                       WHERE id = ?5"#,
                    input.bind_addr,
                    input.port,
                    input.enabled,
                    input.proxy_protocol,
                    id,
                )
                .execute(self.pool())
                .await?;
                Ok((result.rows_affected() > 0).then(|| id.to_owned()))
            }
            None => {
                let id = new_id();
                sqlx::query!(
                    r#"INSERT INTO proxy_listeners (id, bind_addr, port, enabled, proxy_protocol)
                       VALUES (?1, ?2, ?3, ?4, ?5)"#,
                    id,
                    input.bind_addr,
                    input.port,
                    input.enabled,
                    input.proxy_protocol,
                )
                .execute(self.pool())
                .await?;
                Ok(Some(id))
            }
        }
    }

    /// Deletes a listener (and routes limited to it). Returns `false` if missing.
    pub async fn delete_listener(&self, id: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM proxy_listeners WHERE id = ?1", id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Gives a server a port of its own: a listener on `bind_addr:port`
    /// made for it, with a `*` route sending everyone on it to the server.
    /// Both are made or neither is. Returns the listener's and route's IDs.
    pub async fn add_server_port(
        &self,
        server_id: &str,
        bind_addr: &str,
        port: i64,
    ) -> Result<(String, String)> {
        let (listener, route) = (new_id(), new_id());
        let mut tx = self.pool().begin().await?;
        sqlx::query!(
            r#"INSERT INTO proxy_listeners (id, bind_addr, port, enabled, proxy_protocol, made_for_server)
               VALUES (?1, ?2, ?3, 1, 0, ?4)"#,
            listener,
            bind_addr,
            port,
            server_id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            r#"INSERT INTO routes (id, server_id, hostname, listener_id, priority)
               VALUES (?1, ?2, '*', ?3, 0)"#,
            route,
            server_id,
            listener,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((listener, route))
    }

    /// Removes listeners made for `server_id` that no route uses any more
    /// (once the server is deleted, its routes are gone). Returns how many.
    pub async fn remove_unused_server_ports(&self, server_id: &str) -> Result<u64> {
        let result = sqlx::query!(
            r#"DELETE FROM proxy_listeners
               WHERE made_for_server = ?1
                 AND NOT EXISTS (SELECT 1 FROM routes WHERE routes.listener_id = proxy_listeners.id)"#,
            server_id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected())
    }

    /// Creates the default listener if no listeners exist. Returns whether one was created.
    pub async fn ensure_default_listener(&self, bind_addr: &str, port: i64) -> Result<bool> {
        let id = new_id();
        let result = sqlx::query!(
            r#"INSERT INTO proxy_listeners (id, bind_addr, port, enabled, proxy_protocol)
               SELECT ?1, ?2, ?3, 1, 0 WHERE NOT EXISTS (SELECT 1 FROM proxy_listeners)"#,
            id,
            bind_addr,
            port,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// All routes with their destination server's name and slug.
    pub async fn list_routes(&self) -> Result<Vec<Route>> {
        let rows = sqlx::query_as!(
            Route,
            r#"SELECT r.id AS "id!", r.server_id, r.hostname, r.listener_id, r.priority,
                      s.name AS "server_name?", s.slug AS "server_slug?"
               FROM routes r LEFT JOIN servers s ON s.id = r.server_id
               ORDER BY r.hostname"#,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Creates a route, or updates the one with `id`. Returns its ID, or
    /// `None` if `id` was given but doesn't exist.
    pub async fn upsert_route(
        &self,
        id: Option<&str>,
        input: RouteInput<'_>,
    ) -> Result<Option<String>> {
        match id {
            Some(id) => {
                let result = sqlx::query!(
                    r#"UPDATE routes SET server_id = ?1, hostname = ?2, listener_id = ?3, priority = ?4
                       WHERE id = ?5"#,
                    input.server_id,
                    input.hostname,
                    input.listener_id,
                    input.priority,
                    id,
                )
                .execute(self.pool())
                .await?;
                Ok((result.rows_affected() > 0).then(|| id.to_owned()))
            }
            None => {
                let id = new_id();
                sqlx::query!(
                    r#"INSERT INTO routes (id, server_id, hostname, listener_id, priority)
                       VALUES (?1, ?2, ?3, ?4, ?5)"#,
                    id,
                    input.server_id,
                    input.hostname,
                    input.listener_id,
                    input.priority,
                )
                .execute(self.pool())
                .await?;
                Ok(Some(id))
            }
        }
    }

    /// Deletes a route. Returns `false` if missing.
    pub async fn delete_route(&self, id: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM routes WHERE id = ?1", id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servers::test_server;
    use crate::test_store;

    fn listener(port: i64) -> ListenerInput<'static> {
        ListenerInput {
            bind_addr: "0.0.0.0",
            port,
            enabled: true,
            proxy_protocol: false,
        }
    }

    #[tokio::test]
    async fn default_listener_only_when_empty() {
        let store = test_store().await;
        assert!(
            store
                .ensure_default_listener("0.0.0.0", 25565)
                .await
                .unwrap()
        );
        assert!(
            !store
                .ensure_default_listener("0.0.0.0", 25565)
                .await
                .unwrap()
        );
        let listeners = store.list_listeners().await.unwrap();
        assert_eq!(listeners.len(), 1);
        assert!(listeners[0].enabled && !listeners[0].proxy_protocol);
    }

    #[tokio::test]
    async fn listener_crud() {
        let store = test_store().await;
        let id = store
            .upsert_listener(None, listener(25565))
            .await
            .unwrap()
            .unwrap();
        let updated = ListenerInput {
            proxy_protocol: true,
            ..listener(25566)
        };
        assert_eq!(
            store
                .upsert_listener(Some(&id), updated)
                .await
                .unwrap()
                .as_deref(),
            Some(id.as_str())
        );
        assert_eq!(store.list_listeners().await.unwrap()[0].port, 25566);
        assert!(
            store
                .upsert_listener(Some("missing"), listener(1))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store.upsert_listener(None, listener(25566)).await.is_err(),
            "duplicate bind"
        );
        assert!(store.delete_listener(&id).await.unwrap());
    }

    #[tokio::test]
    async fn servers_get_ports_of_their_own() {
        let store = test_store().await;
        let a = store.insert_server(test_server("a", 30001)).await.unwrap();
        let b = store.insert_server(test_server("b", 30002)).await.unwrap();
        // A `*` route for all listeners already exists.
        store
            .upsert_route(
                None,
                RouteInput {
                    server_id: Some(&a.id),
                    hostname: "*",
                    listener_id: None,
                    priority: 0,
                },
            )
            .await
            .unwrap();
        let (port_a, _) = store
            .add_server_port(&a.id, "0.0.0.0", 25566)
            .await
            .unwrap();
        let (port_b, _) = store
            .add_server_port(&b.id, "0.0.0.0", 25567)
            .await
            .unwrap();
        let routes = store.list_routes().await.unwrap();
        assert_eq!(routes.iter().filter(|r| r.hostname == "*").count(), 3);

        // A taken port makes neither the listener nor the route.
        assert!(
            store
                .add_server_port(&b.id, "0.0.0.0", 25566)
                .await
                .is_err()
        );
        assert_eq!(store.list_listeners().await.unwrap().len(), 2);
        assert_eq!(store.list_routes().await.unwrap().len(), 3);

        // Still in use by another server's route: kept.
        store
            .upsert_route(
                None,
                RouteInput {
                    server_id: Some(&a.id),
                    hostname: "a.example.com",
                    listener_id: Some(&port_b),
                    priority: 0,
                },
            )
            .await
            .unwrap();
        sqlx::query("DELETE FROM servers WHERE id = ?1")
            .bind(&b.id)
            .execute(store.pool())
            .await
            .unwrap();
        assert_eq!(store.remove_unused_server_ports(&b.id).await.unwrap(), 0);
        sqlx::query("DELETE FROM servers WHERE id = ?1")
            .bind(&a.id)
            .execute(store.pool())
            .await
            .unwrap();
        assert_eq!(store.remove_unused_server_ports(&a.id).await.unwrap(), 1);
        assert_eq!(store.remove_unused_server_ports(&b.id).await.unwrap(), 1);
        let left: Vec<String> = store
            .list_listeners()
            .await
            .unwrap()
            .into_iter()
            .map(|l| l.id)
            .collect();
        assert!(
            !left.contains(&port_a) && !left.contains(&port_b),
            "{left:?}"
        );
    }

    #[tokio::test]
    async fn route_crud_and_constraints() {
        let store = test_store().await;
        let server = store
            .insert_server(test_server("smp", 30001))
            .await
            .unwrap();
        let route = |hostname| RouteInput {
            server_id: Some(&server.id),
            hostname,
            listener_id: None,
            priority: 0,
        };

        let id = store
            .upsert_route(None, route("smp.example.com"))
            .await
            .unwrap()
            .unwrap();
        let routes = store.list_routes().await.unwrap();
        assert_eq!(routes[0].server_slug.as_deref(), Some("smp"));
        assert!(
            store
                .upsert_route(None, route("smp.example.com"))
                .await
                .is_err(),
            "unique hostname (for all listeners)"
        );

        // Slug wildcards may omit the server; other patterns may not.
        let wildcard = RouteInput {
            server_id: None,
            hostname: "*.example.com",
            listener_id: None,
            priority: 0,
        };
        assert!(store.upsert_route(None, wildcard).await.is_ok());
        let orphan = RouteInput {
            server_id: None,
            hostname: "x.example.com",
            listener_id: None,
            priority: 0,
        };
        assert!(store.upsert_route(None, orphan).await.is_err());

        assert!(store.delete_route(&id).await.unwrap());
        // Deleting a server removes its routes.
        store
            .upsert_route(None, route("again.example.com"))
            .await
            .unwrap();
        sqlx::query("DELETE FROM servers")
            .execute(store.pool())
            .await
            .unwrap();
        assert_eq!(
            store.list_routes().await.unwrap().len(),
            1,
            "only the slug wildcard remains"
        );
    }
}
