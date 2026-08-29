use std::collections::BTreeMap;
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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxDbErrorCode {
    UnknownResource,
    UniqueViolation,
    ConstraintViolation,
    InvalidQuery,
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
            Self::ConnectionFailed => "Database connection failed.",
            Self::Timeout => "Database operation timed out.",
            Self::DriverError => "Database operation failed.",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::UnknownResource | Self::InvalidQuery => 400,
            Self::UniqueViolation | Self::ConstraintViolation => 409,
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
            } else if normalized.contains("unable to open database file")
                || normalized.contains("database is locked")
            {
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
            } else if normalized.contains("timeout") {
                AxDbErrorCode::Timeout
            } else if normalized.contains("connection refused")
                || normalized.contains("could not connect")
                || normalized.contains("connection error")
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
        })
    }

    pub fn sql_dialect(&self) -> AxRuntimeResult<Option<AxSqlDialect>> {
        Ok(self.database_driver()?.sql_dialect())
    }
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

pub trait AxDatabaseAdapter {
    fn driver(&self) -> AxDatabaseDriver;
    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value>;
    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value>;
    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value>;
    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value>;
    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value>;
}

impl<T> AxDatabaseAdapter for Box<T>
where
    T: AxDatabaseAdapter + ?Sized,
{
    fn driver(&self) -> AxDatabaseDriver {
        (**self).driver()
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
}

pub trait AxQueryExecutor {
    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value>;
    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value>;
}

pub trait AxMutationExecutor {
    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value>;
    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value>;
    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value>;
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
}

impl<A> AxDatabaseRuntime<A> {
    pub fn new(env: AxEnv, adapter: A) -> Self {
        Self { env, adapter }
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
        self.adapter.load(request)
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        self.adapter.query(request)
    }
}

impl<A> AxMutationExecutor for AxDatabaseRuntime<A>
where
    A: AxDatabaseAdapter,
{
    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        self.adapter.insert(request)
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        self.adapter.update(request)
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        self.adapter.delete(request)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostgresAdapter {
    pub url: Option<String>,
    pub transport: AxDataTransport,
    pub api_url: Option<String>,
}

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
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryAdapter;

impl AxDatabaseAdapter for PostgresAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::Postgres
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        dispatch_load(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        dispatch_raw_query(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        dispatch_insert(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        dispatch_update(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        dispatch_delete(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }
}

impl AxDatabaseAdapter for MySqlAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::MySql
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        dispatch_load(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        dispatch_raw_query(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        dispatch_insert(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        dispatch_update(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        dispatch_delete(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }
}

impl AxDatabaseAdapter for SqliteAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::Sqlite
    }

    fn load(&self, request: &AxQueryRequest) -> AxRuntimeResult<Value> {
        dispatch_load(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn query(&self, request: &AxRawSqlRequest) -> AxRuntimeResult<Value> {
        dispatch_raw_query(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn insert(&self, request: &AxInsertRequest) -> AxRuntimeResult<Value> {
        dispatch_insert(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn update(&self, request: &AxUpdateRequest) -> AxRuntimeResult<Value> {
        dispatch_update(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }

    fn delete(&self, request: &AxDeleteRequest) -> AxRuntimeResult<Value> {
        dispatch_delete(
            self.driver(),
            self.transport,
            &self.url,
            &self.api_url,
            request,
        )
    }
}

impl AxDatabaseAdapter for MemoryAdapter {
    fn driver(&self) -> AxDatabaseDriver {
        AxDatabaseDriver::Memory
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
        AxDatabaseDriver::Postgres => Box::new(PostgresAdapter {
            url: config.url.clone(),
            transport: config.transport,
            api_url: config.api_url.clone(),
        }),
        AxDatabaseDriver::MySql => Box::new(MySqlAdapter {
            url: config.url.clone(),
            transport: config.transport,
            api_url: config.api_url.clone(),
        }),
        AxDatabaseDriver::Sqlite => Box::new(SqliteAdapter {
            url: config.url.clone(),
            transport: config.transport,
            api_url: config.api_url.clone(),
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

fn dispatch_load(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    request: &AxQueryRequest,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => direct_load_plan(driver, url, request),
        AxDataTransport::Api => api_load_plan(driver, api_url, request),
    }
}

fn dispatch_raw_query(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    request: &AxRawSqlRequest,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => direct_raw_query_plan(driver, url, request),
        AxDataTransport::Api => api_raw_query_plan(driver, api_url, request),
    }
}

fn dispatch_insert(
    driver: AxDatabaseDriver,
    transport: AxDataTransport,
    url: &Option<String>,
    api_url: &Option<String>,
    request: &AxInsertRequest,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => direct_insert_plan(driver, url, request),
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
    request: &AxUpdateRequest,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => direct_update_plan(driver, url, request),
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
    request: &AxDeleteRequest,
) -> AxRuntimeResult<Value> {
    match transport {
        AxDataTransport::Direct => direct_delete_plan(driver, url, request),
        AxDataTransport::Api => api_delete_plan(driver, api_url, request),
    }
}

fn direct_load_plan(
    driver: AxDatabaseDriver,
    url: &Option<String>,
    request: &AxQueryRequest,
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

    if driver == AxDatabaseDriver::Sqlite {
        return sqlite_execute_query(url, &request.collection, &execution.sql, &execution.params)
            .map(|value| apply_query_mode(request.mode, value));
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
    request: &AxRawSqlRequest,
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

    if driver == AxDatabaseDriver::Sqlite {
        return sqlite_execute_query(url, "raw_sql", &execution.sql, &execution.params);
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
    request: &AxInsertRequest,
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

    if driver == AxDatabaseDriver::Sqlite {
        return sqlite_execute_mutation(
            url,
            "insert",
            &request.collection,
            &execution.sql,
            &execution.params,
        );
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
    request: &AxUpdateRequest,
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

    if driver == AxDatabaseDriver::Sqlite {
        return sqlite_execute_mutation(
            url,
            "update",
            &request.collection,
            &execution.sql,
            &execution.params,
        );
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
    request: &AxDeleteRequest,
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

    if driver == AxDatabaseDriver::Sqlite {
        return sqlite_execute_mutation(
            url,
            "delete",
            &request.collection,
            &execution.sql,
            &execution.params,
        );
    }

    Ok(json!({
        "driver": driver.as_str(),
        "transport": "direct",
        "action": "delete",
        "execution": execution,
    }))
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
) -> AxRuntimeResult<Value> {
    let connection = sqlite_open_connection(url, resource)?;
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
) -> AxRuntimeResult<Value> {
    let connection = sqlite_open_connection(url, resource)?;
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

fn sqlite_open_connection(
    url: &Option<String>,
    resource: &str,
) -> AxRuntimeResult<rusqlite::Connection> {
    let Some(url) = url else {
        return Err(AxRuntimeError::database(
            AxDbError::new(AxDbErrorCode::ConnectionFailed)
                .with_driver(AxDatabaseDriver::Sqlite)
                .with_resource(resource)
                .with_detail("missing sqlite database url"),
        ));
    };

    rusqlite::Connection::open(sqlite_database_path(url))
        .map_err(|error| sqlite_runtime_error(resource, error))
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
    pub use super::AxBackendRuntime;
    pub use super::AxDataTransport;
    pub use super::AxDatabaseAdapter;
    pub use super::AxDatabaseConfig;
    pub use super::AxDatabaseDriver;
    pub use super::AxDatabaseRuntime;
    pub use super::AxDbError;
    pub use super::AxDbErrorCode;
    pub use super::AxDeleteRequest;
    pub use super::AxEnv;
    pub use super::AxInsertRequest;
    pub use super::AxLoaderContext;
    pub use super::AxMessenger;
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
    pub use super::AxUpdateRequest;
    pub use super::MemoryAdapter;
    pub use super::MySqlAdapter;
    pub use super::PostgresAdapter;
    pub use super::SqliteAdapter;
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

        assert_eq!(sqlite.code, "db.connection_failed");
        assert_eq!(sqlite.status, 503);
        assert_eq!(postgres.code, "db.timeout");
        assert_eq!(postgres.status, 503);
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
            }
        );
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
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_url", "postgres://local/axonyx");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let value = runtime
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
                limit: Some(12),
                offset: None,
                mode: AxQueryMode::Many,
            })
            .expect("query should execute");

        assert_eq!(value["transport"], "direct");
        assert_eq!(value["execution"]["dialect"], "postgres");
        assert_eq!(
            value["execution"]["sql"],
            r#"select * from "posts" where "status" = $1 order by "created_at" desc limit 12"#
        );
        assert_eq!(value["execution"]["params"][0], json!("published"));
    }

    #[test]
    fn direct_transport_emits_extended_filter_sql_plan() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_url", "postgres://local/axonyx");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let value = runtime
            .load(&AxQueryRequest {
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
            })
            .expect("query should execute");

        assert_eq!(
            value["execution"]["sql"],
            r#"select * from "posts" where "archived" != $1 and "status" in ($2, $3) and "deleted_at" is null and "published_at" is not null"#
        );
        assert_eq!(
            value["execution"]["params"],
            json!([true, "published", "featured"])
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
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_url", "postgres://local/axonyx");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let value = runtime
            .update(&AxUpdateRequest {
                collection: "posts".to_string(),
                fields: BTreeMap::from([("title".to_string(), json!("Hello"))]),
                filters: vec![AxQueryFilterRequest {
                    field: "id".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!(7),
                }],
            })
            .expect("update should execute");

        assert_eq!(
            value["execution"]["sql"],
            r#"update "posts" set "title" = $1 where "id" = $2"#
        );
        assert_eq!(value["execution"]["params"][0], json!("Hello"));
        assert_eq!(value["execution"]["params"][1], json!(7));
    }

    #[test]
    fn direct_delete_emits_where_clause_when_filters_exist() {
        let env = AxEnv::new()
            .with_secret("db_dialect", "postgres")
            .with_secret("db_url", "postgres://local/axonyx");
        let runtime = runtime_from_env(env).expect("runtime should initialize");

        let value = runtime
            .delete(&AxDeleteRequest {
                collection: "posts".to_string(),
                filters: vec![AxQueryFilterRequest {
                    field: "id".to_string(),
                    op: AxQueryFilterOp::Eq,
                    value: json!(7),
                }],
            })
            .expect("delete should execute");

        assert_eq!(
            value["execution"]["sql"],
            r#"delete from "posts" where "id" = $1"#
        );
        assert_eq!(value["execution"]["params"][0], json!(7));
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
