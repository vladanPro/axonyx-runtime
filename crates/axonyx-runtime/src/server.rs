use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::backend::AxEnv;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxServerMode {
    Dev,
    Start,
}

impl AxServerMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Start => "start",
        }
    }

    pub fn inject_dev_client(self) -> bool {
        matches!(self, Self::Dev)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxServerConfig {
    pub host: String,
    pub port: u16,
    pub mode: AxServerMode,
}

impl AxServerConfig {
    pub fn new(host: impl Into<String>, port: u16, mode: AxServerMode) -> Self {
        Self {
            host: host.into(),
            port,
            mode,
        }
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxHttpRequest {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub struct AxAuth;

impl AxAuth {
    pub fn bearer<'a>(request: &'a AxHttpRequest) -> Option<&'a str> {
        request.bearer_token()
    }

    pub fn session<'a>(request: &'a AxHttpRequest) -> Option<&'a str> {
        request.cookie_value("session")
    }

    pub fn signed_session<'a>(request: &'a AxHttpRequest, secret: &str) -> Option<&'a str> {
        let cookie = request.cookie_value("session")?;
        let (value, signature) = cookie.rsplit_once('.')?;
        Self::verify_signature(value, signature, secret).then_some(value)
    }

    pub fn sign_session(value: &str, secret: &str) -> String {
        format!("{value}.{}", Self::signature_hex(value, secret))
    }

    fn verify_signature(value: &str, signature: &str, secret: &str) -> bool {
        let Some(signature) = hex_decode(signature) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(value.as_bytes());
        mac.verify_slice(&signature).is_ok()
    }

    fn signature_hex(value: &str, secret: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(value.as_bytes());
        hex_encode(&mac.finalize().into_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxRequestContext {
    pub request: AxHttpRequest,
    pub route: String,
    pub env: Option<AxEnv>,
}

impl AxRequestContext {
    pub fn new(request: AxHttpRequest, route: impl Into<String>) -> Self {
        Self {
            request,
            route: route.into(),
            env: None,
        }
    }

    pub fn request(&self) -> &AxHttpRequest {
        &self.request
    }

    pub fn with_env(mut self, env: AxEnv) -> Self {
        self.env = Some(env);
        self
    }

    pub fn env(&self) -> Option<&AxEnv> {
        self.env.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxResponseContext {
    pub response: AxHttpResponse,
}

impl AxResponseContext {
    pub fn new(response: AxHttpResponse) -> Self {
        Self { response }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.response = self.response.with_header(name, value);
        self
    }

    pub fn with_cookie(mut self, cookie: AxCookie) -> Self {
        self.response = self.response.with_cookie(cookie);
        self
    }

    pub fn into_response(self) -> AxHttpResponse {
        self.response
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxMiddlewareResult {
    Continue,
    Stop(AxHttpResponse),
}

impl AxMiddlewareResult {
    pub fn unauthorized() -> Self {
        Self::Stop(
            AxHttpResponse::json(401, &serde_json::json!({ "error": "unauthorized" }))
                .expect("static unauthorized JSON should serialize"),
        )
    }
}

pub type AxBeforeMiddleware = fn(&AxRequestContext) -> AxMiddlewareResult;
pub type AxAfterMiddleware = fn(&AxRequestContext, AxResponseContext) -> AxResponseContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxMiddlewarePhase {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxUnknownMiddlewareHook {
    pub phase: AxMiddlewarePhase,
    pub hook: String,
}

impl AxUnknownMiddlewareHook {
    pub fn new(phase: AxMiddlewarePhase, hook: impl Into<String>) -> Self {
        Self {
            phase,
            hook: hook.into(),
        }
    }
}

impl fmt::Display for AxUnknownMiddlewareHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown {:?} middleware hook `{}`",
            self.phase, self.hook
        )
    }
}

impl Error for AxUnknownMiddlewareHook {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxRouteBuildError {
    pub route: String,
    pub source: AxUnknownMiddlewareHook,
}

impl AxRouteBuildError {
    pub fn new(route: impl Into<String>, source: AxUnknownMiddlewareHook) -> Self {
        Self {
            route: route.into(),
            source,
        }
    }
}

impl fmt::Display for AxRouteBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to build route `{}`: {}",
            self.route, self.source
        )
    }
}

impl Error for AxRouteBuildError {}

#[derive(Debug, Clone, Default)]
pub struct AxMiddlewareChain {
    before: Vec<AxBeforeMiddleware>,
    after: Vec<AxAfterMiddleware>,
}

impl AxMiddlewareChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn before(mut self, middleware: AxBeforeMiddleware) -> Self {
        self.before.push(middleware);
        self
    }

    pub fn after(mut self, middleware: AxAfterMiddleware) -> Self {
        self.after.push(middleware);
        self
    }

    pub fn try_builtin_hook(
        self,
        phase: AxMiddlewarePhase,
        hook: impl AsRef<str>,
    ) -> Result<Self, AxUnknownMiddlewareHook> {
        match (phase, hook.as_ref()) {
            (AxMiddlewarePhase::Before, "Auth.session") => {
                Ok(self.before(require_session_middleware))
            }
            (AxMiddlewarePhase::Before, "Auth.bearer") => {
                Ok(self.before(require_bearer_middleware))
            }
            (AxMiddlewarePhase::Before, "Auth.signedSession") => {
                Ok(self.before(require_signed_session_middleware))
            }
            (_, "Security.headers") => Ok(self.after(security_headers_middleware)),
            (_, "Cache.noStore") => Ok(self.after(no_store_middleware)),
            (phase, hook) => Err(AxUnknownMiddlewareHook::new(phase, hook)),
        }
    }

    pub fn try_builtin_hooks<'a>(
        mut self,
        hooks: impl IntoIterator<Item = (AxMiddlewarePhase, &'a str)>,
    ) -> Result<Self, AxUnknownMiddlewareHook> {
        for (phase, hook) in hooks {
            self = self.try_builtin_hook(phase, hook)?;
        }
        Ok(self)
    }

    pub fn run(
        &self,
        context: &AxRequestContext,
        handler: impl FnOnce(&AxRequestContext) -> AxHttpResponse,
    ) -> AxHttpResponse {
        for middleware in &self.before {
            match middleware(context) {
                AxMiddlewareResult::Continue => {}
                AxMiddlewareResult::Stop(response) => {
                    return self.run_after(context, AxResponseContext::new(response));
                }
            }
        }

        self.run_after(context, AxResponseContext::new(handler(context)))
    }

    pub fn run_after(
        &self,
        context: &AxRequestContext,
        mut response: AxResponseContext,
    ) -> AxHttpResponse {
        for middleware in &self.after {
            response = middleware(context, response);
        }
        response.into_response()
    }
}

pub fn security_headers_middleware(
    _context: &AxRequestContext,
    response: AxResponseContext,
) -> AxResponseContext {
    response
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("Referrer-Policy", "strict-origin-when-cross-origin")
}

pub fn no_store_middleware(
    _context: &AxRequestContext,
    response: AxResponseContext,
) -> AxResponseContext {
    response.with_header("Cache-Control", "no-store")
}

pub fn require_session_middleware(context: &AxRequestContext) -> AxMiddlewareResult {
    if AxAuth::session(context.request()).is_some() {
        AxMiddlewareResult::Continue
    } else {
        AxMiddlewareResult::unauthorized()
    }
}

pub fn require_bearer_middleware(context: &AxRequestContext) -> AxMiddlewareResult {
    if AxAuth::bearer(context.request()).is_some() {
        AxMiddlewareResult::Continue
    } else {
        AxMiddlewareResult::unauthorized()
    }
}

pub fn require_signed_session_middleware(context: &AxRequestContext) -> AxMiddlewareResult {
    let Some(env) = context.env() else {
        return AxMiddlewareResult::unauthorized();
    };
    let Ok(secret) = env.secret("session_key") else {
        return AxMiddlewareResult::unauthorized();
    };

    if AxAuth::signed_session(context.request(), &secret).is_some() {
        AxMiddlewareResult::Continue
    } else {
        AxMiddlewareResult::unauthorized()
    }
}

pub type AxRouteHandler = fn(&AxRequestContext) -> AxHttpResponse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxRouteHook {
    pub phase: AxMiddlewarePhase,
    pub hook: String,
}

impl AxRouteHook {
    pub fn new(phase: AxMiddlewarePhase, hook: impl Into<String>) -> Self {
        Self {
            phase,
            hook: hook.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AxRouteDefinition {
    pub method: String,
    pub path: String,
    pub middleware: AxMiddlewareChain,
    pub handler: AxRouteHandler,
}

impl AxRouteDefinition {
    pub fn new(
        method: impl Into<String>,
        path: impl Into<String>,
        handler: AxRouteHandler,
    ) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            middleware: AxMiddlewareChain::new(),
            handler,
        }
    }

    pub fn with_middleware(mut self, middleware: AxMiddlewareChain) -> Self {
        self.middleware = middleware;
        self
    }

    pub fn with_builtin_hooks(mut self, hooks: &[AxRouteHook]) -> Result<Self, AxRouteBuildError> {
        let route = self.path.clone();
        for hook in hooks {
            self.middleware = self
                .middleware
                .try_builtin_hook(hook.phase, hook.hook.as_str())
                .map_err(|error| AxRouteBuildError::new(route.clone(), error))?;
        }
        Ok(self)
    }

    pub fn matches(&self, request: &AxHttpRequest) -> bool {
        self.method.eq_ignore_ascii_case(&request.method)
            && request_path_without_query(&request.target) == self.path
    }

    pub fn handle(&self, request: AxHttpRequest, env: Option<AxEnv>) -> AxHttpResponse {
        let mut context = AxRequestContext::new(request, self.path.clone());
        if let Some(env) = env {
            context = context.with_env(env);
        }
        self.middleware.run(&context, self.handler)
    }
}

#[derive(Debug, Clone, Default)]
pub struct AxRouteTable {
    pub routes: Vec<AxRouteDefinition>,
    pub env: Option<AxEnv>,
}

impl AxRouteTable {
    pub fn new(routes: impl IntoIterator<Item = AxRouteDefinition>) -> Self {
        Self {
            routes: routes.into_iter().collect(),
            env: None,
        }
    }

    pub fn with_env(mut self, env: AxEnv) -> Self {
        self.env = Some(env);
        self
    }

    pub fn push(&mut self, route: AxRouteDefinition) {
        self.routes.push(route);
    }

    pub fn route_for(&self, request: &AxHttpRequest) -> Option<&AxRouteDefinition> {
        self.routes.iter().find(|route| route.matches(request))
    }

    pub fn handle(&self, request: AxHttpRequest) -> Option<AxHttpResponse> {
        self.route_for(&request)
            .map(|route| route.handle(request, self.env.clone()))
    }
}

#[derive(Debug, Clone)]
pub struct AxMemoryServerAdapter {
    pub table: AxRouteTable,
    pub config: AxServerConfig,
}

impl AxMemoryServerAdapter {
    pub fn new(config: AxServerConfig, routes: Vec<AxRouteDefinition>) -> Self {
        Self {
            table: AxRouteTable::new(routes),
            config,
        }
    }

    pub fn with_env(mut self, env: AxEnv) -> Self {
        self.table = self.table.with_env(env);
        self
    }

    pub fn handle(&self, request: AxHttpRequest) -> AxHttpResponse {
        self.table
            .handle(request)
            .unwrap_or_else(|| AxHttpResponse::text(404, "Not Found"))
    }
}

impl AxServerAdapter for AxMemoryServerAdapter {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn serve_routes(
        &self,
        _config: &AxServerConfig,
        _routes: Vec<AxRouteDefinition>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone)]
pub struct AxAxumServerAdapter {
    pub table: AxRouteTable,
    pub body_limit: usize,
}

#[cfg(feature = "axum")]
impl AxAxumServerAdapter {
    pub fn new(routes: Vec<AxRouteDefinition>) -> Self {
        Self {
            table: AxRouteTable::new(routes),
            body_limit: 1024 * 1024,
        }
    }

    pub fn with_env(mut self, env: AxEnv) -> Self {
        self.table = self.table.with_env(env);
        self
    }

    pub fn with_body_limit(mut self, body_limit: usize) -> Self {
        self.body_limit = body_limit;
        self
    }

    pub fn router(&self) -> axum::Router {
        axum_router_from_table_with_limit(self.table.clone(), self.body_limit)
    }
}

#[cfg(feature = "axum")]
impl AxServerAdapter for AxAxumServerAdapter {
    fn name(&self) -> &'static str {
        "axum"
    }

    fn serve_routes(
        &self,
        config: &AxServerConfig,
        routes: Vec<AxRouteDefinition>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let table = AxRouteTable::new(routes);
        let router = axum_router_from_table_with_limit(table, self.body_limit);
        let bind_addr = config.bind_addr();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_io()
            .build()?;

        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind(bind_addr).await?;
            axum::serve(listener, router).await?;
            Ok::<(), Box<dyn Error + Send + Sync>>(())
        })
    }
}

