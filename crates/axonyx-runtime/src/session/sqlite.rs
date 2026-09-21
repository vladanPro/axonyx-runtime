use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};

use super::{AxSession, AxSessionStore};
use crate::backend::{sqlite_runtime_error, AxRuntimeError, AxRuntimeResult};

const SESSION_RESOURCE: &str = "ax_sessions";

#[derive(Debug)]
pub struct AxSqliteSessionStore {
    connection: Mutex<Connection>,
}

impl AxSqliteSessionStore {
    pub fn open(path: impl AsRef<Path>) -> AxRuntimeResult<Self> {
        let connection = Connection::open(path)
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))?;
        Self::from_connection(connection)
    }

    pub fn in_memory() -> AxRuntimeResult<Self> {
        let connection = Connection::open_in_memory()
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> AxRuntimeResult<Self> {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS ax_sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    subject TEXT NOT NULL,
                    data_json TEXT NOT NULL,
                    created_at_unix INTEGER NOT NULL,
                    last_seen_at_unix INTEGER NOT NULL,
                    expires_at_unix INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS ax_sessions_expires_at_idx
                    ON ax_sessions (expires_at_unix);",
            )
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn purge_expired(&self, now_unix: i64) -> AxRuntimeResult<usize> {
        self.connection()?
            .execute(
                "DELETE FROM ax_sessions WHERE expires_at_unix <= ?1",
                params![now_unix],
            )
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))
    }

    fn connection(&self) -> AxRuntimeResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| AxRuntimeError::message("SQLite session store lock is poisoned"))
    }
}

impl AxSessionStore for AxSqliteSessionStore {
    fn save(&self, session: &AxSession) -> AxRuntimeResult<()> {
        let data_json = serde_json::to_string(&session.data).map_err(|error| {
            AxRuntimeError::message(format!("session data serialization failed: {error}"))
        })?;
        self.connection()?
            .execute(
                "INSERT INTO ax_sessions (
                    id, subject, data_json, created_at_unix, last_seen_at_unix, expires_at_unix
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(id) DO UPDATE SET
                    subject = excluded.subject,
                    data_json = excluded.data_json,
                    created_at_unix = excluded.created_at_unix,
                    last_seen_at_unix = excluded.last_seen_at_unix,
                    expires_at_unix = excluded.expires_at_unix",
                params![
                    session.id,
                    session.subject,
                    data_json,
                    session.created_at_unix,
                    session.last_seen_at_unix,
                    session.expires_at_unix,
                ],
            )
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))?;
        Ok(())
    }

    fn load(&self, session_id: &str) -> AxRuntimeResult<Option<AxSession>> {
        let record = self
            .connection()?
            .query_row(
                "SELECT id, subject, data_json, created_at_unix, last_seen_at_unix, expires_at_unix
                 FROM ax_sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))?;
        let Some((id, subject, data_json, created_at_unix, last_seen_at_unix, expires_at_unix)) =
            record
        else {
            return Ok(None);
        };
        let data = serde_json::from_str(&data_json).map_err(|error| {
            AxRuntimeError::message(format!("stored session data is invalid: {error}"))
        })?;
        Ok(Some(AxSession {
            id,
            subject,
            data,
            created_at_unix,
            last_seen_at_unix,
            expires_at_unix,
        }))
    }

    fn delete(&self, session_id: &str) -> AxRuntimeResult<()> {
        self.connection()?
            .execute("DELETE FROM ax_sessions WHERE id = ?1", params![session_id])
            .map_err(|error| sqlite_runtime_error(SESSION_RESOURCE, error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Arc;

    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::server::AxHttpRequest;
    use crate::session::{AxSessionCookiePolicy, AxSessionManager};

    fn request_with_cookie(name: &str, value: &str) -> AxHttpRequest {
        AxHttpRequest::new("GET", "/account").with_header("Cookie", format!("{name}={value}"))
    }

    #[test]
    fn sqlite_store_persists_session_lifecycle_across_connections() {
        let path =
            std::env::temp_dir().join(format!("axonyx-session-{}.sqlite", Uuid::new_v4().simple()));
        let mut data = BTreeMap::new();
        data.insert("role".to_string(), json!("editor"));

        let cookie = {
            let store = Arc::new(AxSqliteSessionStore::open(&path).expect("store should open"));
            let manager = AxSessionManager::new(
                store,
                AxSessionCookiePolicy {
                    ttl_seconds: 60,
                    ..AxSessionCookiePolicy::development()
                },
            )
            .expect("manager should initialize");
            manager
                .create("user-7", data, "secret", 1_000)
                .expect("session should persist")
                .1
        };

        let store = Arc::new(AxSqliteSessionStore::open(&path).expect("store should reopen"));
        let manager = AxSessionManager::new(
            store.clone(),
            AxSessionCookiePolicy {
                ttl_seconds: 60,
                ..AxSessionCookiePolicy::development()
            },
        )
        .expect("manager should initialize");
        let request = request_with_cookie(&cookie.name, &cookie.value);
        let loaded = manager
            .load(&request, "secret", 1_010)
            .expect("session should load")
            .expect("session should exist");
        assert_eq!(loaded.subject, "user-7");
        assert_eq!(loaded.data["role"], "editor");

        manager
            .destroy(&request, "secret")
            .expect("session should be destroyed");
        assert!(store
            .load(&loaded.id)
            .expect("store should remain readable")
            .is_none());

        drop(manager);
        drop(store);
        fs::remove_file(path).expect("temporary database should clean up");
    }

    #[test]
    fn sqlite_store_purges_expired_sessions_without_removing_live_ones() {
        let store = AxSqliteSessionStore::in_memory().expect("store should open");
        store
            .save(&AxSession {
                id: "expired".to_string(),
                subject: "user-1".to_string(),
                data: BTreeMap::new(),
                created_at_unix: 1,
                last_seen_at_unix: 1,
                expires_at_unix: 10,
            })
            .expect("expired session should save");
        store
            .save(&AxSession {
                id: "live".to_string(),
                subject: "user-2".to_string(),
                data: BTreeMap::new(),
                created_at_unix: 1,
                last_seen_at_unix: 1,
                expires_at_unix: 20,
            })
            .expect("live session should save");

        assert_eq!(store.purge_expired(10).expect("purge should run"), 1);
        assert!(store.load("expired").expect("load should run").is_none());
        assert!(store.load("live").expect("load should run").is_some());
    }

    #[test]
    fn sqlite_store_is_safe_for_parallel_session_writes() {
        let store = Arc::new(AxSqliteSessionStore::in_memory().expect("store should open"));
        let workers = (0..8)
            .map(|index| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store.save(&AxSession {
                        id: format!("session-{index}"),
                        subject: format!("user-{index}"),
                        data: BTreeMap::new(),
                        created_at_unix: 1,
                        last_seen_at_unix: 1,
                        expires_at_unix: 100,
                    })
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker
                .join()
                .expect("worker should not panic")
                .expect("session should save");
        }
        for index in 0..8 {
            assert!(store
                .load(&format!("session-{index}"))
                .expect("session should load")
                .is_some());
        }
    }
}
