use crate::backend::{AxDatabaseRuntime, AxEnv, AxRuntimeEnvAccess, MemoryAdapter};
use crate::native::{AxNativeError, AxNativeLimits, AxNativeRegistry};
use crate::native_response::{
    AxNativeBinaryResponse, AxNativeBinaryType, AxNativeResponseContext, AxNativeResponseError,
    MAX_NATIVE_BINARY_BYTES,
};
use crate::server::{AxHttpRequest, AxHttpResponse};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageInput {
    slug: String,
}

fn runtime(registry: AxNativeRegistry) -> AxDatabaseRuntime<MemoryAdapter> {
    // Public reads must work without session configuration or a signing secret.
    AxDatabaseRuntime::new(AxEnv::new(), MemoryAdapter).with_native_functions(registry)
}

fn image() -> Result<AxNativeBinaryResponse, AxNativeResponseError> {
    AxNativeBinaryResponse::new(AxNativeBinaryType::Png, vec![0, 255, 7, 128])
}

fn headers(response: &AxHttpResponse) {
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

#[test]
fn anonymous_reads_recheck_product_grants_and_ignore_cache_validators() {
    let published = Arc::new(AtomicBool::new(true));
    let grant = published.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut registry = AxNativeRegistry::default();
    registry
        .register_public_response("post.image", 128, move |input: ImageInput| {
            count.fetch_add(1, Ordering::SeqCst);
            if input.slug != "story" || !grant.load(Ordering::SeqCst) {
                return Err(AxNativeResponseError::NotFound);
            }
            image()
        })
        .unwrap();
    let runtime = runtime(registry);
    let request = AxHttpRequest::new("GET", "/posts/story/image")
        .with_header("Cookie", "session=forged")
        .with_header("If-None-Match", "*")
        .with_header("If-Modified-Since", "Tue, 20 Oct 2026 00:00:00 GMT");
    let response = runtime
        .runtime_native_public_response(&request, "post.image", json!({"slug":"story"}))
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "image/png");
    headers(&response);
    assert_eq!(response.body.into_bytes(), vec![0, 255, 7, 128]);
    published.store(false, Ordering::SeqCst);
    let response = runtime
        .runtime_native_public_response(&request, "post.image", json!({"slug":"story"}))
        .unwrap();
    assert_eq!(response.status, 404);
    headers(&response);
    assert_eq!(response.body.into_bytes(), b"not found");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn callback_channels_never_dispatch_into_each_other() {
    let mut registry = AxNativeRegistry::default();
    registry
        .register_public_response("public.image", 128, |_: ()| image())
        .unwrap();
    registry
        .register_private_response("private.image", 128, |_, _: ()| {
            panic!("private callback was exposed")
        })
        .unwrap();
    registry
        .register("json.result", |_: ()| Ok::<_, ()>(42))
        .unwrap();
    assert_eq!(
        registry.call("public.image", Value::Null),
        Err(AxNativeError::UnknownFunction)
    );
    assert!(matches!(
        registry.private_response(
            &AxNativeResponseContext::verified("owner".into()),
            "public.image",
            Value::Null
        ),
        Err(AxNativeError::UnknownFunction)
    ));
    let runtime = runtime(registry);
    for name in ["private.image", "json.result", "missing"] {
        let response = runtime
            .runtime_native_public_response(&AxHttpRequest::new("GET", "/image"), name, Value::Null)
            .unwrap();
        assert_eq!(response.status, 500);
        headers(&response);
        assert_eq!(response.body.into_bytes(), b"binary response unavailable");
    }
}

#[test]
fn names_are_unique_across_all_registration_orders() {
    fn register(registry: &mut AxNativeRegistry, kind: usize) -> Result<(), AxNativeError> {
        match kind {
            0 => registry.register("image.read", |_: ()| Ok::<_, ()>(())),
            1 => registry.register_private_response("image.read", 128, |_, _: ()| image()),
            _ => registry.register_public_response("image.read", 128, |_: ()| image()),
        }
    }
    for first in 0..3 {
        for second in 0..3 {
            let mut registry = AxNativeRegistry::default();
            register(&mut registry, first).unwrap();
            assert_eq!(
                register(&mut registry, second),
                Err(AxNativeError::DuplicateRegistration)
            );
        }
    }
    for (name, limit) in [
        ("bad/name", 1),
        ("image.read", 0),
        ("image.read", MAX_NATIVE_BINARY_BYTES + 1),
    ] {
        assert_eq!(
            AxNativeRegistry::default().register_public_response(name, limit, |_: ()| image()),
            Err(AxNativeError::InvalidRegistration)
        );
    }
}

#[test]
fn malformed_and_oversized_inputs_are_rejected_before_product_callbacks() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut registry = AxNativeRegistry::new(AxNativeLimits {
        input_bytes: 128,
        ..Default::default()
    });
    registry
        .register_public_response("post.image", 128, move |_: ImageInput| {
            count.fetch_add(1, Ordering::SeqCst);
            image()
        })
        .unwrap();
    let runtime = runtime(registry);
    for input in [
        json!({}),
        json!({"slug":42}),
        json!({"slug":"story","subject":"admin"}),
        json!({"slug":"secret".repeat(50)}),
    ] {
        let response = runtime
            .runtime_native_public_response(
                &AxHttpRequest::new("GET", "/image"),
                "post.image",
                input,
            )
            .unwrap();
        assert_eq!(response.status, 500);
        headers(&response);
        assert_eq!(response.body.into_bytes(), b"binary response unavailable");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for limits in [
        AxNativeLimits {
            depth: 1,
            ..Default::default()
        },
        AxNativeLimits {
            nodes: 1,
            ..Default::default()
        },
    ] {
        let mut registry = AxNativeRegistry::new(limits);
        registry
            .register_public_response("image.read", 128, |_: Value| panic!("shape limit bypassed"))
            .unwrap();
        assert!(registry
            .public_response("image.read", json!({"nested":{"key":true}}))
            .is_err());
    }
}

#[test]
fn methods_never_invoke_callbacks_and_callback_errors_remain_closed() {
    let mut registry = AxNativeRegistry::default();
    registry
        .register_public_response("image.method", 128, |_: ()| panic!("method bypassed"))
        .unwrap();
    for (name, error) in [
        ("image.denied", AxNativeResponseError::Forbidden),
        ("image.missing", AxNativeResponseError::NotFound),
        ("image.failed", AxNativeResponseError::Unavailable),
    ] {
        registry
            .register_public_response(name, 128, move |_: ()| Err(error))
            .unwrap();
    }
    let runtime = runtime(registry);
    for method in ["HEAD", "POST", "PUT", "DELETE", "OPTIONS", "get"] {
        let response = runtime
            .runtime_native_public_response(
                &AxHttpRequest::new(method, "/image"),
                "image.method",
                Value::Null,
            )
            .unwrap();
        assert_eq!(response.status, 405);
        assert_eq!(response.header_value("Allow"), Some("GET"));
        headers(&response);
    }
    for (name, status, body) in [
        ("image.denied", 403, "forbidden"),
        ("image.missing", 404, "not found"),
        ("image.failed", 500, "binary response unavailable"),
    ] {
        let response = runtime
            .runtime_native_public_response(&AxHttpRequest::new("GET", "/image"), name, Value::Null)
            .unwrap();
        assert_eq!(response.status, status);
        headers(&response);
        assert_eq!(response.body.into_bytes(), body.as_bytes());
    }
    let unavailable = AxDatabaseRuntime::new(AxEnv::new(), MemoryAdapter)
        .runtime_native_public_response(
            &AxHttpRequest::new("GET", "/image"),
            "not.registered",
            Value::Null,
        )
        .unwrap();
    assert_eq!(unavailable.status, 500);
    headers(&unavailable);
}

#[test]
fn binary_output_limits_are_independent_from_json_output_limits() {
    let mut registry = AxNativeRegistry::new(AxNativeLimits {
        output_bytes: 1,
        ..Default::default()
    });
    registry
        .register_public_response("image.large", 80_000, |_: ()| {
            AxNativeBinaryResponse::new(AxNativeBinaryType::Jpeg, vec![7; 70_000])
        })
        .unwrap();
    registry
        .register_public_response("image.overflow", 3, |_: ()| image())
        .unwrap();
    let runtime = runtime(registry);
    let request = AxHttpRequest::new("GET", "/image");
    let response = runtime
        .runtime_native_public_response(&request, "image.large", Value::Null)
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "image/jpeg");
    headers(&response);
    assert_eq!(response.body.into_bytes(), vec![7; 70_000]);
    let response = runtime
        .runtime_native_public_response(&request, "image.overflow", Value::Null)
        .unwrap();
    assert_eq!(response.status, 500);
    headers(&response);
    assert_eq!(response.body.into_bytes(), b"binary response unavailable");
}

