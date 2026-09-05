use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fs;
use std::path::Path;

use axonyx_core::ax_backend_lowering_prelude::{
    AxAssignmentPlan, AxQueryFilterOpPlan, AxQueryFilterPlan, AxQueryModePlan,
    AxQueryOrderDirectionPlan, AxQueryOrderPlan, AxQueryPlan, AxQuerySourcePlan, AxRustExpr,
};
use axonyx_core::ax_sql_prelude::{
    compile_delete_plan_to_sql, compile_insert_plan_to_sql, compile_query_plan_to_sql,
    compile_update_plan_to_sql, AxSqlDialect, AxSqlParam,
};
use bytes::BytesMut;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime};
use postgres::types::{to_sql_checked, IsNull, ToSql, Type};
use postgres::Client;
use r2d2::{Pool, PooledConnection};
use r2d2_postgres::PostgresConnectionManager;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio_postgres_rustls::MakeRustlsConnect;
use uuid::Uuid;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AxRuntimeError {
    #[error("runtime operation failed: {message}")]
    Message { message: String },
    #[error("runtime database operation failed: {error}")]
    Database { error: Box<AxDbError> },
}

impl AxRuntimeError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
        }
    }

    pub fn database(error: AxDbError) -> Self {
        Self::Database {
            error: Box::new(error),
        }
    }

    pub fn public_error_payload(&self) -> Value {
        match self {
            Self::Message { message } => json!({
                "ok": false,
                "code": "runtime.error",
                "message": message,
            }),
            Self::Database { error } => error.public_payload(),
        }
    }
}

pub type AxRuntimeResult<T> = Result<T, AxRuntimeError>;

pub const DEFAULT_DB_POOL_MAX_SIZE: u32 = 10;
pub const DEFAULT_DB_POOL_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_DB_QUERY_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_DB_READ_RETRY_ATTEMPTS: u32 = 1;
pub const DEFAULT_DB_READ_RETRY_BACKOFF_MS: u64 = 50;
pub const DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
pub const MAX_DB_READ_RETRY_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxDbErrorCode {
    UnknownResource,
    UniqueViolation,
    ConstraintViolation,
    InvalidQuery,
    UnsupportedOperation,
    MigrationConflict,
    MigrationChecksumMismatch,
    ConnectionFailed,
    Timeout,
    DriverError,
}

impl AxDbErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UnknownResource => "db.unknown_resource",
            Self::UniqueViolation => "db.unique_violation",
            Self::ConstraintViolation => "db.constraint_violation",
            Self::InvalidQuery => "db.invalid_query",
            Self::UnsupportedOperation => "db.unsupported_operation",
            Self::MigrationConflict => "db.migration_conflict",
            Self::MigrationChecksumMismatch => "db.migration_checksum_mismatch",
            Self::ConnectionFailed => "db.connection_failed",
            Self::Timeout => "db.timeout",
            Self::DriverError => "db.driver_error",
        }
    }

    pub fn default_message(&self) -> &'static str {
        match self {
            Self::UnknownResource => "Database resource was not found.",
            Self::UniqueViolation => "Record already exists.",
            Self::ConstraintViolation => "Database constraint failed.",
            Self::InvalidQuery => "Database query is invalid.",
            Self::UnsupportedOperation => "Database operation is not supported.",
            Self::MigrationConflict => {
                "Database migration state conflicts with the requested operation."
            }
            Self::MigrationChecksumMismatch => {
                "Applied database migration checksum does not match the local file."
            }
            Self::ConnectionFailed => "Database connection failed.",
            Self::Timeout => "Database operation timed out.",
            Self::DriverError => "Database operation failed.",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::UnknownResource | Self::InvalidQuery => 400,
            Self::UniqueViolation
            | Self::ConstraintViolation
            | Self::MigrationConflict
            | Self::MigrationChecksumMismatch => 409,
            Self::UnsupportedOperation => 501,
            Self::ConnectionFailed | Self::Timeout => 503,
            Self::DriverError => 500,
        }
    }
}

#[derive(Debug, Error, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct AxDbError {
    pub code: String,
    pub message: String,
    pub status: u16,
    pub driver: Option<String>,
    pub resource: Option<String>,
    pub field: Option<String>,
    pub detail: Option<String>,
}

