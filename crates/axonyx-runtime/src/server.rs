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

    pub fn new(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            headers: BTreeMap::new(),
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
            body: AxBody::chunks(chunks),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn with_no_store(self) -> Self {
        self.with_header("Cache-Control", "no-store")
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
        204 => "No Content",
        303 => "See Other",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        415 => "Unsupported Media Type",
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
        status_reason, AxBody, AxBodyChunks, AxHttpRequest, AxHttpResponse, AxServer,
        AxServerConfig, AxServerMode,
    };
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
}