#[cfg(feature = "axum")]
#[test]
fn actual_anonymous_axum_http_rechecks_grant_and_preserves_binary_bytes() {
    use crate::server::{axum_router_from_compiled_handler, AxCompiledHandler};
    use std::io::{Read, Write};
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let published = Arc::new(AtomicBool::new(true));
        let grant = published.clone();
        let mut registry = AxNativeRegistry::default();
        registry
            .register_public_response("post.image", 128, move |_: ()| {
                if grant.load(Ordering::SeqCst) {
                    image()
                } else {
                    Err(AxNativeResponseError::NotFound)
                }
            })
            .unwrap();
        let runtime = Arc::new(runtime(registry));
        let handler: AxCompiledHandler = Arc::new(move |request| {
            if request.target != "/posts/story/image" {
                return AxHttpResponse::text(404, "not found");
            }
            runtime
                .runtime_native_public_response(&request, "post.image", Value::Null)
                .unwrap()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum_router_from_compiled_handler(handler, 1024);
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        for status in [200, 404] {
            published.store(status == 200, Ordering::SeqCst);
            let response = tokio::task::spawn_blocking(move || {
                let mut stream = std::net::TcpStream::connect(address).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                write!(stream, "GET /posts/story/image HTTP/1.1\r\nHost: localhost\r\nIf-None-Match: *\r\nConnection: close\r\n\r\n").unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).unwrap();
                response
            })
            .await
            .unwrap();
            let split = response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let headers = String::from_utf8_lossy(&response[..split]).to_ascii_lowercase();
            assert!(headers.starts_with(&format!("http/1.1 {status}")));
            assert!(headers.contains("cache-control: private, no-store"));
            assert!(headers.contains("x-content-type-options: nosniff"));
            assert!(headers.contains("cross-origin-resource-policy: same-origin"));
            assert!(!headers.contains("set-cookie:"));
            if status == 200 {
                assert!(headers.contains("content-type: image/png"));
                assert_eq!(&response[split + 4..], &[0, 255, 7, 128]);
            } else {
                assert_eq!(&response[split + 4..], b"not found");
            }
        }
        server.abort();
    });
}