#[cfg(feature = "axum")]
pub fn axum_router_from_table(table: AxRouteTable) -> axum::Router {
    axum_router_from_table_with_limit(table, 1024 * 1024)
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone)]
pub struct AxAxumRouter {
    pub table: AxRouteTable,
    pub body_limit: usize,
}

#[cfg(feature = "axum")]
impl AxAxumRouter {
    pub fn new(table: AxRouteTable) -> Self {
        Self {
            table,
            body_limit: 1024 * 1024,
        }
    }

    pub fn with_body_limit(mut self, body_limit: usize) -> Self {
        self.body_limit = body_limit;
        self
    }

    pub fn into_router(self) -> axum::Router {
        use std::sync::Arc;

        axum::Router::new()
            .fallback(axum::routing::any(axum_route_table_handler))
            .with_state(Arc::new(AxAxumState {
                table: self.table,
                body_limit: self.body_limit,
            }))
    }
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone)]
struct AxAxumState {
    table: AxRouteTable,
    body_limit: usize,
}

#[cfg(feature = "axum")]
pub fn axum_router_from_table_with_limit(table: AxRouteTable, body_limit: usize) -> axum::Router {
    use std::sync::Arc;

    axum::Router::new()
        .fallback(axum::routing::any(axum_route_table_handler))
        .with_state(Arc::new(AxAxumState { table, body_limit }))
}

