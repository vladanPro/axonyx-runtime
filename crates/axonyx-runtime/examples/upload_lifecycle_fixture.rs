use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axonyx_core::ax_lowering_prelude::AxValue;
use axonyx_runtime::server_prelude::{serve_compiled_axum, AxCompiledHandler, AxHttpResponse};
use axonyx_runtime::storage_prelude::{AxCapabilityStorage, AxStorageRegistry};
use axonyx_runtime::{
    execute_preview_action_request_sources_with_storage, preview_ax_route_with_backend,
    AxPreviewStore,
};

const PAGE_SOURCE: &str = r#"
page UploadLifecycleProbe() {
  return ASX {
    <main>
      <h1>Upload lifecycle probe</h1>
      <ActionForm name="UploadImage">
        <input aria-label="Fixture file" name="image" type="file" />
        <button type="submit">Upload fixture</button>
        <ActionProgress />
        <ActionStatus state="pending">Uploading...</ActionStatus>
        <ActionStatus state="complete">Stored.</ActionStatus>
        <ActionStatus state="error">Upload failed.</ActionStatus>
      </ActionForm>
    </main>
  }
}
"#;

const ACTION_SOURCE: &str = r#"
action UploadImage(image: File) -> FileRef {
  data saved = Storage.save("media", input.image)
  return json(saved)
}
"#;

fn ax_value_to_json(value: &AxValue) -> serde_json::Value {
    match value {
        AxValue::Null => serde_json::Value::Null,
        AxValue::String(value) => serde_json::Value::String(value.clone()),
        AxValue::Number(value) => serde_json::Value::Number((*value).into()),
        AxValue::Float(value) => serde_json::Number::from_f64(value.get())
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        AxValue::Bool(value) => serde_json::Value::Bool(*value),
        AxValue::Record(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), ax_value_to_json(value)))
                .collect(),
        ),
        AxValue::List(items) => {
            serde_json::Value::Array(items.iter().map(ax_value_to_json).collect())
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let output = env::var_os("AXONYX_E2E_OUTPUT")
        .map(PathBuf::from)
        .expect("AXONYX_E2E_OUTPUT must name the fixture output directory");
    let port = env::var("AXONYX_E2E_PORT").unwrap_or_else(|_| "4175".to_string());
    let html = Arc::new(preview_ax_route_with_backend(
        &[],
        &[],
        &[ACTION_SOURCE],
        PAGE_SOURCE,
        "/",
        &AxPreviewStore::default(),
    )?);

    let mut storage = AxStorageRegistry::new();
    storage.register(AxCapabilityStorage::open(
        "media",
        output.join("storage/media"),
        1024 * 1024,
    )?)?;
    let storage = Arc::new(storage);
    let store = Arc::new(Mutex::new(AxPreviewStore::default()));

    let handler: AxCompiledHandler = Arc::new(move |request| {
        if request.method == "GET" && request.target == "/__axonyx/e2e-shutdown" {
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                std::process::exit(0);
            });
            return AxHttpResponse::no_content();
        }
        if request.method == "GET" && request.target == "/favicon.ico" {
            return AxHttpResponse::no_content();
        }
        if request.method == "GET" && request.target == "/" {
            return AxHttpResponse::html(200, html.as_str()).with_no_store();
        }
        if request.method == "GET" && request.target == "/__axonyx/csrf" {
            return AxHttpResponse::json(200, &serde_json::json!({ "token": null }))
                .expect("fixture JSON should serialize")
                .with_no_store();
        }
        if request.method != "POST" || !request.target.starts_with("/__axonyx/action?") {
            return AxHttpResponse::text(404, "Not Found");
        }

        let mut store = match store.lock() {
            Ok(store) => store,
            Err(_) => return AxHttpResponse::text(500, "fixture store lock failed"),
        };
        match execute_preview_action_request_sources_with_storage(
            &[ACTION_SOURCE],
            "UploadImage",
            &request,
            storage.as_ref(),
            &mut store,
        ) {
            Ok(result) => {
                if let Some(error) = result.error {
                    return AxHttpResponse::bytes(
                        error.status,
                        "application/ax-error+json; charset=utf-8",
                        serde_json::to_vec(&serde_json::json!({
                            "ok": false,
                            "error": error.message,
                            "value": ax_value_to_json(&error.value),
                        }))
                        .expect("fixture error payload should serialize"),
                    );
                }
                AxHttpResponse::bytes(
                    200,
                    "application/ax-patch+json; charset=utf-8",
                    serde_json::to_vec(&serde_json::json!({
                        "ok": true,
                        "redirect": result.redirect_to,
                        "value": ax_value_to_json(&result.value),
                        "patches": [],
                        "invalidations": [],
                        "refreshes": [],
                    }))
                    .expect("fixture action payload should serialize"),
                )
            }
            Err(error) => AxHttpResponse::bytes(
                500,
                "application/ax-error+json; charset=utf-8",
                serde_json::to_vec(&serde_json::json!({
                    "ok": false,
                    "error": error.to_string(),
                }))
                .expect("fixture runtime error should serialize"),
            ),
        }
    });

    serve_compiled_axum(format!("127.0.0.1:{port}"), 2 * 1024 * 1024, handler)
}
