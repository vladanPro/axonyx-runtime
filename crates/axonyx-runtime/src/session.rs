use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::backend::{AxRuntimeError, AxRuntimeResult};
use crate::server::{AxAuth, AxCookie, AxHttpRequest};

mod csrf;
mod postgres;
mod sqlite;

pub use postgres::AxPostgresSessionStore;
pub use sqlite::AxSqliteSessionStore;

pub const DEFAULT_SESSION_TTL_SECONDS: i64 = 60 * 60 * 24 * 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxSameSite {
    Strict,
    Lax,
    None,
}

impl AxSameSite {
    pub fn label(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxSessionCookiePolicy {
    pub name: String,
    pub path: String,
    pub domain: Option<String>,
    pub ttl_seconds: i64,
    pub secure: bool,
    pub same_site: AxSameSite,
}

impl Default for AxSessionCookiePolicy {
    fn default() -> Self {
        Self {
            name: "session".to_string(),
            path: "/".to_string(),
            domain: None,
            ttl_seconds: DEFAULT_SESSION_TTL_SECONDS,
            secure: true,
            same_site: AxSameSite::Lax,
        }
    }
}

impl AxSessionCookiePolicy {
    pub fn development() -> Self {
        Self {
            secure: false,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> AxRuntimeResult<()> {
        if self.name.trim().is_empty() {
            return Err(AxRuntimeError::message(
                "session cookie name must not be empty",
            ));
        }
        if self.path.trim().is_empty() {
            return Err(AxRuntimeError::message(
                "session cookie path must not be empty",
            ));
        }
        if self.ttl_seconds <= 0 {
            return Err(AxRuntimeError::message(
                "session TTL must be greater than zero",
            ));
        }
        if self.same_site == AxSameSite::None && !self.secure {
            return Err(AxRuntimeError::message(
                "SameSite=None session cookies must be Secure",
            ));
        }
        Ok(())
    }

    fn cookie(&self, value: impl Into<String>, max_age: i64) -> AxCookie {
        let mut cookie = AxCookie::new(&self.name, value)
            .with_path(&self.path)
            .with_max_age(max_age)
            .http_only()
            .same_site(self.same_site.label());
        if let Some(domain) = &self.domain {
            cookie = cookie.with_domain(domain);
        }
        if self.secure {
            cookie = cookie.secure();
        }
        cookie
    }

    pub fn issue_cookie(&self, session_id: &str, secret: &str) -> AxRuntimeResult<AxCookie> {
        self.validate()?;
        validate_secret(secret)?;
        Ok(self.cookie(AxAuth::sign_session(session_id, secret), self.ttl_seconds))
    }

    pub fn clear_cookie(&self) -> AxRuntimeResult<AxCookie> {
        self.validate()?;
        Ok(self.cookie("", 0))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxSession {
    pub id: String,
    pub subject: String,
    #[serde(default)]
    pub data: BTreeMap<String, Value>,
    pub created_at_unix: i64,
    pub last_seen_at_unix: i64,
    pub expires_at_unix: i64,
}

impl AxSession {
    pub fn is_expired(&self, now_unix: i64) -> bool {
        self.expires_at_unix <= now_unix
    }
}

pub trait AxSessionStore: Send + Sync {
    fn save(&self, session: &AxSession) -> AxRuntimeResult<()>;
    fn load(&self, session_id: &str) -> AxRuntimeResult<Option<AxSession>>;
    fn delete(&self, session_id: &str) -> AxRuntimeResult<()>;
}

#[derive(Debug, Default)]
pub struct AxMemorySessionStore {
    sessions: RwLock<BTreeMap<String, AxSession>>,
}

impl AxMemorySessionStore {
    pub fn len(&self) -> AxRuntimeResult<usize> {
        Ok(self.read()?.len())
    }

    pub fn is_empty(&self) -> AxRuntimeResult<bool> {
        Ok(self.read()?.is_empty())
    }

    fn read(&self) -> AxRuntimeResult<std::sync::RwLockReadGuard<'_, BTreeMap<String, AxSession>>> {
        self.sessions
            .read()
            .map_err(|_| AxRuntimeError::message("session store read lock is poisoned"))
    }

    fn write(
        &self,
    ) -> AxRuntimeResult<std::sync::RwLockWriteGuard<'_, BTreeMap<String, AxSession>>> {
        self.sessions
            .write()
            .map_err(|_| AxRuntimeError::message("session store write lock is poisoned"))
    }
}

impl AxSessionStore for AxMemorySessionStore {
    fn save(&self, session: &AxSession) -> AxRuntimeResult<()> {
        self.write()?.insert(session.id.clone(), session.clone());
        Ok(())
    }

    fn load(&self, session_id: &str) -> AxRuntimeResult<Option<AxSession>> {
        Ok(self.read()?.get(session_id).cloned())
    }

    fn delete(&self, session_id: &str) -> AxRuntimeResult<()> {
        self.write()?.remove(session_id);
        Ok(())
    }
}

#[derive(Clone)]
pub struct AxSessionManager {
    store: Arc<dyn AxSessionStore>,
    policy: AxSessionCookiePolicy,
}

impl AxSessionManager {
    pub fn new(
        store: Arc<dyn AxSessionStore>,
        policy: AxSessionCookiePolicy,
    ) -> AxRuntimeResult<Self> {
        policy.validate()?;
        Ok(Self { store, policy })
    }

    pub fn policy(&self) -> &AxSessionCookiePolicy {
        &self.policy
    }

    pub fn create(
        &self,
        subject: impl Into<String>,
        data: BTreeMap<String, Value>,
        secret: &str,
        now_unix: i64,
    ) -> AxRuntimeResult<(AxSession, AxCookie)> {
        validate_secret(secret)?;
        let subject = subject.into();
        if subject.trim().is_empty() {
            return Err(AxRuntimeError::message("session subject must not be empty"));
        }
        let session = AxSession {
            id: Uuid::new_v4().simple().to_string(),
            subject,
            data,
            created_at_unix: now_unix,
            last_seen_at_unix: now_unix,
            expires_at_unix: now_unix.saturating_add(self.policy.ttl_seconds),
        };
        self.store.save(&session)?;
        let cookie = self.policy.issue_cookie(&session.id, secret)?;
        Ok((session, cookie))
    }

    pub fn load(
        &self,
        request: &AxHttpRequest,
        secret: &str,
        now_unix: i64,
    ) -> AxRuntimeResult<Option<AxSession>> {
        validate_secret(secret)?;
        let Some(session_id) = AxAuth::signed_cookie(request, &self.policy.name, secret) else {
            return Ok(None);
        };
        let Some(session) = self.store.load(session_id)? else {
            return Ok(None);
        };
        if session.is_expired(now_unix) {
            self.store.delete(session_id)?;
            return Ok(None);
        }
        Ok(Some(session))
    }

    pub fn refresh(
        &self,
        request: &AxHttpRequest,
        secret: &str,
        now_unix: i64,
    ) -> AxRuntimeResult<Option<(AxSession, AxCookie)>> {
        let Some(mut session) = self.load(request, secret, now_unix)? else {
            return Ok(None);
        };
        session.last_seen_at_unix = now_unix;
        session.expires_at_unix = now_unix.saturating_add(self.policy.ttl_seconds);
        self.store.save(&session)?;
        let cookie = self.policy.issue_cookie(&session.id, secret)?;
        Ok(Some((session, cookie)))
    }

    /// Issue a proof only for an authenticated, unexpired server-side session.
    /// Deliver it through a no-store same-origin response or an escaped form field.
    pub fn csrf_token(
        &self,
        request: &AxHttpRequest,
        secret: &str,
        now_unix: i64,
    ) -> AxRuntimeResult<Option<String>> {
        let Some(session) = self.load(request, secret, now_unix)? else {
            return Ok(None);
        };
        csrf::issue(&session.id, secret).map(Some)
    }

    /// Validate proof against the active session, not a session ID from the client.
    /// Call together with origin validation, before running a mutation.
    pub fn verify_csrf(
        &self,
        request: &AxHttpRequest,
        token: &str,
        secret: &str,
        now_unix: i64,
    ) -> AxRuntimeResult<bool> {
        let Some(session) = self.load(request, secret, now_unix)? else {
            return Ok(false);
        };
        csrf::verify(&session.id, token, secret)
    }

    pub fn destroy(&self, request: &AxHttpRequest, secret: &str) -> AxRuntimeResult<AxCookie> {
        validate_secret(secret)?;
        if let Some(session_id) = AxAuth::signed_cookie(request, &self.policy.name, secret) {
            self.store.delete(session_id)?;
        }
        self.policy.clear_cookie()
    }
}

fn validate_secret(secret: &str) -> AxRuntimeResult<()> {
    if secret.trim().is_empty() {
        return Err(AxRuntimeError::message(
            "session signing secret must not be empty",
        ));
    }
    Ok(())
}

pub mod prelude {
    pub use super::{
        AxMemorySessionStore, AxPostgresSessionStore, AxSameSite, AxSession, AxSessionCookiePolicy,
        AxSessionManager, AxSessionStore, AxSqliteSessionStore, DEFAULT_SESSION_TTL_SECONDS,
    };
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    fn request_with_cookie(cookie: &AxCookie) -> AxHttpRequest {
        AxHttpRequest::new("GET", "/account")
            .with_header("Cookie", format!("{}={}", cookie.name, cookie.value))
    }

    #[test]
    fn csrf_proofs_follow_memory_session_lifecycle() {
        assert_csrf_lifecycle(Arc::new(AxMemorySessionStore::default()));
    }

    #[test]
    fn csrf_proofs_follow_sqlite_session_lifecycle() {
        let path = std::env::temp_dir().join(format!("axonyx-csrf-{}.sqlite", Uuid::new_v4()));
        assert_csrf_lifecycle(Arc::new(AxSqliteSessionStore::open(&path).unwrap()));
        std::fs::remove_file(path).unwrap();
    }

    fn assert_csrf_lifecycle(store: Arc<dyn AxSessionStore>) {
        let manager = AxSessionManager::new(
            store,
            AxSessionCookiePolicy {
                ttl_seconds: 60,
                ..AxSessionCookiePolicy::development()
            },
        )
        .unwrap();
        let secret = "0123456789abcdef0123456789abcdef";
        let anonymous = AxHttpRequest::new("POST", "/save");
        assert_eq!(manager.csrf_token(&anonymous, secret, 100).unwrap(), None);
        assert!(!manager
            .verify_csrf(&anonymous, "anything", secret, 100)
            .unwrap());
        let (session, cookie) = manager
            .create("user-1", BTreeMap::new(), secret, 100)
            .unwrap();
        let request = request_with_cookie(&cookie);
        let token = manager.csrf_token(&request, secret, 101).unwrap().unwrap();
        assert_eq!(token.len(), 72);
        assert!(!token.contains(&session.id));
        assert!(!token.contains(secret));
        assert!(manager.verify_csrf(&request, &token, secret, 101).unwrap());
        assert_eq!(
            manager.csrf_token(&request, secret, 102).unwrap().unwrap(),
            token
        );
        for bad in [
            String::new(),
            "x".repeat(100_000),
            "axcsrf2.invalid".into(),
            format!("axcsrf1.{}", "0".repeat(64)),
            format!("{token}x"),
            format!("axcsrf1.{}", "é".repeat(32)),
        ] {
            assert!(!manager.verify_csrf(&request, &bad, secret, 101).unwrap());
        }
        let (_, other_cookie) = manager
            .create("user-1", BTreeMap::new(), secret, 102)
            .unwrap();
        let other_request = request_with_cookie(&other_cookie);
        assert!(!manager
            .verify_csrf(&other_request, &token, secret, 102)
            .unwrap());
        assert!(!manager
            .verify_csrf(&request, &token, "abcdef0123456789abcdef0123456789", 102)
            .unwrap());
        let rotated_secret = "abcdef0123456789abcdef0123456789ab";
        let rotated_cookie = manager
            .policy()
            .issue_cookie(&session.id, rotated_secret)
            .unwrap();
        let rotated_request = request_with_cookie(&rotated_cookie);
        assert!(manager
            .load(&rotated_request, rotated_secret, 102)
            .unwrap()
            .is_some());
        assert!(!manager
            .verify_csrf(&rotated_request, &token, rotated_secret, 102)
            .unwrap());
        manager.refresh(&request, secret, 110).unwrap().unwrap();
        assert!(manager.verify_csrf(&request, &token, secret, 165).unwrap());
        manager.destroy(&request, secret).unwrap();
        assert!(!manager.verify_csrf(&request, &token, secret, 166).unwrap());
        assert!(manager.csrf_token(&request, secret, 166).unwrap().is_none());
        let other_token = manager
            .csrf_token(&other_request, secret, 103)
            .unwrap()
            .unwrap();
        assert!(!manager
            .verify_csrf(&other_request, &other_token, secret, 162)
            .unwrap());
        assert!(manager
            .csrf_token(&other_request, secret, 162)
            .unwrap()
            .is_none());
    }

    #[test]
    fn csrf_rejects_weak_keys_and_modified_signatures() {
        let manager = AxSessionManager::new(
            Arc::new(AxMemorySessionStore::default()),
            AxSessionCookiePolicy::development(),
        )
        .unwrap();
        let (_, cookie) = manager
            .create("user", BTreeMap::new(), "short-key", 100)
            .unwrap();
        assert!(manager
            .csrf_token(&request_with_cookie(&cookie), "short-key", 101)
            .is_err());
        assert!(manager
            .verify_csrf(&request_with_cookie(&cookie), "anything", "short-key", 101)
            .is_err());
        let secret = "0123456789abcdef0123456789abcdef";
        let (_, cookie) = manager
            .create("user", BTreeMap::new(), secret, 100)
            .unwrap();
        let request = request_with_cookie(&cookie);
        let token = manager.csrf_token(&request, secret, 101).unwrap().unwrap();
        let mut bytes = token.into_bytes();
        bytes[8] = if bytes[8] == b'0' { b'1' } else { b'0' };
        assert!(!manager
            .verify_csrf(&request, &String::from_utf8(bytes).unwrap(), secret, 101)
            .unwrap());
    }

    #[test]
    fn session_lifecycle_creates_loads_refreshes_and_destroys() {
        let store = Arc::new(AxMemorySessionStore::default());
        let manager = AxSessionManager::new(
            store.clone(),
            AxSessionCookiePolicy {
                ttl_seconds: 60,
                ..AxSessionCookiePolicy::development()
            },
        )
        .expect("policy should be valid");
        let mut data = BTreeMap::new();
        data.insert("role".to_string(), json!("admin"));

        let (session, cookie) = manager
            .create("user-1", data, "secret", 1_000)
            .expect("session should be created");
        assert_eq!(store.len().expect("store should be readable"), 1);
        assert_eq!(session.expires_at_unix, 1_060);
        assert_eq!(session.data["role"], "admin");
        assert!(cookie.render().contains("HttpOnly"));
        assert!(!cookie.render().contains("Secure"));

        let request = request_with_cookie(&cookie);
        let loaded = manager
            .load(&request, "secret", 1_030)
            .expect("session should load")
            .expect("session should exist");
        assert_eq!(loaded.subject, "user-1");

        let (refreshed, refreshed_cookie) = manager
            .refresh(&request, "secret", 1_040)
            .expect("session should refresh")
            .expect("session should exist");
        assert_eq!(refreshed.last_seen_at_unix, 1_040);
        assert_eq!(refreshed.expires_at_unix, 1_100);
        assert_eq!(refreshed_cookie.value, cookie.value);

        let cleared = manager
            .destroy(&request, "secret")
            .expect("session should be destroyed");
        assert_eq!(cleared.max_age, Some(0));
        assert_eq!(store.len().expect("store should be readable"), 0);
    }

    #[test]
    fn invalid_signature_never_reaches_the_store_session() {
        let store = Arc::new(AxMemorySessionStore::default());
        let manager = AxSessionManager::new(store, AxSessionCookiePolicy::development())
            .expect("policy should be valid");
        let (_, cookie) = manager
            .create("user-1", BTreeMap::new(), "secret", 1_000)
            .expect("session should be created");
        let request = request_with_cookie(&cookie);

        assert!(manager
            .load(&request, "wrong-secret", 1_001)
            .expect("invalid signatures should be handled")
            .is_none());
    }

    #[test]
    fn expired_session_is_deleted_on_load() {
        let store = Arc::new(AxMemorySessionStore::default());
        let manager = AxSessionManager::new(
            store.clone(),
            AxSessionCookiePolicy {
                ttl_seconds: 10,
                ..AxSessionCookiePolicy::development()
            },
        )
        .expect("policy should be valid");
        let (_, cookie) = manager
            .create("user-1", BTreeMap::new(), "secret", 1_000)
            .expect("session should be created");
        let request = request_with_cookie(&cookie);

        assert!(manager
            .load(&request, "secret", 1_010)
            .expect("expired session should be handled")
            .is_none());
        assert_eq!(store.len().expect("store should be readable"), 0);
    }

    #[test]
    fn production_cookie_policy_is_secure_and_rejects_unsafe_none() {
        let policy = AxSessionCookiePolicy::default();
        let cookie = policy
            .issue_cookie("opaque", "secret")
            .expect("production policy should be valid")
            .render();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=Lax"));

        let unsafe_policy = AxSessionCookiePolicy {
            secure: false,
            same_site: AxSameSite::None,
            ..AxSessionCookiePolicy::default()
        };
        assert!(unsafe_policy.validate().is_err());
        assert!(policy.issue_cookie("opaque", "").is_err());
    }

    #[test]
    fn rejected_session_create_does_not_write_to_the_store() {
        let store = Arc::new(AxMemorySessionStore::default());
        let manager = AxSessionManager::new(store.clone(), AxSessionCookiePolicy::development())
            .expect("policy should be valid");

        assert!(manager
            .create("user-1", BTreeMap::new(), "", 1_000)
            .is_err());
        assert!(manager
            .create("  ", BTreeMap::new(), "secret", 1_000)
            .is_err());
        assert!(store.is_empty().expect("store should be readable"));
    }
}