impl AxDbError {
    pub fn new(code: AxDbErrorCode) -> Self {
        Self {
            code: code.as_str().to_string(),
            message: code.default_message().to_string(),
            status: code.status(),
            driver: None,
            resource: None,
            field: None,
            detail: None,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn with_driver(mut self, driver: AxDatabaseDriver) -> Self {
        self.driver = Some(driver.as_str().to_string());
        self
    }

    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn invalid_query(
        driver: AxDatabaseDriver,
        resource: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(AxDbErrorCode::InvalidQuery)
            .with_driver(driver)
            .with_resource(resource)
            .with_detail(detail)
    }

    pub fn from_driver_detail(
        driver: AxDatabaseDriver,
        resource: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        let detail = detail.into();
        let code = classify_driver_error(driver.clone(), &detail);
        let mut error = Self::new(code)
            .with_driver(driver)
            .with_resource(resource)
            .with_detail(detail.clone());
        if let Some(field) = extract_driver_error_field(&detail) {
            error = error.with_field(field);
        }
        error
    }

    pub fn public_payload(&self) -> Value {
        let mut error = serde_json::Map::from_iter([
            ("code".to_string(), json!(self.code)),
            ("message".to_string(), json!(self.message)),
            ("status".to_string(), json!(self.status)),
        ]);

        if let Some(resource) = &self.resource {
            error.insert("resource".to_string(), json!(resource));
        }
        if let Some(field) = &self.field {
            error.insert("field".to_string(), json!(field));
        }

        json!({
            "ok": false,
            "error": Value::Object(error),
        })
    }
}

fn classify_driver_error(driver: AxDatabaseDriver, detail: &str) -> AxDbErrorCode {
    let normalized = detail.to_ascii_lowercase();

    match driver {
        AxDatabaseDriver::Sqlite => {
            if normalized.contains("no such table") {
                AxDbErrorCode::UnknownResource
            } else if normalized.contains("unique constraint failed") {
                AxDbErrorCode::UniqueViolation
            } else if normalized.contains("constraint failed")
                || normalized.contains("not null constraint failed")
                || normalized.contains("foreign key constraint failed")
            {
                AxDbErrorCode::ConstraintViolation
            } else if normalized.contains("no such column") {
                AxDbErrorCode::InvalidQuery
            } else if normalized.contains("database is locked")
                || normalized.contains("database is busy")
            {
                AxDbErrorCode::Timeout
            } else if normalized.contains("unable to open database file") {
                AxDbErrorCode::ConnectionFailed
            } else {
                AxDbErrorCode::DriverError
            }
        }
        AxDatabaseDriver::Postgres => {
            if normalized.contains("42p01") || normalized.contains("undefined table") {
                AxDbErrorCode::UnknownResource
            } else if normalized.contains("23505") || normalized.contains("duplicate key") {
                AxDbErrorCode::UniqueViolation
            } else if normalized.contains("23503")
                || normalized.contains("23502")
                || normalized.contains("check violation")
            {
                AxDbErrorCode::ConstraintViolation
            } else if normalized.contains("error serializing parameter")
                || normalized.contains("cannot be encoded as postgres type")
            {
                AxDbErrorCode::InvalidQuery
            } else if normalized.contains("timeout") {
                AxDbErrorCode::Timeout
            } else if normalized.contains("connection refused")
                || normalized.contains("could not connect")
                || normalized.contains("connection error")
                || normalized.contains("error connecting to server")
                || normalized.contains("tls handshake")
                || normalized.contains("certificate")
            {
                AxDbErrorCode::ConnectionFailed
            } else {
                AxDbErrorCode::DriverError
            }
        }
        AxDatabaseDriver::MySql => {
            if normalized.contains("duplicate entry") || normalized.contains("1062") {
                AxDbErrorCode::UniqueViolation
            } else if normalized.contains("foreign key constraint")
                || normalized.contains("cannot be null")
                || normalized.contains("1452")
            {
                AxDbErrorCode::ConstraintViolation
            } else if normalized.contains("timeout") {
                AxDbErrorCode::Timeout
            } else if normalized.contains("connection") {
                AxDbErrorCode::ConnectionFailed
            } else {
                AxDbErrorCode::DriverError
            }
        }
        AxDatabaseDriver::Memory => AxDbErrorCode::DriverError,
    }
}

fn extract_driver_error_field(detail: &str) -> Option<String> {
    let marker = "constraint failed:";
    let lower = detail.to_ascii_lowercase();
    let start = lower.find(marker)?;
    let path = detail[start + marker.len()..].trim();
    let field = path
        .rsplit_once('.')
        .map(|(_, field)| field)
        .unwrap_or(path);
    let field = field.trim_matches(['`', '"', '\'', ' ', '.']);
    if field.is_empty() {
        None
    } else {
        Some(field.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxQueryRequest {
    pub collection: String,
    pub filters: Vec<AxQueryFilterRequest>,
    pub orders: Vec<AxQueryOrderRequest>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub mode: AxQueryMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQueryMode {
    Many,
    First,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxRawSqlRequest {
    pub sql: String,
    pub params: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxQueryFilterRequest {
    pub field: String,
    pub op: AxQueryFilterOp,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQueryFilterOp {
    Eq,
    Ne,
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxQueryOrderRequest {
    pub field: String,
    pub direction: AxQueryOrderDirection,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxQueryOrderDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxInsertRequest {
    pub collection: String,
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxUpdateRequest {
    pub collection: String,
    pub fields: BTreeMap<String, Value>,
    pub filters: Vec<AxQueryFilterRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxDeleteRequest {
    pub collection: String,
    pub filters: Vec<AxQueryFilterRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AxTransactionOperation {
    Insert(AxInsertRequest),
    Update(AxUpdateRequest),
    Delete(AxDeleteRequest),
}

impl AxTransactionOperation {
    pub fn resource(&self) -> &str {
        match self {
            Self::Insert(request) => &request.collection,
            Self::Update(request) => &request.collection,
            Self::Delete(request) => &request.collection,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxTransactionRequest {
    pub operations: Vec<AxTransactionOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxMigration {
    pub version: String,
    pub name: String,
    pub checksum: String,
    pub up_sql: String,
    pub down_sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxAppliedMigration {
    pub version: String,
    pub name: String,
    pub checksum: String,
    pub applied_at: String,
    pub execution_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxSendRequest {
    pub target: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxLoaderContext {
    pub params: BTreeMap<String, String>,
    pub query: BTreeMap<String, String>,
}

impl AxLoaderContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    pub fn with_query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.insert(name.into(), value.into());
        self
    }

    pub fn param(&self, name: &str) -> AxRuntimeResult<String> {
        self.params
            .get(name)
            .cloned()
            .ok_or_else(|| AxRuntimeError::message(format!("missing route param `{name}`")))
    }

    pub fn query(&self, name: &str) -> AxRuntimeResult<String> {
        self.query
            .get(name)
            .cloned()
            .ok_or_else(|| AxRuntimeError::message(format!("missing query param `{name}`")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxDirectSqlPlan {
    pub dialect: String,
    pub sql: String,
    pub params: Vec<Value>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AxApiRequestPlan {
    pub dialect: String,
    pub base_url: String,
    pub token: String,
    pub action: String,
    pub resource: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxDatabaseDriver {
    Postgres,
    MySql,
    Sqlite,
    Memory,
}

impl AxDatabaseDriver {
    pub fn parse(input: &str) -> AxRuntimeResult<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "" | "postgres" | "postgresql" => Ok(Self::Postgres),
            "mysql" => Ok(Self::MySql),
            "sqlite" => Ok(Self::Sqlite),
            "memory" | "inmemory" | "in-memory" => Ok(Self::Memory),
            other => Err(AxRuntimeError::message(format!(
                "unsupported database driver `{other}`"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::MySql => "mysql",
            Self::Sqlite => "sqlite",
            Self::Memory => "memory",
        }
    }

    pub fn sql_dialect(&self) -> Option<AxSqlDialect> {
        match self {
            Self::Postgres => Some(AxSqlDialect::Postgres),
            Self::MySql => Some(AxSqlDialect::MySql),
            Self::Sqlite => Some(AxSqlDialect::Sqlite),
            Self::Memory => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxDataTransport {
    Direct,
    Api,
}

impl AxDataTransport {
    pub fn parse(input: &str) -> AxRuntimeResult<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "" | "direct" => Ok(Self::Direct),
            "api" => Ok(Self::Api),
            other => Err(AxRuntimeError::message(format!(
                "unsupported data transport `{other}`"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Api => "api",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxDatabaseConfig {
    pub driver: AxDatabaseDriver,
    pub transport: AxDataTransport,
    pub url: Option<String>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub pool_max_size: u32,
    pub pool_timeout_ms: u64,
    pub policy: AxDatabasePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxDatabasePolicy {
    pub query_timeout_ms: u64,
    pub read_retry_attempts: u32,
    pub read_retry_backoff_ms: u64,
    pub sqlite_busy_timeout_ms: u64,
}

impl Default for AxDatabasePolicy {
    fn default() -> Self {
        Self {
            query_timeout_ms: DEFAULT_DB_QUERY_TIMEOUT_MS,
            read_retry_attempts: DEFAULT_DB_READ_RETRY_ATTEMPTS,
            read_retry_backoff_ms: DEFAULT_DB_READ_RETRY_BACKOFF_MS,
            sqlite_busy_timeout_ms: DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxDatabasePoolHealth {
    pub max_size: u32,
    pub connections: u32,
    pub idle_connections: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxDatabaseMetricsSnapshot {
    pub reads: u64,
    pub writes: u64,
    pub failures: u64,
    pub timeouts: u64,
    pub connection_failures: u64,
    pub total_duration_us: u64,
    pub last_duration_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxDatabaseHealthReport {
    pub ok: bool,
    pub driver: String,
    pub transport: String,
    pub probe: String,
    pub latency_ms: u64,
    pub pool: Option<AxDatabasePoolHealth>,
    pub metrics: AxDatabaseMetricsSnapshot,
}

impl AxDatabaseHealthReport {
    fn ready(
        driver: AxDatabaseDriver,
        transport: AxDataTransport,
        probe: impl Into<String>,
        started: Instant,
        pool: Option<AxDatabasePoolHealth>,
    ) -> Self {
        Self {
            ok: true,
            driver: driver.as_str().to_string(),
            transport: transport.as_str().to_string(),
            probe: probe.into(),
            latency_ms: duration_millis(started.elapsed()),
            pool,
            metrics: AxDatabaseMetricsSnapshot::default(),
        }
    }
}

#[derive(Debug, Default)]
struct AxDatabaseMetrics {
    reads: AtomicU64,
    writes: AtomicU64,
    failures: AtomicU64,
    timeouts: AtomicU64,
    connection_failures: AtomicU64,
    total_duration_us: AtomicU64,
    last_duration_us: AtomicU64,
}

impl AxDatabaseMetrics {
    fn observe<T>(
        &self,
        write: bool,
        operation: impl FnOnce() -> AxRuntimeResult<T>,
    ) -> AxRuntimeResult<T> {
        let started = Instant::now();
        let result = operation();
        let elapsed_us = duration_micros(started.elapsed());

        if write {
            self.writes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.reads.fetch_add(1, Ordering::Relaxed);
        }
        self.total_duration_us
            .fetch_add(elapsed_us, Ordering::Relaxed);
        self.last_duration_us.store(elapsed_us, Ordering::Relaxed);

        if let Err(error) = &result {
            self.failures.fetch_add(1, Ordering::Relaxed);
            if let AxRuntimeError::Database { error } = error {
                match error.code.as_str() {
                    "db.timeout" => {
                        self.timeouts.fetch_add(1, Ordering::Relaxed);
                    }
                    "db.connection_failed" => {
                        self.connection_failures.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }

        result
    }

    fn snapshot(&self) -> AxDatabaseMetricsSnapshot {
        AxDatabaseMetricsSnapshot {
            reads: self.reads.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            connection_failures: self.connection_failures.load(Ordering::Relaxed),
            total_duration_us: self.total_duration_us.load(Ordering::Relaxed),
            last_duration_us: self.last_duration_us.load(Ordering::Relaxed),
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl AxDatabaseConfig {
    pub fn sql_dialect(&self) -> Option<AxSqlDialect> {
        self.driver.sql_dialect()
    }

    pub fn validate(&self) -> AxRuntimeResult<()> {
        match self.transport {
            AxDataTransport::Direct => {
                if matches!(self.driver, AxDatabaseDriver::Memory) {
                    return Ok(());
                }

                if self.url.is_none() {
                    return Err(AxRuntimeError::message(
                        "missing AX_SECRET_DB_URL for direct data transport",
                    ));
                }
            }
            AxDataTransport::Api => {
                if self.api_url.is_none() {
                    return Err(AxRuntimeError::message(
                        "missing AX_PUBLIC_DATA_API_URL for api data transport",
                    ));
                }

                if self.api_key.is_none() {
                    return Err(AxRuntimeError::message(
                        "missing AX_SECRET_DATA_API_KEY for api data transport",
                    ));
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxEnv {
    pub public: BTreeMap<String, String>,
    pub secret: BTreeMap<String, String>,
}

impl AxEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_env() -> Self {
        load_dotenv_file(".env");

        let mut env = Self::new();

        for (key, value) in std::env::vars() {
            if let Some(public_key) = key.strip_prefix("AX_PUBLIC_") {
                env.public.insert(normalize_env_key(public_key), value);
                continue;
            }

            if let Some(secret_key) = key.strip_prefix("AX_SECRET_") {
                env.secret.insert(normalize_env_key(secret_key), value);
                continue;
            }

            env.secret.insert(normalize_env_key(&key), value);
        }

        env
    }

    pub fn with_public(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.public.insert(key.into(), value.into());
        self
    }

    pub fn with_secret(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.secret.insert(key.into(), value.into());
        self
    }

    pub fn public(&self, key: &str) -> AxRuntimeResult<String> {
        self.public
            .get(key)
            .cloned()
            .ok_or_else(|| AxRuntimeError::message(format!("missing public env key `{key}`")))
    }

    pub fn secret(&self, key: &str) -> AxRuntimeResult<String> {
        self.secret
            .get(key)
            .or_else(|| self.secret.get(&normalize_env_key(key)))
            .cloned()
            .ok_or_else(|| AxRuntimeError::message(format!("missing secret env key `{key}`")))
    }

    pub fn value(&self, key: &str) -> AxRuntimeResult<String> {
        let normalized = normalize_env_key(key);
        self.public
            .get(&normalized)
            .or_else(|| self.secret.get(&normalized))
            .cloned()
            .ok_or_else(|| AxRuntimeError::message(format!("missing env key `{key}`")))
    }

    pub fn database_driver(&self) -> AxRuntimeResult<AxDatabaseDriver> {
        match self
            .secret
            .get("db_dialect")
            .or_else(|| self.secret.get("db_driver"))
            .or_else(|| self.secret.get("database_dialect"))
            .or_else(|| self.secret.get("database_driver"))
        {
            Some(driver) => AxDatabaseDriver::parse(driver),
            None => Ok(AxDatabaseDriver::Postgres),
        }
    }

    pub fn data_transport(&self) -> AxRuntimeResult<AxDataTransport> {
        match self
            .secret
            .get("db_transport")
            .or_else(|| self.secret.get("data_transport"))
            .or_else(|| self.secret.get("database_transport"))
        {
            Some(transport) => AxDataTransport::parse(transport),
            None => Ok(AxDataTransport::Direct),
        }
    }

    pub fn database_config(&self) -> AxRuntimeResult<AxDatabaseConfig> {
        Ok(AxDatabaseConfig {
            driver: self.database_driver()?,
            transport: self.data_transport()?,
            url: self
                .secret
                .get("db_url")
                .cloned()
                .or_else(|| self.secret.get("database_url").cloned()),
            api_url: self
                .public
                .get("data_api_url")
                .cloned()
                .or_else(|| self.public.get("supabase_url").cloned()),
            api_key: self
                .secret
                .get("data_api_key")
                .cloned()
                .or_else(|| self.secret.get("supabase_service_role_key").cloned()),
            pool_max_size: self.database_pool_max_size()?,
            pool_timeout_ms: self.database_pool_timeout_ms()?,
            policy: self.database_policy()?,
        })
    }

    pub fn database_policy(&self) -> AxRuntimeResult<AxDatabasePolicy> {
        let policy = AxDatabasePolicy {
            query_timeout_ms: parse_positive_env_value(
                self.secret
                    .get("db_query_timeout_ms")
                    .or_else(|| self.secret.get("database_query_timeout_ms")),
                "DB_QUERY_TIMEOUT_MS",
                DEFAULT_DB_QUERY_TIMEOUT_MS,
            )?,
            read_retry_attempts: parse_non_negative_env_value(
                self.secret
                    .get("db_read_retry_attempts")
                    .or_else(|| self.secret.get("database_read_retry_attempts")),
                "DB_READ_RETRY_ATTEMPTS",
                DEFAULT_DB_READ_RETRY_ATTEMPTS,
            )?,
            read_retry_backoff_ms: parse_positive_env_value(
                self.secret
                    .get("db_read_retry_backoff_ms")
                    .or_else(|| self.secret.get("database_read_retry_backoff_ms")),
                "DB_READ_RETRY_BACKOFF_MS",
                DEFAULT_DB_READ_RETRY_BACKOFF_MS,
            )?,
            sqlite_busy_timeout_ms: parse_positive_env_value(
                self.secret
                    .get("db_sqlite_busy_timeout_ms")
                    .or_else(|| self.secret.get("database_sqlite_busy_timeout_ms")),
                "DB_SQLITE_BUSY_TIMEOUT_MS",
                DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
            )?,
        };
        if policy.read_retry_attempts > MAX_DB_READ_RETRY_ATTEMPTS {
            return Err(AxRuntimeError::message(format!(
                "DB_READ_RETRY_ATTEMPTS must be between 0 and {MAX_DB_READ_RETRY_ATTEMPTS}, got `{}`",
                policy.read_retry_attempts
            )));
        }
        Ok(policy)
    }

    pub fn database_pool_max_size(&self) -> AxRuntimeResult<u32> {
        parse_positive_env_value(
            self.secret
                .get("db_pool_max_size")
                .or_else(|| self.secret.get("database_pool_max_size")),
            "DB_POOL_MAX_SIZE",
            DEFAULT_DB_POOL_MAX_SIZE,
        )
    }

    pub fn database_pool_timeout_ms(&self) -> AxRuntimeResult<u64> {
        parse_positive_env_value(
            self.secret
                .get("db_pool_timeout_ms")
                .or_else(|| self.secret.get("database_pool_timeout_ms")),
            "DB_POOL_TIMEOUT_MS",
            DEFAULT_DB_POOL_TIMEOUT_MS,
        )
    }

    pub fn sql_dialect(&self) -> AxRuntimeResult<Option<AxSqlDialect>> {
        Ok(self.database_driver()?.sql_dialect())
    }
}

fn parse_positive_env_value<T>(value: Option<&String>, name: &str, default: T) -> AxRuntimeResult<T>
where
    T: Copy + From<u8> + std::str::FromStr + PartialOrd,
{
    let Some(value) = value else {
        return Ok(default);
    };
    let parsed = value.parse::<T>().map_err(|_| {
        AxRuntimeError::message(format!("{name} must be a positive integer, got `{value}`"))
    })?;
    if parsed < T::from(1) {
        return Err(AxRuntimeError::message(format!(
            "{name} must be a positive integer, got `{value}`"
        )));
    }
    Ok(parsed)
}

fn parse_non_negative_env_value<T>(
    value: Option<&String>,
    name: &str,
    default: T,
) -> AxRuntimeResult<T>
where
    T: Copy + std::str::FromStr,
{
    let Some(value) = value else {
        return Ok(default);
    };

    value.trim().parse::<T>().map_err(|_| {
        AxRuntimeError::message(format!(
            "{name} must be a non-negative integer; received `{value}`"
        ))
    })
}

fn normalize_env_key(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

fn load_dotenv_file(path: impl AsRef<Path>) {
    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if key.is_empty() {
            continue;
        }

        if std::env::var_os(key).is_none() {
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
}

pub trait AxRuntimeEnvAccess {
    fn env(&self) -> &AxEnv;
}

pub trait AxDatabaseAdapter: Send + Sync {
    fn driver(&self) -> AxDatabaseDriver;
    fn health(&self) -> AxRuntimeResult<AxDatabaseHealthReport>;
    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value>;
    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value>;
    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value>;
    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value>;
    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value>;
    fn transaction(&self, request: &AxTransactionRequest) -> AxRuntimeResult<Vec<Value>> {
        let resource = request
            .operations
            .first()
            .map(AxTransactionOperation::resource)
            .unwrap_or("transaction");
        Err(unsupported_transaction_error(self.driver(), resource))
    }
    fn migration_history(&self) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        Err(unsupported_migration_error(self.driver()))
    }
    fn apply_migration(&self, migration: &AxMigration) -> AxRuntimeResult<AxAppliedMigration> {
        let _ = migration;
        Err(unsupported_migration_error(self.driver()))
    }
    fn apply_migrations(
        &self,
        migrations: &[AxMigration],
    ) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        migrations
            .iter()
            .map(|migration| self.apply_migration(migration))
            .collect()
    }
    fn rollback_migration(&self, migration: &AxMigration) -> AxRuntimeResult<()> {
        let _ = migration;
        Err(unsupported_migration_error(self.driver()))
    }
}

impl<T> AxDatabaseAdapter for Box<T>
where
    T: AxDatabaseAdapter + ?Sized,
{
    fn driver(&self) -> AxDatabaseDriver {
        (**self).driver()
    }

    fn health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        (**self).health()
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        (**self).load(request)
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        (**self).query(request)
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        (**self).insert(request)
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        (**self).update(request)
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        (**self).delete(request)
    }

    fn transaction(&self, request: &AxTransactionRequest) -> AxRuntimeResult<Vec<Value>> {
        (**self).transaction(request)
    }

    fn migration_history(&self) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        (**self).migration_history()
    }

    fn apply_migration(&self, migration: &AxMigration) -> AxRuntimeResult<AxAppliedMigration> {
        (**self).apply_migration(migration)
    }

    fn apply_migrations(
        &self,
        migrations: &[AxMigration],
    ) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        (**self).apply_migrations(migrations)
    }

    fn rollback_migration(&self, migration: &AxMigration) -> AxRuntimeResult<()> {
        (**self).rollback_migration(migration)
    }
}

pub trait AxQueryExecutor {
    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value>;
    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value>;
    fn database_health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        Err(AxRuntimeError::message(
            "database health is unavailable for this backend runtime",
        ))
    }
}

pub trait AxMutationExecutor {
    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value>;
    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value>;
    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value>;
    fn transaction(&self, request: &AxTransactionRequest) -> AxRuntimeResult<Vec<Value>> {
        let resource = request
            .operations
            .first()
            .map(AxTransactionOperation::resource)
            .unwrap_or("transaction");
        Err(AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::UnsupportedOperation)
                .with_resource(resource)
                .with_detail("this backend runtime does not support atomic transactions"),
        ))
    }
}

pub trait AxMigrationExecutor {
    fn migration_history(&self) -> AxRuntimeResult<Vec<AxAppliedMigration>>;
    fn apply_migration(&self, migration: &AxMigration) -> AxRuntimeResult<AxAppliedMigration>;
    fn apply_migrations(
        &self,
        migrations: &[AxMigration],
    ) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        migrations
            .iter()
            .map(|migration| self.apply_migration(migration))
            .collect()
    }
    fn rollback_migration(&self, migration: &AxMigration) -> AxRuntimeResult<()>;
}

pub trait AxRevalidator {
    fn revalidate(&self, target: &str) -> AxRuntimeResult<()>;
}

pub trait AxMessenger {
    fn send(&self, request: &AxSendRequest) -> AxRuntimeResult<()>;
}

pub trait AxBackendRuntime:
    AxQueryExecutor + AxMutationExecutor + AxRevalidator + AxMessenger + AxRuntimeEnvAccess
{
}

impl<T> AxBackendRuntime for T where
    T: AxQueryExecutor + AxMutationExecutor + AxRevalidator + AxMessenger + AxRuntimeEnvAccess
{
}

pub struct AxDatabaseRuntime<A> {
    env: AxEnv,
    adapter: A,
    metrics: AxDatabaseMetrics,
}

impl<A> AxDatabaseRuntime<A> {
    pub fn new(env: AxEnv, adapter: A) -> Self {
        Self {
            env,
            adapter,
            metrics: AxDatabaseMetrics::default(),
        }
    }
}

impl<A> AxRuntimeEnvAccess for AxDatabaseRuntime<A> {
    fn env(&self) -> &AxEnv {
        &self.env
    }
}

impl<A> AxQueryExecutor for AxDatabaseRuntime<A>
where
    A: AxDatabaseAdapter,
{
    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        self.metrics.observe(false, || self.adapter.load(request))
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        self.metrics.observe(false, || self.adapter.query(request))
    }

    fn database_health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        self.env.database_config()?.validate()?;
        let mut report = self.adapter.health()?;
        report.metrics = self.metrics.snapshot();
        Ok(report)
    }
}

impl<A> AxMutationExecutor for AxDatabaseRuntime<A>
where
    A: AxDatabaseAdapter,
{
    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        self.metrics.observe(true, || self.adapter.insert(request))
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        self.metrics.observe(true, || self.adapter.update(request))
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        self.metrics.observe(true, || self.adapter.delete(request))
    }

    fn transaction(&self, request: &AxTransactionRequest) -> AxRuntimeResult<Vec<Value>> {
        self.metrics
            .observe(true, || self.adapter.transaction(request))
    }
}

impl<A> AxMigrationExecutor for AxDatabaseRuntime<A>
where
    A: AxDatabaseAdapter,
{
    fn migration_history(&self) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        self.adapter.migration_history()
    }

    fn apply_migration(&self, migration: &AxMigration) -> AxRuntimeResult<AxAppliedMigration> {
        self.adapter.apply_migration(migration)
    }

    fn apply_migrations(
        &self,
        migrations: &[AxMigration],
    ) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        self.adapter.apply_migrations(migrations)
    }

    fn rollback_migration(&self, migration: &AxMigration) -> AxRuntimeResult<()> {
        self.adapter.rollback_migration(migration)
    }
}

impl<A> AxRevalidator for AxDatabaseRuntime<A> {
    fn revalidate(&self, _target: &str) -> AxRuntimeResult<()> {
        Ok(())
    }
}

impl<A> AxMessenger for AxDatabaseRuntime<A> {
    fn send(&self, _request: &AxSendRequest) -> AxRuntimeResult<()> {
        Ok(())
    }
}

type AxPostgresManager = PostgresConnectionManager<MakeRustlsConnect>;
type AxPostgresPool = Pool<AxPostgresManager>;
type AxPooledPostgresConnection = PooledConnection<AxPostgresManager>;

#[derive(Debug)]
struct AxPostgresConnectionCustomizer {
    query_timeout_ms: u64,
}

impl r2d2::CustomizeConnection<Client, postgres::Error> for AxPostgresConnectionCustomizer {
    fn on_acquire(&self, client: &mut Client) -> Result<(), postgres::Error> {
        client.batch_execute(&format!(
            "set statement_timeout = {}",
            self.query_timeout_ms
        ))
    }
}

#[derive(Clone)]
pub struct PostgresAdapter {
    pub url: Option<String>,
    pub transport: AxDataTransport,
    pub api_url: Option<String>,
    pub pool_max_size: u32,
    pub pool_timeout_ms: u64,
    pub policy: AxDatabasePolicy,
    pool: Arc<OnceLock<AxPostgresPool>>,
}

impl std::fmt::Debug for PostgresAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresAdapter")
            .field("url", &self.url)
            .field("transport", &self.transport)
            .field("api_url", &self.api_url)
            .field("pool_max_size", &self.pool_max_size)
            .field("pool_timeout_ms", &self.pool_timeout_ms)
            .field("policy", &self.policy)
            .field("pool_initialized", &self.pool.get().is_some())
            .finish()
    }
}

impl PostgresAdapter {
    pub fn new(
        url: Option<String>,
        transport: AxDataTransport,
        api_url: Option<String>,
        pool_max_size: u32,
        pool_timeout_ms: u64,
        policy: AxDatabasePolicy,
    ) -> Self {
        Self {
            url,
            transport,
            api_url,
            pool_max_size,
            pool_timeout_ms,
            policy,
            pool: Arc::new(OnceLock::new()),
        }
    }

    fn direct_pool(&self, resource: &str) -> AxRuntimeResult<Option<&AxPostgresPool>> {
        if self.transport != AxDataTransport::Direct {
            return Ok(None);
        }

        if self.pool.get().is_none() {
            let pool = postgres_create_pool(
                &self.url,
                resource,
                self.pool_max_size,
                self.pool_timeout_ms,
                self.policy.query_timeout_ms,
            )?;
            let _ = self.pool.set(pool);
        }

        self.pool.get().map(Some).ok_or_else(|| {
            AxRuntimeError::database(
                AxDbError::new(AxDbErrorCode::ConnectionFailed)
                    .with_driver(AxDatabaseDriver::Postgres)
                    .with_resource(resource)
                    .with_detail("postgres pool failed to initialize"),
            )
        })
    }
}

impl PartialEq for PostgresAdapter {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url
            && self.transport == other.transport
            && self.api_url == other.api_url
            && self.pool_max_size == other.pool_max_size
            && self.pool_timeout_ms == other.pool_timeout_ms
            && self.policy == other.policy
    }
}

impl Eq for PostgresAdapter {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MySqlAdapter {
    pub url: Option<String>,
    pub transport: AxDataTransport,
    pub api_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteAdapter {
    pub url: Option<String>,
    pub transport: AxDataTransport,
    pub api_url: Option<String>,
    pub policy: AxDatabasePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryAdapter;

impl AxDatabaseAdapter for PostgresAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::Postgres
    }

    fn health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        let started = Instant::now();
        if self.transport == AxDataTransport::Api {
            return Ok(AxDatabaseHealthReport::ready(
                self.driver(),
                self.transport,
                "configuration",
                started,
                None,
            ));
        }

        let pool = self
            .direct_pool("_axonyx_health")?
            .ok_or_else(|| database_health_error(self.driver(), "database pool is unavailable"))?;
        let mut connection = pool.get().map_err(|error| {
            database_health_error(
                self.driver(),
                format!("database pool checkout failed: {error}"),
            )
        })?;
        connection
            .simple_query("select 1")
            .map_err(|error| postgres_runtime_error("_axonyx_health", error))?;
        drop(connection);
        let state = pool.state();

        Ok(AxDatabaseHealthReport::ready(
            self.driver(),
            self.transport,
            "query",
            started,
            Some(AxDatabasePoolHealth {
                max_size: pool.max_size(),
                connections: state.connections,
                idle_connections: state.idle_connections,
            }),
        ))
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        retry_database_read(self.policy, || {
            dispatch_load(
                self.driver(),
                self.transport,
                &self.url,
                &self.api_url,
                self.direct_pool(&request.collection)?,
                request,
                DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
            )
        })
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        retry_database_read(self.policy, || {
            dispatch_raw_query(
                self.driver(),
                self.transport,
                &self.url,
                &self.api_url,
                self.direct_pool("raw_sql")?,
                request,
                DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
            )
        })
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        dispatch_insert(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            self.direct_pool(&request.collection)?,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        dispatch_update(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            self.direct_pool(&request.collection)?,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        dispatch_delete(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            self.direct_pool(&request.collection)?,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn transaction(&self, request: &AxTransactionRequest) -> AxRuntimeResult<Vec<Value>> {
        let resource = transaction_resource(request)?;
        dispatch_transaction(
            self.driver(),
            self.transport,
            &self.url,
            self.direct_pool(resource)?,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn migration_history(&self) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        postgres_migration_history(self.direct_pool("_axonyx_migrations")?, &self.url)
    }

    fn apply_migration(&self, migration: &AxMigration) -> AxRuntimeResult<AxAppliedMigration> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        postgres_apply_migration(
            self.direct_pool("_axonyx_migrations")?,
            &self.url,
            migration,
        )
    }

    fn apply_migrations(
        &self,
        migrations: &[AxMigration],
    ) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        postgres_apply_migrations(
            self.direct_pool("_axonyx_migrations")?,
            &self.url,
            migrations,
        )
    }

    fn rollback_migration(&self, migration: &AxMigration) -> AxRuntimeResult<()> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        postgres_rollback_migration(
            self.direct_pool("_axonyx_migrations")?,
            &self.url,
            migration,
        )
    }
}

impl AxDatabaseAdapter for MySqlAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::MySql
    }

    fn health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        let started = Instant::now();
        if self.transport == AxDataTransport::Api {
            return Ok(AxDatabaseHealthReport::ready(
                self.driver(),
                self.transport,
                "configuration",
                started,
                None,
            ));
        }

        Err(database_health_error(
            self.driver(),
            "direct MySQL health probes are not implemented",
        ))
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        dispatch_load(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        dispatch_raw_query(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        dispatch_insert(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        dispatch_update(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        dispatch_delete(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS,
        )
    }
}

impl AxDatabaseAdapter for SqliteAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::Sqlite
    }

    fn health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        let started = Instant::now();
        if self.transport == AxDataTransport::Api {
            return Ok(AxDatabaseHealthReport::ready(
                self.driver(),
                self.transport,
                "configuration",
                started,
                None,
            ));
        }

        let connection = sqlite_open_connection_with_timeout(
            &self.url,
            "_axonyx_health",
            self.policy.sqlite_busy_timeout_ms,
        )?;
        connection
            .query_row("select 1", [], |_| Ok(()))
            .map_err(|error| sqlite_runtime_error("_axonyx_health", error))?;

        Ok(AxDatabaseHealthReport::ready(
            self.driver(),
            self.transport,
            "query",
            started,
            None,
        ))
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        retry_database_read(self.policy, || {
            dispatch_load(
                self.driver(),
                self.transport,
                &self.url,
                &self.api_url,
                None,
                request,
                self.policy.sqlite_busy_timeout_ms,
            )
        })
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        retry_database_read(self.policy, || {
            dispatch_raw_query(
                self.driver(),
                self.transport,
                &self.url,
                &self.api_url,
                None,
                request,
                self.policy.sqlite_busy_timeout_ms,
            )
        })
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        dispatch_insert(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            self.policy.sqlite_busy_timeout_ms,
        )
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        dispatch_update(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            self.policy.sqlite_busy_timeout_ms,
        )
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        dispatch_delete(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            None,
            request,
            self.policy.sqlite_busy_timeout_ms,
        )
    }

    fn transaction(&self, request: &AxTransactionRequest) -> AxRuntimeResult<Vec<Value>> {
        dispatch_transaction(
            self.driver(),
            self.transport,
            &self.url,
            None,
            request,
            self.policy.sqlite_busy_timeout_ms,
        )
    }

    fn migration_history(&self) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        sqlite_migration_history(&self.url)
    }

    fn apply_migration(&self, migration: &AxMigration) -> AxRuntimeResult<AxAppliedMigration> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        sqlite_apply_migration(&self.url, migration)
    }

    fn apply_migrations(
        &self,
        migrations: &[AxMigration],
    ) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        sqlite_apply_migrations(&self.url, migrations)
    }

    fn rollback_migration(&self, migration: &AxMigration) -> AxRuntimeResult<()> {
        ensure_direct_migration_transport(self.driver(), self.transport)?;
        sqlite_rollback_migration(&self.url, migration)
    }
}

impl AxDatabaseAdapter for MemoryAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::Memory
    }

    fn health(&self) -> AxRuntimeResult<AxDatabaseHealthReport> {
        Ok(AxDatabaseHealthReport::ready(
            self.driver(),
            AxDataTransport::Direct,
            "memory",
            Instant::now(),
            None,
        ))
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        Ok(adapter_payload(
            self.driver(),
            AxDataTransport::Direct,
            &None,
            &None,
            request.collection.clone(),
            json!({
                "filters": request.filters,
                "orders": request.orders,
                "limit": request.limit,
                "offset": request.offset,
            }),
        ))
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        Ok(json!({
            "driver": self.driver().as_str(),
            "transport": "direct",
            "action": "query",
            "sql": request.sql,
            "params": request.params,
        }))
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        Ok(mutation_payload(
            self.driver(),
            AxDataTransport::Direct,
            &None,
            &None,
            "insert",
            &request.collection,
            &request.fields,
        ))
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        Ok(mutation_payload(
            self.driver(),
            AxDataTransport::Direct,
            &None,
            &None,
            "update",
            &request.collection,
            &request.fields,
        ))
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        Ok(json!({
            "driver": self.driver().as_str(),
            "transport": "direct",
            "action": "delete",
            "collection": request.collection,
            "filters": request.filters,
        }))
    }
}

pub fn adapter_from_config(config: &AxDatabaseConfig) -> Box<dyn AxDatabaseAdapter> {
    match config.driver {
        AxDatabaseDriver::Postgres => Box::new(PostgresAdapter::new(
            config.url.clone(),
            config.transport,
            config.api_url.clone(),
            config.pool_max_size,
            config.pool_timeout_ms,
            config.policy,
        )),
        AxDatabaseDriver::MySql => Box::new(MySqlAdapter {
            url: config.url.clone(),
            transport: config.transport,
            api_url: config.api_url.clone(),
        }),
        AxDatabaseDriver::Sqlite => Box::new(SqliteAdapter {
            url: config.url.clone(),
            transport: config.transport,
            api_url: config.api_url.clone(),
            policy: config.policy,
        }),
        AxDatabaseDriver::Memory => Box::new(MemoryAdapter),
    }
}

pub fn runtime_from_env(
    env: AxEnv,
) -> AxRuntimeResult<AxDatabaseRuntime<Box<dyn AxDatabaseAdapter>>> {
    let config = env.database_config()?;
    config.validate()?;
    let adapter = adapter_from_config(&config);
    Ok(AxDatabaseRuntime::new(env, adapter))
}

pub fn lazy_runtime_from_env(
    env: AxEnv,
) -> AxRuntimeResult<AxDatabaseRuntime<Box<dyn AxDatabaseAdapter>>> {
    let config = env.database_config()?;
    let adapter = adapter_from_config(&config);
    Ok(AxDatabaseRuntime::new(env, adapter))
}

pub fn ok_payload() -> Value {
    json!({ "ok": true })
}

fn adapter_payload(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    collection: String,
    details: Value,
) -> Value {
    json!({
        "driver": driver.as_str(),
        "transport": transport.as_str(),
        "url": url,
        "api_url": api_url,
        "collection": collection,
        "details": details,
    })
}

fn mutation_payload(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    action: &str,
    collection: &str,
    fields: &BTreeMap<String, Value>,
) -> Value {
    json!({
        "driver": driver.as_str(),
        "transport": transport.as_str(),
        "url": url,
        "api_url": api_url,
        "action": action,
        "collection": collection,
        "fields": fields,
    })
}

fn retry_database_read<T>(
    policy: AxDatabasePolicy,
    mut operation: impl FnMut() -> AxRuntimeResult<T>,
) -> AxRuntimeResult<T> {
    let mut attempt = 0_u32;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error)
                if attempt < policy.read_retry_attempts
                    && is_retryable_database_read_error(&error) =>
            {
                attempt += 1;
                let delay_ms = policy
                    .read_retry_backoff_ms
                    .saturating_mul(u64::from(attempt));
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            Err(error) => return Err(error),
        }
    }
}

fn is_retryable_database_read_error(error: &AxRuntimeError) -> bool {
    matches!(
        error,
        AxRuntimeError::Database { error }
            if matches!(
                error.code.as_str(),
                "db.timeout" | "db.connection_failed"
            )
    )
}

fn dispatch_load(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxQueryRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => {
            direct_load_plan(driver, url, postgres_pool, request, sqlite_busy_timeout_ms)
        }
        AxDataTransport::Api => api_load_plan(driver, api_url, request),
    }
}

fn dispatch_raw_query(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxRawSqlRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => {
            direct_raw_query_plan(driver, url, postgres_pool, request, sqlite_busy_timeout_ms)
        }
        AxDataTransport::Api => api_raw_query_plan(driver, api_url, request),
    }
}

fn dispatch_insert(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxInsertRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => {
            direct_insert_plan(driver, url, postgres_pool, request, sqlite_busy_timeout_ms)
        }
        AxDataTransport::Api => api_mutation_plan(
            driver,
            api_url,
            "insert",
            &request.collection,
            &request.fields,
            &[],
        ),
    }
}

fn dispatch_update(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxUpdateRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => {
            direct_update_plan(driver, url, postgres_pool, request, sqlite_busy_timeout_ms)
        }
        AxDataTransport::Api => api_mutation_plan(
            driver,
            api_url,
            "update",
            &request.collection,
            &request.fields,
            &request.filters,
        ),
    }
}

fn dispatch_delete(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxDeleteRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => {
            direct_delete_plan(driver, url, postgres_pool, request, sqlite_busy_timeout_ms)
        }
        AxDataTransport::Api => api_delete_plan(driver, api_url, request),
    }
}

fn dispatch_transaction(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxTransactionRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Vec<Value>> {
    let resource = transaction_resource(request)?;
    if transport != AxDataTransport::Direct {
        return Err(unsupported_transaction_error(driver, resource));
    }

    let operations = prepare_transaction_operations(driver.clone(), request)?;
    match driver {
        AxDatabaseDriver::Sqlite => {
            sqlite_execute_transaction(url, &operations, sqlite_busy_timeout_ms)
        }
        AxDatabaseDriver::Postgres => {
            postgres_execute_transaction(postgres_pool, url, resource, &operations)
        }
        AxDatabaseDriver::MySql | AxDatabaseDriver::Memory => {
            Err(unsupported_transaction_error(driver, resource))
        }
    }
}

fn transaction_resource(request: &AxTransactionRequest) -> AxRuntimeResult<&str> {
    request
        .operations
        .first()
        .map(AxTransactionOperation::resource)
        .ok_or_else(|| {
            AxRuntimeError::database(
                AxDbError::new(AxDbErrorCode::InvalidQuery)
                    .with_resource("transaction")
                    .with_detail("database transaction requires at least one operation"),
            )
        })
}

fn unsupported_transaction_error(driver: AxDatabaseDriver, resource: &str) -> AxRuntimeError {
    AxRuntimeError::database(
        AxDbError::new(AxDbErrorCode::UnsupportedOperation)
            .with_driver(driver)
            .with_resource(resource)
            .with_detail("atomic transactions require the direct SQLite or Postgres transport"),
    )
}

fn direct_load_plan(
    driver: AxDatabaseDriver,
    url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxQueryRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    let Some(dialect) = driver.sql_dialect() else {
        return Ok(apply_query_mode(
            request.mode,
            adapter_payload(
                driver,
                AxDataTransport::Direct,
                url,
                &None,
                request.collection.clone(),
                json!({
                    "filters": request.filters,
                    "orders": request.orders,
                    "limit": request.limit,
                    "offset": request.offset,
                }),
            ),
        ));
    };

    let plan =
        compile_query_plan_to_sql(&query_request_to_plan(request), dialect).map_err(|error| {
            AxRuntimeError::database(AxDbError::invalid_query(
                driver.clone(),
                request.collection.clone(),
                error.to_string(),
            ))
        })?;
    let execution = AxDirectSqlPlan {
        dialect: dialect.name().to_string(),
        sql: plan.sql,
        params: sql_params_to_json(&plan.params),
        url: url.clone(),
    };

    match driver {
        AxDatabaseDriver::Sqlite => {
            return sqlite_execute_query(
                url,
                &request.collection,
                &execution.sql,
                &execution.params,
                sqlite_busy_timeout_ms,
            )
            .map(|value| apply_query_mode(request.mode, value));
        }
        AxDatabaseDriver::Postgres => {
            return postgres_execute_query(
                postgres_pool,
                url,
                &request.collection,
                &execution.sql,
                &execution.params,
            )
            .map(|value| apply_query_mode(request.mode, value));
        }
        AxDatabaseDriver::MySql | AxDatabaseDriver::Memory => {}
    }

    Ok(apply_query_mode(
        request.mode,
        json!({
            "driver": driver.as_str(),
            "transport": "direct",
            "execution": execution,
        }),
    ))
}

fn direct_raw_query_plan(
    driver: AxDatabaseDriver,
    url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxRawSqlRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    validate_raw_select_sql(&driver, &request.sql)?;

    let Some(dialect) = driver.sql_dialect() else {
        return Ok(json!({
            "driver": driver.as_str(),
            "transport": "direct",
            "action": "query",
            "sql": request.sql,
            "params": request.params,
        }));
    };

    let execution = AxDirectSqlPlan {
        dialect: dialect.name().to_string(),
        sql: request.sql.clone(),
        params: request.params.clone(),
        url: url.clone(),
    };

    match driver {
        AxDatabaseDriver::Sqlite => {
            return sqlite_execute_query(
                url,
                "raw_sql",
                &execution.sql,
                &execution.params,
                sqlite_busy_timeout_ms,
            );
        }
        AxDatabaseDriver::Postgres => {
            return postgres_execute_query(
                postgres_pool,
                url,
                "raw_sql",
                &execution.sql,
                &execution.params,
            );
        }
        AxDatabaseDriver::MySql | AxDatabaseDriver::Memory => {}
    }

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "direct",
        "action": "query",
        "execution": execution,
    }))
}

fn apply_query_mode(mode: AxQueryMode, value: Value) -> Value {
    match mode {
        AxQueryMode::Many => value,
        AxQueryMode::First => match value {
            Value::Array(items) => items.into_iter().next().unwrap_or(Value::Null),
            other => other,
        },
    }
}

fn direct_insert_plan(
    driver: AxDatabaseDriver,
    url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxInsertRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    let Some(dialect) = driver.sql_dialect() else {
        return Ok(mutation_payload(
            driver,
            AxDataTransport::Direct,
            url,
            &None,
            "insert",
            &request.collection,
            &request.fields,
        ));
    };

    let fields = fields_to_assignment_plans(&request.fields);
    let plan =
        compile_insert_plan_to_sql(&request.collection, &fields, dialect).map_err(|error| {
            AxRuntimeError::database(AxDbError::invalid_query(
                driver.clone(),
                request.collection.clone(),
                error.to_string(),
            ))
        })?;
    let execution = AxDirectSqlPlan {
        dialect: dialect.name().to_string(),
        sql: plan.sql,
        params: sql_params_to_json(&plan.params),
        url: url.clone(),
    };

    match driver {
        AxDatabaseDriver::Sqlite => {
            return sqlite_execute_mutation(
                url,
                "insert",
                &request.collection,
                &execution.sql,
                &execution.params,
                sqlite_busy_timeout_ms,
            );
        }
        AxDatabaseDriver::Postgres => {
            return postgres_execute_mutation(
                postgres_pool,
                url,
                "insert",
                &request.collection,
                &execution.sql,
                &execution.params,
            );
        }
        AxDatabaseDriver::MySql | AxDatabaseDriver::Memory => {}
    }

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "direct",
        "action": "insert",
        "execution": execution,
    }))
}

fn direct_update_plan(
    driver: AxDatabaseDriver,
    url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxUpdateRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    let Some(dialect) = driver.sql_dialect() else {
        return Ok(mutation_payload(
            driver,
            AxDataTransport::Direct,
            url,
            &None,
            "update",
            &request.collection,
            &request.fields,
        ));
    };

    let fields = fields_to_assignment_plans(&request.fields);
    let filters = query_filters_to_plan(&request.filters);
    let plan = compile_update_plan_to_sql(&request.collection, &fields, &filters, dialect)
        .map_err(|error| {
            AxRuntimeError::database(AxDbError::invalid_query(
                driver.clone(),
                request.collection.clone(),
                error.to_string(),
            ))
        })?;
    let execution = AxDirectSqlPlan {
        dialect: dialect.name().to_string(),
        sql: plan.sql,
        params: sql_params_to_json(&plan.params),
        url: url.clone(),
    };

    match driver {
        AxDatabaseDriver::Sqlite => {
            return sqlite_execute_mutation(
                url,
                "update",
                &request.collection,
                &execution.sql,
                &execution.params,
                sqlite_busy_timeout_ms,
            );
        }
        AxDatabaseDriver::Postgres => {
            return postgres_execute_mutation(
                postgres_pool,
                url,
                "update",
                &request.collection,
                &execution.sql,
                &execution.params,
            );
        }
        AxDatabaseDriver::MySql | AxDatabaseDriver::Memory => {}
    }

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "direct",
        "action": "update",
        "execution": execution,
    }))
}

fn direct_delete_plan(
    driver: AxDatabaseDriver,
    url: &Option<String>,
    postgres_pool: Option<&AxPostgresPool>,
    request: &AxDeleteRequest,
    sqlite_busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    let Some(dialect) = driver.sql_dialect() else {
        return Ok(json!({
            "driver": driver.as_str(),
            "transport": "direct",
            "action": "delete",
            "collection": request.collection,
            "filters": request.filters,
        }));
    };

    let filters = query_filters_to_plan(&request.filters);
    let plan =
        compile_delete_plan_to_sql(&request.collection, &filters, dialect).map_err(|error| {
            AxRuntimeError::database(AxDbError::invalid_query(
                driver.clone(),
                request.collection.clone(),
                error.to_string(),
            ))
        })?;
    let execution = AxDirectSqlPlan {
        dialect: dialect.name().to_string(),
        sql: plan.sql,
        params: sql_params_to_json(&plan.params),
        url: url.clone(),
    };

    match driver {
        AxDatabaseDriver::Sqlite => {
            return sqlite_execute_mutation(
                url,
                "delete",
                &request.collection,
                &execution.sql,
                &execution.params,
                sqlite_busy_timeout_ms,
            );
        }
        AxDatabaseDriver::Postgres => {
            return postgres_execute_mutation(
                postgres_pool,
                url,
                "delete",
                &request.collection,
                &execution.sql,
                &execution.params,
            );
        }
        AxDatabaseDriver::MySql | AxDatabaseDriver::Memory => {}
    }

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "direct",
        "action": "delete",
        "execution": execution,
    }))
}

#[derive(Debug, Clone, PartialEq)]
struct AxPreparedTransactionOperation {
    action: &'static str,
    resource: String,
    sql: String,
    params: Vec<Value>,
}

fn prepare_transaction_operations(
    driver: AxDatabaseDriver,
    request: &AxTransactionRequest,
) -> AxRuntimeResult<Vec<AxPreparedTransactionOperation>> {
    let Some(dialect) = driver.sql_dialect() else {
        return Err(unsupported_transaction_error(
            driver,
            transaction_resource(request)?,
        ));
    };

    request
        .operations
        .iter()
        .map(|operation| match operation {
            AxTransactionOperation::Insert(request) => {
                let fields = fields_to_assignment_plans(&request.fields);
                let plan = compile_insert_plan_to_sql(&request.collection, &fields, dialect)
                    .map_err(|error| {
                        AxRuntimeError::database(AxDbError::invalid_query(
                            driver.clone(),
                            request.collection.clone(),
                            error.to_string(),
                        ))
                    })?;
                Ok(AxPreparedTransactionOperation {
                    action: "insert",
                    resource: request.collection.clone(),
                    sql: plan.sql,
                    params: sql_params_to_json(&plan.params),
                })
            }
            AxTransactionOperation::Update(request) => {
                let fields = fields_to_assignment_plans(&request.fields);
                let filters = query_filters_to_plan(&request.filters);
                let plan =
                    compile_update_plan_to_sql(&request.collection, &fields, &filters, dialect)
                        .map_err(|error| {
                            AxRuntimeError::database(AxDbError::invalid_query(
                                driver.clone(),
                                request.collection.clone(),
                                error.to_string(),
                            ))
                        })?;
                Ok(AxPreparedTransactionOperation {
                    action: "update",
                    resource: request.collection.clone(),
                    sql: plan.sql,
                    params: sql_params_to_json(&plan.params),
                })
            }
            AxTransactionOperation::Delete(request) => {
                let filters = query_filters_to_plan(&request.filters);
                let plan = compile_delete_plan_to_sql(&request.collection, &filters, dialect)
                    .map_err(|error| {
                        AxRuntimeError::database(AxDbError::invalid_query(
                            driver.clone(),
                            request.collection.clone(),
                            error.to_string(),
                        ))
                    })?;
                Ok(AxPreparedTransactionOperation {
                    action: "delete",
                    resource: request.collection.clone(),
                    sql: plan.sql,
                    params: sql_params_to_json(&plan.params),
                })
            }
        })
        .collect()
}

fn sql_params_to_json(params: &[AxSqlParam]) -> Vec<Value> {
    params
        .iter()
        .map(|param| rust_expr_code_to_json(&param.value.code))
        .collect()
}

fn rust_expr_code_to_json(code: &str) -> Value {
    serde_json::from_str(code.trim()).unwrap_or_else(|_| json!(code.trim()))
}

fn sqlite_execute_query(
    url: &Option<String>,
    resource: &str,
    sql: &str,
    params: &[Value],
    busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    let connection = sqlite_open_connection_with_timeout(url, resource, busy_timeout_ms)?;
    let sqlite_params = json_params_to_sqlite(params)?;
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| sqlite_runtime_error(resource, error))?;
    let column_names = statement
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let rows = statement
        .query_map(rusqlite::params_from_iter(sqlite_params), |row| {
            sqlite_row_to_json(row, &column_names)
        })
        .map_err(|error| sqlite_runtime_error(resource, error))?;

    let mut items = Vec::new();
    for row in rows {
        items.push(row.map_err(|error| sqlite_runtime_error(resource, error))?);
    }

    Ok(Value::Array(items))
}

fn sqlite_execute_mutation(
    url: &Option<String>,
    action: &str,
    resource: &str,
    sql: &str,
    params: &[Value],
    busy_timeout_ms: u64,
) -> AxRuntimeResult<Value> {
    let connection = sqlite_open_connection_with_timeout(url, resource, busy_timeout_ms)?;
    let sqlite_params = json_params_to_sqlite(params)?;
    let changes = connection
        .execute(sql, rusqlite::params_from_iter(sqlite_params))
        .map_err(|error| sqlite_runtime_error(resource, error))?;

    Ok(json!({
        "ok": true,
        "driver": "sqlite",
        "action": action,
        "resource": resource,
        "changes": changes,
        "last_insert_rowid": connection.last_insert_rowid(),
    }))
}

fn sqlite_execute_transaction(
    url: &Option<String>,
    operations: &[AxPreparedTransactionOperation],
    busy_timeout_ms: u64,
) -> AxRuntimeResult<Vec<Value>> {
    let resource = operations
        .first()
        .map(|operation| operation.resource.as_str())
        .unwrap_or("transaction");
    let mut connection = sqlite_open_connection_with_timeout(url, resource, busy_timeout_ms)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| sqlite_runtime_error(resource, error))?;
    let mut results = Vec::with_capacity(operations.len());

    for operation in operations {
        let params = json_params_to_sqlite(&operation.params)?;
        let changes = transaction
            .execute(&operation.sql, rusqlite::params_from_iter(params))
            .map_err(|error| sqlite_runtime_error(&operation.resource, error))?;
        results.push(json!({
            "ok": true,
            "driver": "sqlite",
            "action": operation.action,
            "resource": operation.resource,
            "changes": changes,
            "last_insert_rowid": transaction.last_insert_rowid(),
        }));
    }

    transaction
        .commit()
        .map_err(|error| sqlite_runtime_error(resource, error))?;
    Ok(results)
}

const AXONYX_MIGRATIONS_RESOURCE: &str = "_axonyx_migrations";
const AXONYX_POSTGRES_MIGRATION_LOCK_KEY: i64 = 0x4158_4f4e_5958_4d47;

fn ensure_direct_migration_transport(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
) -> AxRuntimeResult<()> {
    if transport == AxDataTransport::Direct {
        return Ok(());
    }
    Err(unsupported_migration_error(driver))
}

fn unsupported_migration_error(driver: AxDatabaseDriver) -> AxRuntimeError {
    AxRuntimeError::database(
        AxDbError::new(AxDbErrorCode::UnsupportedOperation)
            .with_driver(driver)
            .with_resource(AXONYX_MIGRATIONS_RESOURCE)
            .with_detail("database migrations require direct SQLite or Postgres transport"),
    )
}

fn validate_migration(migration: &AxMigration) -> AxRuntimeResult<()> {
    let valid_version = !migration.version.is_empty()
        && migration
            .version
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '_' | '-'));
    let valid_name = !migration.name.is_empty()
        && migration
            .name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'));
    let valid_checksum = migration.checksum.len() == 64
        && migration.checksum.chars().all(|ch| ch.is_ascii_hexdigit());

    if !valid_version || !valid_name || !valid_checksum {
        return Err(migration_error(
            AxDbErrorCode::InvalidQuery,
            &migration.version,
            "migration version, name, or SHA-256 checksum is invalid",
        ));
    }
    if !migration_sql_has_executable_statement(&migration.up_sql)
        || !migration_sql_has_executable_statement(&migration.down_sql)
    {
        return Err(migration_error(
            AxDbErrorCode::InvalidQuery,
            &migration.version,
            "migration requires non-empty up.sql and down.sql",
        ));
    }
    reject_migration_transaction_control(&migration.version, &migration.up_sql)?;
    reject_migration_transaction_control(&migration.version, &migration.down_sql)
}

fn migration_sql_has_executable_statement(sql: &str) -> bool {
    sql.lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .any(|line| !line.trim().is_empty())
}

fn reject_migration_transaction_control(version: &str, sql: &str) -> AxRuntimeResult<()> {
    for keywords in migration_statement_prefixes(sql) {
        let first = keywords.first().map(String::as_str).unwrap_or_default();
        let second = keywords.get(1).map(String::as_str).unwrap_or_default();
        if matches!(
            first,
            "begin" | "commit" | "rollback" | "savepoint" | "release" | "abort"
        ) || matches!(
            (first, second),
            ("start", "transaction") | ("set", "transaction")
        ) {
            return Err(migration_error(
                AxDbErrorCode::InvalidQuery,
                version,
                "migration SQL must not manage transactions directly",
            ));
        }
    }
    Ok(())
}

fn migration_statement_prefixes(sql: &str) -> Vec<Vec<String>> {
    let bytes = sql.as_bytes();
    let mut statements = Vec::new();
    let mut keywords = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"--") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index..].starts_with(b"/*") {
            index += 2;
            let mut depth = 1_u32;
            while index < bytes.len() && depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            continue;
        }
        if matches!(bytes[index], b'\'' | b'"') {
            let quote = bytes[index];
            index += 1;
            while index < bytes.len() {
                if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 2;
                    } else {
                        index += 1;
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            continue;
        }
        if bytes[index] == b'$' {
            let tag_end = bytes[index + 1..]
                .iter()
                .position(|byte| *byte == b'$')
                .map(|offset| index + 1 + offset);
            if let Some(tag_end) = tag_end.filter(|tag_end| {
                bytes[index + 1..*tag_end]
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            }) {
                let delimiter = &sql[index..=tag_end];
                let body_start = tag_end + 1;
                if let Some(close) = sql[body_start..].find(delimiter) {
                    index = body_start + close + delimiter.len();
                    continue;
                }
            }
        }
        if bytes[index] == b';' {
            if !keywords.is_empty() {
                statements.push(std::mem::take(&mut keywords));
            }
            index += 1;
            continue;
        }
        if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            if keywords.len() < 2 {
                keywords.push(sql[start..index].to_ascii_lowercase());
            }
            continue;
        }
        index += 1;
    }

    if !keywords.is_empty() {
        statements.push(keywords);
    }
    statements
}

fn migration_error(
    code: AxDbErrorCode,
    version: &str,
    detail: impl Into<String>,
) -> AxRuntimeError {
    AxRuntimeError::database(
        AxDbError::new(code)
            .with_resource(AXONYX_MIGRATIONS_RESOURCE)
            .with_field(version)
            .with_detail(detail),
    )
}

fn sqlite_ensure_migrations_table(
    connection: &rusqlite::Connection,
) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        r#"
create table if not exists _axonyx_migrations (
  version text primary key,
  name text not null,
  checksum text not null,
  applied_at text not null default current_timestamp,
  execution_ms integer not null
)
"#,
    )
}

fn sqlite_migrations_table_exists(
    connection: &rusqlite::Connection,
) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "select exists(select 1 from sqlite_master where type = 'table' and name = '_axonyx_migrations')",
        [],
        |row| row.get(0),
    )
}

