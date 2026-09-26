use postgres::Client;

use super::{AxSession, AxSessionStore};
use crate::backend::{
    postgres_create_pool, postgres_pool_connection, postgres_runtime_error, AxPostgresPool,
    AxRuntimeResult,
};

const SESSION_RESOURCE: &str = "ax_sessions";
const DEFAULT_POOL_MAX_SIZE: u32 = 16;
const DEFAULT_POOL_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_QUERY_TIMEOUT_MS: u64 = 10_000;

#[derive(Clone)]
pub struct AxPostgresSessionStore {
    pool: AxPostgresPool,
}

impl std::fmt::Debug for AxPostgresSessionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.pool.state();
        formatter
            .debug_struct("AxPostgresSessionStore")
            .field("connections", &state.connections)
            .field("idle_connections", &state.idle_connections)
            .finish()
    }
}

impl AxPostgresSessionStore {
    pub fn connect(url: impl Into<String>) -> AxRuntimeResult<Self> {
        Self::connect_with_config(
            url,
            DEFAULT_POOL_MAX_SIZE,
            DEFAULT_POOL_TIMEOUT_MS,
            DEFAULT_QUERY_TIMEOUT_MS,
        )
    }

    pub fn connect_with_config(
        url: impl Into<String>,
        pool_max_size: u32,
        pool_timeout_ms: u64,
        query_timeout_ms: u64,
    ) -> AxRuntimeResult<Self> {
        let pool = postgres_create_pool(
            &Some(url.into()),
            SESSION_RESOURCE,
            pool_max_size,
            pool_timeout_ms,
            query_timeout_ms,
        )?;
        let store = Self { pool };
        store.with_client(|client| {
            client
                .batch_execute(
                    "CREATE TABLE IF NOT EXISTS ax_sessions (
                        id TEXT PRIMARY KEY NOT NULL,
                        subject TEXT NOT NULL,
                        data_json JSONB NOT NULL,
                        created_at_unix BIGINT NOT NULL,
                        last_seen_at_unix BIGINT NOT NULL,
                        expires_at_unix BIGINT NOT NULL
                    );
                    CREATE INDEX IF NOT EXISTS ax_sessions_expires_at_idx
                        ON ax_sessions (expires_at_unix);",
                )
                .map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))?;
            Ok(())
        })?;
        Ok(store)
    }

    pub fn purge_expired(&self, now_unix: i64) -> AxRuntimeResult<u64> {
        self.with_client(|client| {
            client
                .execute(
                    "DELETE FROM ax_sessions WHERE expires_at_unix <= $1",
                    &[&now_unix],
                )
                .map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))
        })
    }

    fn with_client<T>(
        &self,
        operation: impl FnOnce(&mut Client) -> AxRuntimeResult<T>,
    ) -> AxRuntimeResult<T> {
        let mut client = postgres_pool_connection(&self.pool, SESSION_RESOURCE)?;
        operation(&mut client)
    }
}