#[cfg(feature = "axum")]
async fn axum_route_table_handler(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<AxAxumState>>,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    match axum_request_to_axonyx_with_limit(request, state.body_limit).await {
        Ok(request) => axonyx_response_to_axum(
            state
                .table
                .handle(request)
                .unwrap_or_else(|| AxHttpResponse::text(404, "Not Found")),
        ),
        Err(error) => axonyx_response_to_axum(AxHttpResponse::text(400, error.to_string())),
    }
}

#[cfg(feature = "axum")]
pub async fn axum_request_to_axonyx(
    request: axum::http::Request<axum::body::Body>,
) -> Result<AxHttpRequest, Box<dyn Error + Send + Sync>> {
    axum_request_to_axonyx_with_limit(request, usize::MAX).await
}

#[cfg(feature = "axum")]
pub async fn axum_request_to_axonyx_with_limit(
    request: axum::http::Request<axum::body::Body>,
    limit: usize,
) -> Result<AxHttpRequest, Box<dyn Error + Send + Sync>> {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, limit).await?;
    let target = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());
    let mut request = AxHttpRequest::new(parts.method.as_str(), target).with_body(body.to_vec());

    for (name, value) in parts.headers.iter() {
        if let Ok(value) = value.to_str() {
            request = request.with_header(name.as_str(), value);
        }
    }

    Ok(request)
}