fn sqlite_migration_history(url: &Option<String>) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
    ensure_persistent_sqlite_migration_url(url)?;
    let connection = sqlite_open_connection(url, AXONYX_MIGRATIONS_RESOURCE)?;
    if !sqlite_migrations_table_exists(&connection)
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?
    {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "select version, name, checksum, applied_at, execution_ms from _axonyx_migrations order by version asc",
        )
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    let rows = statement
        .query_map([], |row| {
            Ok(AxAppliedMigration {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
                applied_at: row.get(3)?,
                execution_ms: row.get::<_, u64>(4)?,
            })
        })
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))
}

fn sqlite_apply_migration(
    url: &Option<String>,
    migration: &AxMigration,
) -> AxRuntimeResult<AxAppliedMigration> {
    sqlite_apply_migrations_locked(url, std::slice::from_ref(migration), false)?
        .pop()
        .ok_or_else(|| {
            migration_error(
                AxDbErrorCode::MigrationConflict,
                &migration.version,
                "migration was already applied",
            )
        })
}

fn sqlite_apply_migrations(
    url: &Option<String>,
    migrations: &[AxMigration],
) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
    sqlite_apply_migrations_locked(url, migrations, true)
}

fn sqlite_apply_migrations_locked(
    url: &Option<String>,
    migrations: &[AxMigration],
    skip_applied: bool,
) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
    for migration in migrations {
        validate_migration(migration)?;
    }
    if migrations.is_empty() {
        return Ok(Vec::new());
    }
    ensure_persistent_sqlite_migration_url(url)?;
    let mut connection = sqlite_open_connection(url, AXONYX_MIGRATIONS_RESOURCE)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    sqlite_ensure_migrations_table(&transaction)
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    let mut applied = Vec::with_capacity(migrations.len());

    for migration in migrations {
        let existing = transaction
            .query_row(
                "select checksum from _axonyx_migrations where version = ?1",
                [&migration.version],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        if let Some(checksum) = existing {
            if checksum != migration.checksum {
                return Err(migration_error(
                    AxDbErrorCode::MigrationChecksumMismatch,
                    &migration.version,
                    "migration version is already present with a different checksum",
                ));
            }
            if skip_applied {
                continue;
            }
            return Err(migration_error(
                AxDbErrorCode::MigrationConflict,
                &migration.version,
                "migration version is already present in database history",
            ));
        }

        let started = Instant::now();
        transaction
            .execute_batch(&migration.up_sql)
            .map_err(|error| sqlite_runtime_error(&migration.version, error))?;
        let execution_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        transaction
            .execute(
                "insert into _axonyx_migrations (version, name, checksum, execution_ms) values (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    migration.version,
                    migration.name,
                    migration.checksum,
                    execution_ms
                ],
            )
            .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        let applied_at = transaction
            .query_row(
                "select applied_at from _axonyx_migrations where version = ?1",
                [&migration.version],
                |row| row.get::<_, String>(0),
            )
            .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        applied.push(AxAppliedMigration {
            version: migration.version.clone(),
            name: migration.name.clone(),
            checksum: migration.checksum.clone(),
            applied_at,
            execution_ms,
        });
    }

    transaction
        .commit()
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    Ok(applied)
}

