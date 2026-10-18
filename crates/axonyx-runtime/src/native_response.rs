//! Private, bounded binary callback results. Never serialized through Native.call.
use crate::server::AxHttpResponse;

pub const MAX_NATIVE_BINARY_BYTES: usize = 8 * 1024 * 1024;

pub struct AxNativeResponseContext {
    subject: String,
}

impl AxNativeResponseContext {
    pub(crate) fn verified(subject: String) -> Self {
        Self { subject }
    }

    /// Session-verified identity, not a role grant. Recheck product authority.
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

#[derive(Clone, Copy)]
pub enum AxNativeBinaryType {
    Png,
    Jpeg,
    OctetStream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxNativeResponseError {
    Forbidden,
    NotFound,
    Unavailable,
}

/// Fields are private and this type deliberately does not implement Serialize.
pub struct AxNativeBinaryResponse {
    kind: AxNativeBinaryType,
    bytes: Vec<u8>,
}

impl AxNativeBinaryResponse {
    pub fn new(kind: AxNativeBinaryType, bytes: Vec<u8>) -> Result<Self, AxNativeResponseError> {
        if bytes.is_empty() || bytes.len() > MAX_NATIVE_BINARY_BYTES {
            return Err(AxNativeResponseError::Unavailable);
        }
        Ok(Self { kind, bytes })
    }