#[cfg(feature = "axum")]
pub fn axonyx_response_to_axum(response: AxHttpResponse) -> axum::response::Response {
    use axum::http::header::{CONTENT_TYPE, SET_COOKIE};
    use axum::http::{HeaderName, HeaderValue, StatusCode};

    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
    let mut out = axum::response::Response::builder()
        .status(status)
        .header(CONTENT_TYPE, response.content_type.clone())
        .body(ax_body_to_axum_body(response.body))
        .expect("Axonyx response should build an Axum response");

    for (name, value) in response.headers {
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            continue;
        };
        out.headers_mut().insert(name, value);
    }

    for cookie in response.set_cookies {
        if let Ok(cookie) = HeaderValue::from_str(&cookie) {
            out.headers_mut().append(SET_COOKIE, cookie);
        }
    }

    out
}

#[cfg(feature = "axum")]
pub fn ax_body_to_axum_body(body: AxBody) -> axum::body::Body {
    match body {
        AxBody::Fixed(body) => axum::body::Body::from(body),
        AxBody::Chunks(chunks) => {
            let stream = futures_util::stream::iter(
                chunks
                    .into_iter()
                    .map(|chunk| Ok::<_, std::convert::Infallible>(bytes::Bytes::from(chunk))),
            );
            axum::body::Body::from_stream(stream)
        }
    }
}

pub trait AxServerAdapter {
    fn name(&self) -> &'static str;

    fn serve_routes(
        &self,
        config: &AxServerConfig,
        routes: Vec<AxRouteDefinition>,
    ) -> Result<(), Box<dyn Error + Send + Sync>>;
}

fn request_path_without_query(target: &str) -> &str {
    target
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(target)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }

    input
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let high = hex_value(chunk[0])?;
            let low = hex_value(chunk[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl AxHttpRequest {
    pub fn new(method: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            target: target.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn cookie_value(&self, name: &str) -> Option<&str> {
        self.header_value("Cookie").and_then(|cookies| {
            cookies.split(';').find_map(|pair| {
                let (key, value) = pair.trim().split_once('=')?;
                (key == name).then_some(value)
            })
        })
    }

    pub fn body_text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn form_value(&self, name: &str) -> Option<String> {
        parse_urlencoded_fields(&self.body_text_lossy()).remove(name)
    }

    pub fn json_field_value(&self, name: &str) -> Option<serde_json::Value> {
        let value = serde_json::from_slice::<serde_json::Value>(&self.body).ok()?;
        value.as_object()?.get(name).cloned()
    }

    pub fn json_field_string(&self, name: &str) -> Option<String> {
        self.json_field_value(name).map(|value| match value {
            serde_json::Value::Null => String::new(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::String(value) => value,
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => value.to_string(),
        })
    }

    pub fn bearer_token(&self) -> Option<&str> {
        let authorization = self.header_value("Authorization")?.trim();
        authorization
            .strip_prefix("Bearer ")
            .or_else(|| authorization.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())
    }
}

fn parse_urlencoded_fields(body: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for pair in body.split('&') {
        if pair.is_empty() {
            continue;
        }

        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        fields.insert(url_decode(key), url_decode(value));
    }
    fields
}

fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                    out.push(decoded as char);
                    index += 3;
                } else {
                    out.push('%');
                    index += 1;
                }
            }
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }

    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxHttpResponse {
    pub status: u16,
    pub content_type: String,
    pub headers: BTreeMap<String, String>,
    pub set_cookies: Vec<String>,
    pub body: AxBody,
}

impl AxHttpResponse {
    pub fn html(status: u16, body: impl Into<String>) -> Self {
        Self::new(status, "text/html; charset=utf-8", body.into().into_bytes())
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self::new(
            status,
            "text/plain; charset=utf-8",
            body.into().into_bytes(),
        )
    }

    pub fn bytes(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self::new(status, content_type, body)
    }

    pub fn json(status: u16, value: &impl Serialize) -> Result<Self, serde_json::Error> {
        Ok(Self::bytes(
            status,
            "application/json; charset=utf-8",
            serde_json::to_vec(value)?,
        ))
    }

    pub fn no_content() -> Self {
        Self::new(204, "text/plain; charset=utf-8", Vec::new())
    }

    pub fn redirect(location: impl Into<String>) -> Self {
        Self::redirect_with_status(303, location)
    }

    pub fn redirect_with_status(status: u16, location: impl Into<String>) -> Self {
        Self::text(status, "").with_header("Location", location)
    }

    pub fn new(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            headers: BTreeMap::new(),
            set_cookies: Vec::new(),
            body: AxBody::fixed(body),
        }
    }

    pub fn stream_chunks(
        status: u16,
        content_type: impl Into<String>,
        chunks: Vec<Vec<u8>>,
    ) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            headers: BTreeMap::new(),
            set_cookies: Vec::new(),
            body: AxBody::chunks(chunks),
        }
    }

    pub fn sse_events(events: impl IntoIterator<Item = AxSseEvent>) -> Self {
        Self::stream_chunks(
            200,
            "text/event-stream; charset=utf-8",
            events
                .into_iter()
                .map(|event| event.render().into_bytes())
                .collect::<Vec<_>>(),
        )
        .with_header("X-Accel-Buffering", "no")
        .with_no_store()
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn with_no_store(self) -> Self {
        self.with_header("Cache-Control", "no-store")
    }

    pub fn with_cookie(mut self, cookie: AxCookie) -> Self {
        self.set_cookies.push(cookie.render());
        self
    }

    pub fn without_cookie(self, name: impl Into<String>) -> Self {
        self.with_cookie(
            AxCookie::new(name, "")
                .with_path("/")
                .with_max_age(0)
                .http_only()
                .same_site("Lax"),
        )
    }

    pub fn status_line(&self) -> String {
        format!("{} {}", self.status, status_reason(self.status))
    }

    pub fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn body_len(&self) -> usize {
        self.body.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxCookie {
    pub name: String,
    pub value: String,
    pub path: Option<String>,
    pub domain: Option<String>,
    pub max_age: Option<i64>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<String>,
}

impl AxCookie {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            path: None,
            domain: None,
            max_age: None,
            http_only: false,
            secure: false,
            same_site: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    pub fn with_max_age(mut self, max_age: i64) -> Self {
        self.max_age = Some(max_age);
        self
    }

    pub fn http_only(mut self) -> Self {
        self.http_only = true;
        self
    }

    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    pub fn same_site(mut self, same_site: impl Into<String>) -> Self {
        self.same_site = Some(same_site.into());
        self
    }

    pub fn render(&self) -> String {
        let mut parts = vec![format!(
            "{}={}",
            sanitize_cookie_part(&self.name),
            sanitize_cookie_part(&self.value)
        )];
        if let Some(path) = &self.path {
            parts.push(format!("Path={}", sanitize_cookie_part(path)));
        }
        if let Some(domain) = &self.domain {
            parts.push(format!("Domain={}", sanitize_cookie_part(domain)));
        }
        if let Some(max_age) = self.max_age {
            parts.push(format!("Max-Age={max_age}"));
        }
        if self.http_only {
            parts.push("HttpOnly".to_string());
        }
        if self.secure {
            parts.push("Secure".to_string());
        }
        if let Some(same_site) = &self.same_site {
            parts.push(format!("SameSite={}", sanitize_cookie_part(same_site)));
        }
        parts.join("; ")
    }
}

fn sanitize_cookie_part(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, '\r' | '\n' | ';'))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxSseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry_ms: Option<u64>,
}