fn sqlite_rollback_migration(url: &Option<String>, migration: &AxMigration) -> AxRuntimeResult<()> {
    validate_migration(migration)?;
    ensure_persistent_sqlite_migration_url(url)?;
    let mut connection = sqlite_open_connection(url, AXONYX_MIGRATIONS_RESOURCE)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    if !sqlite_migrations_table_exists(&transaction)
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?
    {
        return Err(migration_error(
            AxDbErrorCode::MigrationConflict,
            &migration.version,
            "database migration history is empty",
        ));
    }
    let latest = transaction
        .query_row(
            "select version, checksum from _axonyx_migrations order by version desc limit 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    validate_rollback_target(
        migration,
        latest.as_ref().map(|(v, c)| (v.as_str(), c.as_str())),
    )?;

    transaction
        .execute_batch(&migration.down_sql)
        .map_err(|error| sqlite_runtime_error(&migration.version, error))?;
    transaction
        .execute(
            "delete from _axonyx_migrations where version = ?1",
            [&migration.version],
        )
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
    transaction
        .commit()
        .map_err(|error| sqlite_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))
}

fn ensure_persistent_sqlite_migration_url(url: &Option<String>) -> AxRuntimeResult<()> {
    if url
        .as_deref()
        .is_some_and(|url| sqlite_database_path(url) == ":memory:")
    {
        return Err(migration_error(
            AxDbErrorCode::InvalidQuery,
            AXONYX_MIGRATIONS_RESOURCE,
            "database migrations require a persistent SQLite file",
        ));
    }
    Ok(())
}

fn validate_rollback_target(
    migration: &AxMigration,
    latest: Option<(&str, &str)>,
) -> AxRuntimeResult<()> {
    let Some((version, checksum)) = latest else {
        return Err(migration_error(
            AxDbErrorCode::MigrationConflict,
            &migration.version,
            "database migration history is empty",
        ));
    };
    if version != migration.version {
        return Err(migration_error(
            AxDbErrorCode::MigrationConflict,
            &migration.version,
            format!("only latest migration `{version}` can be rolled back"),
        ));
    }
    if checksum != migration.checksum {
        return Err(migration_error(
            AxDbErrorCode::MigrationChecksumMismatch,
            &migration.version,
            "local migration differs from the applied migration",
        ));
    }
    Ok(())
}

fn sqlite_open_connection(
    url: &Option<String>,
    resource: &str,
) -> AxRuntimeResult<rusqlite::Connection> {
    sqlite_open_connection_with_timeout(url, resource, DEFAULT_DB_SQLITE_BUSY_TIMEOUT_MS)
}

fn sqlite_open_connection_with_timeout(
    url: &Option<String>,
    resource: &str,
    busy_timeout_ms: u64,
) -> AxRuntimeResult<rusqlite::Connection> {
    let Some(url) = url else {
        return Err(AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::ConnectionFailed)
                .with_driver(AxDatabaseDriver::Sqlite)
                .with_resource(resource)
                .with_detail("missing sqlite database url"),
        ));
    };

    let connection = rusqlite::Connection::open(sqlite_database_path(url))
        .map_err(|error| sqlite_runtime_error(resource, error))?;
    connection
        .busy_timeout(Duration::from_millis(busy_timeout_ms))
        .map_err(|error| sqlite_runtime_error(resource, error))?;
    Ok(connection)
}

fn sqlite_database_path(url: &str) -> String {
    if matches!(url, ":memory:" | "sqlite::memory:" | "sqlite://:memory:") {
        return ":memory:".to_string();
    }

    if let Some(path) = url.strip_prefix("sqlite:///") {
        return format!("/{path}");
    }

    if let Some(path) = url.strip_prefix("sqlite://") {
        return path.to_string();
    }

    if let Some(path) = url.strip_prefix("sqlite:") {
        return path.to_string();
    }

    url.to_string()
}

fn json_params_to_sqlite(params: &[Value]) -> AxRuntimeResult<Vec<rusqlite::types::Value>> {
    params
        .iter()
        .map(json_value_to_sqlite)
        .collect::<AxRuntimeResult<Vec<_>>>()
}

fn json_value_to_sqlite(value: &Value) -> AxRuntimeResult<rusqlite::types::Value> {
    match value {
        Value::Null => Ok(rusqlite::types::Value::Null),
        Value::Bool(value) => Ok(rusqlite::types::Value::Integer(i64::from(*value))),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(rusqlite::types::Value::Integer(value))
            } else if let Some(value) = value.as_u64() {
                i64::try_from(value)
                    .map(rusqlite::types::Value::Integer)
                    .map_err(|_| {
                        AxRuntimeError::database(
                            AxDbError::new(AxDbErrorCode::InvalidQuery)
                                .with_driver(AxDatabaseDriver::Sqlite)
                                .with_detail("sqlite integer parameter is out of i64 range"),
                        )
                    })
            } else if let Some(value) = value.as_f64() {
                Ok(rusqlite::types::Value::Real(value))
            } else {
                Err(AxRuntimeError::database(
                    AxDbError::new(AxDbErrorCode::InvalidQuery)
                        .with_driver(AxDatabaseDriver::Sqlite)
                        .with_detail("unsupported sqlite number parameter"),
                ))
            }
        }
        Value::String(value) => Ok(rusqlite::types::Value::Text(value.clone())),
        Value::Array(_) | Value::Object(_) => Err(AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::InvalidQuery)
                .with_driver(AxDatabaseDriver::Sqlite)
                .with_detail("sqlite parameters cannot be arrays or objects"),
        )),
    }
}

fn sqlite_row_to_json(row: &rusqlite::Row<'_>, column_names: &[String]) -> rusqlite::Result<Value> {
    let mut record = serde_json::Map::new();
    for (index, name) in column_names.iter().enumerate() {
        let value = match row.get_ref(index)? {
            rusqlite::types::ValueRef::Null => Value::Null,
            rusqlite::types::ValueRef::Integer(value) => json!(value),
            rusqlite::types::ValueRef::Real(value) => json!(value),
            rusqlite::types::ValueRef::Text(value) => {
                Value::String(String::from_utf8_lossy(value).to_string())
            }
            rusqlite::types::ValueRef::Blob(value) => {
                Value::String(format!("<{} byte sqlite blob>", value.len()))
            }
        };
        record.insert(name.clone(), value);
    }
    Ok(Value::Object(record))
}

fn sqlite_runtime_error(resource: &str, error: rusqlite::Error) -> AxRuntimeError {
    AxRuntimeError::database(AxDbError::from_driver_detail(
        AxDatabaseDriver::Sqlite,
        resource,
        error.to_string(),
    ))
}

fn postgres_execute_query(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    resource: &str,
    sql: &str,
    params: &[Value],
) -> AxRuntimeResult<Value> {
    postgres_with_client(pool, url, resource, |client| {
        let wrapped_sql = postgres_json_query(sql);
        let params = params
            .iter()
            .cloned()
            .map(AxPostgresParam)
            .collect::<Vec<_>>();
        let param_refs = params
            .iter()
            .map(|value| value as &(dyn ToSql + Sync))
            .collect::<Vec<_>>();
        let rows = client
            .query(&wrapped_sql, &param_refs)
            .map_err(|error| postgres_runtime_error(resource, error))?;

        rows.into_iter()
            .map(|row| {
                row.try_get::<_, Value>(0)
                    .map_err(|error| postgres_runtime_error(resource, error))
            })
            .collect::<AxRuntimeResult<Vec<_>>>()
            .map(Value::Array)
    })
}

fn postgres_execute_mutation(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    action: &str,
    resource: &str,
    sql: &str,
    params: &[Value],
) -> AxRuntimeResult<Value> {
    postgres_with_client(pool, url, resource, |client| {
        let params = params
            .iter()
            .cloned()
            .map(AxPostgresParam)
            .collect::<Vec<_>>();
        let param_refs = params
            .iter()
            .map(|value| value as &(dyn ToSql + Sync))
            .collect::<Vec<_>>();
        let changes = client
            .execute(sql, &param_refs)
            .map_err(|error| postgres_runtime_error(resource, error))?;

        Ok(json!({
            "ok": true,
            "driver": "postgres",
            "action": action,
            "resource": resource,
            "changes": changes,
        }))
    })
}

fn postgres_execute_transaction(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    resource: &str,
    operations: &[AxPreparedTransactionOperation],
) -> AxRuntimeResult<Vec<Value>> {
    postgres_with_client(pool, url, resource, |client| {
        let mut transaction = client
            .transaction()
            .map_err(|error| postgres_runtime_error(resource, error))?;
        let mut results = Vec::with_capacity(operations.len());

        for operation in operations {
            let params = operation
                .params
                .iter()
                .cloned()
                .map(AxPostgresParam)
                .collect::<Vec<_>>();
            let param_refs = params
                .iter()
                .map(|value| value as &(dyn ToSql + Sync))
                .collect::<Vec<_>>();
            let changes = transaction
                .execute(&operation.sql, &param_refs)
                .map_err(|error| postgres_runtime_error(&operation.resource, error))?;
            results.push(json!({
                "ok": true,
                "driver": "postgres",
                "action": operation.action,
                "resource": operation.resource,
                "changes": changes,
            }));
        }

        transaction
            .commit()
            .map_err(|error| postgres_runtime_error(resource, error))?;
        Ok(results)
    })
}

fn postgres_ensure_migrations_table(
    client: &mut impl postgres::GenericClient,
) -> Result<(), postgres::Error> {
    client.batch_execute(
        r#"
create table if not exists _axonyx_migrations (
  version text primary key,
  name text not null,
  checksum text not null,
  applied_at timestamptz not null default now(),
  execution_ms bigint not null
)
"#,
    )
}