    pub(crate) fn into_response(self, limit: usize) -> AxHttpResponse {
        if self.bytes.len() > limit {
            return private_error(AxNativeResponseError::Unavailable);
        }
        let content_type = match self.kind {
            AxNativeBinaryType::Png => "image/png",
            AxNativeBinaryType::Jpeg => "image/jpeg",
            AxNativeBinaryType::OctetStream => "application/octet-stream",
        };
        let disposition = match self.kind {
            AxNativeBinaryType::OctetStream => "attachment",
            _ => "inline",
        };
        private_headers(AxHttpResponse::bytes(200, content_type, self.bytes))
            .with_header("Content-Disposition", disposition)
    }
}

pub(crate) fn private_headers(response: AxHttpResponse) -> AxHttpResponse {
    response
        .with_header("Cache-Control", "private, no-store")
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("Cross-Origin-Resource-Policy", "same-origin")
}

pub(crate) fn private_error(error: AxNativeResponseError) -> AxHttpResponse {
    let (status, message) = match error {
        AxNativeResponseError::Forbidden => (403, "forbidden"),
        AxNativeResponseError::NotFound => (404, "not found"),
        AxNativeResponseError::Unavailable => (500, "binary response unavailable"),
    };
    private_headers(AxHttpResponse::text(status, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{AxDatabaseRuntime, AxEnv, AxRuntimeEnvAccess, MemoryAdapter};
    use crate::native::{AxNativeError, AxNativeLimits, AxNativeRegistry};
    use crate::server::AxHttpRequest;
    use serde::Deserialize;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        key: String,
    }

    fn runtime(registry: AxNativeRegistry) -> AxDatabaseRuntime<MemoryAdapter> {
        AxDatabaseRuntime::new(
            AxEnv::new()
                .with_secret("db_driver", "memory")
                .with_secret("session_key", "test-private-response-key")
                .with_secret("session_cookie_secure", "false"),
            MemoryAdapter,
        )
        .with_native_functions(registry)
    }

    fn request(runtime: &impl AxRuntimeEnvAccess) -> AxHttpRequest {
        let (_, cookie) = runtime
            .runtime_create_session("owner", BTreeMap::new())
            .unwrap();
        AxHttpRequest::new("GET", "/private-image")
            .with_header("Cookie", format!("{}={}", cookie.name, cookie.value))
    }

    #[test]
    fn verified_session_is_separate_from_typed_input_and_rechecked_after_logout() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut registry = AxNativeRegistry::default();
        registry
            .register_private_response("image.read", 1024, move |context, input: Input| {
                assert_eq!(context.subject(), "owner");
                assert_eq!(input.key, "image");
                counter.fetch_add(1, Ordering::SeqCst);
                AxNativeBinaryResponse::new(AxNativeBinaryType::Png, vec![1, 2, 3])
            })
            .unwrap();
        let runtime = runtime(registry);
        for request in [
            AxHttpRequest::new("GET", "/private-image?subject=owner"),
            AxHttpRequest::new("GET", "/private-image").with_header("Cookie", "session=forged"),
        ] {
            assert_eq!(
                runtime
                    .runtime_native_private_response(&request, "image.read", json!({"key":"image"}))
                    .unwrap()
                    .status,
                403
            );
        }
        let request = request(&runtime);
        assert!(runtime
            .runtime_native_private_response(
                &request,
                "image.read",
                json!({"key":"image","subject":"admin"})
            )
            .is_err());
        assert!(runtime
            .runtime_native_private_response(&request, "image.read", json!({"key":42}))
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let response = runtime
            .runtime_native_private_response(&request, "image.read", json!({"key":"image"}))
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "image/png");
        assert_eq!(response.body.into_bytes(), vec![1, 2, 3]);
        runtime.runtime_destroy_session(&request).unwrap();
        assert_eq!(
            runtime
                .runtime_native_private_response(&request, "image.read", json!({"key":"image"}))
                .unwrap()
                .status,
            403
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn binary_bytes_have_independent_output_limit_but_inputs_keep_json_limits() {
        let mut registry = AxNativeRegistry::new(AxNativeLimits {
            input_bytes: 128,
            output_bytes: 1,
            ..Default::default()
        });
        registry
            .register_private_response("image.large", 80_000, |_, _: ()| {
                AxNativeBinaryResponse::new(AxNativeBinaryType::Jpeg, vec![7; 70_000])
            })
            .unwrap();
        registry
            .register_private_response("image.oversize", 10, |_, _: ()| {
                AxNativeBinaryResponse::new(AxNativeBinaryType::Png, vec![7; 11])
            })
            .unwrap();
        registry
            .register_private_response("image.input", 80_000, |_, _: Value| {
                panic!("oversized input invoked callback")
            })
            .unwrap();
        let runtime = runtime(registry);
        let request = request(&runtime);
        let response = runtime
            .runtime_native_private_response(&request, "image.large", Value::Null)
            .unwrap();
        assert_eq!(response.body.into_bytes(), vec![7; 70_000]);
        let response = runtime
            .runtime_native_private_response(&request, "image.oversize", Value::Null)
            .unwrap();
        assert_eq!(response.status, 500);
        assert_eq!(response.body.into_bytes(), b"binary response unavailable");
        assert!(runtime
            .runtime_native_private_response(
                &request,
                "image.input",
                json!({"key":"x".repeat(256)})
            )
            .is_err());
    }

    #[test]
    fn errors_and_methods_preserve_private_headers_and_never_invoke_on_head_or_post() {
        let mut registry = AxNativeRegistry::default();
        for (name, error) in [
            ("image.denied", AxNativeResponseError::Forbidden),
            ("image.missing", AxNativeResponseError::NotFound),
            ("image.failed", AxNativeResponseError::Unavailable),
        ] {
            registry
                .register_private_response(name, 1024, move |_, _: ()| Err(error))
                .unwrap();
        }
        registry
            .register_private_response("image.method", 1024, |_, _: ()| {
                panic!("wrong method invoked callback")
            })
            .unwrap();
        let runtime = runtime(registry);
        let mut request = request(&runtime);
        for (name, status) in [
            ("image.denied", 403),
            ("image.missing", 404),
            ("image.failed", 500),
        ] {
            let response = runtime
                .runtime_native_private_response(&request, name, Value::Null)
                .unwrap();
            assert_eq!(response.status, status);
            assert_eq!(
                response.header_value("Cache-Control"),
                Some("private, no-store")
            );
            assert_eq!(
                response.header_value("X-Content-Type-Options"),
                Some("nosniff")
            );
            assert_eq!(
                response.header_value("Cross-Origin-Resource-Policy"),
                Some("same-origin")
            );
            assert!(response.set_cookies.is_empty());
        }
        for method in ["HEAD", "POST"] {
            request.method = method.into();
            let response = runtime
                .runtime_native_private_response(&request, "image.method", Value::Null)
                .unwrap();
            assert_eq!(response.status, 405);
            assert_eq!(response.header_value("Allow"), Some("GET"));
        }
    }

    #[test]
    fn registration_and_json_namespace_cannot_silently_replace_each_other() {
        let mut registry = AxNativeRegistry::default();
        for limit in [0, MAX_NATIVE_BINARY_BYTES + 1] {
            assert_eq!(
                registry.register_private_response("image.read", limit, |_, _: ()| Err(
                    AxNativeResponseError::Unavailable
                )),
                Err(AxNativeError::InvalidRegistration)
            );
        }
        assert_eq!(
            registry.register_private_response("bad/name", 10, |_, _: ()| Err(
                AxNativeResponseError::Unavailable
            )),
            Err(AxNativeError::InvalidRegistration)
        );
        registry
            .register_private_response("image.read", 10, |_, _: ()| {
                Err(AxNativeResponseError::NotFound)
            })
            .unwrap();
        assert_eq!(
            registry.register("image.read", |_: ()| Ok::<_, ()>(Value::Null)),
            Err(AxNativeError::DuplicateRegistration)
        );
        assert_eq!(
            registry.call("image.read", Value::Null),
            Err(AxNativeError::UnknownFunction)
        );
        registry
            .register("json.read", |_: ()| Ok::<_, ()>(Value::Null))
            .unwrap();
        assert_eq!(
            registry.register_private_response("json.read", 10, |_, _: ()| Err(
                AxNativeResponseError::NotFound
            )),
            Err(AxNativeError::DuplicateRegistration)
        );
        assert_eq!(
            registry.call("json.read", Value::Null).unwrap(),
            Value::Null
        );
        assert!(AxNativeBinaryResponse::new(AxNativeBinaryType::Png, vec![]).is_err());
        assert!(AxNativeBinaryResponse::new(
            AxNativeBinaryType::Png,
            vec![0; MAX_NATIVE_BINARY_BYTES + 1]
        )
        .is_err());
    }

    #[cfg(feature = "axum")]
    #[test]
    fn actual_axum_http_preserves_private_bytes_and_rejects_anonymous_or_revoked_sessions() {
        use crate::server::{axum_router_from_compiled_handler, AxCompiledHandler};
        use std::io::{Read, Write};
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let bytes = vec![0, 255, 128, 0, 42];
            let output = bytes.clone();
            let mut registry = AxNativeRegistry::default();
            registry.register_private_response("file.read",1024, move |context, _: ()| {
                assert_eq!(context.subject(),"owner");
                AxNativeBinaryResponse::new(AxNativeBinaryType::OctetStream,output.clone())
            }).unwrap();
            let runtime=Arc::new(runtime(registry));
            let authenticated=request(runtime.as_ref());
            let cookie=authenticated.header_value("Cookie").unwrap().to_owned();
            let handler_runtime=runtime.clone();
            let handler: AxCompiledHandler=Arc::new(move |request| {
                handler_runtime.runtime_native_private_response(&request,"file.read",Value::Null)
                    .unwrap_or_else(|_| private_error(AxNativeResponseError::Unavailable))
            });
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address=listener.local_addr().unwrap();
            let router=axum_router_from_compiled_handler(handler,1024);
            let server=tokio::spawn(async move { axum::serve(listener,router).await.unwrap(); });
            for (cookie,status) in [(String::new(),403),(cookie.clone(),200)] {
                let response=tokio::task::spawn_blocking(move || {
                    let mut stream=std::net::TcpStream::connect(address).unwrap();
                    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                    write!(stream,"GET /private HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n").unwrap();
                    let mut response=Vec::new();stream.read_to_end(&mut response).unwrap();response
                }).await.unwrap();
                let split=response.windows(4).position(|window| window==b"\r\n\r\n").unwrap();
                let headers=String::from_utf8_lossy(&response[..split]).to_ascii_lowercase();
                assert!(headers.starts_with(&format!("http/1.1 {status}")));
                assert!(headers.contains("cache-control: private, no-store"));
                assert!(headers.contains("x-content-type-options: nosniff"));
                if status==200 {
                    assert!(headers.contains("content-type: application/octet-stream"));
                    assert!(headers.contains("content-disposition: attachment"));
                    assert_eq!(&response[split+4..],bytes.as_slice());
                }
            }
            runtime.runtime_destroy_session(&authenticated).unwrap();
            assert_eq!(runtime.runtime_native_private_response(&authenticated,"file.read",Value::Null).unwrap().status,403);
            server.abort();
        });
    }
}
