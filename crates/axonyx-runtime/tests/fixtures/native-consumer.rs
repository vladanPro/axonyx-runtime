mod backend;

use axonyx_runtime::backend::{AxDatabaseRuntime, AxEnv, AxRuntimeEnvAccess, MemoryAdapter};
use axonyx_runtime::native::{AxNativeLimits, AxNativeRegistry};
use axonyx_runtime::server::{AxHttpRequest, AxUnavailableFileStorage};
use axonyx_runtime::{
    execute_preview_action_request_sources_with_runtime_and_storage,
    execute_preview_route_request_sources_with_runtime, AxPreviewStore,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Content {
    text: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    items: Vec<Content>,
    #[serde(rename = "meta:kind")]
    kind: String,
}

fn runtime(limits: AxNativeLimits) -> AxDatabaseRuntime<MemoryAdapter> {
    let mut native = AxNativeRegistry::new(limits);
    native
        .register("content.prepare", |input: Content| {
            if input.text == "reject" {
                return Err("private diagnostic");
            }
            Ok(Content {
                text: input.text.trim().to_owned(),
            })
        })
        .unwrap();
    native
        .register("content.batch", |input: Batch| Ok::<_, ()>(input))
        .unwrap();
    native.register("content.echo", |input: serde_json::Value| Ok::<_, ()>(input)).unwrap();
    AxDatabaseRuntime::new(AxEnv::default(), MemoryAdapter).with_native_functions(native)
}

fn main() {
    private_binary_proof();
    let valid = backend::validatePost("t".repeat(160), "s".repeat(120), "b".repeat(20000), "published".into()).unwrap();
    assert!(valid.title && valid.slug && valid.body && valid.status);
    let copied = backend::copyValidation(&valid).unwrap();
    assert!(valid.title && copied.title);
    let invalid = backend::validatePost("t".repeat(161), "s".repeat(121), "b".repeat(20001), "unknown".into()).unwrap();
    assert!(!invalid.title && !invalid.slug && !invalid.body && !invalid.status);
    let empty = backend::validatePost("".into(), "".into(), "".into(), "draft".into()).unwrap();
    assert!(!empty.title && !empty.slug && !empty.body && empty.status);
    assert_eq!(backend::countText("\u{1f642}a".into()).unwrap(), 3);
    assert_eq!(backend::countItems(vec!["a".into(), "b".into()]).unwrap(), 2);
    assert_eq!(backend::countItems(vec![]).unwrap(), 0);
    assert_eq!(backend::repeatText("hello".into()).unwrap(), "hellohello");
    assert!(backend::countDynamic(serde_json::json!(42)).unwrap_err().to_string().contains("length expects"));
    let bad_record = backend::invalidValidation().unwrap_err().to_string();
    assert!(bad_record.contains("does not match declared record"));
    assert!(!bad_record.contains("private details"));
    let source = include_str!("native-backend.ax");
    let runtime = runtime(AxNativeLimits::default());
    for (target, header, status) in [
        ("/guard/allowed?key=fixture-only&mode=compiled", "yes", 200),
        ("/guard/allowed?key=fixture-only&mode=preview", "yes", 200),
        ("/guard/allowed?key=wrong&mode=compiled", "yes", 403),
        ("/guard/denied?key=fixture-only&mode=compiled", "yes", 403),
        ("/guard/allowed?key=fixture-only&mode=compiled", "no", 403),
        ("/guard/allowed?key=fixture-only&mode=unknown", "yes", 403),
        ("/short-guard?public=yes", "", 200),
        ("/short-guard?public=no&key=fixture-only", "", 200),
        ("/short-guard?public=no&key=wrong", "", 403),
        ("/and-guard?public=no", "", 403),
        ("/and-guard?public=yes&key=fixture-only", "", 200),
    ] {
        let method = if target.starts_with("/guard/") { "POST" } else { "GET" };
        let request = AxHttpRequest::new(method, target).with_header("x-fixture", header);
        let compiled = backend::dispatch_api_route(&runtime, &request, true).unwrap().unwrap();
        let preview = execute_preview_route_request_sources_with_runtime(
            &[source], &request, &runtime, &mut AxPreviewStore::default(),
        ).unwrap().unwrap();
        assert_eq!(compiled.status, status, "compiled: {target}");
        assert_eq!(preview.status, status, "preview: {target}");
        assert_eq!(compiled.body.into_bytes(), preview.body, "{target}");
    }
    let empty=backend::dispatch_api_route(&runtime,&AxHttpRequest::new("GET","/empty-native"),true).unwrap().unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&empty.body.into_bytes()).unwrap(),serde_json::json!({"nested":{},"items":[]}));
    let request = AxHttpRequest::new("GET", "/prepare?t%65xt=%20hello%20");
    let compiled = backend::dispatch_api_route(&runtime, &request, false)
        .unwrap()
        .unwrap();
    let preview = execute_preview_route_request_sources_with_runtime(
        &[source],
        &request,
        &runtime,
        &mut AxPreviewStore::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(compiled.status, preview.status);
    assert_eq!(compiled.body.into_bytes(), preview.body);
    for request in [
        AxHttpRequest::new("GET", "/nested?text=hello"),
        AxHttpRequest::new("GET", "/members?text=hello"),
        AxHttpRequest::new("GET", "/prepare?text=old&text=hello+world"),
        AxHttpRequest::new("POST", "/form").with_body(b"text=hello".to_vec()),
    ] {
        let compiled = backend::dispatch_api_route(&runtime, &request, false)
            .unwrap()
            .unwrap();
        let preview = execute_preview_route_request_sources_with_runtime(
            &[source],
            &request,
            &runtime,
            &mut AxPreviewStore::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(compiled.status, preview.status);
        assert_eq!(compiled.body.into_bytes(), preview.body);
    }
    let members = backend::dispatch_api_route(&runtime, &AxHttpRequest::new("GET", "/members?text=hello"), false)
        .unwrap().unwrap();
    let members: serde_json::Value = serde_json::from_slice(&members.body.into_bytes()).unwrap();
    assert_eq!(members, serde_json::json!({"text":"hello", "grouped":"hello", "missing":null, "literal":"prepared.items[0].text"}));
    for target in ["/invalid", "/unknown", "/prepare?text=reject"] {
        let request = AxHttpRequest::new("GET", target);
        let compiled = backend::dispatch_api_route(&runtime, &request, false)
            .unwrap_err()
            .to_string();
        let preview = execute_preview_route_request_sources_with_runtime(
            &[source],
            &request,
            &runtime,
            &mut AxPreviewStore::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(preview.contains(&compiled), "{preview} != {compiled}");
        assert!(!compiled.contains("private diagnostic"));
        assert!(!compiled.contains("content.missing"));
    }
    let limited = self::runtime(AxNativeLimits {
        input_bytes: 4,
        ..Default::default()
    });
    assert!(backend::dispatch_api_route(&limited, &request, false)
        .unwrap_err()
        .to_string()
        .contains("payload limit exceeded"));
    let absent = AxDatabaseRuntime::new(AxEnv::default(), MemoryAdapter);
    assert!(backend::dispatch_api_route(&absent, &request, false)
        .unwrap_err()
        .to_string()
        .contains("unavailable"));
    let request = AxHttpRequest::new("POST", "/prepare").with_body(b"text=%20hello%20".to_vec());
    let compiled = backend::dispatch_action(
        &runtime,
        &AxUnavailableFileStorage,
        "PrepareContent",
        &request,
    )
    .unwrap()
    .unwrap();
    let preview = execute_preview_action_request_sources_with_runtime_and_storage(
        &[source],
        "PrepareContent",
        &request,
        &runtime,
        &AxUnavailableFileStorage,
        &mut AxPreviewStore::default(),
    )
    .unwrap();
    assert_eq!(
        compiled.payload["value"],
        serde_json::json!({"text":"hello"})
    );
    assert!(preview.error.is_none());
    assert_eq!(
        preview.value,
        axonyx_core::ax_lowering::AxValue::record([(
            "text",
            axonyx_core::ax_lowering::AxValue::from("hello")
        ),])
    );
    for (body, valid) in [(b"text=hello".as_slice(), true), (b"text=%20".as_slice(), false)] {
        let request = AxHttpRequest::new("POST", "/check").with_body(body.to_vec());
        let compiled = backend::dispatch_action(&runtime, &AxUnavailableFileStorage, "CheckMember", &request)
            .unwrap().unwrap();
        let preview = execute_preview_action_request_sources_with_runtime_and_storage(
            &[source], "CheckMember", &request, &runtime, &AxUnavailableFileStorage,
            &mut AxPreviewStore::default()
        ).unwrap();
        if valid {
            assert_eq!(compiled.payload["value"], "hello");
            assert!(preview.error.is_none());
            assert_eq!(preview.value, axonyx_core::ax_lowering::AxValue::from("hello"));
        } else {
            assert_eq!(compiled.payload["error"]["status"], 422);
            assert_eq!(compiled.payload["error"]["value"]["fields"]["text"], "Required");
            assert_eq!(preview.error.unwrap().status, 422);
        }
    }
    for (body, valid) in [("title=hello&slug=hello&body=content&status=draft", true), ("title=&slug=hello&body=content&status=draft", false)] {
        let request = AxHttpRequest::new("POST", "/check-post").with_body(body.as_bytes().to_vec());
        let compiled = backend::dispatch_action(&runtime, &AxUnavailableFileStorage, "CheckPost", &request).unwrap().unwrap();
        let preview = execute_preview_action_request_sources_with_runtime_and_storage(&[source], "CheckPost", &request, &runtime, &AxUnavailableFileStorage, &mut AxPreviewStore::default()).unwrap();
        if valid {
            assert_eq!(compiled.payload["value"]["title"], true);
            assert!(preview.error.is_none());
        } else {
            assert_eq!(compiled.payload["error"]["status"], 422);
            assert_eq!(preview.error.unwrap().status, 422);
        }
    }
    let request = AxHttpRequest::new("POST", "/reuse").with_body(b"text=hello".to_vec());
    let reused = backend::dispatch_action(&runtime, &AxUnavailableFileStorage, "ReuseHelper", &request).unwrap().unwrap();
    assert_eq!(reused.payload["value"], serde_json::json!({"first":"hello", "second":"hello", "original":"hello", "literal":"identityText(input.text)"}));
    println!("Native/Domain calls: generated Rust and preview parity passed");
}

fn private_binary_proof() {
    use axonyx_runtime::native_response::{AxNativeBinaryResponse, AxNativeBinaryType};
    use serde_json::{json, Value};
    let mut registry = AxNativeRegistry::default();
    registry.register_private_response("file.read", 100, |context, input: Value| {
        assert_eq!(context.subject(), "owner");
        assert_eq!(input, json!({"id":"image"}));
        AxNativeBinaryResponse::new(AxNativeBinaryType::OctetStream, vec![0,255,128,42])
    }).unwrap();
    registry.register_private_response("file.empty", 100, |_, input: Value| {
        assert_eq!(input, json!({}));
        AxNativeBinaryResponse::new(AxNativeBinaryType::Png, vec![1,2,3])
    }).unwrap();
    let runtime = AxDatabaseRuntime::new(AxEnv::new().with_secret("db_driver","memory").with_secret("session_key","private-fixture-secret").with_secret("session_cookie_secure","false"), MemoryAdapter).with_native_functions(registry);
    let (_,cookie)=runtime.runtime_create_session("owner",Default::default()).unwrap();
    let request=AxHttpRequest::new("GET","/private/image").with_header("Cookie",format!("{}={}",cookie.name,cookie.value));
    for (cookie_header, status) in [(String::new(), 403), (format!("{}={}", cookie.name, cookie.value), 200)] {
        let account = AxHttpRequest::new("GET", "/account-guard").with_header("Cookie", cookie_header);
        let compiled = backend::dispatch_api_route(&runtime, &account, true).unwrap().unwrap();
        let preview = execute_preview_route_request_sources_with_runtime(
            &[include_str!("native-backend.ax")], &account, &runtime, &mut AxPreviewStore::default(),
        ).unwrap().unwrap();
        assert_eq!(compiled.status, status);
        assert_eq!(preview.status, status);
        assert_eq!(compiled.body.into_bytes(), preview.body);
    }
    for validate in [false,true] {
        let response=backend::dispatch_api_route(&runtime,&request,validate).unwrap().unwrap();
        assert_eq!(response.status,200);
        assert_eq!(response.body.into_bytes(),vec![0,255,128,42]);
        let mut empty=request.clone();empty.target="/private-empty".into();
        assert_eq!(backend::dispatch_api_route(&runtime,&empty,validate).unwrap().unwrap().body.into_bytes(),vec![1,2,3]);
    }
    let mut unknown=request.clone();unknown.target="/private-missing".into();
    let error=backend::dispatch_api_route(&runtime,&unknown,true).unwrap().unwrap();
    assert_eq!(error.status,500);
    assert_eq!(error.header_value("Cache-Control"),Some("private, no-store"));
    assert_eq!(error.body.into_bytes(),b"binary response unavailable");
    let preview=execute_preview_route_request_sources_with_runtime(&[include_str!("native-backend.ax")],&request,&runtime,&mut AxPreviewStore::default());
    assert!(preview.err().unwrap().to_string().contains("compiled server"));
    runtime.runtime_destroy_session(&request).unwrap();
    let denied=backend::dispatch_api_route(&runtime,&request,true).unwrap().unwrap();
    assert_eq!(denied.status,403);
    assert_eq!(denied.header_value("Cache-Control"),Some("private, no-store"));
    assert_eq!(denied.body.into_bytes(),b"forbidden");
}