fn postgres_acquire_migration_lock(
    client: &mut impl postgres::GenericClient,
) -> Result<(), postgres::Error> {
    client
        .query_one(
            "select pg_advisory_xact_lock($1)",
            &[&AXONYX_POSTGRES_MIGRATION_LOCK_KEY],
        )
        .map(|_| ())
}

fn postgres_migrations_table_exists(
    client: &mut impl postgres::GenericClient,
) -> Result<bool, postgres::Error> {
    client
        .query_one("select to_regclass('_axonyx_migrations') is not null", &[])
        .map(|row| row.get(0))
}

fn postgres_migration_history(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
    postgres_with_client(pool, url, AXONYX_MIGRATIONS_RESOURCE, |client| {
        if !postgres_migrations_table_exists(client)
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?
        {
            return Ok(Vec::new());
        }
        client
            .query(
                "select version, name, checksum, applied_at::text, execution_ms from _axonyx_migrations order by version asc",
                &[],
            )
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?
            .into_iter()
            .map(|row| {
                let execution_ms = row.get::<_, i64>(4);
                Ok(AxAppliedMigration {
                    version: row.get(0),
                    name: row.get(1),
                    checksum: row.get(2),
                    applied_at: row.get(3),
                    execution_ms: u64::try_from(execution_ms).map_err(|_| {
                        migration_error(
                            AxDbErrorCode::DriverError,
                            AXONYX_MIGRATIONS_RESOURCE,
                            "stored migration execution time is negative",
                        )
                    })?,
                })
            })
            .collect()
    })
}

fn postgres_apply_migration(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    migration: &AxMigration,
) -> AxRuntimeResult<AxAppliedMigration> {
    postgres_apply_migrations_locked(pool, url, std::slice::from_ref(migration), false)?
        .pop()
        .ok_or_else(|| {
            migration_error(
                AxDbErrorCode::MigrationConflict,
                &migration.version,
                "migration was already applied",
            )
        })
}

fn postgres_apply_migrations(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    migrations: &[AxMigration],
) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
    postgres_apply_migrations_locked(pool, url, migrations, true)
}

fn postgres_apply_migrations_locked(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    migrations: &[AxMigration],
    skip_applied: bool,
) -> AxRuntimeResult<Vec<AxAppliedMigration>> {
    for migration in migrations {
        validate_migration(migration)?;
    }
    if migrations.is_empty() {
        return Ok(Vec::new());
    }
    postgres_with_client(pool, url, AXONYX_MIGRATIONS_RESOURCE, |client| {
        let mut transaction = client
            .transaction()
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        postgres_acquire_migration_lock(&mut transaction)
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        postgres_ensure_migrations_table(&mut transaction)
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        let mut applied = Vec::with_capacity(migrations.len());

        for migration in migrations {
            let existing = transaction
                .query_opt(
                    "select checksum from _axonyx_migrations where version = $1",
                    &[&migration.version],
                )
                .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
            if let Some(row) = existing {
                let checksum = row.get::<_, String>(0);
                if checksum != migration.checksum {
                    return Err(migration_error(
                        AxDbErrorCode::MigrationChecksumMismatch,
                        &migration.version,
                        "migration version is already present with a different checksum",
                    ));
                }
                if skip_applied {
                    continue;
                }
                return Err(migration_error(
                    AxDbErrorCode::MigrationConflict,
                    &migration.version,
                    "migration version is already present in database history",
                ));
            }

            let started = Instant::now();
            transaction
                .batch_execute(&migration.up_sql)
                .map_err(|error| postgres_runtime_error(&migration.version, error))?;
            let execution_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let stored_execution_ms = i64::try_from(execution_ms).unwrap_or(i64::MAX);
            let row = transaction
                .query_one(
                    "insert into _axonyx_migrations (version, name, checksum, execution_ms) values ($1, $2, $3, $4) returning applied_at::text",
                    &[&migration.version, &migration.name, &migration.checksum, &stored_execution_ms],
                )
                .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
            applied.push(AxAppliedMigration {
                version: migration.version.clone(),
                name: migration.name.clone(),
                checksum: migration.checksum.clone(),
                applied_at: row.get(0),
                execution_ms,
            });
        }

        transaction
            .commit()
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        Ok(applied)
    })
}

fn postgres_rollback_migration(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    migration: &AxMigration,
) -> AxRuntimeResult<()> {
    validate_migration(migration)?;
    postgres_with_client(pool, url, AXONYX_MIGRATIONS_RESOURCE, |client| {
        let mut transaction = client
            .transaction()
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        postgres_acquire_migration_lock(&mut transaction)
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        if !postgres_migrations_table_exists(&mut transaction)
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?
        {
            return Err(migration_error(
                AxDbErrorCode::MigrationConflict,
                &migration.version,
                "database migration history is empty",
            ));
        }
        let latest = transaction
            .query_opt(
                "select version, checksum from _axonyx_migrations order by version desc limit 1",
                &[],
            )
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?
            .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)));
        validate_rollback_target(
            migration,
            latest
                .as_ref()
                .map(|(version, checksum)| (version.as_str(), checksum.as_str())),
        )?;

        transaction
            .batch_execute(&migration.down_sql)
            .map_err(|error| postgres_runtime_error(&migration.version, error))?;
        transaction
            .execute(
                "delete from _axonyx_migrations where version = $1",
                &[&migration.version],
            )
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))?;
        transaction
            .commit()
            .map_err(|error| postgres_runtime_error(AXONYX_MIGRATIONS_RESOURCE, error))
    })
}

fn postgres_with_client<T>(
    pool: Option<&AxPostgresPool>,
    url: &Option<String>,
    resource: &str,
    operation: impl FnOnce(&mut Client) -> AxRuntimeResult<T>,
) -> AxRuntimeResult<T> {
    match pool {
        Some(pool) => {
            let mut client = postgres_pool_connection(pool, resource)?;
            operation(&mut client)
        }
        None => {
            let mut client = postgres_open_connection(url, resource)?;
            operation(&mut client)
        }
    }
}

fn postgres_pool_connection(
    pool: &AxPostgresPool,
    resource: &str,
) -> AxRuntimeResult<AxPooledPostgresConnection> {
    pool.get().map_err(|error| {
        let state = pool.state();
        let code = if state.connections >= pool.max_size() && state.idle_connections == 0 {
            AxDbErrorCode::Timeout
        } else {
            AxDbErrorCode::ConnectionFailed
        };
        AxRuntimeError::database(
            AxDbError::new(code)
                .with_driver(AxDatabaseDriver::Postgres)
                .with_resource(resource)
                .with_detail(error.to_string()),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AxPostgresTlsMode {
    Disable,
    Require,
    VerifyFull,
}

#[derive(Debug)]
struct AxPostgresRequireVerifier;

impl rustls::client::danger::ServerCertVerifier for AxPostgresRequireVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

fn postgres_open_connection(url: &Option<String>, resource: &str) -> AxRuntimeResult<Client> {
    let (connection_url, connector) = postgres_connection_parts(url, resource)?;

    Client::connect(&connection_url, connector)
        .map_err(|error| postgres_runtime_error(resource, error))
}

fn postgres_create_pool(
    url: &Option<String>,
    resource: &str,
    max_size: u32,
    timeout_ms: u64,
    query_timeout_ms: u64,
) -> AxRuntimeResult<AxPostgresPool> {
    let (connection_url, connector) = postgres_connection_parts(url, resource)?;
    let config = connection_url
        .parse::<postgres::Config>()
        .map_err(|error| {
            postgres_connection_config_error(resource, format!("invalid postgres URL: {error}"))
        })?;
    let manager = PostgresConnectionManager::new(config, connector);

    Ok(Pool::builder()
        .max_size(max_size)
        .min_idle(Some(0))
        .connection_timeout(Duration::from_millis(timeout_ms))
        .connection_customizer(Box::new(AxPostgresConnectionCustomizer {
            query_timeout_ms,
        }))
        .build_unchecked(manager))
}

fn postgres_connection_parts(
    url: &Option<String>,
    resource: &str,
) -> AxRuntimeResult<(String, MakeRustlsConnect)> {
    let Some(url) = url else {
        return Err(AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::ConnectionFailed)
                .with_driver(AxDatabaseDriver::Postgres)
                .with_resource(resource)
                .with_detail("missing postgres database url"),
        ));
    };

    let (connection_url, tls_mode, root_cert) = postgres_tls_settings(url, resource)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| {
            AxRuntimeError::database(
                AxDbError::new(AxDbErrorCode::ConnectionFailed)
                    .with_driver(AxDatabaseDriver::Postgres)
                    .with_resource(resource)
                    .with_detail(format!("failed to initialize postgres rustls: {error}")),
            )
        })?;
    let config = match tls_mode {
        AxPostgresTlsMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AxPostgresRequireVerifier))
            .with_no_client_auth(),
        AxPostgresTlsMode::Disable | AxPostgresTlsMode::VerifyFull => builder
            .with_root_certificates(postgres_root_store(root_cert.as_ref(), resource)?)
            .with_no_client_auth(),
    };
    let connector = MakeRustlsConnect::new(config);

    Ok((connection_url, connector))
}

fn postgres_tls_settings(
    connection_url: &str,
    resource: &str,
) -> AxRuntimeResult<(String, AxPostgresTlsMode, Option<PathBuf>)> {
    let (base, query) = connection_url
        .split_once('?')
        .map_or((connection_url, ""), |(base, query)| (base, query));
    if !base.starts_with("postgres://") && !base.starts_with("postgresql://") {
        return Err(postgres_connection_config_error(
            resource,
            "postgres connection must use a postgres:// or postgresql:// URL",
        ));
    }
    let mut mode = AxPostgresTlsMode::VerifyFull;
    let mut root_cert = None;
    let mut retained = Vec::new();

    for part in query.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        match key {
            "sslmode" => {
                mode = match value {
                    "disable" => AxPostgresTlsMode::Disable,
                    "require" => AxPostgresTlsMode::Require,
                    "verify-full" => AxPostgresTlsMode::VerifyFull,
                    "prefer" | "allow" => {
                        return Err(postgres_connection_config_error(
                            resource,
                            "sslmode=prefer/allow can fall back to plaintext; use disable, require, or verify-full",
                        ));
                    }
                    value => {
                        return Err(postgres_connection_config_error(
                            resource,
                            format!("unsupported postgres sslmode `{value}`"),
                        ));
                    }
                };
            }
            "sslrootcert" => {
                root_cert = Some(PathBuf::from(postgres_percent_decode(value, resource)?))
            }
            _ => retained.push(part.to_string()),
        }
    }

    if root_cert.is_some() && mode != AxPostgresTlsMode::VerifyFull {
        return Err(postgres_connection_config_error(
            resource,
            "sslrootcert requires sslmode=verify-full",
        ));
    }

    if mode != AxPostgresTlsMode::Disable {
        retained.push("sslmode=require".to_string());
    } else {
        retained.push("sslmode=disable".to_string());
    }

    Ok((format!("{base}?{}", retained.join("&")), mode, root_cert))
}

fn postgres_percent_decode(value: &str, resource: &str) -> AxRuntimeResult<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(postgres_connection_config_error(
                    resource,
                    "invalid percent encoding in postgres sslrootcert",
                ));
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).map_err(|_| {
                postgres_connection_config_error(
                    resource,
                    "invalid percent encoding in postgres sslrootcert",
                )
            })?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| {
                postgres_connection_config_error(
                    resource,
                    "invalid percent encoding in postgres sslrootcert",
                )
            })?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| {
        postgres_connection_config_error(resource, "postgres sslrootcert is not valid UTF-8")
    })
}

fn postgres_root_store(
    root_cert: Option<&PathBuf>,
    resource: &str,
) -> AxRuntimeResult<rustls::RootCertStore> {
    let mut roots =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let Some(path) = root_cert else {
        return Ok(roots);
    };

    let file = std::fs::File::open(path).map_err(|error| {
        postgres_connection_config_error(
            resource,
            format!("failed to open postgres sslrootcert: {error}"),
        )
    })?;
    let mut reader = BufReader::new(file);
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            postgres_connection_config_error(
                resource,
                format!("failed to parse postgres sslrootcert: {error}"),
            )
        })?;
    if certificates.is_empty() {
        return Err(postgres_connection_config_error(
            resource,
            "postgres sslrootcert contains no PEM certificates",
        ));
    }
    for certificate in certificates {
        roots.add(certificate).map_err(|error| {
            postgres_connection_config_error(
                resource,
                format!("invalid certificate in postgres sslrootcert: {error}"),
            )
        })?;
    }
    Ok(roots)
}

fn postgres_connection_config_error(resource: &str, detail: impl Into<String>) -> AxRuntimeError {
    AxRuntimeError::database(
        AxDbError::new(AxDbErrorCode::ConnectionFailed)
            .with_driver(AxDatabaseDriver::Postgres)
            .with_resource(resource)
            .with_detail(detail),
    )
}

fn postgres_json_query(sql: &str) -> String {
    let sql = sql.trim().trim_end_matches(';').trim();
    format!("select row_to_json(\"__ax_row\") from ({sql}) as \"__ax_row\"")
}

fn postgres_runtime_error(resource: &str, error: postgres::Error) -> AxRuntimeError {
    let detail = if let Some(db_error) = error.as_db_error() {
        format!("{}: {}", db_error.code().code(), db_error.message())
    } else {
        let mut detail = error.to_string();
        let mut source = error.source();
        while let Some(current) = source {
            detail.push_str(": ");
            detail.push_str(&current.to_string());
            source = current.source();
        }
        detail
    };
    AxRuntimeError::database(AxDbError::from_driver_detail(
        AxDatabaseDriver::Postgres,
        resource,
        detail,
    ))
}

fn database_health_error(driver: AxDatabaseDriver, detail: impl Into<String>) -> AxRuntimeError {
    AxRuntimeError::database(AxDbError::from_driver_detail(
        driver,
        "_axonyx_health",
        detail,
    ))
}

#[derive(Debug)]
struct AxPostgresParam(Value);

impl ToSql for AxPostgresParam {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
        match &self.0 {
            Value::Null => Ok(IsNull::Yes),
            Value::Bool(value) => value.to_sql(ty, out),
            Value::Number(value) => postgres_number_to_sql(value, ty, out),
            Value::String(value) => postgres_string_to_sql(value, ty, out),
            Value::Array(_) | Value::Object(_) if matches!(*ty, Type::JSON | Type::JSONB) => {
                self.0.to_sql(ty, out)
            }
            Value::Array(values) if *ty == Type::BYTEA => values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|value| u8::try_from(value).ok())
                        .ok_or_else(|| postgres_param_error("bytea arrays require values 0..255"))
                })
                .collect::<Result<Vec<_>, _>>()?
                .to_sql(ty, out),
            Value::Array(_) | Value::Object(_) => Err(postgres_param_error(&format!(
                "Axonyx JSON value cannot be encoded as postgres type {ty}"
            ))),
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

fn postgres_number_to_sql(
    value: &serde_json::Number,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    match *ty {
        Type::INT2 => i16::try_from(number_i64(value)?)
            .map_err(postgres_box_error)?
            .to_sql(ty, out),
        Type::INT4 => i32::try_from(number_i64(value)?)
            .map_err(postgres_box_error)?
            .to_sql(ty, out),
        Type::INT8 => number_i64(value)?.to_sql(ty, out),
        Type::OID => u32::try_from(number_u64(value)?)
            .map_err(postgres_box_error)?
            .to_sql(ty, out),
        Type::FLOAT4 => (number_f64(value)? as f32).to_sql(ty, out),
        Type::FLOAT8 => number_f64(value)?.to_sql(ty, out),
        _ => Err(postgres_param_error(&format!(
            "Axonyx number cannot be encoded as postgres type {ty}"
        ))),
    }
}

fn postgres_string_to_sql(
    value: &str,
    ty: &Type,
    out: &mut BytesMut,
) -> Result<IsNull, Box<dyn StdError + Sync + Send>> {
    match *ty {
        Type::VARCHAR | Type::TEXT | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
            value.to_sql(ty, out)
        }
        Type::UUID => Uuid::parse_str(value)?.to_sql(ty, out),
        Type::DATE => NaiveDate::parse_from_str(value, "%Y-%m-%d")?.to_sql(ty, out),
        Type::TIME => NaiveTime::parse_from_str(value, "%H:%M:%S%.f")?.to_sql(ty, out),
        Type::TIMESTAMP => parse_postgres_timestamp(value)?.to_sql(ty, out),
        Type::TIMESTAMPTZ => DateTime::parse_from_rfc3339(value)?.to_sql(ty, out),
        Type::JSON | Type::JSONB => serde_json::from_str::<Value>(value)?.to_sql(ty, out),
        Type::BYTEA => value.as_bytes().to_sql(ty, out),
        _ => Err(postgres_param_error(&format!(
            "Axonyx string cannot be encoded as postgres type {ty}"
        ))),
    }
}

fn parse_postgres_timestamp(value: &str) -> Result<NaiveDateTime, Box<dyn StdError + Sync + Send>> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f"))
        .map_err(postgres_box_error)
}

fn number_i64(value: &serde_json::Number) -> Result<i64, Box<dyn StdError + Sync + Send>> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .ok_or_else(|| postgres_param_error("postgres integer parameter is out of i64 range"))
}

fn number_u64(value: &serde_json::Number) -> Result<u64, Box<dyn StdError + Sync + Send>> {
    value
        .as_u64()
        .ok_or_else(|| postgres_param_error("postgres unsigned parameter must be non-negative"))
}

fn number_f64(value: &serde_json::Number) -> Result<f64, Box<dyn StdError + Sync + Send>> {
    value
        .as_f64()
        .ok_or_else(|| postgres_param_error("invalid postgres floating-point parameter"))
}

fn postgres_param_error(message: &str) -> Box<dyn StdError + Sync + Send> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.to_string(),
    ))
}

fn postgres_box_error<E>(error: E) -> Box<dyn StdError + Sync + Send>
where
    E: StdError + Sync + Send + 'static,
{
    Box::new(error)
}

fn validate_raw_select_sql(driver: &AxDatabaseDriver, sql: &str) -> AxRuntimeResult<()> {
    let trimmed = sql.trim();
    let analysis = analyze_raw_sql(trimmed);
    let is_select = analysis
        .top_level_words
        .iter()
        .find(|word| {
            matches!(
                word.as_str(),
                "select" | "insert" | "update" | "delete" | "replace"
            )
        })
        .is_some_and(|word| word == "select");
    let has_multiple_statements = analysis.statement_separators > 0;

    if !is_select || has_multiple_statements {
        return Err(AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::InvalidQuery)
                .with_driver(driver.clone())
                .with_resource("raw_sql")
                .with_detail("db.query only supports a single SELECT/WITH statement"),
        ));
    }

    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RawSqlAnalysis {
    top_level_words: Vec<String>,
    statement_separators: usize,
}