impl AxSessionStore for AxPostgresSessionStore {
    fn refresh_live(&self, session: &AxSession, now_unix: i64) -> AxRuntimeResult<bool> {
        self.with_client(|client| {
            client.execute(
                "UPDATE ax_sessions SET last_seen_at_unix = GREATEST(last_seen_at_unix, $1), expires_at_unix = GREATEST(expires_at_unix, $2) WHERE id = $3 AND created_at_unix = $4 AND expires_at_unix > $5",
                &[&session.last_seen_at_unix, &session.expires_at_unix, &session.id, &session.created_at_unix, &now_unix],
            ).map(|count| count == 1).map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))
        })
    }
    fn save(&self, session: &AxSession) -> AxRuntimeResult<()> {
        let data_json = serde_json::to_value(&session.data).map_err(|error| {
            crate::backend::AxRuntimeError::message(format!(
                "session data serialization failed: {error}"
            ))
        })?;
        self.with_client(|client| {
            client
                .execute(
                    "INSERT INTO ax_sessions (
                        id, subject, data_json, created_at_unix, last_seen_at_unix, expires_at_unix
                    ) VALUES ($1, $2, $3, $4, $5, $6)
                    ON CONFLICT(id) DO UPDATE SET
                        subject = EXCLUDED.subject,
                        data_json = EXCLUDED.data_json,
                        created_at_unix = EXCLUDED.created_at_unix,
                        last_seen_at_unix = EXCLUDED.last_seen_at_unix,
                        expires_at_unix = EXCLUDED.expires_at_unix",
                    &[
                        &session.id,
                        &session.subject,
                        &data_json,
                        &session.created_at_unix,
                        &session.last_seen_at_unix,
                        &session.expires_at_unix,
                    ],
                )
                .map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))?;
            Ok(())
        })
    }

    fn load(&self, session_id: &str) -> AxRuntimeResult<Option<AxSession>> {
        self.with_client(|client| {
            let record = client
                .query_opt(
                    "SELECT id, subject, data_json, created_at_unix, last_seen_at_unix, expires_at_unix
                     FROM ax_sessions WHERE id = $1",
                    &[&session_id],
                )
                .map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))?;
            let Some(row) = record else {
                return Ok(None);
            };
            let data_json: serde_json::Value = row.get(2);
            let data = serde_json::from_value(data_json).map_err(|error| {
                crate::backend::AxRuntimeError::message(format!(
                    "stored session data is invalid: {error}"
                ))
            })?;
            Ok(Some(AxSession {
                id: row.get(0),
                subject: row.get(1),
                data,
                created_at_unix: row.get(3),
                last_seen_at_unix: row.get(4),
                expires_at_unix: row.get(5),
            }))
        })
    }

    fn delete(&self, session_id: &str) -> AxRuntimeResult<()> {
        self.with_client(|client| {
            client
                .execute("DELETE FROM ax_sessions WHERE id = $1", &[&session_id])
                .map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::server::AxHttpRequest;
    use crate::session::{AxSessionCookiePolicy, AxSessionManager};

    fn request_with_cookie(name: &str, value: &str) -> AxHttpRequest {
        AxHttpRequest::new("GET", "/account").with_header("Cookie", format!("{name}={value}"))
    }

    #[test]
    fn postgres_live_session_lifecycle_runs_when_test_url_is_configured() {
        let Ok(url) = std::env::var("AXONYX_TEST_POSTGRES_URL") else {
            return;
        };
        let store = Arc::new(
            AxPostgresSessionStore::connect_with_config(&url, 4, 5_000, 10_000)
                .expect("postgres session store should connect"),
        );
        store
            .with_client(|client| {
                client
                    .execute("DELETE FROM ax_sessions", &[])
                    .map_err(|error| postgres_runtime_error(SESSION_RESOURCE, error))?;
                Ok(())
            })
            .expect("session table should clean before test");
        let manager = AxSessionManager::new(
            store.clone(),
            AxSessionCookiePolicy {
                ttl_seconds: 60,
                ..AxSessionCookiePolicy::development()
            },
        )
        .expect("manager should initialize");
        let mut data = BTreeMap::new();
        data.insert("role".to_string(), json!("editor"));
        let (_, cookie) = manager
            .create("user-9", data, "secret", 1_000)
            .expect("session should persist");
        let request = request_with_cookie(&cookie.name, &cookie.value);
        let loaded = manager
            .load(&request, "secret", 1_010)
            .expect("session should load")
            .expect("session should exist");
        assert_eq!(loaded.subject, "user-9");
        assert_eq!(loaded.data["role"], "editor");

        manager
            .refresh(&request, "secret", 1_020)
            .expect("session should refresh")
            .expect("session should exist");
        assert_eq!(
            store
                .load(&loaded.id)
                .expect("session should remain readable")
                .expect("session should remain present")
                .expires_at_unix,
            1_080
        );

        manager
            .destroy(&request, "secret")
            .expect("session should be destroyed");
        assert!(store
            .load(&loaded.id)
            .expect("store should remain readable")
            .is_none());
        assert!(!store.refresh_live(&loaded, 1_030).unwrap());
    }
}