impl AxSseEvent {
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            event: None,
            data: data.into(),
            id: None,
            retry_ms: None,
        }
    }

    pub fn named(event: impl Into<String>, data: impl Into<String>) -> Self {
        Self::data(data).with_event(event)
    }

    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn with_retry(mut self, retry_ms: u64) -> Self {
        self.retry_ms = Some(retry_ms);
        self
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(id) = &self.id {
            push_sse_field(&mut out, "id", id);
        }
        if let Some(event) = &self.event {
            push_sse_field(&mut out, "event", event);
        }
        if let Some(retry_ms) = self.retry_ms {
            out.push_str("retry: ");
            out.push_str(&retry_ms.to_string());
            out.push('\n');
        }
        for line in self.data.lines() {
            push_sse_field(&mut out, "data", line);
        }
        if self.data.ends_with('\n') {
            push_sse_field(&mut out, "data", "");
        }
        out.push('\n');
        out
    }
}

fn push_sse_field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxBody {
    Fixed(Vec<u8>),
    Chunks(Vec<Vec<u8>>),
}

impl AxBody {
    pub fn fixed(body: Vec<u8>) -> Self {
        Self::Fixed(body)
    }

    pub fn chunks(chunks: Vec<Vec<u8>>) -> Self {
        Self::Chunks(chunks)
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Fixed(body) => body.len(),
            Self::Chunks(chunks) => chunks.iter().map(Vec::len).sum(),
        }
    }

    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Chunks(_))
    }

    pub fn chunks_iter(&self) -> AxBodyChunks<'_> {
        match self {
            Self::Fixed(body) => AxBodyChunks::Fixed(std::iter::once(body.as_slice())),
            Self::Chunks(chunks) => AxBodyChunks::Chunks(chunks.iter().map(Vec::as_slice)),
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Fixed(body) => body,
            Self::Chunks(chunks) => chunks.into_iter().flatten().collect(),
        }
    }
}

pub enum AxBodyChunks<'a> {
    Fixed(std::iter::Once<&'a [u8]>),
    Chunks(std::iter::Map<std::slice::Iter<'a, Vec<u8>>, fn(&'a Vec<u8>) -> &'a [u8]>),
}

impl<'a> Iterator for AxBodyChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Fixed(iter) => iter.next(),
            Self::Chunks(iter) => iter.next(),
        }
    }
}

pub fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        409 => "Conflict",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