fn analyze_raw_sql(sql: &str) -> RawSqlAnalysis {
    let chars = sql.chars().collect::<Vec<_>>();
    let mut analysis = RawSqlAnalysis::default();
    let mut depth = 0usize;
    let mut index = 0usize;

    while index < chars.len() {
        match chars[index] {
            '\'' | '"' | '`' => {
                let quote = chars[index];
                index += 1;
                while index < chars.len() {
                    if chars[index] == quote {
                        if index + 1 < chars.len() && chars[index + 1] == quote {
                            index += 2;
                            continue;
                        }
                        index += 1;
                        break;
                    }
                    if chars[index] == '\\' && index + 1 < chars.len() {
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
            }
            '-' if chars.get(index + 1) == Some(&'-') => {
                index += 2;
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
            }
            '/' if chars.get(index + 1) == Some(&'*') => {
                index += 2;
                while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                    index += 1;
                }
                index = (index + 2).min(chars.len());
            }
            '(' => {
                depth += 1;
                index += 1;
            }
            ')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            ';' if depth == 0 => {
                if chars[index + 1..]
                    .iter()
                    .any(|ch| !ch.is_whitespace() && *ch != ';')
                {
                    analysis.statement_separators += 1;
                }
                index += 1;
            }
            ch if depth == 0 && (ch.is_ascii_alphabetic() || ch == '_') => {
                let start = index;
                index += 1;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
                {
                    index += 1;
                }
                analysis.top_level_words.push(
                    chars[start..index]
                        .iter()
                        .collect::<String>()
                        .to_ascii_lowercase(),
                );
            }
            _ => index += 1,
        }
    }

    analysis
}

fn api_load_plan(
    driver: AxDatabaseDriver,
    api_url: &Option<String>,
    request: &AxQueryRequest,
) -> AxRuntimeResult<Value> {
    let plan = AxApiRequestPlan {
        dialect: driver
            .sql_dialect()
            .map(|dialect| dialect.name().to_string())
            .unwrap_or_else(|| driver.as_str().to_string()),
        base_url: api_url.clone().unwrap_or_default(),
        token: "<redacted-by-runtime-config>".to_string(),
        action: "load".to_string(),
        resource: request.collection.clone(),
        payload: json!({
            "filters": request.filters,
            "orders": request.orders,
            "limit": request.limit,
            "offset": request.offset,
            "mode": request.mode,
        }),
    };

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "api",
        "request": plan,
    }))
}

fn api_raw_query_plan(
    driver: AxDatabaseDriver,
    api_url: &Option<String>,
    request: &AxRawSqlRequest,
) -> AxRuntimeResult<Value> {
    validate_raw_select_sql(&driver, &request.sql)?;
    let plan = AxApiRequestPlan {
        dialect: driver
            .sql_dialect()
            .map(|dialect| dialect.name().to_string())
            .unwrap_or_else(|| driver.as_str().to_string()),
        base_url: api_url.clone().unwrap_or_default(),
        token: "<redacted-by-runtime-config>".to_string(),
        action: "query".to_string(),
        resource: "raw_sql".to_string(),
        payload: json!({
            "sql": request.sql,
            "params": request.params,
        }),
    };

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "api",
        "request": plan,
    }))
}

fn api_mutation_plan(
    driver: AxDatabaseDriver,
    api_url: &Option<String>,
    action: &str,
    collection: &str,
    fields: &BTreeMap<String, Value>,
    filters: &[AxQueryFilterRequest],
) -> AxRuntimeResult<Value> {
    let plan = AxApiRequestPlan {
        dialect: driver
            .sql_dialect()
            .map(|dialect| dialect.name().to_string())
            .unwrap_or_else(|| driver.as_str().to_string()),
        base_url: api_url.clone().unwrap_or_default(),
        token: "<redacted-by-runtime-config>".to_string(),
        action: action.to_string(),
        resource: collection.to_string(),
        payload: json!({
            "fields": fields,
            "filters": request_filters_payload(filters),
        }),
    };

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "api",
        "request": plan,
    }))
}

fn api_delete_plan(
    driver: AxDatabaseDriver,
    api_url: &Option<String>,
    request: &AxDeleteRequest,
) -> AxRuntimeResult<Value> {
    let plan = AxApiRequestPlan {
        dialect: driver
            .sql_dialect()
            .map(|dialect| dialect.name().to_string())
            .unwrap_or_else(|| driver.as_str().to_string()),
        base_url: api_url.clone().unwrap_or_default(),
        token: "<redacted-by-runtime-config>".to_string(),
        action: "delete".to_string(),
        resource: request.collection.clone(),
        payload: json!({
            "filters": request.filters,
        }),
    };

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "api",
        "request": plan,
    }))
}

fn query_request_to_plan(request: &AxQueryRequest) -> AxQueryPlan {
    AxQueryPlan {
        source: AxQuerySourcePlan::Stream {
            collection: request.collection.clone(),
        },
        filters: query_filters_to_plan(&request.filters),
        orders: request
            .orders
            .iter()
            .map(|order| AxQueryOrderPlan {
                field: order.field.clone(),
                direction: match order.direction {
                    AxQueryOrderDirection::Asc => AxQueryOrderDirectionPlan::Asc,
                    AxQueryOrderDirection::Desc => AxQueryOrderDirectionPlan::Desc,
                },
            })
            .collect(),
        limit: request.limit,
        offset: request.offset,
        mode: match request.mode {
            AxQueryMode::Many => AxQueryModePlan::Many,
            AxQueryMode::First => AxQueryModePlan::First,
        },
    }
}

fn query_filters_to_plan(filters: &[AxQueryFilterRequest]) -> Vec<AxQueryFilterPlan> {
    filters
        .iter()
        .map(|filter| AxQueryFilterPlan {
            field: filter.field.clone(),
            op: query_filter_op_to_plan(filter.op),
            value: json_value_to_expr(&filter.value),
        })
        .collect()
}

fn query_filter_op_to_plan(op: AxQueryFilterOp) -> AxQueryFilterOpPlan {
    match op {
        AxQueryFilterOp::Eq => AxQueryFilterOpPlan::Eq,
        AxQueryFilterOp::Ne => AxQueryFilterOpPlan::Ne,
        AxQueryFilterOp::In => AxQueryFilterOpPlan::In,
        AxQueryFilterOp::NotIn => AxQueryFilterOpPlan::NotIn,
        AxQueryFilterOp::IsNull => AxQueryFilterOpPlan::IsNull,
        AxQueryFilterOp::IsNotNull => AxQueryFilterOpPlan::IsNotNull,
    }
}

fn fields_to_assignment_plans(fields: &BTreeMap<String, Value>) -> Vec<AxAssignmentPlan> {
    fields
        .iter()
        .map(|(name, value)| AxAssignmentPlan {
            name: name.clone(),
            value: json_value_to_expr(value),
        })
        .collect()
}

fn json_value_to_expr(value: &Value) -> AxRustExpr {
    AxRustExpr::new(value.to_string())
}

fn request_filters_payload(filters: &[AxQueryFilterRequest]) -> Value {
    json!(filters)
}

pub mod prelude {
    pub use super::adapter_from_config;
    pub use super::lazy_runtime_from_env;
    pub use super::ok_payload;
    pub use super::runtime_from_env;
    pub use super::AxAppliedMigration;
    pub use super::AxBackendRuntime;
    pub use super::AxDataTransport;
    pub use super::AxDatabaseAdapter;
    pub use super::AxDatabaseConfig;
    pub use super::AxDatabaseDriver;
    pub use super::AxDatabaseHealthReport;
    pub use super::AxDatabaseMetricsSnapshot;
    pub use super::AxDatabasePolicy;
    pub use super::AxDatabasePoolHealth;
    pub use super::AxDatabaseRuntime;
    pub use super::AxDbError;
    pub use super::AxDbErrorCode;
    pub use super::AxDeleteRequest;
    pub use super::AxEnv;
    pub use super::AxInsertRequest;
    pub use super::AxLoaderContext;
    pub use super::AxMessenger;
    pub use super::AxMigration;
    pub use super::AxMigrationExecutor;
    pub use super::AxMutationExecutor;
    pub use super::AxQueryExecutor;
    pub use super::AxQueryFilterOp;
    pub use super::AxQueryFilterRequest;
    pub use super::AxQueryMode;
    pub use super::AxQueryOrderDirection;
    pub use super::AxQueryOrderRequest;
    pub use super::AxQueryRequest;
    pub use super::AxRawSqlRequest;
    pub use super::AxRevalidator;
    pub use super::AxRuntimeEnvAccess;
    pub use super::AxRuntimeError;
    pub use super::AxRuntimeResult;
    pub use super::AxSendRequest;
    pub use super::AxTransactionOperation;
    pub use super::AxTransactionRequest;
    pub use super::AxUpdateRequest;
    pub use super::MemoryAdapter;
    pub use super::MySqlAdapter;
    pub use super::PostgresAdapter;
    pub use super::SqliteAdapter;
    pub use super::MAX_DB_READ_RETRY_ATTEMPTS;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryRuntime {
        env: AxEnv,
    }

    impl AxRuntimeEnvAccess for MemoryRuntime {
        fn env(&self) -> &AxEnv {
            &self.env
        }
    }

    impl AxQueryExecutor for MemoryRuntime {
        fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
            Ok(json!({
                "collection": request.collection,
                "limit": request.limit,
            }))
        }

        fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
            Ok(json!({
                "sql": request.sql,
                "params": request.params,
            }))
        }
    }

    impl AxMutationExecutor for MemoryRuntime {
        fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
            Ok(json!({
                "inserted": request.collection,
                "fields": request.fields,
            }))
        }

        fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
            Ok(json!({
                "updated": request.collection,
                "fields": request.fields,
            }))
        }

        fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
            Ok(json!({
                "deleted": request.collection,
                "filters": request.filters,
            }))
        }
    }

    impl AxRevalidator for MemoryRuntime {
        fn revalidate(&self, target: &str) -> AxRuntimeResult<()> {
            if target.is_empty() {
                return Err(AxRuntimeError::message("missing revalidation target"));
            }
            Ok(())
        }
    }

    impl AxMessenger for MemoryRuntime {
        fn send(&self, request: &AxSendRequest) -> AxRuntimeResult<()> {
            if request.target.is_empty() {
                return Err(AxRuntimeError::message("missing send target"));
            }
            Ok(())
        }
    }

    #[test]
    fn memory_runtime_can_execute_query_contract() {
        let runtime = MemoryRuntime::default();
        let result = runtime
            .load(&AxQueryRequest {
                collection: "posts".to_string(),
                filters: vec![AxQueryFilterRequest {
                    field: "status".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("published"),
                }],
                orders: vec![AxQueryOrderRequest {
                    field: "created_at".to_string(),
                    direction: AxQueryOrderDirection::Desc,
                }],
                limit: Some(20),
                offset: None,
                mode: AxQueryMode::Many,
            })
            .expect("query should execute");

        assert_eq!(
            result,
            json!({
                "collection": "posts",
                "limit": 20,
            })
        );
    }

    #[test]
    fn ok_payload_returns_framework_success_shape() {
        assert_eq!(ok_payload(), json!({ "ok": true }));
    }

    #[test]
    fn db_error_public_payload_hides_internal_detail() {
        let error = AxDbError::new(AxDbErrorCode::UniqueViolation)
            .with_driver(AxDatabaseDriver::Sqlite)
            .with_resource("posts")
            .with_field("slug")
            .with_detail("UNIQUE constraint failed: posts.slug");

        assert_eq!(
            error.public_payload(),
            json!({
                "ok": false,
                "error": {
                    "code": "db.unique_violation",
                    "message": "Record already exists.",
                    "status": 409,
                    "resource": "posts",
                    "field": "slug",
                }
            })
        );
        assert_eq!(
            error.detail.as_deref(),
            Some("UNIQUE constraint failed: posts.slug")
        );
    }

    #[test]
    fn runtime_error_can_render_safe_database_payload() {
        let error = AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::ConnectionFailed)
                .with_driver(AxDatabaseDriver::Postgres)
                .with_detail("connection refused on 127.0.0.1:5432"),
        );

        assert_eq!(
            error.public_error_payload(),
            json!({
                "ok": false,
                "error": {
                    "code": "db.connection_failed",
                    "message": "Database connection failed.",
                    "status": 503,
                }
            })
        );
    }

    #[test]
    fn db_error_translator_maps_sqlite_unique_violation() {
        let error = AxDbError::from_driver_detail(
            AxDatabaseDriver::Sqlite,
            "posts",
            "UNIQUE constraint failed: posts.slug",
        );

        assert_eq!(error.code, "db.unique_violation");
        assert_eq!(error.status, 409);
        assert_eq!(error.field.as_deref(), Some("slug"));
        assert_eq!(
            error.public_payload()["error"]["message"],
            json!("Record already exists.")
        );
    }

    #[test]
    fn db_error_translator_maps_postgres_unique_violation() {
        let error = AxDbError::from_driver_detail(
            AxDatabaseDriver::Postgres,
            "users",
            "ERROR: duplicate key value violates unique constraint \"users_email_key\" SQLSTATE 23505",
        );

        assert_eq!(error.code, "db.unique_violation");
        assert_eq!(error.status, 409);
        assert_eq!(error.resource.as_deref(), Some("users"));
    }

    #[test]
    fn db_error_translator_maps_connection_and_timeout_errors() {
        let sqlite = AxDbError::from_driver_detail(
            AxDatabaseDriver::Sqlite,
            "posts",
            "unable to open database file",
        );
        let postgres = AxDbError::from_driver_detail(
            AxDatabaseDriver::Postgres,
            "posts",
            "statement timeout after 2000ms",
        );
        let postgres_tls = AxDbError::from_driver_detail(
            AxDatabaseDriver::Postgres,
            "posts",
            "TLS handshake failed: invalid certificate",
        );
        let postgres_param = AxDbError::from_driver_detail(
            AxDatabaseDriver::Postgres,
            "posts",
            "error serializing parameter 0: Axonyx number cannot be encoded as postgres type numeric",
        );

        assert_eq!(sqlite.code, "db.connection_failed");
        assert_eq!(sqlite.status, 503);
        assert_eq!(postgres.code, "db.timeout");
        assert_eq!(postgres.status, 503);
        assert_eq!(postgres_tls.code, "db.connection_failed");
        assert_eq!(postgres_param.code, "db.invalid_query");
        assert_eq!(postgres_param.status, 400);
    }

    #[test]
    fn env_access_can_read_public_and_secret_values() {
        let runtime = MemoryRuntime {
            env: AxEnv::new()
                .with_public("app_name", "Axonyx")
                .with_secret("db_url", "postgres://local/axonyx"),
        };

        assert_eq!(
            runtime
                .env()
                .public("app_name")
                .expect("public key should exist"),
            "Axonyx".to_string()
        );
        assert_eq!(
            runtime
                .env()
                .secret("db_url")
                .expect("secret key should exist"),
            "postgres://local/axonyx".to_string()
        );
        assert_eq!(
            runtime
                .env()
                .value("AXONYX_CUSTOM_VALUE")
                .unwrap_or_else(|_| "missing".to_string()),
            "missing".to_string()
        );
    }

    #[test]
    fn env_value_can_read_plain_backend_keys() {
        let env = AxEnv::new().with_secret("database_url", "postgres://local/axonyx");

        assert_eq!(
            env.value("DATABASE_URL").expect("env key should resolve"),
            "postgres://local/axonyx".to_string()
        );
    }

    #[test]
    fn from_env_collects_ax_public_and_secret_namespaces() {
        let public_prev = std::env::var("AX_PUBLIC_APP_NAME").ok();
        let secret_prev = std::env::var("AX_SECRET_DB_URL").ok();

        std::env::set_var("AX_PUBLIC_APP_NAME", "Axonyx");
        std::env::set_var("AX_SECRET_DB_URL", "postgres://local/axonyx");

        let env = AxEnv::from_env();

        assert_eq!(
            env.public("app_name").expect("public key should exist"),
            "Axonyx".to_string()
        );
        assert_eq!(
            env.secret("db_url").expect("secret key should exist"),
            "postgres://local/axonyx".to_string()
        );

        if let Some(value) = public_prev {
            std::env::set_var("AX_PUBLIC_APP_NAME", value);
        } else {
            std::env::remove_var("AX_PUBLIC_APP_NAME");
        }

        if let Some(value) = secret_prev {
            std::env::set_var("AX_SECRET_DB_URL", value);
        } else {
            std::env::remove_var("AX_SECRET_DB_URL");
        }
    }

    #[test]
    fn env_can_resolve_database_config_for_mysql() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "mysql")
            .with_secret("db_url", "mysql://root:root@localhost:3306/axonyx");

        let config = env.database_config().expect("config should resolve");

        assert_eq!(
            config,
            AxDatabaseConfig {
                driver: AxDatabaseDriver::MySql,
                transport: AxDataTransport::Direct,
                url: Some("mysql://root:root@localhost:3306/axonyx".to_string()),
                api_url: None,
                api_key: None,
                pool_max_size: DEFAULT_DB_POOL_MAX_SIZE,
                pool_timeout_ms: DEFAULT_DB_POOL_TIMEOUT_MS,
                policy: AxDatabasePolicy::default(),
            }
        );
    }

    #[test]
    fn env_can_resolve_database_config_from_plain_database_keys() {
        let env = AxEnv::new()
            .with_secret("database_driver", "sqlite")
            .with_secret("database_url", "sqlite://data/app.db");

        let config = env.database_config().expect("config should resolve");

        assert_eq!(
            config,
            AxDatabaseConfig {
                driver: AxDatabaseDriver::Sqlite,
                transport: AxDataTransport::Direct,
                url: Some("sqlite://data/app.db".to_string()),
                api_url: None,
                api_key: None,
                pool_max_size: DEFAULT_DB_POOL_MAX_SIZE,
                pool_timeout_ms: DEFAULT_DB_POOL_TIMEOUT_MS,
                policy: AxDatabasePolicy::default(),
            }
        );
    }

    #[test]
    fn env_resolves_database_pool_defaults_and_overrides() {
        let defaults = AxEnv::new()
            .database_config()
            .expect("default database config should resolve");
        assert_eq!(defaults.pool_max_size, DEFAULT_DB_POOL_MAX_SIZE);
        assert_eq!(defaults.pool_timeout_ms, DEFAULT_DB_POOL_TIMEOUT_MS);

        let configured = AxEnv::new()
            .with_secret("db_pool_max_size", "24")
            .with_secret("db_pool_timeout_ms", "1250")
            .database_config()
            .expect("configured database pool should resolve");
        assert_eq!(configured.pool_max_size, 24);
        assert_eq!(configured.pool_timeout_ms, 1_250);
    }

    #[test]
    fn env_resolves_database_policy_defaults_and_overrides() {
        let defaults = AxEnv::new()
            .database_policy()
            .expect("default database policy should resolve");
        assert_eq!(defaults, AxDatabasePolicy::default());

        let configured = AxEnv::new()
            .with_secret("db_query_timeout_ms", "2400")
            .with_secret("db_read_retry_attempts", "3")
            .with_secret("db_read_retry_backoff_ms", "25")
            .with_secret("db_sqlite_busy_timeout_ms", "900")
            .database_policy()
            .expect("configured database policy should resolve");
        assert_eq!(
            configured,
            AxDatabasePolicy {
                query_timeout_ms: 2_400,
                read_retry_attempts: 3,
                read_retry_backoff_ms: 25,
                sqlite_busy_timeout_ms: 900,
            }
        );
    }

    #[test]
    fn env_rejects_invalid_database_policy_values() {
        for (key, value, name) in [
            ("db_query_timeout_ms", "0", "DB_QUERY_TIMEOUT_MS"),
            ("db_read_retry_attempts", "later", "DB_READ_RETRY_ATTEMPTS"),
            ("db_read_retry_backoff_ms", "0", "DB_READ_RETRY_BACKOFF_MS"),
            (
                "db_sqlite_busy_timeout_ms",
                "never",
                "DB_SQLITE_BUSY_TIMEOUT_MS",
            ),
            ("db_read_retry_attempts", "6", "DB_READ_RETRY_ATTEMPTS"),
        ] {
            let error = AxEnv::new()
                .with_secret(key, value)
                .database_policy()
                .expect_err("invalid database policy should fail");
            assert!(error.to_string().contains(name));
        }
    }

    #[test]
    fn read_retry_only_repeats_transient_database_errors() {
        let policy = AxDatabasePolicy {
            read_retry_attempts: 2,
            read_retry_backoff_ms: 1,
            ..AxDatabasePolicy::default()
        };
        let mut attempts = 0;
        let value = retry_database_read(policy, || {
            attempts += 1;
            if attempts < 3 {
                return Err(AxRuntimeError::database(
                    AxDbError::new(AxDbErrorCode::Timeout)
                        .with_driver(AxDatabaseDriver::Postgres)
                        .with_resource("posts"),
                ));
            }
            Ok("ready")
        })
        .expect("transient read should retry");
        assert_eq!(value, "ready");
        assert_eq!(attempts, 3);

        let mut invalid_attempts = 0;
        let error = retry_database_read(policy, || -> AxRuntimeResult<()> {
            invalid_attempts += 1;
            Err(AxRuntimeError::database(AxDbError::new(
                AxDbErrorCode::InvalidQuery,
            )))
        })
        .expect_err("invalid query should not retry");
        assert!(matches!(error, AxRuntimeError::Database { .. }));
        assert_eq!(invalid_attempts, 1);
    }

    #[test]
    fn sqlite_connection_applies_busy_timeout_policy() {
        let connection =
            sqlite_open_connection_with_timeout(&Some(":memory:".to_string()), "posts", 321)
                .expect("sqlite connection should open");
        let timeout = connection
            .query_row("pragma busy_timeout", [], |row| row.get::<_, u64>(0))
            .expect("sqlite busy timeout should read");
        assert_eq!(timeout, 321);
    }

    #[test]
    fn sqlite_health_probes_the_database_without_exposing_connection_details() {
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_driver", "sqlite")
                .with_secret("db_url", ":memory:"),
        )
        .expect("sqlite runtime should initialize");

        let report = runtime
            .database_health()
            .expect("sqlite health probe should pass");

        assert!(report.ok);
        assert_eq!(report.driver, "sqlite");
        assert_eq!(report.transport, "direct");
        assert_eq!(report.probe, "query");
        assert_eq!(report.pool, None);
        assert_eq!(report.metrics, AxDatabaseMetricsSnapshot::default());
        let payload = serde_json::to_string(&report).expect("health report should serialize");
        assert!(!payload.contains(":memory:"));
        assert!(!payload.contains("db_url"));
    }

    #[test]
    fn database_health_reports_process_local_operation_metrics() {
        let runtime = runtime_from_env(AxEnv::new().with_secret("db_driver", "memory"))
            .expect("memory runtime should initialize");

        runtime
            .load(&AxQueryRequest {
                collection: "posts".to_string(),
                filters: Vec::new(),
                orders: Vec::new(),
                limit: Some(1),
                offset: None,
                mode: AxQueryMode::Many,
            })
            .expect("memory read should pass");
        runtime
            .insert(&AxInsertRequest {
                collection: "posts".to_string(),
                fields: BTreeMap::new(),
            })
            .expect("memory write should pass");

        let report = runtime
            .database_health()
            .expect("memory health probe should pass");
        assert_eq!(report.probe, "memory");
        assert_eq!(report.metrics.reads, 1);
        assert_eq!(report.metrics.writes, 1);
        assert_eq!(report.metrics.failures, 0);
    }

    #[test]
    fn env_rejects_invalid_database_pool_values() {
        for (key, value, name) in [
            ("db_pool_max_size", "0", "DB_POOL_MAX_SIZE"),
            ("db_pool_timeout_ms", "soon", "DB_POOL_TIMEOUT_MS"),
        ] {
            let error = AxEnv::new()
                .with_secret(key, value)
                .database_config()
                .expect_err("invalid database pool config should fail");
            assert!(error.to_string().contains(name));
        }
    }

    #[test]
    fn postgres_pool_is_lazy_and_uses_runtime_limits() {
        let adapter = PostgresAdapter::new(
            Some("postgres://127.0.0.1:1/axonyx?sslmode=disable".to_string()),
            AxDataTransport::Direct,
            None,
            3,
            75,
            AxDatabasePolicy::default(),
        );

        assert!(adapter.pool.get().is_none());
        let pool = adapter
            .direct_pool("posts")
            .expect("pool configuration should initialize")
            .expect("direct postgres should provide a pool");

        assert_eq!(pool.max_size(), 3);
        assert_eq!(pool.connection_timeout(), Duration::from_millis(75));
        assert_eq!(pool.state().connections, 0);
    }

    #[test]
    fn runtime_from_env_can_select_mysql_adapter() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "mysql")
            .with_secret("db_url", "mysql://root:root@localhost:3306/axonyx");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let value = runtime
            .load(&AxQueryRequest {
                collection: "posts".to_string(),
                filters: Vec::new(),
                orders: Vec::new(),
                limit: Some(10),
                offset: None,
                mode: AxQueryMode::Many,
            })
            .expect("query should execute");

        assert_eq!(value["driver"], "mysql");
        assert_eq!(value["transport"], "direct");
        assert_eq!(value["execution"]["dialect"], "mysql");
        assert_eq!(
            value["execution"]["params"].as_array().map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn direct_transport_emits_sql_execution_plan() {
        let request = AxQueryRequest {
            collection: "posts".to_string(),
            filters: vec![AxQueryFilterRequest {
                field: "status".to_string(),
                op: AxQueryFilterOp::Eq,
                value: json!("published"),
            }],
            orders: vec![AxQueryOrderRequest {
                field: "created_at".to_string(),
                direction: AxQueryOrderDirection::Desc,
            }],
            limit: Some(12),
            offset: None,
            mode: AxQueryMode::Many,
        };
        let plan =
            compile_query_plan_to_sql(&query_request_to_plan(&request), AxSqlDialect::Postgres)
                .expect("postgres query should compile");

        assert_eq!(
            plan.sql,
            r#"select * from "posts" where "status" = $1 order by "created_at" desc limit 12"#
        );
        assert_eq!(sql_params_to_json(&plan.params), vec![json!("published")]);
    }

    #[test]
    fn direct_transport_emits_extended_filter_sql_plan() {
        let request = AxQueryRequest {
            collection: "posts".to_string(),
            filters: vec![
                AxQueryFilterRequest {
                    field: "archived".to_string(),
                    op: AxQueryFilterOp::Ne,
                    value: json!(true),
                },
                AxQueryFilterRequest {
                    field: "status".to_string(),
                    op: AxQueryFilterOp::In,
                    value: json!(["published", "featured"]),
                },
                AxQueryFilterRequest {
                    field: "deleted_at".to_string(),
                    op: AxQueryFilterOp::IsNull,
                    value: json!(true),
                },
                AxQueryFilterRequest {
                    field: "published_at".to_string(),
                    op: AxQueryFilterOp::IsNotNull,
                    value: json!(true),
                },
            ],
            orders: Vec::new(),
            limit: None,
            offset: None,
            mode: AxQueryMode::Many,
        };
        let plan =
            compile_query_plan_to_sql(&query_request_to_plan(&request), AxSqlDialect::Postgres)
                .expect("postgres query should compile");

        assert_eq!(
            plan.sql,
            r#"select * from "posts" where "archived" != $1 and "status" in ($2, $3) and "deleted_at" is null and "published_at" is not null"#
        );
        assert_eq!(
            sql_params_to_json(&plan.params),
            vec![json!(true), json!("published"), json!("featured")]
        );
    }

    #[test]
    fn api_transport_emits_request_plan() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_transport", "api")
            .with_secret("data_api_key", "secret-token")
            .with_public("data_api_url", "https://data.example.com");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let value = runtime
            .insert(&AxInsertRequest {
                collection: "posts".to_string(),
                fields: BTreeMap::from([("title".to_string(), json!("Hello"))]),
            })
            .expect("insert should execute");

        assert_eq!(value["transport"], "api");
        assert_eq!(value["request"]["base_url"], "https://data.example.com");
        assert_eq!(value["request"]["action"], "insert");
        assert_eq!(value["request"]["resource"], "posts");
        assert_eq!(
            value["request"]["payload"]["fields"]["title"],
            json!("Hello")
        );
    }

    #[test]
    fn direct_update_emits_where_clause_when_filters_exist() {
        let fields = BTreeMap::from([("title".to_string(), json!("Hello"))]);
        let filters = vec![AxQueryFilterRequest {
            field: "id".to_string(),
            op: AxQueryFilterOp::Eq,
            value: json!(7),
        }];
        let plan = compile_update_plan_to_sql(
            "posts",
            &fields_to_assignment_plans(&fields),
            &query_filters_to_plan(&filters),
            AxSqlDialect::Postgres,
        )
        .expect("postgres update should compile");

        assert_eq!(
            plan.sql,
            r#"update "posts" set "title" = $1 where "id" = $2"#
        );
        assert_eq!(
            sql_params_to_json(&plan.params),
            vec![json!("Hello"), json!(7)]
        );
    }

    #[test]
    fn direct_delete_emits_where_clause_when_filters_exist() {
        let filters = vec![AxQueryFilterRequest {
            field: "id".to_string(),
            op: AxQueryFilterOp::Eq,
            value: json!(7),
        }];
        let plan = compile_delete_plan_to_sql(
            "posts",
            &query_filters_to_plan(&filters),
            AxSqlDialect::Postgres,
        )
        .expect("postgres delete should compile");

        assert_eq!(plan.sql, r#"delete from "posts" where "id" = $1"#);
        assert_eq!(sql_params_to_json(&plan.params), vec![json!(7)]);
    }

    #[test]
    fn postgres_query_wraps_rows_as_json_without_trailing_semicolon() {
        assert_eq!(
            postgres_json_query("select id, title from posts;"),
            "select row_to_json(\"__ax_row\") from (select id, title from posts) as \"__ax_row\""
        );
    }

    #[test]
    fn postgres_parameters_follow_expected_scalar_types() {
        for (value, ty) in [
            (json!(7), Type::INT4),
            (json!(9), Type::INT8),
            (json!(1.5), Type::FLOAT8),
            (json!(true), Type::BOOL),
            (json!("published"), Type::TEXT),
            (json!("2026-08-31"), Type::DATE),
        ] {
            let mut output = BytesMut::new();
            AxPostgresParam(value)
                .to_sql(&ty, &mut output)
                .expect("supported postgres parameter should encode");
        }
    }

    #[test]
    fn postgres_connection_failures_use_public_database_error() {
        let error = match postgres_open_connection(
            &Some("postgres://127.0.0.1:1/axonyx?connect_timeout=1".to_string()),
            "posts",
        ) {
            Ok(_) => panic!("closed local port should fail"),
            Err(error) => error,
        };
        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.connection_failed");
        assert_eq!(error.resource.as_deref(), Some("posts"));
        assert!(!error.public_payload().to_string().contains("127.0.0.1"));
    }

    #[test]
    fn postgres_tls_defaults_to_verified_encryption() {
        let (url, mode, root_cert) =
            postgres_tls_settings("postgresql://user:secret@db.example.com:5432/app", "posts")
                .expect("postgres URL should parse");

        assert_eq!(mode, AxPostgresTlsMode::VerifyFull);
        assert_eq!(root_cert, None);
        assert!(url.contains("sslmode=require"));
    }

    #[test]
    fn postgres_tls_supports_explicit_require_mode() {
        let (url, mode, root_cert) = postgres_tls_settings(
            "postgresql://user:secret@db.example.com:5432/app?application_name=axonyx&sslmode=require",
            "posts",
        )
        .expect("postgres URL should parse");

        assert_eq!(mode, AxPostgresTlsMode::Require);
        assert_eq!(root_cert, None);
        assert!(url.contains("application_name=axonyx"));
        assert_eq!(url.matches("sslmode=require").count(), 1);
    }

    #[test]
    fn postgres_tls_extracts_verify_full_root_certificate() {
        let (url, mode, root_cert) = postgres_tls_settings(
            "postgresql://user:secret@db.example.com:5432/app?sslmode=verify-full&sslrootcert=C%3A%5Ccerts%5Cprod-ca.crt",
            "posts",
        )
        .expect("postgres URL should parse");

        assert_eq!(mode, AxPostgresTlsMode::VerifyFull);
        assert_eq!(root_cert, Some(PathBuf::from(r"C:\certs\prod-ca.crt")));
        assert!(url.contains("sslmode=require"));
        assert!(!url.contains("sslrootcert"));
    }

    #[test]
    fn postgres_tls_rejects_plaintext_fallback_modes() {
        let error = postgres_tls_settings(
            "postgresql://user:secret@db.example.com:5432/app?sslmode=prefer",
            "posts",
        )
        .expect_err("plaintext fallback must be rejected");

        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.connection_failed");
        assert!(error
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("fall back to plaintext")));
    }

    #[test]
    fn postgres_live_query_runs_when_test_url_is_configured() {
        let Ok(url) = std::env::var("AXONYX_TEST_POSTGRES_URL") else {
            return;
        };
        let table = format!("axonyx_runtime_test_{}", std::process::id());
        let mut admin = postgres_open_connection(&Some(url.clone()), &table)
            .expect("postgres test connection should open");
        admin
            .batch_execute(&format!(
                "drop table if exists \"{table}\"; create table \"{table}\" (id serial primary key, title text not null unique, published boolean not null default false)"
            ))
            .expect("postgres test table should create");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "postgres")
                .with_secret("db_url", &url)
                .with_secret("db_pool_max_size", "1"),
        )
        .expect("postgres runtime should initialize");

        let result = (|| -> AxRuntimeResult<()> {
            let value = runtime.query(&AxRawSqlRequest {
                sql: "select $1::integer as value".to_string(),
                params: vec![json!(7)],
            })?;
            assert_eq!(value, json!([{ "value": 7 }]));

            let first_backend = runtime.query(&AxRawSqlRequest {
                sql: "select pg_backend_pid()::bigint as pid".to_string(),
                params: Vec::new(),
            })?;
            let second_backend = runtime.query(&AxRawSqlRequest {
                sql: "select pg_backend_pid()::bigint as pid".to_string(),
                params: Vec::new(),
            })?;
            assert_eq!(first_backend[0]["pid"], second_backend[0]["pid"]);

            let inserted = runtime.insert(&AxInsertRequest {
                collection: table.clone(),
                fields: BTreeMap::from([
                    ("title".to_string(), json!("Foundry")),
                    ("published".to_string(), json!(false)),
                ]),
            })?;
            assert_eq!(inserted["changes"], 1);

            let rows = runtime.load(&AxQueryRequest {
                collection: table.clone(),
                filters: vec![AxQueryFilterRequest {
                    field: "title".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("Foundry"),
                }],
                orders: Vec::new(),
                limit: None,
                offset: None,
                mode: AxQueryMode::Many,
            })?;
            assert_eq!(rows[0]["published"], false);

            let updated = runtime.update(&AxUpdateRequest {
                collection: table.clone(),
                fields: BTreeMap::from([("published".to_string(), json!(true))]),
                filters: vec![AxQueryFilterRequest {
                    field: "title".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("Foundry"),
                }],
            })?;
            assert_eq!(updated["changes"], 1);

            let deleted = runtime.delete(&AxDeleteRequest {
                collection: table.clone(),
                filters: vec![AxQueryFilterRequest {
                    field: "title".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("Foundry"),
                }],
            })?;
            assert_eq!(deleted["changes"], 1);

            let transaction_results = runtime.transaction(&AxTransactionRequest {
                operations: vec![
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: table.clone(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Transaction A")),
                            ("published".to_string(), json!(false)),
                        ]),
                    }),
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: table.clone(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Transaction B")),
                            ("published".to_string(), json!(true)),
                        ]),
                    }),
                ],
            })?;
            assert_eq!(transaction_results.len(), 2);

            let rollback_error = runtime.transaction(&AxTransactionRequest {
                operations: vec![
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: table.clone(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Must Roll Back")),
                            ("published".to_string(), json!(false)),
                        ]),
                    }),
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: table.clone(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Transaction A")),
                            ("published".to_string(), json!(false)),
                        ]),
                    }),
                ],
            });
            assert!(matches!(
                rollback_error,
                Err(AxRuntimeError::Database { .. })
            ));
            let rolled_back = runtime.load(&AxQueryRequest {
                collection: table.clone(),
                filters: vec![AxQueryFilterRequest {
                    field: "title".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("Must Roll Back"),
                }],
                orders: Vec::new(),
                limit: None,
                offset: None,
                mode: AxQueryMode::Many,
            })?;
            assert_eq!(rolled_back, json!([]));
            Ok(())
        })();

        admin
            .batch_execute(&format!("drop table if exists \"{table}\""))
            .expect("postgres test table should clean up");
        result.expect("live postgres CRUD should execute");
    }

    #[test]
    fn sqlite_direct_load_reads_rows_from_database() {
        let (_path, url) = temp_sqlite_database("load");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let value = runtime
            .load(&AxQueryRequest {
                collection: "posts".to_string(),
                filters: vec![AxQueryFilterRequest {
                    field: "status".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("published"),
                }],
                orders: vec![AxQueryOrderRequest {
                    field: "id".to_string(),
                    direction: AxQueryOrderDirection::Asc,
                }],
                limit: Some(10),
                offset: None,
                mode: AxQueryMode::Many,
            })
            .expect("sqlite query should execute");

        assert_eq!(
            value,
            json!([
                {
                    "id": 1,
                    "title": "Hello",
                    "slug": "hello",
                    "status": "published"
                }
            ])
        );
    }

    #[test]
    fn sqlite_direct_first_load_reads_single_row_from_database() {
        let (_path, url) = temp_sqlite_database("first_load");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let value = runtime
            .load(&AxQueryRequest {
                collection: "posts".to_string(),
                filters: vec![AxQueryFilterRequest {
                    field: "status".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("published"),
                }],
                orders: vec![AxQueryOrderRequest {
                    field: "id".to_string(),
                    direction: AxQueryOrderDirection::Asc,
                }],
                limit: Some(10),
                offset: None,
                mode: AxQueryMode::First,
            })
            .expect("sqlite first query should execute");

        assert_eq!(
            value,
            json!({
                "id": 1,
                "title": "Hello",
                "slug": "hello",
                "status": "published"
            })
        );
    }

    #[test]
    fn sqlite_raw_query_reads_rows_with_params() {
        let (_path, url) = temp_sqlite_database("raw_query");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let value = runtime
            .query(&AxRawSqlRequest {
                sql: "select title from posts where status = ?".to_string(),
                params: vec![json!("published")],
            })
            .expect("raw sqlite query should execute");

        assert_eq!(value, json!([{ "title": "Hello" }]));
    }

    #[test]
    fn raw_query_rejects_mutating_sql() {
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", ":memory:"),
        )
        .expect("runtime should initialize");

        let error = runtime
            .query(&AxRawSqlRequest {
                sql: "delete from posts".to_string(),
                params: vec![],
            })
            .expect_err("raw query should only support reads");

        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.invalid_query");
    }

    #[test]
    fn raw_query_rejects_mutating_ctes() {
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", ":memory:"),
        )
        .expect("runtime should initialize");

        for sql in [
            "with selected as (select 1) delete from posts returning id",
            "with selected as (select 1) update posts set title = 'Changed' returning id",
            "with selected as (select 1) insert into posts(title) values ('Injected') returning id",
        ] {
            let error = runtime
                .query(&AxRawSqlRequest {
                    sql: sql.to_string(),
                    params: vec![],
                })
                .expect_err("mutating CTE should be rejected");
            assert!(matches!(error, AxRuntimeError::Database { .. }));
        }
    }

    #[test]
    fn raw_query_allows_read_only_ctes_and_semicolons_inside_strings() {
        validate_raw_select_sql(
            &AxDatabaseDriver::Sqlite,
            "with selected as (select ';' as marker) select marker from selected;",
        )
        .expect("read-only CTE should be accepted");
    }

    #[test]
    fn sqlite_direct_mutations_write_to_database() {
        let (_path, url) = temp_sqlite_database("mutations");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let inserted = runtime
            .insert(&AxInsertRequest {
                collection: "posts".to_string(),
                fields: BTreeMap::from([
                    ("title".to_string(), json!("Draft")),
                    ("slug".to_string(), json!("draft")),
                    ("status".to_string(), json!("draft")),
                ]),
            })
            .expect("sqlite insert should execute");
        assert_eq!(inserted["ok"], json!(true));
        assert_eq!(inserted["changes"], json!(1));

        let updated = runtime
            .update(&AxUpdateRequest {
                collection: "posts".to_string(),
                fields: BTreeMap::from([("status".to_string(), json!("published"))]),
                filters: vec![AxQueryFilterRequest {
                    field: "slug".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("draft"),
                }],
            })
            .expect("sqlite update should execute");
        assert_eq!(updated["changes"], json!(1));

        let deleted = runtime
            .delete(&AxDeleteRequest {
                collection: "posts".to_string(),
                filters: vec![AxQueryFilterRequest {
                    field: "slug".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!("draft"),
                }],
            })
            .expect("sqlite delete should execute");
        assert_eq!(deleted["changes"], json!(1));
    }

    #[test]
    fn sqlite_transaction_commits_all_mutations() {
        let (_path, url) = temp_sqlite_database("transaction_commit");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let results = runtime
            .transaction(&AxTransactionRequest {
                operations: vec![
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: "posts".to_string(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("First")),
                            ("slug".to_string(), json!("first")),
                            ("status".to_string(), json!("draft")),
                        ]),
                    }),
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: "posts".to_string(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Second")),
                            ("slug".to_string(), json!("second")),
                            ("status".to_string(), json!("published")),
                        ]),
                    }),
                ],
            })
            .expect("sqlite transaction should commit");

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result["changes"] == json!(1)));
        let rows = runtime
            .query(&AxRawSqlRequest {
                sql: "select slug from posts where slug in (?, ?) order by slug".to_string(),
                params: vec![json!("first"), json!("second")],
            })
            .expect("committed rows should be readable");
        assert_eq!(rows, json!([{ "slug": "first" }, { "slug": "second" }]));
    }

    #[test]
    fn sqlite_transaction_rolls_back_every_mutation_after_failure() {
        let (_path, url) = temp_sqlite_database("transaction_rollback");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let error = runtime
            .transaction(&AxTransactionRequest {
                operations: vec![
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: "posts".to_string(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Must roll back")),
                            ("slug".to_string(), json!("rolled-back")),
                            ("status".to_string(), json!("draft")),
                        ]),
                    }),
                    AxTransactionOperation::Insert(AxInsertRequest {
                        collection: "posts".to_string(),
                        fields: BTreeMap::from([
                            ("title".to_string(), json!("Duplicate")),
                            ("slug".to_string(), json!("hello")),
                            ("status".to_string(), json!("draft")),
                        ]),
                    }),
                ],
            })
            .expect_err("duplicate slug should roll back the transaction");

        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.unique_violation");
        let rows = runtime
            .query(&AxRawSqlRequest {
                sql: "select slug from posts where slug = ?".to_string(),
                params: vec![json!("rolled-back")],
            })
            .expect("rollback state should be readable");
        assert_eq!(rows, json!([]));
    }

    #[test]
    fn transaction_rejects_empty_operation_lists() {
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", ":memory:"),
        )
        .expect("runtime should initialize");

        let error = runtime
            .transaction(&AxTransactionRequest {
                operations: Vec::new(),
            })
            .expect_err("empty transaction should fail");
        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.invalid_query");
    }

    #[test]
    fn sqlite_migrations_apply_in_order_and_only_latest_can_roll_back() {
        let (_path, url) = temp_sqlite_database("migrations_order");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");
        let first = test_migration(
            "20260901_001",
            "create_posts",
            'a',
            "create table posts (id integer primary key, title text not null);",
            "drop table posts;",
        );
        let second = test_migration(
            "20260901_002",
            "create_audit",
            'b',
            "create table audit (id integer primary key, event text not null);",
            "drop table audit;",
        );

        runtime
            .apply_migration(&first)
            .expect("first migration should apply");
        runtime
            .apply_migration(&second)
            .expect("second migration should apply");
        let history = runtime.migration_history().expect("history should load");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].version, first.version);
        assert_eq!(history[1].version, second.version);

        let error = runtime
            .rollback_migration(&first)
            .expect_err("non-latest migration must not roll back");
        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.migration_conflict");

        runtime
            .rollback_migration(&second)
            .expect("latest migration should roll back");
        let tables = runtime
            .query(&AxRawSqlRequest {
                sql: "select name from sqlite_master where type = 'table' and name = 'audit'"
                    .to_string(),
                params: Vec::new(),
            })
            .expect("table state should load");
        assert_eq!(tables, json!([]));
        assert_eq!(
            runtime
                .migration_history()
                .expect("history should reload")
                .len(),
            1
        );
    }

    #[test]
    fn sqlite_migration_batch_is_idempotent_after_lock_recheck() {
        let (_path, url) = temp_sqlite_database("migration_batch_idempotent");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");
        let migrations = vec![
            test_migration(
                "20260902_001",
                "create_posts",
                'a',
                "create table posts (id integer primary key);",
                "drop table posts;",
            ),
            test_migration(
                "20260902_002",
                "create_audit",
                'b',
                "create table audit (id integer primary key);",
                "drop table audit;",
            ),
        ];

        let first = runtime
            .apply_migrations(&migrations)
            .expect("first batch should apply");
        let second = runtime
            .apply_migrations(&migrations)
            .expect("repeated batch should be a no-op");

        assert_eq!(first.len(), 2);
        assert!(second.is_empty());
        assert_eq!(
            runtime
                .migration_history()
                .expect("history should load")
                .len(),
            2
        );
    }

    #[test]
    fn failed_sqlite_migration_batch_rolls_back_every_pending_version() {
        let (_path, url) = temp_sqlite_database("migration_batch_atomic_failure");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");
        let migrations = vec![
            test_migration(
                "20260902_001",
                "create_posts",
                'a',
                "create table posts (id integer primary key);",
                "drop table posts;",
            ),
            test_migration(
                "20260902_002",
                "broken",
                'b',
                "create table partial_write (id integer primary key); invalid migration sql;",
                "drop table partial_write;",
            ),
        ];

        runtime
            .apply_migrations(&migrations)
            .expect_err("invalid migration batch should fail");

        let tables = runtime
            .query(&AxRawSqlRequest {
                sql: "select name from sqlite_master where type = 'table' and name in ('posts', 'partial_write')"
                    .to_string(),
                params: Vec::new(),
            })
            .expect("table state should load");
        assert_eq!(tables, json!([]));
        assert!(runtime
            .migration_history()
            .expect("history should load")
            .is_empty());
    }

    #[test]
    fn concurrent_sqlite_migration_batches_apply_each_version_once() {
        let (_path, url) = temp_sqlite_database("migration_batch_concurrent");
        let migration = test_migration(
            "20260902_001",
            "create_lock_probe",
            'a',
            "create table migration_lock_probe (id integer primary key);",
            "drop table migration_lock_probe;",
        );
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let url = url.clone();
                let migration = migration.clone();
                std::thread::spawn(move || {
                    let runtime = runtime_from_env(
                        AxEnv::new()
                            .with_secret("db_dialect", "sqlite")
                            .with_secret("db_url", &url),
                    )
                    .expect("runtime should initialize");
                    barrier.wait();
                    runtime.apply_migrations(&[migration])
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();

        let results = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("migration thread should finish")
                    .expect("migration batch should succeed")
            })
            .collect::<Vec<_>>();
        let applied_count = results.iter().map(Vec::len).sum::<usize>();
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        assert_eq!(applied_count, 1);
        assert_eq!(
            runtime
                .migration_history()
                .expect("history should load")
                .len(),
            1
        );
    }

    #[test]
    fn sqlite_migration_history_is_read_only_before_first_apply() {
        let (path, url) = temp_sqlite_database("migration_history_read_only");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        assert!(runtime
            .migration_history()
            .expect("empty history should load")
            .is_empty());

        let connection = rusqlite::Connection::open(path).expect("sqlite should open");
        let tracking_tables: i64 = connection
            .query_row(
                "select count(*) from sqlite_master where type = 'table' and name = '_axonyx_migrations'",
                [],
                |row| row.get(0),
            )
            .expect("tracking table count should load");
        assert_eq!(tracking_tables, 0);
    }

    #[test]
    fn sqlite_migration_rejects_changed_applied_checksum() {
        let (_path, url) = temp_sqlite_database("migration_checksum");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");
        let migration = test_migration(
            "20260901_001",
            "create_posts",
            'a',
            "create table posts (id integer primary key);",
            "drop table posts;",
        );
        runtime
            .apply_migration(&migration)
            .expect("migration should apply");
        let changed = AxMigration {
            checksum: "b".repeat(64),
            ..migration
        };

        let error = runtime
            .apply_migration(&changed)
            .expect_err("changed migration should fail");
        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.migration_checksum_mismatch");
    }

    #[test]
    fn sqlite_failed_migration_rolls_back_schema_and_history() {
        let (_path, url) = temp_sqlite_database("migration_atomic_failure");
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");
        let migration = test_migration(
            "20260901_001",
            "broken",
            'a',
            "create table partial_write (id integer primary key); invalid migration sql;",
            "drop table partial_write;",
        );

        runtime
            .apply_migration(&migration)
            .expect_err("invalid migration should fail");
        let tables = runtime
            .query(&AxRawSqlRequest {
                sql:
                    "select name from sqlite_master where type = 'table' and name = 'partial_write'"
                        .to_string(),
                params: Vec::new(),
            })
            .expect("table state should load");
        assert_eq!(tables, json!([]));
        assert!(runtime
            .migration_history()
            .expect("history should load")
            .is_empty());
    }

    #[test]
    fn migration_rejects_author_managed_transaction_control() {
        let migration = test_migration(
            "20260901_001",
            "unsafe_transaction",
            'a',
            "begin; create table posts (id integer primary key); commit;",
            "drop table posts;",
        );

        let error = validate_migration(&migration).expect_err("transaction SQL should fail");
        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.invalid_query");
    }

    #[test]
    fn migration_transaction_guard_handles_comments_literals_and_postgres_blocks() {
        let safe = test_migration(
            "20260901_001",
            "safe_literals",
            'a',
            "insert into audit (message) values ('ok; rollback later');\n\
             create function demo() returns void as $$ begin perform 1; commit; end $$ language plpgsql;",
            "drop function demo; drop table audit;",
        );
        validate_migration(&safe).expect("transaction words in literals should be safe");

        for sql in [
            "/* migration comment */ commit;",
            "-- migration comment\nstart transaction;",
            "set transaction isolation level serializable;",
            "abort;",
        ] {
            let migration =
                test_migration("20260901_002", "unsafe_transaction", 'b', sql, "select 1;");
            let error = validate_migration(&migration)
                .expect_err("author transaction control should be rejected");
            let AxRuntimeError::Database { error } = error else {
                panic!("expected database error");
            };
            assert_eq!(error.code, "db.invalid_query");
        }
    }

    #[test]
    fn sqlite_migrations_reject_ephemeral_memory_databases() {
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", ":memory:"),
        )
        .expect("runtime should initialize");

        let error = runtime
            .migration_history()
            .expect_err("ephemeral migration history should fail");
        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.invalid_query");
        assert!(error
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("persistent SQLite file")));
    }

    #[test]
    fn sqlite_direct_errors_use_db_translator() {
        let (_path, url) = temp_sqlite_database("errors");
        seed_sqlite_posts(&url);
        let runtime = runtime_from_env(
            AxEnv::new()
                .with_secret("db_dialect", "sqlite")
                .with_secret("db_url", &url),
        )
        .expect("runtime should initialize");

        let error = runtime
            .insert(&AxInsertRequest {
                collection: "posts".to_string(),
                fields: BTreeMap::from([
                    ("title".to_string(), json!("Again")),
                    ("slug".to_string(), json!("hello")),
                    ("status".to_string(), json!("draft")),
                ]),
            })
            .expect_err("duplicate slug should fail");

        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.unique_violation");
        assert_eq!(error.field.as_deref(), Some("slug"));
        assert!(!error.public_payload().to_string().contains("UNIQUE"));
    }

    #[test]
    fn direct_sql_errors_become_database_errors() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "sqlite")
            .with_secret("db_url", "sqlite://local/axonyx.db");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let error = runtime
            .delete(&AxDeleteRequest {
                collection: "posts".to_string(),
                filters: Vec::new(),
            })
            .expect_err("delete without filters should fail safely");

        let AxRuntimeError::Database { error } = error else {
            panic!("expected database error");
        };
        assert_eq!(error.code, "db.invalid_query");
        assert_eq!(error.status, 400);
        assert_eq!(error.driver.as_deref(), Some("sqlite"));
        assert_eq!(error.resource.as_deref(), Some("posts"));
        assert!(error
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("delete")));
        assert!(!error.public_payload().to_string().contains("delete"));
    }

    #[test]
    fn runtime_defaults_to_postgres_when_driver_is_missing() {
        let env = AxEnv::new().with_secret("db_url", "postgres://local/axonyx");
        let config = env.database_config().expect("config should resolve");

        assert_eq!(config.driver, AxDatabaseDriver::Postgres);
        assert_eq!(config.transport, AxDataTransport::Direct);
    }

    #[test]
    fn database_driver_maps_to_sql_dialect() {
        assert_eq!(
            AxDatabaseDriver::Postgres.sql_dialect(),
            Some(AxSqlDialect::Postgres)
        );
        assert_eq!(
            AxDatabaseDriver::MySql.sql_dialect(),
            Some(AxSqlDialect::MySql)
        );
        assert_eq!(
            AxDatabaseDriver::Sqlite.sql_dialect(),
            Some(AxSqlDialect::Sqlite)
        );
        assert_eq!(AxDatabaseDriver::Memory.sql_dialect(), None);
    }

    #[test]
    fn env_can_resolve_sql_dialect_from_driver() {
        let env = AxEnv::new().with_secret("db_driver", "sqlite");

        assert_eq!(
            env.sql_dialect().expect("sql dialect should resolve"),
            Some(AxSqlDialect::Sqlite)
        );
    }

    #[test]
    fn env_defaults_transport_to_direct() {
        let env = AxEnv::new().with_secret("db_url", "postgres://local/axonyx");

        assert_eq!(
            env.data_transport().expect("transport should resolve"),
            AxDataTransport::Direct
        );
    }

    #[test]
    fn env_can_resolve_api_transport_config() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_transport", "api")
            .with_secret("data_api_key", "secret-token")
            .with_public("data_api_url", "https://data.example.com");

        let config = env.database_config().expect("config should resolve");

        assert_eq!(config.driver, AxDatabaseDriver::Postgres);
        assert_eq!(config.transport, AxDataTransport::Api);
        assert_eq!(config.api_url.as_deref(), Some("https://data.example.com"));
        assert_eq!(config.api_key.as_deref(), Some("secret-token"));
    }

    #[test]
    fn direct_transport_requires_db_url() {
        let config = AxDatabaseConfig {
            driver: AxDatabaseDriver::Postgres,
            transport: AxDataTransport::Direct,
            url: None,
            api_url: None,
            api_key: None,
            pool_max_size: DEFAULT_DB_POOL_MAX_SIZE,
            pool_timeout_ms: DEFAULT_DB_POOL_TIMEOUT_MS,
            policy: AxDatabasePolicy::default(),
        };

        let error = config
            .validate()
            .expect_err("direct transport should require db url");
        assert_eq!(
            error,
            AxRuntimeError::message("missing AX_SECRET_DB_URL for direct data transport")
        );
    }

    #[test]
    fn api_transport_requires_api_fields() {
        let config = AxDatabaseConfig {
            driver: AxDatabaseDriver::Postgres,
            transport: AxDataTransport::Api,
            url: None,
            api_url: None,
            api_key: None,
            pool_max_size: DEFAULT_DB_POOL_MAX_SIZE,
            pool_timeout_ms: DEFAULT_DB_POOL_TIMEOUT_MS,
            policy: AxDatabasePolicy::default(),
        };

        let error = config
            .validate()
            .expect_err("api transport should require api url");
        assert_eq!(
            error,
            AxRuntimeError::message("missing AX_PUBLIC_DATA_API_URL for api data transport")
        );
    }

    #[test]
    fn runtime_from_env_validates_api_transport_requirements() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_transport", "api")
            .with_public("data_api_url", "https://data.example.com");

        let error = match runtime_from_env(env) {
            Ok(_) => panic!("missing api key should fail"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            AxRuntimeError::message("missing AX_SECRET_DATA_API_KEY for api data transport")
        );
    }

    #[test]
    fn lazy_runtime_allows_routes_without_database_configuration() {
        let runtime = lazy_runtime_from_env(AxEnv::new())
            .expect("non-database routes should be able to initialize a lazy runtime");

        assert!(runtime.env().public.is_empty());
        assert!(runtime.env().secret.is_empty());
    }

    fn test_migration(
        version: &str,
        name: &str,
        checksum_char: char,
        up_sql: &str,
        down_sql: &str,
    ) -> AxMigration {
        AxMigration {
            version: version.to_string(),
            name: name.to_string(),
            checksum: checksum_char.to_string().repeat(64),
            up_sql: up_sql.to_string(),
            down_sql: down_sql.to_string(),
        }
    }

    fn temp_sqlite_database(name: &str) -> (std::path::PathBuf, String) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "axonyx-runtime-{name}-{}-{nanos}.db",
            std::process::id(),
        ));
        let _ = fs::remove_file(&path);
        let url = path.to_string_lossy().to_string();
        (path, url)
    }

    fn seed_sqlite_posts(url: &str) {
        let connection =
            rusqlite::Connection::open(sqlite_database_path(url)).expect("sqlite should open");
        connection
            .execute(
                "create table posts (
                    id integer primary key autoincrement,
                    title text not null,
                    slug text not null unique,
                    status text not null
                )",
                [],
            )
            .expect("posts table should create");
        connection
            .execute(
                "insert into posts (title, slug, status) values (?1, ?2, ?3)",
                ("Hello", "hello", "published"),
            )
            .expect("published post should insert");
        connection
            .execute(
                "insert into posts (title, slug, status) values (?1, ?2, ?3)",
                ("Hidden", "hidden", "draft"),
            )
            .expect("draft post should insert");
    }
}
