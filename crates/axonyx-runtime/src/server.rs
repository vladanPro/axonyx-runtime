use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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

impl AxHttpRequest {
    pub fn new(method: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            target: target.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxHttpResponse {
    pub status: u16,
    pub content_type: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl AxHttpResponse {
    pub fn html(status: u16, body: impl Into<String>) -> Self {
        Self::new(status, "text/html; charset=utf-8", body.into().into_bytes())
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self::new(status, "text/plain; charset=utf-8", body.into().into_bytes())
    }

    pub fn bytes(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self::new(status, content_type, body)
    }

    pub fn new(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            headers: BTreeMap::new(),
            body,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

pub trait AxServer {
    fn config(&self) -> &AxServerConfig;
    fn serve(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

pub mod prelude {
    pub use super::{AxHttpRequest, AxHttpResponse, AxServer, AxServerConfig, AxServerMode};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_builds_bind_address() {
        let config = AxServerConfig::new("127.0.0.1", 3000, AxServerMode::Dev);

        assert_eq!(config.bind_addr(), "127.0.0.1:3000");
        assert_eq!(config.mode.label(), "dev");
        assert!(config.mode.inject_dev_client());
    }

    #[test]
    fn response_helpers_preserve_headers_and_body() {
        let response = AxHttpResponse::html(200, "<h1>Hello</h1>")
            .with_header("Cache-Control", "no-store");

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert_eq!(response.body, b"<h1>Hello</h1>");
        assert_eq!(
            response.headers.get("Cache-Control").map(String::as_str),
            Some("no-store")
        );
    }
}