pub trait AxServer {
    fn config(&self) -> &AxServerConfig;
    fn serve(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

pub mod prelude {
    pub use super::{
        no_store_middleware, require_bearer_middleware, require_session_middleware,
        require_signed_session_middleware, security_headers_middleware, status_reason,
        AxAfterMiddleware, AxAuth, AxBeforeMiddleware, AxBody, AxBodyChunks, AxCookie,
        AxHttpRequest, AxHttpResponse, AxMemoryServerAdapter, AxMiddlewareChain, AxMiddlewarePhase,
        AxMiddlewareResult, AxRequestContext, AxResponseContext, AxRouteBuildError,
        AxRouteDefinition, AxRouteHandler, AxRouteHook, AxRouteTable, AxServer, AxServerAdapter,
        AxServerConfig, AxServerMode, AxSseEvent, AxUnknownMiddlewareHook,
    };

    #[cfg(feature = "axum")]
    pub use super::{
        ax_body_to_axum_body, axonyx_response_to_axum, axum_request_to_axonyx,
        axum_request_to_axonyx_with_limit, axum_router_from_table,
        axum_router_from_table_with_limit, AxAxumRouter, AxAxumServerAdapter,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_handler(_context: &AxRequestContext) -> AxHttpResponse {
        AxHttpResponse::text(200, "ok")
    }

    #[test]
    fn server_config_builds_bind_address() {
        let config = AxServerConfig::new("127.0.0.1", 3000, AxServerMode::Dev);

        assert_eq!(config.bind_addr(), "127.0.0.1:3000");
        assert_eq!(config.mode.label(), "dev");
        assert!(config.mode.inject_dev_client());
    }

    #[test]
    fn response_helpers_preserve_headers_and_body() {
        let response =
            AxHttpResponse::html(200, "<h1>Hello</h1>").with_header("Cache-Control", "no-store");

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert_eq!(response.body_len(), "<h1>Hello</h1>".len());
        assert_eq!(
            response.body.chunks_iter().collect::<Vec<_>>(),
            vec![b"<h1>Hello</h1>".as_slice()]
        );
        assert_eq!(
            response.headers.get("Cache-Control").map(String::as_str),
            Some("no-store")
        );
    }

    #[test]
    fn request_helpers_read_headers_and_cookies() {
        let request = AxHttpRequest::new("GET", "/settings")
            .with_header("cookie", "theme=gold; session=abc123")
            .with_body(b"body".to_vec());

        assert_eq!(
            request.header_value("Cookie"),
            Some("theme=gold; session=abc123")
        );
        assert_eq!(request.cookie_value("theme"), Some("gold"));
        assert_eq!(request.cookie_value("session"), Some("abc123"));
        assert_eq!(request.cookie_value("missing"), None);
        assert_eq!(request.body, b"body".to_vec());
        assert_eq!(request.body_text_lossy(), "body");

        let form = AxHttpRequest::new("POST", "/form")
            .with_body(b"title=Hello+Axonyx&excerpt=Fast%20forms".to_vec());
        assert_eq!(form.form_value("title"), Some("Hello Axonyx".to_string()));
        assert_eq!(form.form_value("excerpt"), Some("Fast forms".to_string()));
        assert_eq!(form.form_value("missing"), None);

        let json = AxHttpRequest::new("POST", "/json")
            .with_body(br#"{"title":"Hello","count":3}"#.to_vec());
        assert_eq!(json.json_field_string("title"), Some("Hello".to_string()));
        assert_eq!(json.json_field_string("count"), Some("3".to_string()));

        let auth = AxHttpRequest::new("GET", "/admin").with_header("Authorization", "Bearer abc");
        assert_eq!(auth.bearer_token(), Some("abc"));
        assert_eq!(AxAuth::bearer(&auth), Some("abc"));

        let session = AxHttpRequest::new("GET", "/admin").with_header("Cookie", "session=s123");
        assert_eq!(AxAuth::session(&session), Some("s123"));

        let signed = AxAuth::sign_session("s123", "secret");
        let session =
            AxHttpRequest::new("GET", "/admin").with_header("Cookie", format!("session={signed}"));
        assert_eq!(AxAuth::signed_session(&session, "secret"), Some("s123"));
        assert_eq!(AxAuth::signed_session(&session, "wrong"), None);
    }

    #[test]
    fn middleware_chain_runs_before_handler_and_after_response() {
        let context = AxRequestContext::new(
            AxHttpRequest::new("GET", "/api/admin").with_header("Cookie", "session=abc123"),
            "/api/admin",
        );
        let chain = AxMiddlewareChain::new()
            .before(require_session_middleware)
            .after(security_headers_middleware)
            .after(no_store_middleware);

        let response = chain.run(&context, |_context| AxHttpResponse::text(200, "ok"));

        assert_eq!(response.status, 200);
        assert_eq!(
            response.header_value("X-Content-Type-Options"),
            Some("nosniff")
        );
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));
    }

    #[test]
    fn middleware_chain_can_stop_before_handler_but_still_runs_after_hooks() {
        let context = AxRequestContext::new(AxHttpRequest::new("GET", "/api/admin"), "/api/admin");
        let chain = AxMiddlewareChain::new()
            .before(require_session_middleware)
            .after(no_store_middleware);

        let response = chain.run(&context, |_context| {
            panic!("handler should not run when middleware stops")
        });

        assert_eq!(response.status, 401);
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));
    }

    #[test]
    fn middleware_chain_can_register_builtin_hooks_by_ax_name() {
        let context = AxRequestContext::new(
            AxHttpRequest::new("GET", "/api/admin").with_header("Authorization", "Bearer token"),
            "/api/admin",
        );
        let chain = AxMiddlewareChain::new()
            .try_builtin_hooks([
                (AxMiddlewarePhase::Before, "Auth.bearer"),
                (AxMiddlewarePhase::Before, "Security.headers"),
                (AxMiddlewarePhase::After, "Cache.noStore"),
            ])
            .expect("built-in hooks should register");

        let response = chain.run(&context, |_context| AxHttpResponse::text(200, "ok"));

        assert_eq!(response.status, 200);
        assert_eq!(
            response.header_value("X-Content-Type-Options"),
            Some("nosniff")
        );
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));
    }

    #[test]
    fn middleware_chain_reports_unknown_builtin_hooks() {
        let error = AxMiddlewareChain::new()
            .try_builtin_hook(AxMiddlewarePhase::Before, "Project.custom")
            .expect_err("custom hook should need future registry");

        assert_eq!(error.phase, AxMiddlewarePhase::Before);
        assert_eq!(error.hook, "Project.custom");
    }

    #[test]
    fn middleware_chain_can_require_signed_session_with_env_context() {
        let signed = AxAuth::sign_session("s123", "secret");
        let context = AxRequestContext::new(
            AxHttpRequest::new("GET", "/api/admin")
                .with_header("Cookie", format!("session={signed}")),
            "/api/admin",
        )
        .with_env(AxEnv::new().with_secret("session_key", "secret"));
        let chain = AxMiddlewareChain::new()
            .try_builtin_hook(AxMiddlewarePhase::Before, "Auth.signedSession")
            .expect("signed session hook should register");

        let response = chain.run(&context, |_context| AxHttpResponse::text(200, "ok"));

        assert_eq!(response.status, 200);

        let missing_env_context =
            AxRequestContext::new(AxHttpRequest::new("GET", "/api/admin"), "/api/admin");
        let response = chain.run(&missing_env_context, |_context| {
            panic!("handler should not run without env/secret")
        });

        assert_eq!(response.status, 401);
    }

    #[test]
    fn route_definition_matches_and_runs_through_middleware_chain() {
        let route = AxRouteDefinition::new("GET", "/api/admin", ok_handler)
            .with_builtin_hooks(&[
                AxRouteHook::new(AxMiddlewarePhase::Before, "Auth.session"),
                AxRouteHook::new(AxMiddlewarePhase::After, "Cache.noStore"),
            ])
            .expect("built-in hooks should register");

        let request = AxHttpRequest::new("GET", "/api/admin?tab=profile")
            .with_header("Cookie", "session=s123");
        assert!(route.matches(&request));

        let response = route.handle(request, None);

        assert_eq!(response.status, 200);
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));

        assert!(!route.matches(&AxHttpRequest::new("POST", "/api/admin")));
    }

    #[test]
    fn route_definition_reports_unknown_hook_with_route_context() {
        let error = AxRouteDefinition::new("GET", "/api/admin", ok_handler)
            .with_builtin_hooks(&[AxRouteHook::new(
                AxMiddlewarePhase::Before,
                "Project.custom",
            )])
            .expect_err("custom hooks should need future registry");

        assert_eq!(error.route, "/api/admin");
        assert_eq!(error.source.hook, "Project.custom");
    }

    #[test]
    fn route_table_dispatches_first_matching_route_with_shared_env() {
        let signed = AxAuth::sign_session("s123", "secret");
        let admin = AxRouteDefinition::new("GET", "/api/admin", ok_handler)
            .with_builtin_hooks(&[
                AxRouteHook::new(AxMiddlewarePhase::Before, "Auth.signedSession"),
                AxRouteHook::new(AxMiddlewarePhase::After, "Security.headers"),
            ])
            .expect("built-in hooks should register");
        let posts = AxRouteDefinition::new("GET", "/api/posts", ok_handler);
        let table = AxRouteTable::new([admin, posts])
            .with_env(AxEnv::new().with_secret("session_key", "secret"));

        let response = table
            .handle(
                AxHttpRequest::new("GET", "/api/admin")
                    .with_header("Cookie", format!("session={signed}")),
            )
            .expect("admin route should match");

        assert_eq!(response.status, 200);
        assert_eq!(
            response.header_value("X-Content-Type-Options"),
            Some("nosniff")
        );
        assert!(table
            .handle(AxHttpRequest::new("GET", "/api/missing"))
            .is_none());
    }

    #[test]
    fn memory_server_adapter_handles_routes_without_network() {
        let config = AxServerConfig::new("127.0.0.1", 3000, AxServerMode::Dev);
        let route = AxRouteDefinition::new("GET", "/api/posts", ok_handler)
            .with_builtin_hooks(&[AxRouteHook::new(AxMiddlewarePhase::After, "Cache.noStore")])
            .expect("built-in hooks should register");
        let adapter = AxMemoryServerAdapter::new(config, vec![route]);

        assert_eq!(adapter.name(), "memory");

        let response = adapter.handle(AxHttpRequest::new("GET", "/api/posts"));

        assert_eq!(response.status, 200);
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));

        let missing = adapter.handle(AxHttpRequest::new("GET", "/api/missing"));

        assert_eq!(missing.status, 404);
    }

    #[test]
    fn response_json_redirect_and_no_content_helpers() {
        let json = AxHttpResponse::json(201, &serde_json::json!({ "ok": true }))
            .expect("json response should serialize");
        assert_eq!(json.status_line(), "201 Created");
        assert_eq!(json.content_type, "application/json; charset=utf-8");
        assert_eq!(
            json.body.chunks_iter().collect::<Vec<_>>(),
            vec![br#"{"ok":true}"#.as_slice()]
        );

        let redirect = AxHttpResponse::redirect("/next");
        assert_eq!(redirect.status_line(), "303 See Other");
        assert_eq!(redirect.header_value("Location"), Some("/next"));

        let empty = AxHttpResponse::no_content();
        assert_eq!(empty.status_line(), "204 No Content");
        assert_eq!(empty.body_len(), 0);
    }

    #[test]
    fn response_cookie_helpers_render_set_cookie_headers() {
        let response = AxHttpResponse::text(200, "ok")
            .with_cookie(
                AxCookie::new("session", "abc123")
                    .with_path("/")
                    .http_only()
                    .secure()
                    .same_site("Lax"),
            )
            .without_cookie("flash");

        assert_eq!(
            response.set_cookies,
            vec![
                "session=abc123; Path=/; HttpOnly; Secure; SameSite=Lax".to_string(),
                "flash=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax".to_string(),
            ]
        );
    }

    #[test]
    fn response_knows_status_line_and_case_insensitive_headers() {
        let response = AxHttpResponse::text(303, "")
            .with_header("location", "/next")
            .with_no_store();

        assert_eq!(response.status_line(), "303 See Other");
        assert_eq!(response.header_value("Location"), Some("/next"));
        assert_eq!(response.header_value("cache-control"), Some("no-store"));
    }

    #[test]
    fn response_can_describe_streaming_chunks_without_transport() {
        let response = AxHttpResponse::stream_chunks(
            200,
            "text/html; charset=utf-8",
            vec![b"<main>".to_vec(), b"Hello".to_vec(), b"</main>".to_vec()],
        );

        assert!(response.body.is_streaming());
        assert_eq!(response.body_len(), "<main>Hello</main>".len());
        assert_eq!(
            response.body.chunks_iter().collect::<Vec<_>>(),
            vec![
                b"<main>".as_slice(),
                b"Hello".as_slice(),
                b"</main>".as_slice()
            ]
        );
    }

    #[test]
    fn sse_events_render_as_event_stream_chunks() {
        let response = AxHttpResponse::sse_events([
            AxSseEvent::named("state", r#"{"ready":true}"#)
                .with_id("1")
                .with_retry(1500),
            AxSseEvent::data("line one\nline two"),
        ]);

        assert_eq!(response.content_type, "text/event-stream; charset=utf-8");
        assert!(response.body.is_streaming());
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));
        assert_eq!(response.header_value("X-Accel-Buffering"), Some("no"));
        let body = response
            .body
            .chunks_iter()
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect::<String>();
        assert!(body.contains("id: 1\n"));
        assert!(body.contains("event: state\n"));
        assert!(body.contains("retry: 1500\n"));
        assert!(body.contains("data: {\"ready\":true}\n\n"));
        assert!(body.contains("data: line one\ndata: line two\n\n"));
    }

    #[cfg(feature = "axum")]
    #[test]
    fn axum_response_conversion_preserves_status_headers_cookies_and_body() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_io()
            .build()
            .expect("Tokio runtime should build for Axum adapter tests");

        runtime.block_on(async {
            let response = AxHttpResponse::stream_chunks(
                202,
                "text/plain; charset=utf-8",
                vec![b"hello ".to_vec(), b"axum".to_vec()],
            )
            .with_header("X-Axonyx", "foundry")
            .with_cookie(AxCookie::new("session", "s123").with_path("/").http_only());

            let response = axonyx_response_to_axum(response);

            assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("text/plain; charset=utf-8")
            );
            assert_eq!(
                response
                    .headers()
                    .get("X-Axonyx")
                    .and_then(|value| value.to_str().ok()),
                Some("foundry")
            );
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::SET_COOKIE)
                    .and_then(|value| value.to_str().ok()),
                Some("session=s123; Path=/; HttpOnly")
            );

            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("Axum body should be readable");
            assert_eq!(body.as_ref(), b"hello axum");
        });
    }

    #[cfg(feature = "axum")]
    #[test]
    fn axum_body_conversion_streams_chunked_bodies() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_io()
            .build()
            .expect("Tokio runtime should build for Axum adapter tests");

        runtime.block_on(async {
            let body = ax_body_to_axum_body(AxBody::chunks(vec![
                b"<main>".to_vec(),
                b"stream".to_vec(),
                b"</main>".to_vec(),
            ]));
            let body = axum::body::to_bytes(body, usize::MAX)
                .await
                .expect("streaming Axum body should be readable");

            assert_eq!(body.as_ref(), b"<main>stream</main>");
        });
    }

    #[cfg(feature = "axum")]
    #[test]
    fn axum_request_conversion_preserves_method_target_headers_and_body() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_io()
            .build()
            .expect("Tokio runtime should build for Axum adapter tests");

        runtime.block_on(async {
            let request = axum::http::Request::builder()
                .method("POST")
                .uri("/api/posts?draft=true")
                .header("Authorization", "Bearer token")
                .body(axum::body::Body::from("title=Hello"))
                .expect("Axum request should build");

            let request = axum_request_to_axonyx_with_limit(request, 1024)
                .await
                .expect("Axum request should convert to Axonyx request");

            assert_eq!(request.method, "POST");
            assert_eq!(request.target, "/api/posts?draft=true");
            assert_eq!(request.header_value("authorization"), Some("Bearer token"));
            assert_eq!(request.body_text_lossy(), "title=Hello");
        });
    }

    #[cfg(feature = "axum")]
    #[test]
    fn axum_adapter_can_build_router_from_route_table() {
        let config = AxServerConfig::new("127.0.0.1", 3000, AxServerMode::Start);
        let route = AxRouteDefinition::new("GET", "/health", ok_handler);
        let adapter = AxAxumServerAdapter::new(vec![route]).with_body_limit(2048);

        assert_eq!(adapter.name(), "axum");
        assert_eq!(adapter.body_limit, 2048);
        let _router = adapter.router();
        let _server_config = config;
    }
}
