pub mod backend;
pub mod server;

use std::cell::RefCell;
use std::collections::BTreeMap;

use axonyx_core::ax_ast_prelude::{
    AxBinaryOp, AxBody, AxComponent, AxDocument, AxExpr, AxFloat, AxHead, AxHeadTag, AxPipeline,
    AxPipelineStage, AxProp, AxStatement, AxUnaryOp,
};
use axonyx_core::ax_backend_lowering::AxBackendLowerError;
use axonyx_core::ax_backend_lowering_prelude::{
    lower_backend_document, AxFieldPlan, AxFunctionPlan, AxHandlerKind, AxHandlerPlan,
    AxHookPhasePlan, AxQueryFilterOpPlan, AxQueryModePlan, AxQueryOrderDirectionPlan, AxQueryPlan,
    AxQuerySourcePlan, AxReturnPlan, AxRustExpr, AxStepPlan, AxValuePlan,
};
use axonyx_core::ax_backend_parser::AxBackendParseError;
use axonyx_core::ax_backend_parser_prelude::parse_backend_ax;
use axonyx_core::ax_lowering::AxLowerError;
use axonyx_core::ax_lowering_prelude::{
    lower_document_with_scope_and_imports, AxImportResolver, AxValue,
};
use axonyx_core::ax_parser_auto::AxAutoParseError;
use axonyx_core::ax_parser_auto_prelude::parse_ax_auto;
use axonyx_core::prelude::{Attribute, AxNode};
use axonyx_core::{AxonyxIr, SourceKind, TransformKind, ViewKind};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

pub use backend::prelude as backend_prelude;
pub use serde;
pub use server::prelude as server_prelude;

pub const AX_STATE_WASM_PATH: &str = "/_ax/runtime/axonyx-state-v2.wasm";

pub fn ax_state_wasm_bytes() -> &'static [u8] {
    include_bytes!("../assets/axonyx-state-v2.wasm")
}

pub fn route_hooks_from_handler_plan(handler: &AxHandlerPlan) -> Vec<server::AxRouteHook> {
    handler
        .steps
        .iter()
        .filter_map(|step| {
            let AxStepPlan::Hook { phase, value } = step else {
                return None;
            };
            let phase = match phase {
                AxHookPhasePlan::Before => server::AxMiddlewarePhase::Before,
                AxHookPhasePlan::After => server::AxMiddlewarePhase::After,
            };
            Some(server::AxRouteHook::new(phase, value.code.clone()))
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RenderPlan {
    pub source: String,
    pub layout: LayoutPlan,
    pub view: ViewPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutPlan {
    pub kind: String,
    pub columns: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewPlan {
    pub component: String,
    pub props: serde_json::Value,
}

pub fn execute(ir: &AxonyxIr) -> RenderPlan {
    let source = match &ir.source.kind {
        SourceKind::Collection { name } => name.clone(),
    };

    let mut columns = 1;
    for transform in &ir.transforms {
        match transform.kind {
            TransformKind::Grid { columns: c } => columns = c,
        }
    }

    let component = match &ir.view.kind {
        ViewKind::Card => "Card".to_string(),
        ViewKind::Named { name } => name.clone(),
    };

    RenderPlan {
        source,
        layout: LayoutPlan {
            kind: "grid".to_string(),
            columns,
        },
        view: ViewPlan {
            component,
            props: json!({
                "runtime": "axonyx-runtime-v1",
            }),
        },
    }
}

pub fn execute_json(ir_json: &str) -> Result<RenderPlan, serde_json::Error> {
    let ir: AxonyxIr = serde_json::from_str(ir_json)?;
    Ok(execute(&ir))
}

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("failed to parse .ax file")]
    Parse(#[from] AxAutoParseError),
    #[error("failed to parse backend .ax file")]
    BackendParse(#[from] AxBackendParseError),
    #[error("failed to lower backend .ax file")]
    BackendLower(#[from] AxBackendLowerError),
    #[error("failed to lower .ax file")]
    Lower(#[from] AxLowerError),
    #[error("failed to execute preview runtime: {message}")]
    Runtime { message: String },
}

impl From<backend::AxRuntimeError> for PreviewError {
    fn from(error: backend::AxRuntimeError) -> Self {
        Self::Runtime {
            message: error.to_string(),
        }
    }
}

pub fn preview_ax_page(ax_source: &str) -> Result<String, PreviewError> {
    preview_ax_app(None, ax_source)
}

pub fn render_compiled_page_fragment(
    document_json: &str,
    import_sources: &[(&str, &str)],
    request_target: &str,
    route_params: &BTreeMap<String, String>,
    loader_values: &BTreeMap<String, serde_json::Value>,
) -> Result<String, PreviewError> {
    let document = serde_json::from_str::<AxDocument>(document_json).map_err(|error| {
        PreviewError::Runtime {
            message: format!("failed to decode compiled page AST: {error}"),
        }
    })?;
    let scope = build_preview_route_scope(
        request_target,
        route_params,
        &parse_preview_query_fields(request_target),
    );
    let resolver = |path: &[String], args: &[AxValue]| {
        let args = args.iter().map(preview_value_to_json).collect::<Vec<_>>();
        path.last()
            .map(|name| compiled_loader_call_key(name, &args))
            .and_then(|key| loader_values.get(&key))
            .cloned()
            .map(preview_json_to_value)
    };
    let import_resolver = |source: &str| {
        import_sources
            .iter()
            .find_map(|(name, contents)| (*name == source).then(|| (*contents).to_string()))
    };
    let node =
        lower_document_with_scope_and_imports(&document, scope, &resolver, &import_resolver)?;
    let mut html = String::new();
    render_node(&node, &mut html);
    Ok(html)
}

pub fn compiled_loader_call_key(name: &str, args: &[serde_json::Value]) -> String {
    let encoded = serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string());
    format!("{name}:{encoded}")
}

pub fn preview_ax_page_stream_response(
    ax_source: &str,
) -> Result<server::AxHttpResponse, PreviewError> {
    let document = parse_ax_auto(ax_source)?;
    let resolver = |_: &[String], _: &[AxValue]| None;
    let import_resolver = |_: &str| None;
    let node = lower_document_with_scope_and_imports(
        &document,
        BTreeMap::new(),
        &resolver,
        &import_resolver,
    )?;

    Ok(render_preview_document_response(&document, &node))
}

pub fn ax_page_route_definition(
    method: impl Into<String>,
    path: impl Into<String>,
    page_source: &str,
) -> Result<server::AxRouteDefinition, PreviewError> {
    Ok(server::AxRouteDefinition::new_response(
        method,
        path,
        preview_ax_page_stream_response(page_source)?,
    ))
}

// Compatibility facade keeps route sources explicit; grouping them would break the public API.
#[allow(clippy::too_many_arguments)]
pub fn ax_page_route_definition_with_backend(
    method: impl Into<String>,
    path: impl Into<String>,
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    store: &AxPreviewStore,
) -> Result<server::AxRouteDefinition, PreviewError> {
    Ok(server::AxRouteDefinition::new_response(
        method,
        path,
        preview_ax_route_stream_response_with_backend(
            layout_sources,
            loader_sources,
            action_sources,
            page_source,
            request_target,
            store,
        )?,
    ))
}

pub fn preview_ax_page_with_imports(
    ax_source: &str,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    preview_ax_app_with_imports(None, ax_source, import_resolver)
}

pub fn preview_ax_app(
    layout_source: Option<&str>,
    page_source: &str,
) -> Result<String, PreviewError> {
    let layout_sources = layout_source.into_iter().collect::<Vec<_>>();
    preview_ax_route(&layout_sources, page_source)
}

pub fn preview_ax_app_with_imports(
    layout_source: Option<&str>,
    page_source: &str,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    let layout_sources = layout_source.into_iter().collect::<Vec<_>>();
    preview_ax_route_with_imports(&layout_sources, page_source, import_resolver)
}

pub fn preview_ax_route(
    layout_sources: &[&str],
    page_source: &str,
) -> Result<String, PreviewError> {
    preview_ax_route_with_loaders(layout_sources, &[], page_source)
}

pub fn preview_ax_route_with_imports(
    layout_sources: &[&str],
    page_source: &str,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    let response = preview_ax_route_stream_response_with_imports(
        layout_sources,
        page_source,
        import_resolver,
    )?;
    Ok(String::from_utf8(response.body.into_bytes())
        .expect("preview renderer only emits UTF-8 HTML"))
}

pub fn preview_ax_route_stream_response_with_imports(
    layout_sources: &[&str],
    page_source: &str,
    import_resolver: &impl AxImportResolver,
) -> Result<server::AxHttpResponse, PreviewError> {
    let store = AxPreviewStore::default();
    preview_ax_route_stream_response_with_backend_and_imports(
        layout_sources,
        &[],
        &[],
        page_source,
        "/",
        &store,
        import_resolver,
    )
}

pub fn preview_ax_route_with_loaders(
    layout_sources: &[&str],
    loader_sources: &[&str],
    page_source: &str,
) -> Result<String, PreviewError> {
    let store = AxPreviewStore::default();
    let response = preview_ax_route_stream_response_with_backend(
        layout_sources,
        loader_sources,
        &[],
        page_source,
        "/",
        &store,
    )?;
    Ok(String::from_utf8(response.body.into_bytes())
        .expect("preview renderer only emits UTF-8 HTML"))
}

pub fn preview_ax_route_stream_response(
    layout_sources: &[&str],
    page_source: &str,
) -> Result<server::AxHttpResponse, PreviewError> {
    preview_ax_route_stream_response_with_loaders(layout_sources, &[], page_source)
}

pub fn preview_ax_route_stream_response_with_loaders(
    layout_sources: &[&str],
    loader_sources: &[&str],
    page_source: &str,
) -> Result<server::AxHttpResponse, PreviewError> {
    let store = AxPreviewStore::default();
    preview_ax_route_stream_response_with_backend(
        layout_sources,
        loader_sources,
        &[],
        page_source,
        "/",
        &store,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPreviewStore {
    collections: BTreeMap<String, Vec<AxValue>>,
}

impl Default for AxPreviewStore {
    fn default() -> Self {
        let mut collections = BTreeMap::new();
        collections.insert(
            "posts".to_string(),
            sample_preview_collection_items("posts"),
        );
        collections.insert(
            "users".to_string(),
            sample_preview_collection_items("users"),
        );
        Self { collections }
    }
}

impl AxPreviewStore {
    pub fn with_collection(mut self, collection: impl Into<String>, items: Vec<AxValue>) -> Self {
        self.collections.insert(collection.into(), items);
        self
    }

    pub fn collection_items(&self, collection: &str) -> Vec<AxValue> {
        self.collections
            .get(collection)
            .cloned()
            .unwrap_or_else(|| sample_preview_collection_items(collection))
    }

    fn ensure_collection(&mut self, collection: &str) -> &mut Vec<AxValue> {
        if !self.collections.contains_key(collection) {
            self.collections.insert(
                collection.to_string(),
                sample_preview_collection_items(collection),
            );
        }

        self.collections
            .get_mut(collection)
            .expect("collection should exist after ensure")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPreviewActionResult {
    pub redirect_to: Option<String>,
    pub value: AxValue,
    pub patches: Vec<AxPreviewStatePatch>,
    pub invalidations: Vec<AxPreviewInvalidation>,
    pub error: Option<AxPreviewActionError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPreviewInvalidation {
    pub target: String,
    pub query_key: Vec<String>,
}

impl AxPreviewInvalidation {
    pub fn new(target: impl Into<String>) -> Self {
        let target = target.into();
        Self {
            query_key: vec![normalize_preview_invalidation_target(&target)],
            target,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPreviewActionError {
    pub message: String,
    pub status: u16,
    pub value: AxValue,
}

impl AxPreviewActionError {
    pub fn validation(message: impl Into<String>, value: AxValue) -> Self {
        Self {
            message: message.into(),
            status: 422,
            value,
        }
    }
}

fn normalize_preview_invalidation_target(target: &str) -> String {
    let target = target.trim().trim_matches('"').trim_matches('/');
    if target.is_empty() {
        "root".to_string()
    } else {
        target.replace('/', ".")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPreviewStatePatch {
    pub op: String,
    pub signal: String,
    pub value: AxValue,
    pub source: Option<String>,
}

impl AxPreviewStatePatch {
    pub fn set(signal: impl Into<String>, value: AxValue) -> Self {
        Self {
            op: "set".to_string(),
            signal: signal.into(),
            value,
            source: Some("action".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxPreviewHttpResponse {
    pub status: u16,
    pub content_type: String,
    pub headers: BTreeMap<String, String>,
    pub set_cookies: Vec<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewHandlers {
    routes: Vec<AxHandlerPlan>,
    loaders: BTreeMap<String, AxHandlerPlan>,
    actions: BTreeMap<String, AxHandlerPlan>,
    functions: BTreeMap<String, AxFunctionPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewFilter {
    field: String,
    op: AxQueryFilterOpPlan,
    value: AxValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewRouteMatch<'a> {
    handler: &'a AxHandlerPlan,
    params: BTreeMap<String, AxValue>,
}

pub fn preview_ax_route_with_backend(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    store: &AxPreviewStore,
) -> Result<String, PreviewError> {
    let import_resolver = |_: &str| None;
    let response = preview_ax_route_stream_response_with_backend_and_imports(
        layout_sources,
        loader_sources,
        action_sources,
        page_source,
        request_target,
        store,
        &import_resolver,
    )?;
    Ok(String::from_utf8(response.body.into_bytes())
        .expect("preview renderer only emits UTF-8 HTML"))
}

pub fn preview_ax_route_with_backend_and_imports(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    store: &AxPreviewStore,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    let response = preview_ax_route_stream_response_with_backend_and_imports(
        layout_sources,
        loader_sources,
        action_sources,
        page_source,
        request_target,
        store,
        import_resolver,
    )?;
    Ok(String::from_utf8(response.body.into_bytes())
        .expect("preview renderer only emits UTF-8 HTML"))
}

pub fn preview_ax_route_stream_response_with_backend(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    store: &AxPreviewStore,
) -> Result<server::AxHttpResponse, PreviewError> {
    let import_resolver = |_: &str| None;
    preview_ax_route_stream_response_with_backend_and_imports(
        layout_sources,
        loader_sources,
        action_sources,
        page_source,
        request_target,
        store,
        &import_resolver,
    )
}

pub fn preview_ax_route_stream_response_with_backend_and_imports(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    store: &AxPreviewStore,
    import_resolver: &impl AxImportResolver,
) -> Result<server::AxHttpResponse, PreviewError> {
    let page_document = parse_ax_auto(page_source)?;
    let mut document = page_document;

    for layout_source in layout_sources.iter().rev() {
        let layout_document = parse_ax_auto(layout_source)?;
        document = compose_layout_with_page(layout_document, document);
    }

    let handlers = collect_preview_handlers(loader_sources, action_sources, &[])?;
    let cache = RefCell::new(BTreeMap::new());
    let env = backend::AxEnv::from_env();
    let route_scope = build_preview_route_scope(
        request_target,
        &BTreeMap::new(),
        &parse_preview_query_fields(request_target),
    );
    let resolve_context = PreviewResolveContext {
        handlers: &handlers,
        cache: &cache,
        env: &env,
        runtime: None,
        request_target,
        route_scope: &route_scope,
        store,
    };
    let resolver_error = RefCell::new(None);
    let resolver = |path: &[String], args: &[AxValue]| -> Option<AxValue> {
        match preview_resolve_call(&resolve_context, path, args) {
            Ok(value) => value,
            Err(error) => {
                let mut slot = resolver_error.borrow_mut();
                if slot.is_none() {
                    *slot = Some(error);
                }
                None
            }
        }
    };

    let node = match lower_document_with_scope_and_imports(
        &document,
        route_scope.clone(),
        &resolver,
        import_resolver,
    ) {
        Ok(node) => node,
        Err(error) => {
            if let Some(runtime_error) = resolver_error.into_inner() {
                return Err(runtime_error);
            }
            return Err(error.into());
        }
    };

    if let Some(runtime_error) = resolver_error.into_inner() {
        return Err(runtime_error);
    }

    Ok(render_preview_document_response(&document, &node))
}

pub fn preview_ax_route_with_request_context(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    route_params: &BTreeMap<String, String>,
    store: &AxPreviewStore,
) -> Result<String, PreviewError> {
    let import_resolver = |_: &str| None;
    preview_ax_route_with_request_context_and_imports(
        layout_sources,
        loader_sources,
        action_sources,
        page_source,
        request_target,
        route_params,
        store,
        &import_resolver,
    )
}

// Compatibility facade mirrors the generated route inputs without an opaque options bag.
#[allow(clippy::too_many_arguments)]
pub fn preview_ax_route_with_request_context_and_imports(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    route_params: &BTreeMap<String, String>,
    store: &AxPreviewStore,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    preview_ax_route_with_request_context_runtime_and_imports(
        layout_sources,
        loader_sources,
        action_sources,
        page_source,
        request_target,
        route_params,
        None,
        store,
        import_resolver,
    )
}

// Compatibility facade mirrors the generated route inputs without an opaque options bag.
#[allow(clippy::too_many_arguments)]
pub fn preview_ax_route_with_request_context_and_runtime_and_imports(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    route_params: &BTreeMap<String, String>,
    runtime: &dyn backend::AxBackendRuntime,
    store: &AxPreviewStore,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    preview_ax_route_with_request_context_runtime_and_imports(
        layout_sources,
        loader_sources,
        action_sources,
        page_source,
        request_target,
        route_params,
        Some(runtime),
        store,
        import_resolver,
    )
}

// Internal counterpart intentionally matches the public compatibility facade above.
#[allow(clippy::too_many_arguments)]
fn preview_ax_route_with_request_context_runtime_and_imports(
    layout_sources: &[&str],
    loader_sources: &[&str],
    action_sources: &[&str],
    page_source: &str,
    request_target: &str,
    route_params: &BTreeMap<String, String>,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &AxPreviewStore,
    import_resolver: &impl AxImportResolver,
) -> Result<String, PreviewError> {
    let page_document = parse_ax_auto(page_source)?;
    let mut document = page_document;

    for layout_source in layout_sources.iter().rev() {
        let layout_document = parse_ax_auto(layout_source)?;
        document = compose_layout_with_page(layout_document, document);
    }

    let handlers = collect_preview_handlers(loader_sources, action_sources, &[])?;
    let cache = RefCell::new(BTreeMap::new());
    let fallback_env;
    let env = if let Some(runtime) = runtime {
        runtime.env()
    } else {
        fallback_env = backend::AxEnv::from_env();
        &fallback_env
    };
    let route_scope = build_preview_route_scope(
        request_target,
        route_params,
        &parse_preview_query_fields(request_target),
    );
    let resolve_context = PreviewResolveContext {
        handlers: &handlers,
        cache: &cache,
        env,
        runtime,
        request_target,
        route_scope: &route_scope,
        store,
    };
    let resolver_error = RefCell::new(None);
    let resolver = |path: &[String], args: &[AxValue]| -> Option<AxValue> {
        match preview_resolve_call(&resolve_context, path, args) {
            Ok(value) => value,
            Err(error) => {
                let mut slot = resolver_error.borrow_mut();
                if slot.is_none() {
                    *slot = Some(error);
                }
                None
            }
        }
    };

    let node = match lower_document_with_scope_and_imports(
        &document,
        route_scope.clone(),
        &resolver,
        import_resolver,
    ) {
        Ok(node) => node,
        Err(error) => {
            if let Some(runtime_error) = resolver_error.into_inner() {
                return Err(runtime_error);
            }
            return Err(error.into());
        }
    };

    if let Some(runtime_error) = resolver_error.into_inner() {
        return Err(runtime_error);
    }

    Ok(render_preview_document(&document, &node))
}

pub fn execute_preview_action_sources(
    action_sources: &[&str],
    action_name: &str,
    input_fields: &BTreeMap<String, String>,
    store: &mut AxPreviewStore,
) -> Result<AxPreviewActionResult, PreviewError> {
    let handlers = collect_preview_handlers(&[], action_sources, &[])?;
    let env = backend::AxEnv::from_env();
    execute_preview_action(
        &handlers.actions,
        &handlers.functions,
        action_name,
        input_fields,
        &env,
        None,
        store,
    )
}

pub fn execute_preview_action_sources_with_runtime(
    action_sources: &[&str],
    action_name: &str,
    input_fields: &BTreeMap<String, String>,
    runtime: &dyn backend::AxBackendRuntime,
    store: &mut AxPreviewStore,
) -> Result<AxPreviewActionResult, PreviewError> {
    let handlers = collect_preview_handlers(&[], action_sources, &[])?;
    execute_preview_action(
        &handlers.actions,
        &handlers.functions,
        action_name,
        input_fields,
        runtime.env(),
        Some(runtime),
        store,
    )
}

pub fn execute_preview_route_sources(
    route_sources: &[&str],
    method: &str,
    request_target: &str,
    store: &mut AxPreviewStore,
) -> Result<Option<AxPreviewHttpResponse>, PreviewError> {
    let request = server::AxHttpRequest::new(method, request_target);
    execute_preview_route_request_sources(route_sources, &request, store)
}

pub fn execute_preview_route_request_sources(
    route_sources: &[&str],
    request: &server::AxHttpRequest,
    store: &mut AxPreviewStore,
) -> Result<Option<AxPreviewHttpResponse>, PreviewError> {
    let handlers = collect_preview_handlers(&[], &[], route_sources)?;
    let env = backend::AxEnv::from_env();
    let request_path = normalize_preview_request_path(&request.target)?;
    let query = parse_preview_query_fields(&request.target);
    execute_preview_route(
        &handlers.routes,
        request,
        &request_path,
        &query,
        &env,
        None,
        store,
    )
}

pub fn execute_preview_route_request_sources_with_runtime(
    route_sources: &[&str],
    request: &server::AxHttpRequest,
    runtime: &dyn backend::AxBackendRuntime,
    store: &mut AxPreviewStore,
) -> Result<Option<AxPreviewHttpResponse>, PreviewError> {
    let handlers = collect_preview_handlers(&[], &[], route_sources)?;
    let env = runtime.env();
    let request_path = normalize_preview_request_path(&request.target)?;
    let query = parse_preview_query_fields(&request.target);
    execute_preview_route(
        &handlers.routes,
        request,
        &request_path,
        &query,
        env,
        Some(runtime),
        store,
    )
}

pub fn preview_action_endpoint_path(request_path: &str, action_name: &str) -> String {
    format!(
        "/__axonyx/action?path={}&name={}",
        url_encode(request_path),
        url_encode(action_name)
    )
}

fn collect_preview_handlers(
    loader_sources: &[&str],
    action_sources: &[&str],
    route_sources: &[&str],
) -> Result<PreviewHandlers, PreviewError> {
    let mut routes = Vec::new();
    let mut loaders = BTreeMap::new();
    let mut actions = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let mut globals = Vec::new();

    for source in route_sources {
        let document = parse_backend_ax(source)?;
        let plan = lower_backend_document(&document)?;
        globals.extend(plan.globals);
        collect_preview_functions(plan.functions, &mut functions);

        for handler in plan.handlers {
            if !matches!(handler.kind, AxHandlerKind::Route { .. }) {
                continue;
            }

            routes.push(handler);
        }
    }

    for source in loader_sources {
        let document = parse_backend_ax(source)?;
        let plan = lower_backend_document(&document)?;
        globals.extend(plan.globals);
        collect_preview_functions(plan.functions, &mut functions);

        for handler in plan.handlers {
            if matches!(handler.kind, AxHandlerKind::Loader { .. }) {
                loaders.insert(handler.name.clone(), handler);
            }
        }
    }

    for source in action_sources {
        let document = parse_backend_ax(source)?;
        let plan = lower_backend_document(&document)?;
        globals.extend(plan.globals);
        collect_preview_functions(plan.functions, &mut functions);

        for handler in plan.handlers {
            if matches!(handler.kind, AxHandlerKind::Action { .. }) {
                actions.insert(handler.name.clone(), handler);
            }
        }
    }

    routes = routes
        .into_iter()
        .map(|handler| with_preview_globals(handler, &globals))
        .collect();
    loaders = loaders
        .into_iter()
        .map(|(name, handler)| (name, with_preview_globals(handler, &globals)))
        .collect();
    actions = actions
        .into_iter()
        .map(|(name, handler)| (name, with_preview_globals(handler, &globals)))
        .collect();

    Ok(PreviewHandlers {
        routes,
        loaders,
        actions,
        functions,
    })
}

fn collect_preview_functions(
    functions: impl IntoIterator<Item = AxFunctionPlan>,
    out: &mut BTreeMap<String, AxFunctionPlan>,
) {
    for function in functions {
        out.insert(function.name.clone(), function);
    }
}

struct PreviewResolveContext<'a> {
    handlers: &'a PreviewHandlers,
    cache: &'a RefCell<BTreeMap<String, AxValue>>,
    env: &'a backend::AxEnv,
    runtime: Option<&'a dyn backend::AxBackendRuntime>,
    request_target: &'a str,
    route_scope: &'a BTreeMap<String, AxValue>,
    store: &'a AxPreviewStore,
}

fn preview_resolve_call(
    context: &PreviewResolveContext<'_>,
    path: &[String],
    args: &[AxValue],
) -> Result<Option<AxValue>, PreviewError> {
    let PreviewResolveContext {
        handlers,
        cache,
        env,
        runtime,
        request_target,
        route_scope,
        store,
    } = context;
    let runtime = *runtime;
    if path == ["load".to_string()] {
        let [AxValue::String(loader_name)] = args else {
            return Err(PreviewError::Runtime {
                message: "load(...) expects a single loader name".to_string(),
            });
        };

        let cache_key = preview_loader_cache_key(loader_name, &[]);
        if let Some(cached) = cache.borrow().get(&cache_key).cloned() {
            return Ok(Some(cached));
        }

        let loader = handlers
            .loaders
            .get(loader_name)
            .ok_or_else(|| PreviewError::Runtime {
                message: format!("loader `{loader_name}` was not found for this route"),
            })?;
        let value = execute_preview_loader(
            loader,
            &[],
            route_scope,
            env,
            runtime,
            store,
            &handlers.functions,
        )?;
        cache.borrow_mut().insert(cache_key, value.clone());
        return Ok(Some(value));
    }

    if path.len() == 1 {
        let loader_name = &path[0];
        let cache_key = preview_loader_cache_key(loader_name, args);
        if let Some(cached) = cache.borrow().get(&cache_key).cloned() {
            return Ok(Some(cached));
        }

        if let Some(loader) = handlers.loaders.get(loader_name) {
            let value = execute_preview_loader(
                loader,
                args,
                route_scope,
                env,
                runtime,
                store,
                &handlers.functions,
            )?;
            cache.borrow_mut().insert(cache_key, value.clone());
            return Ok(Some(value));
        }

        if let Some(function) = handlers.functions.get(loader_name) {
            let value =
                execute_preview_function(function, args, route_scope, env, &handlers.functions)?;
            cache.borrow_mut().insert(cache_key, value.clone());
            return Ok(Some(value));
        }
    }

    if path == ["action".to_string()] {
        let [AxValue::String(action_name)] = args else {
            return Err(PreviewError::Runtime {
                message: "action(...) expects a single action name".to_string(),
            });
        };

        if !handlers.actions.contains_key(action_name) {
            return Err(PreviewError::Runtime {
                message: format!("action `{action_name}` was not found for this route"),
            });
        }

        return Ok(Some(AxValue::String(preview_action_endpoint_path(
            request_target,
            action_name,
        ))));
    }

    if path.len() == 3 && path[0] == "db" && path[2] == "all" {
        if !args.is_empty() {
            return Err(PreviewError::Runtime {
                message: "db.<collection>.all() does not accept arguments".to_string(),
            });
        }

        return Ok(Some(AxValue::List(store.collection_items(&path[1]))));
    }

    if path == ["Content".to_string(), "Collection".to_string()] {
        let [AxValue::String(collection)] = args else {
            return Err(PreviewError::Runtime {
                message: "Content.Collection(...) expects a collection name".to_string(),
            });
        };

        return Ok(Some(AxValue::List(store.collection_items(collection))));
    }

    Ok(None)
}

fn preview_loader_cache_key(loader_name: &str, args: &[AxValue]) -> String {
    if args.is_empty() {
        return loader_name.to_string();
    }

    let args = args
        .iter()
        .map(preview_value_cache_key)
        .collect::<Vec<_>>()
        .join(",");
    format!("{loader_name}({args})")
}

fn preview_value_cache_key(value: &AxValue) -> String {
    match value {
        AxValue::Null => "null".to_string(),
        AxValue::String(value) => format!("s:{value:?}"),
        AxValue::Number(value) => format!("n:{value}"),
        AxValue::Float(value) => format!("f:{:?}", value.get()),
        AxValue::Bool(value) => format!("b:{value}"),
        AxValue::Record(fields) => {
            let fields = fields
                .iter()
                .map(|(key, value)| format!("{key}:{}", preview_value_cache_key(value)))
                .collect::<Vec<_>>()
                .join(",");
            format!("r:{{{fields}}}")
        }
        AxValue::List(items) => {
            let items = items
                .iter()
                .map(preview_value_cache_key)
                .collect::<Vec<_>>()
                .join(",");
            format!("l:[{items}]")
        }
    }
}

fn execute_preview_loader(
    loader: &AxHandlerPlan,
    args: &[AxValue],
    initial_scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &AxPreviewStore,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    let mut scope = initial_scope.clone();
    let AxHandlerKind::Loader { input, .. } = &loader.kind else {
        return Err(PreviewError::Runtime {
            message: format!("handler `{}` is not a loader", loader.name),
        });
    };
    if args.len() > input.len() {
        return Err(PreviewError::Runtime {
            message: format!(
                "loader `{}` expected {} argument(s) but received {}",
                loader.name,
                input.len(),
                args.len()
            ),
        });
    }
    if !input.is_empty() {
        scope.insert(
            "input".to_string(),
            build_preview_loader_input_record(input, args)?,
        );
    }

    for step in &loader.steps {
        match step {
            AxStepPlan::Let { binding, value } => {
                let value = eval_preview_value_with_functions(
                    value, &scope, env, runtime, store, functions,
                )?;
                scope.insert(binding.clone(), value);
            }
            AxStepPlan::Return(value) => {
                return eval_preview_return_with_functions(value, &scope, env, functions)
            }
            AxStepPlan::Insert { .. }
            | AxStepPlan::Update { .. }
            | AxStepPlan::Delete { .. }
            | AxStepPlan::Revalidate { .. }
            | AxStepPlan::Patch { .. }
            | AxStepPlan::Hook { .. }
            | AxStepPlan::Header { .. }
            | AxStepPlan::Cookie { .. }
            | AxStepPlan::ClearCookie { .. }
            | AxStepPlan::Require { .. }
            | AxStepPlan::Send { .. } => {}
        }
    }

    Ok(AxValue::Null)
}

fn execute_preview_function(
    function: &AxFunctionPlan,
    args: &[AxValue],
    initial_scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    if args.len() > function.input.len() {
        return Err(PreviewError::Runtime {
            message: format!(
                "function `{}` expected {} argument(s) but received {}",
                function.name,
                function.input.len(),
                args.len()
            ),
        });
    }

    let mut scope = initial_scope.clone();
    bind_preview_function_inputs(function, args, &mut scope)?;

    for step in &function.steps {
        match step {
            AxStepPlan::Let { binding, value } => match value {
                AxValuePlan::Expr(expr) => {
                    let value = eval_preview_expr_with_functions(expr, &scope, env, functions)?;
                    scope.insert(binding.clone(), value);
                }
                AxValuePlan::Query(_) => {
                    return Err(PreviewError::Runtime {
                        message: format!(
                            "function `{}` cannot use query-backed data binding `{}` in preview yet",
                            function.name, binding
                        ),
                    });
                }
            },
            AxStepPlan::Return(value) => {
                return eval_preview_return_with_functions(value, &scope, env, functions)
            }
            AxStepPlan::Insert { .. }
            | AxStepPlan::Update { .. }
            | AxStepPlan::Delete { .. }
            | AxStepPlan::Revalidate { .. }
            | AxStepPlan::Patch { .. }
            | AxStepPlan::Hook { .. }
            | AxStepPlan::Header { .. }
            | AxStepPlan::Cookie { .. }
            | AxStepPlan::ClearCookie { .. }
            | AxStepPlan::Require { .. }
            | AxStepPlan::Send { .. } => {
                return Err(PreviewError::Runtime {
                    message: format!(
                        "function `{}` only supports data and return steps in preview",
                        function.name
                    ),
                });
            }
        }
    }

    Ok(AxValue::Null)
}

fn bind_preview_function_inputs(
    function: &AxFunctionPlan,
    args: &[AxValue],
    scope: &mut BTreeMap<String, AxValue>,
) -> Result<(), PreviewError> {
    for (index, field) in function.input.iter().enumerate() {
        let Some(value) = args.get(index).cloned() else {
            if let Some(default) = &field.default {
                scope.insert(
                    field.name.clone(),
                    coerce_preview_default_input_value(&field.name, &field.rust_ty, default)?,
                );
                continue;
            }
            if field.optional {
                scope.insert(field.name.clone(), AxValue::Null);
                continue;
            }
            return Err(PreviewError::Runtime {
                message: format!("missing required function input `{}`", field.name),
            });
        };
        scope.insert(
            field.name.clone(),
            coerce_preview_function_input_value(field, value)?,
        );
    }
    Ok(())
}

fn execute_preview_action(
    actions: &BTreeMap<String, AxHandlerPlan>,
    functions: &BTreeMap<String, AxFunctionPlan>,
    action_name: &str,
    input_fields: &BTreeMap<String, String>,
    env: &backend::AxEnv,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &mut AxPreviewStore,
) -> Result<AxPreviewActionResult, PreviewError> {
    let action = actions
        .get(action_name)
        .ok_or_else(|| PreviewError::Runtime {
            message: format!("action `{action_name}` was not found for this route"),
        })?;

    let AxHandlerKind::Action { input, .. } = &action.kind else {
        return Err(PreviewError::Runtime {
            message: format!("handler `{action_name}` is not an action"),
        });
    };

    let mut scope = BTreeMap::new();
    scope.insert(
        "input".to_string(),
        build_preview_input_record(input, input_fields)?,
    );

    let mut redirect_to = None;
    let mut value = AxValue::record([("ok", AxValue::Bool(true))]);
    let mut patches = Vec::new();
    let mut invalidations = Vec::new();

    for step in &action.steps {
        match step {
            AxStepPlan::Let {
                binding,
                value: plan,
            } => {
                let evaluated = eval_preview_value_with_functions(
                    plan, &scope, env, runtime, store, functions,
                )?;
                scope.insert(binding.clone(), evaluated);
            }
            AxStepPlan::Insert { collection, fields } => {
                let mut record =
                    eval_preview_fields_with_functions(fields, &scope, env, functions)?;
                if let Some(runtime) = runtime {
                    runtime.insert(&backend::AxInsertRequest {
                        collection: collection.clone(),
                        fields: record
                            .into_iter()
                            .map(|(key, value)| (key, preview_value_to_json(&value)))
                            .collect(),
                    })?;
                } else {
                    assign_preview_id(&mut record, store.collection_items(collection).len());
                    store
                        .ensure_collection(collection)
                        .push(AxValue::Record(record));
                }
                push_preview_auto_invalidation(&mut invalidations, collection.clone());
            }
            AxStepPlan::Update {
                collection,
                fields,
                filters,
            } => {
                let fields = eval_preview_fields_with_functions(fields, &scope, env, functions)?;
                let filters = eval_preview_filters_with_functions(filters, &scope, env, functions)?;
                if let Some(runtime) = runtime {
                    runtime.update(&backend::AxUpdateRequest {
                        collection: collection.clone(),
                        fields: fields
                            .into_iter()
                            .map(|(key, value)| (key, preview_value_to_json(&value)))
                            .collect(),
                        filters: filters
                            .into_iter()
                            .map(|filter| backend::AxQueryFilterRequest {
                                field: filter.field,
                                op: preview_filter_op_to_runtime(filter.op),
                                value: preview_value_to_json(&filter.value),
                            })
                            .collect(),
                    })?;
                } else {
                    for item in store.ensure_collection(collection).iter_mut() {
                        if preview_record_matches_all(item, &filters) {
                            apply_preview_fields(item, &fields);
                        }
                    }
                }
                push_preview_auto_invalidation(&mut invalidations, collection.clone());
            }
            AxStepPlan::Delete {
                collection,
                filters,
            } => {
                let filters = eval_preview_filters_with_functions(filters, &scope, env, functions)?;
                if let Some(runtime) = runtime {
                    runtime.delete(&backend::AxDeleteRequest {
                        collection: collection.clone(),
                        filters: filters
                            .into_iter()
                            .map(|filter| backend::AxQueryFilterRequest {
                                field: filter.field,
                                op: preview_filter_op_to_runtime(filter.op),
                                value: preview_value_to_json(&filter.value),
                            })
                            .collect(),
                    })?;
                } else {
                    store
                        .ensure_collection(collection)
                        .retain(|item| !preview_record_matches_all(item, &filters));
                }
                push_preview_auto_invalidation(&mut invalidations, collection.clone());
            }
            AxStepPlan::Revalidate { target, literal } => {
                let target = eval_preview_revalidation_target_with_functions(
                    target, *literal, &scope, env, functions,
                )?;
                if target.starts_with('/') {
                    redirect_to = Some(target.clone());
                }
                if let Some(runtime) = runtime {
                    runtime.revalidate(&target)?;
                }
                push_preview_explicit_invalidation(&mut invalidations, target);
            }
            AxStepPlan::Patch { signal, value } => {
                let signal =
                    eval_preview_expr_with_functions(signal, &scope, env, functions)?.as_string();
                let value = eval_preview_expr_with_functions(value, &scope, env, functions)?;
                patches.push(AxPreviewStatePatch::set(signal, value));
            }
            AxStepPlan::Require {
                value: requirement,
                fallback,
            } => {
                if preview_require_passes(&eval_preview_require_expr_with_functions(
                    requirement,
                    &scope,
                    env,
                    functions,
                )?) {
                    continue;
                }
                let error = eval_preview_action_error_fallback_with_functions(
                    fallback.as_ref(),
                    &scope,
                    env,
                    functions,
                )?;
                return Ok(AxPreviewActionResult {
                    redirect_to,
                    value,
                    patches,
                    invalidations,
                    error: Some(error),
                });
            }
            AxStepPlan::Return(result) => {
                value = eval_preview_return_with_functions(result, &scope, env, functions)?;
            }
            AxStepPlan::Header { .. }
            | AxStepPlan::Hook { .. }
            | AxStepPlan::Cookie { .. }
            | AxStepPlan::ClearCookie { .. }
            | AxStepPlan::Send { .. } => {}
        }
    }

    Ok(AxPreviewActionResult {
        redirect_to,
        value,
        patches,
        invalidations,
        error: None,
    })
}

fn with_preview_globals(mut handler: AxHandlerPlan, globals: &[AxStepPlan]) -> AxHandlerPlan {
    if globals.is_empty() {
        return handler;
    }

    let mut steps = globals.to_vec();
    steps.extend(handler.steps);
    handler.steps = steps;
    handler
}

fn execute_preview_route(
    routes: &[AxHandlerPlan],
    request: &server::AxHttpRequest,
    request_path: &str,
    query: &BTreeMap<String, String>,
    env: &backend::AxEnv,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &mut AxPreviewStore,
) -> Result<Option<AxPreviewHttpResponse>, PreviewError> {
    let Some(route_match) = match_preview_route(routes, &request.method, request_path) else {
        return Ok(None);
    };

    let mut scope = BTreeMap::new();
    scope.insert("params".to_string(), AxValue::Record(route_match.params));
    scope.insert("query".to_string(), build_preview_query_record(query));
    scope.insert("request".to_string(), build_preview_request_record(request));
    scope.insert("Auth".to_string(), build_preview_auth_record(request, env));
    if let AxHandlerKind::Route { input, .. } = &route_match.handler.kind {
        if !input.is_empty() {
            scope.insert(
                "input".to_string(),
                build_preview_route_input_record(input, request)?,
            );
        }
    }
    let mut headers = BTreeMap::new();
    let mut set_cookies = Vec::new();
    let mut after_hooks = Vec::new();
    for step in &route_match.handler.steps {
        match step {
            AxStepPlan::Let {
                binding,
                value: plan,
            } => {
                let evaluated = eval_preview_value(plan, &scope, env, runtime, store)?;
                scope.insert(binding.clone(), evaluated);
            }
            AxStepPlan::Insert { collection, fields } => {
                let mut record = eval_preview_fields(fields, &scope, env)?;
                if let Some(runtime) = runtime {
                    runtime.insert(&backend::AxInsertRequest {
                        collection: collection.clone(),
                        fields: record
                            .into_iter()
                            .map(|(key, value)| (key, preview_value_to_json(&value)))
                            .collect(),
                    })?;
                } else {
                    assign_preview_id(&mut record, store.collection_items(collection).len());
                    store
                        .ensure_collection(collection)
                        .push(AxValue::Record(record));
                }
            }
            AxStepPlan::Update {
                collection,
                fields,
                filters,
            } => {
                let fields = eval_preview_fields(fields, &scope, env)?;
                let filters = eval_preview_filters(filters, &scope, env)?;
                if let Some(runtime) = runtime {
                    runtime.update(&backend::AxUpdateRequest {
                        collection: collection.clone(),
                        fields: fields
                            .into_iter()
                            .map(|(key, value)| (key, preview_value_to_json(&value)))
                            .collect(),
                        filters: filters
                            .into_iter()
                            .map(|filter| backend::AxQueryFilterRequest {
                                field: filter.field,
                                op: preview_filter_op_to_runtime(filter.op),
                                value: preview_value_to_json(&filter.value),
                            })
                            .collect(),
                    })?;
                } else {
                    for item in store.ensure_collection(collection).iter_mut() {
                        if preview_record_matches_all(item, &filters) {
                            apply_preview_fields(item, &fields);
                        }
                    }
                }
            }
            AxStepPlan::Delete {
                collection,
                filters,
            } => {
                let filters = eval_preview_filters(filters, &scope, env)?;
                if let Some(runtime) = runtime {
                    runtime.delete(&backend::AxDeleteRequest {
                        collection: collection.clone(),
                        filters: filters
                            .into_iter()
                            .map(|filter| backend::AxQueryFilterRequest {
                                field: filter.field,
                                op: preview_filter_op_to_runtime(filter.op),
                                value: preview_value_to_json(&filter.value),
                            })
                            .collect(),
                    })?;
                } else {
                    store
                        .ensure_collection(collection)
                        .retain(|item| !preview_record_matches_all(item, &filters));
                }
            }
            AxStepPlan::Hook { phase, value } => match phase {
                AxHookPhasePlan::Before => {
                    if let Some(response) =
                        apply_preview_route_hook(value, &scope, env, &mut headers)?
                    {
                        apply_preview_route_after_hooks(&after_hooks, &scope, env, &mut headers)?;
                        return Ok(Some(apply_preview_response_metadata(
                            response,
                            headers,
                            set_cookies,
                        )));
                    }
                }
                AxHookPhasePlan::After => after_hooks.push(value),
            },
            AxStepPlan::Header { name, value } => {
                headers.insert(
                    eval_preview_expr(name, &scope, env)?.as_string(),
                    eval_preview_expr(value, &scope, env)?.as_string(),
                );
            }
            AxStepPlan::Cookie { name, value } => {
                set_cookies.push(
                    server::AxCookie::new(
                        eval_preview_expr(name, &scope, env)?.as_string(),
                        eval_preview_expr(value, &scope, env)?.as_string(),
                    )
                    .with_path("/")
                    .render(),
                );
            }
            AxStepPlan::ClearCookie { name } => {
                set_cookies.push(
                    server::AxCookie::new(eval_preview_expr(name, &scope, env)?.as_string(), "")
                        .with_path("/")
                        .with_max_age(0)
                        .render(),
                );
            }
            AxStepPlan::Require { value, fallback } => {
                if !preview_require_passes(&eval_preview_require_expr(value, &scope, env)?) {
                    let response = render_preview_require_fallback(fallback.as_ref(), &scope, env)?;
                    apply_preview_route_after_hooks(&after_hooks, &scope, env, &mut headers)?;
                    return Ok(Some(apply_preview_response_metadata(
                        response,
                        headers,
                        set_cookies,
                    )));
                }
            }
            AxStepPlan::Return(result) => {
                apply_preview_route_after_hooks(&after_hooks, &scope, env, &mut headers)?;
                return render_preview_route_return(result, &scope, env, headers, set_cookies);
            }
            AxStepPlan::Revalidate { .. } | AxStepPlan::Patch { .. } | AxStepPlan::Send { .. } => {}
        }
    }

    apply_preview_route_after_hooks(&after_hooks, &scope, env, &mut headers)?;
    Ok(Some(apply_preview_response_metadata(
        render_preview_json_response(&AxValue::Null)?,
        headers,
        set_cookies,
    )))
}

fn render_preview_require_fallback(
    fallback: Option<&AxReturnPlan>,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<AxPreviewHttpResponse, PreviewError> {
    let mut response = match fallback {
        Some(AxReturnPlan::Expr(expr)) | Some(AxReturnPlan::Json(expr)) => {
            render_preview_json_response(&eval_preview_expr(expr, scope, env)?)?
        }
        Some(AxReturnPlan::Redirect { target, status }) => {
            let target = eval_preview_expr(target, scope, env)?.as_string();
            AxPreviewHttpResponse {
                status: status.unwrap_or(303),
                content_type: "text/plain; charset=utf-8".to_string(),
                headers: BTreeMap::from([("Location".to_string(), target)]),
                set_cookies: Vec::new(),
                body: Vec::new(),
            }
        }
        Some(AxReturnPlan::NoContent) => AxPreviewHttpResponse {
            status: 204,
            content_type: "text/plain; charset=utf-8".to_string(),
            headers: BTreeMap::new(),
            set_cookies: Vec::new(),
            body: Vec::new(),
        },
        Some(AxReturnPlan::NotFound) => AxPreviewHttpResponse {
            status: 404,
            content_type: "text/plain; charset=utf-8".to_string(),
            headers: BTreeMap::new(),
            set_cookies: Vec::new(),
            body: b"not found".to_vec(),
        },
        Some(AxReturnPlan::Ok) => {
            render_preview_json_response(&AxValue::record([("ok", AxValue::Bool(true))]))?
        }
        None => {
            let mut response = render_preview_json_response(&AxValue::record([(
                "error",
                AxValue::String("unauthorized".to_string()),
            )]))?;
            response.status = 401;
            response
        }
    };

    if matches!(
        fallback,
        Some(AxReturnPlan::Expr(_)) | Some(AxReturnPlan::Json(_))
    ) {
        response.status = 401;
    }

    Ok(response)
}

fn apply_preview_route_hook(
    hook: &AxRustExpr,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    headers: &mut BTreeMap<String, String>,
) -> Result<Option<AxPreviewHttpResponse>, PreviewError> {
    match hook.code.trim() {
        "Security.headers" => {
            headers.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());
            headers.insert(
                "Referrer-Policy".to_string(),
                "strict-origin-when-cross-origin".to_string(),
            );
            Ok(None)
        }
        "Cache.noStore" => {
            headers.insert("Cache-Control".to_string(), "no-store".to_string());
            Ok(None)
        }
        "Auth.session" | "Auth.bearer" | "Auth.signedSession" => {
            if !preview_require_passes(&eval_preview_require_expr(hook, scope, env)?) {
                return render_preview_require_fallback(None, scope, env).map(Some);
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn apply_preview_route_after_hooks(
    hooks: &[&AxRustExpr],
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    headers: &mut BTreeMap<String, String>,
) -> Result<(), PreviewError> {
    for hook in hooks {
        let _ = apply_preview_route_hook(hook, scope, env, headers)?;
    }
    Ok(())
}

fn eval_preview_require_expr(
    expr: &AxRustExpr,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<AxValue, PreviewError> {
    let functions = BTreeMap::new();
    eval_preview_require_expr_with_functions(expr, scope, env, &functions)
}

fn eval_preview_require_expr_with_functions(
    expr: &AxRustExpr,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    match eval_preview_expr_with_functions(expr, scope, env, functions) {
        Ok(value) => Ok(value),
        Err(_error) if expr.code.trim().starts_with("request.") => {
            Ok(AxValue::String(String::new()))
        }
        Err(error) => Err(error),
    }
}

fn preview_require_passes(value: &AxValue) -> bool {
    match value {
        AxValue::Null => false,
        AxValue::Bool(value) => *value,
        AxValue::String(value) => !value.is_empty(),
        AxValue::Number(_) => true,
        AxValue::Float(_) => true,
        AxValue::Record(fields) => !fields.is_empty(),
        AxValue::List(items) => !items.is_empty(),
    }
}

fn eval_preview_action_error_fallback_with_functions(
    fallback: Option<&AxReturnPlan>,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxPreviewActionError, PreviewError> {
    let value = match fallback {
        Some(AxReturnPlan::Expr(expr)) | Some(AxReturnPlan::Json(expr)) => {
            eval_preview_expr_with_functions(expr, scope, env, functions)?
        }
        Some(AxReturnPlan::Redirect { target, .. }) => AxValue::String(
            eval_preview_expr_with_functions(target, scope, env, functions)?.as_string(),
        ),
        Some(AxReturnPlan::Ok)
        | Some(AxReturnPlan::NoContent)
        | Some(AxReturnPlan::NotFound)
        | None => AxValue::String("Action requirement failed.".to_string()),
    };
    let message = match &value {
        AxValue::Record(fields) => fields
            .get("error")
            .or_else(|| fields.get("message"))
            .map(AxValue::as_string)
            .unwrap_or_else(|| "Action requirement failed.".to_string()),
        other => other.as_string(),
    };

    Ok(AxPreviewActionError::validation(message, value))
}

fn eval_preview_value(
    value: &AxValuePlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &AxPreviewStore,
) -> Result<AxValue, PreviewError> {
    let functions = BTreeMap::new();
    eval_preview_value_with_functions(value, scope, env, runtime, store, &functions)
}

fn eval_preview_value_with_functions(
    value: &AxValuePlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &AxPreviewStore,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    match value {
        AxValuePlan::Expr(expr) => eval_preview_expr_with_functions(expr, scope, env, functions),
        AxValuePlan::Query(query) => {
            eval_preview_query_with_functions(query, scope, env, runtime, store, functions)
        }
    }
}

fn eval_preview_return_with_functions(
    value: &AxReturnPlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    match value {
        AxReturnPlan::Expr(expr) => eval_preview_expr_with_functions(expr, scope, env, functions),
        AxReturnPlan::Json(expr) => eval_preview_expr_with_functions(expr, scope, env, functions),
        AxReturnPlan::Redirect { .. } | AxReturnPlan::NoContent | AxReturnPlan::NotFound => {
            Err(PreviewError::Runtime {
                message: "HTTP response helpers are only supported in route blocks".to_string(),
            })
        }
        AxReturnPlan::Ok => Ok(AxValue::record([("ok", AxValue::Bool(true))])),
    }
}

fn render_preview_route_return(
    value: &AxReturnPlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    headers: BTreeMap<String, String>,
    set_cookies: Vec<String>,
) -> Result<Option<AxPreviewHttpResponse>, PreviewError> {
    let response = match value {
        AxReturnPlan::Expr(expr) | AxReturnPlan::Json(expr) => {
            render_preview_json_response(&eval_preview_expr(expr, scope, env)?)?
        }
        AxReturnPlan::Redirect { target, status } => {
            let target = eval_preview_expr(target, scope, env)?.as_string();
            AxPreviewHttpResponse {
                status: status.unwrap_or(303),
                content_type: "text/plain; charset=utf-8".to_string(),
                headers: BTreeMap::from([("Location".to_string(), target)]),
                set_cookies: Vec::new(),
                body: Vec::new(),
            }
        }
        AxReturnPlan::NoContent => AxPreviewHttpResponse {
            status: 204,
            content_type: "text/plain; charset=utf-8".to_string(),
            headers: BTreeMap::new(),
            set_cookies: Vec::new(),
            body: Vec::new(),
        },
        AxReturnPlan::NotFound => AxPreviewHttpResponse {
            status: 404,
            content_type: "text/plain; charset=utf-8".to_string(),
            headers: BTreeMap::new(),
            set_cookies: Vec::new(),
            body: b"not found".to_vec(),
        },
        AxReturnPlan::Ok => {
            render_preview_json_response(&AxValue::record([("ok", AxValue::Bool(true))]))?
        }
    };

    Ok(Some(apply_preview_response_metadata(
        response,
        headers,
        set_cookies,
    )))
}

fn apply_preview_response_metadata(
    mut response: AxPreviewHttpResponse,
    headers: BTreeMap<String, String>,
    set_cookies: Vec<String>,
) -> AxPreviewHttpResponse {
    for (name, value) in headers {
        response.headers.insert(name, value);
    }
    response.set_cookies.extend(set_cookies);
    response
}

fn eval_preview_query_with_functions(
    query: &AxQueryPlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    runtime: Option<&dyn backend::AxBackendRuntime>,
    store: &AxPreviewStore,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    if let Some(runtime) = runtime {
        match &query.source {
            AxQuerySourcePlan::Stream { collection } => {
                let request =
                    preview_query_to_runtime_request(collection, query, scope, env, functions)?;
                return Ok(preview_json_to_value(runtime.load(&request)?));
            }
            AxQuerySourcePlan::RawSql { sql, params } => {
                let params = params
                    .iter()
                    .map(|param| {
                        eval_preview_expr_with_functions(param, scope, env, functions)
                            .map(|value| preview_value_to_json(&value))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(preview_json_to_value(runtime.query(
                    &backend::AxRawSqlRequest {
                        sql: sql.clone(),
                        params,
                    },
                )?));
            }
            AxQuerySourcePlan::ContentCollection { .. } => {}
        }
    }

    let collection = match &query.source {
        AxQuerySourcePlan::Stream { collection } => collection,
        AxQuerySourcePlan::ContentCollection { collection } => collection,
        AxQuerySourcePlan::RawSql { .. } => return Ok(AxValue::List(Vec::new())),
    };
    let mut items = store.collection_items(collection);

    for filter in &query.filters {
        let expected = eval_preview_expr_with_functions(&filter.value, scope, env, functions)?;
        items.retain(|item| preview_record_matches(item, &filter.field, filter.op, &expected));
    }

    for order in query.orders.iter().rev() {
        items.sort_by(|left, right| {
            let left_value = preview_record_field(left, &order.field)
                .map(AxValue::as_string)
                .unwrap_or_default();
            let right_value = preview_record_field(right, &order.field)
                .map(AxValue::as_string)
                .unwrap_or_default();

            match order.direction {
                AxQueryOrderDirectionPlan::Asc => left_value.cmp(&right_value),
                AxQueryOrderDirectionPlan::Desc => right_value.cmp(&left_value),
            }
        });
    }

    if let Some(offset) = query.offset {
        items = items.into_iter().skip(offset as usize).collect();
    }

    if let Some(limit) = query.limit {
        items.truncate(limit as usize);
    }

    Ok(apply_preview_query_mode(query.mode, items))
}

fn apply_preview_query_mode(mode: AxQueryModePlan, items: Vec<AxValue>) -> AxValue {
    match mode {
        AxQueryModePlan::Many => AxValue::List(items),
        AxQueryModePlan::First => items.into_iter().next().unwrap_or(AxValue::Null),
    }
}

fn preview_query_to_runtime_request(
    collection: &str,
    query: &AxQueryPlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<backend::AxQueryRequest, PreviewError> {
    Ok(backend::AxQueryRequest {
        collection: collection.to_string(),
        filters: query
            .filters
            .iter()
            .map(|filter| {
                Ok(backend::AxQueryFilterRequest {
                    field: filter.field.clone(),
                    op: preview_filter_op_to_runtime(filter.op),
                    value: preview_value_to_json(&eval_preview_expr_with_functions(
                        &filter.value,
                        scope,
                        env,
                        functions,
                    )?),
                })
            })
            .collect::<Result<Vec<_>, PreviewError>>()?,
        orders: query
            .orders
            .iter()
            .map(|order| backend::AxQueryOrderRequest {
                field: order.field.clone(),
                direction: match order.direction {
                    AxQueryOrderDirectionPlan::Asc => backend::AxQueryOrderDirection::Asc,
                    AxQueryOrderDirectionPlan::Desc => backend::AxQueryOrderDirection::Desc,
                },
            })
            .collect(),
        limit: query.limit,
        offset: query.offset,
        mode: match query.mode {
            AxQueryModePlan::Many => backend::AxQueryMode::Many,
            AxQueryModePlan::First => backend::AxQueryMode::First,
        },
    })
}

fn preview_filter_op_to_runtime(op: AxQueryFilterOpPlan) -> backend::AxQueryFilterOp {
    match op {
        AxQueryFilterOpPlan::Eq => backend::AxQueryFilterOp::Eq,
        AxQueryFilterOpPlan::Ne => backend::AxQueryFilterOp::Ne,
        AxQueryFilterOpPlan::In => backend::AxQueryFilterOp::In,
        AxQueryFilterOpPlan::NotIn => backend::AxQueryFilterOp::NotIn,
        AxQueryFilterOpPlan::IsNull => backend::AxQueryFilterOp::IsNull,
        AxQueryFilterOpPlan::IsNotNull => backend::AxQueryFilterOp::IsNotNull,
    }
}

fn preview_record_matches(
    item: &AxValue,
    field: &str,
    op: AxQueryFilterOpPlan,
    expected: &AxValue,
) -> bool {
    match op {
        AxQueryFilterOpPlan::IsNull => {
            matches!(
                preview_record_field(item, field),
                None | Some(AxValue::Null)
            )
        }
        AxQueryFilterOpPlan::IsNotNull => !matches!(
            preview_record_field(item, field),
            None | Some(AxValue::Null)
        ),
        _ => {
            let Some(value) = preview_record_field(item, field) else {
                return false;
            };
            match op {
                AxQueryFilterOpPlan::Eq => value == expected,
                AxQueryFilterOpPlan::Ne => value != expected,
                AxQueryFilterOpPlan::In => match expected {
                    AxValue::List(items) => items.iter().any(|item| item == value),
                    _ => false,
                },
                AxQueryFilterOpPlan::NotIn => match expected {
                    AxValue::List(items) => !items.iter().any(|item| item == value),
                    _ => false,
                },
                AxQueryFilterOpPlan::IsNull | AxQueryFilterOpPlan::IsNotNull => unreachable!(),
            }
        }
    }
}

fn preview_record_field<'a>(item: &'a AxValue, field: &str) -> Option<&'a AxValue> {
    match item {
        AxValue::Record(fields) => fields.get(field),
        _ => None,
    }
}

fn eval_preview_expr(
    expr: &AxRustExpr,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<AxValue, PreviewError> {
    let functions = BTreeMap::new();
    eval_preview_expr_with_functions(expr, scope, env, &functions)
}

fn eval_preview_expr_with_functions(
    expr: &AxRustExpr,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<AxValue, PreviewError> {
    let code = expr.code.trim();

    if let Some(value) = parse_preview_string(code) {
        return Ok(AxValue::String(value));
    }

    if code == "true" {
        return Ok(AxValue::Bool(true));
    }

    if code == "false" {
        return Ok(AxValue::Bool(false));
    }

    if let Ok(value) = code.parse::<i64>() {
        return Ok(AxValue::Number(value));
    }
    if let Some(value) = code.strip_suffix("_f64") {
        if let Ok(value) = value.parse::<f64>() {
            if let Some(value) = AxFloat::new(value) {
                return Ok(AxValue::Float(value));
            }
        }
    }

    if let Some(key) = parse_preview_env_call(code, "public") {
        return Ok(AxValue::String(env.public(&key)?));
    }

    if let Some(key) = parse_preview_env_call(code, "secret") {
        return Ok(AxValue::String(env.secret(&key)?));
    }

    if let Some(key) = parse_preview_env_call(code, "value") {
        return Ok(AxValue::String(env.value(&key)?));
    }

    if let Some(args) = parse_preview_call_args(code, "list") {
        let items = args
            .iter()
            .map(|arg| {
                eval_preview_expr_with_functions(&AxRustExpr::new(arg), scope, env, functions)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(AxValue::List(items));
    }

    if let Some(args) = parse_preview_vec_args(code) {
        let items = args
            .iter()
            .map(|arg| {
                eval_preview_expr_with_functions(&AxRustExpr::new(arg), scope, env, functions)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(AxValue::List(items));
    }

    if let Some(args) = parse_preview_call_args(code, "contains") {
        if args.len() != 2 {
            return Err(PreviewError::Runtime {
                message: "contains(list, value) expects exactly two arguments".to_string(),
            });
        }
        let options =
            eval_preview_expr_with_functions(&AxRustExpr::new(&args[0]), scope, env, functions)?;
        let needle =
            eval_preview_expr_with_functions(&AxRustExpr::new(&args[1]), scope, env, functions)?;
        let AxValue::List(items) = options else {
            return Ok(AxValue::Bool(false));
        };
        return Ok(AxValue::Bool(items.iter().any(|item| item == &needle)));
    }

    if let Some(args) = parse_preview_call_args(code, "error") {
        if args.len() != 1 {
            return Err(PreviewError::Runtime {
                message: "error(message) expects exactly one argument".to_string(),
            });
        }
        let message =
            eval_preview_expr_with_functions(&AxRustExpr::new(&args[0]), scope, env, functions)?;
        return Ok(AxValue::record([("error", message)]));
    }

    if let Some(value) = lookup_preview_scope(scope, code) {
        return Ok(value);
    }

    if let Some((name, args)) = parse_preview_named_call(code) {
        if let Some(function) = functions.get(&name) {
            let args = args
                .iter()
                .map(|arg| {
                    eval_preview_expr_with_functions(&AxRustExpr::new(arg), scope, env, functions)
                })
                .collect::<Result<Vec<_>, _>>()?;
            return execute_preview_function(function, &args, scope, env, functions);
        }
    }

    Err(PreviewError::Runtime {
        message: format!("preview loader expression `{code}` is not supported yet"),
    })
}

fn eval_preview_revalidation_target_with_functions(
    expr: &AxRustExpr,
    literal: bool,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<String, PreviewError> {
    let code = expr.code.trim();
    if literal && is_preview_identifier(code) {
        return Ok(code.to_string());
    }

    Ok(eval_preview_expr_with_functions(expr, scope, env, functions)?.as_string())
}

fn push_preview_auto_invalidation(
    invalidations: &mut Vec<AxPreviewInvalidation>,
    target: impl Into<String>,
) {
    let invalidation = AxPreviewInvalidation::new(target);
    if invalidations
        .iter()
        .any(|existing| existing.query_key == invalidation.query_key)
    {
        return;
    }
    invalidations.push(invalidation);
}

fn push_preview_explicit_invalidation(
    invalidations: &mut Vec<AxPreviewInvalidation>,
    target: impl Into<String>,
) {
    let invalidation = AxPreviewInvalidation::new(target);
    if let Some(existing) = invalidations
        .iter_mut()
        .find(|existing| existing.query_key == invalidation.query_key)
    {
        *existing = invalidation;
        return;
    }
    invalidations.push(invalidation);
}

fn is_preview_identifier(code: &str) -> bool {
    let mut chars = code.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn lookup_preview_scope(scope: &BTreeMap<String, AxValue>, code: &str) -> Option<AxValue> {
    let mut parts = code.split('.').map(str::trim);
    let first = parts.next()?;
    let mut value = scope.get(first)?.clone();

    for part in parts {
        let AxValue::Record(fields) = value else {
            return None;
        };
        value = fields.get(part)?.clone();
    }

    Some(value)
}

fn parse_preview_string(code: &str) -> Option<String> {
    if let Some(value) = code.strip_suffix(".to_string()") {
        return parse_preview_string(value.trim());
    }

    if (code.starts_with('"') && code.ends_with('"'))
        || (code.starts_with('\'') && code.ends_with('\''))
    {
        return Some(code[1..code.len() - 1].to_string());
    }

    None
}

fn parse_preview_env_call(code: &str, namespace: &str) -> Option<String> {
    let prefix = format!("runtime.env().{namespace}(\"");
    let suffix = "\")?";
    let key = code.strip_prefix(&prefix)?.strip_suffix(suffix)?;
    Some(key.to_string())
}

fn sample_preview_collection_items(collection: &str) -> Vec<AxValue> {
    match collection {
        "posts" => vec![
            AxValue::record([
                ("id", AxValue::from("1")),
                ("title", AxValue::from("Hello Axonyx")),
                (
                    "excerpt",
                    AxValue::from("A fast page rendered from .ax with almost no JavaScript."),
                ),
                ("slug", AxValue::from("hello-axonyx")),
                ("status", AxValue::from("published")),
                ("created_at", AxValue::from("2026-04-18")),
            ]),
            AxValue::record([
                ("id", AxValue::from("2")),
                ("title", AxValue::from("Docs Without Bloat")),
                (
                    "excerpt",
                    AxValue::from("Author docs pages directly and keep the runtime tiny."),
                ),
                ("slug", AxValue::from("docs-without-bloat")),
                ("status", AxValue::from("published")),
                ("created_at", AxValue::from("2026-04-17")),
            ]),
            AxValue::record([
                ("id", AxValue::from("3")),
                ("title", AxValue::from("Draft Preview")),
                (
                    "excerpt",
                    AxValue::from("A hidden draft entry to prove where filters work."),
                ),
                ("slug", AxValue::from("draft-preview")),
                ("status", AxValue::from("draft")),
                ("created_at", AxValue::from("2026-04-16")),
            ]),
        ],
        "users" => vec![
            AxValue::record([
                ("id", AxValue::from("1")),
                ("name", AxValue::from("Ana")),
                ("role", AxValue::from("editor")),
            ]),
            AxValue::record([
                ("id", AxValue::from("2")),
                ("name", AxValue::from("Luka")),
                ("role", AxValue::from("author")),
            ]),
        ],
        other => vec![
            AxValue::record([
                ("id", AxValue::from("1")),
                ("title", AxValue::from(format!("{other} item 1"))),
                (
                    "excerpt",
                    AxValue::from("Preview data is coming from Axonyx runtime samples."),
                ),
            ]),
            AxValue::record([
                ("id", AxValue::from("2")),
                ("title", AxValue::from(format!("{other} item 2"))),
                (
                    "excerpt",
                    AxValue::from("Connect a real adapter later without changing page syntax."),
                ),
            ]),
        ],
    }
}

fn eval_preview_fields(
    fields: &[axonyx_core::ax_backend_lowering_prelude::AxAssignmentPlan],
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<BTreeMap<String, AxValue>, PreviewError> {
    let functions = BTreeMap::new();
    eval_preview_fields_with_functions(fields, scope, env, &functions)
}

fn eval_preview_fields_with_functions(
    fields: &[axonyx_core::ax_backend_lowering_prelude::AxAssignmentPlan],
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<BTreeMap<String, AxValue>, PreviewError> {
    let mut map = BTreeMap::new();

    for field in fields {
        map.insert(
            field.name.clone(),
            eval_preview_expr_with_functions(&field.value, scope, env, functions)?,
        );
    }

    Ok(map)
}

fn eval_preview_filters(
    filters: &[axonyx_core::ax_backend_lowering_prelude::AxQueryFilterPlan],
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<Vec<PreviewFilter>, PreviewError> {
    let functions = BTreeMap::new();
    eval_preview_filters_with_functions(filters, scope, env, &functions)
}

fn eval_preview_filters_with_functions(
    filters: &[axonyx_core::ax_backend_lowering_prelude::AxQueryFilterPlan],
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    functions: &BTreeMap<String, AxFunctionPlan>,
) -> Result<Vec<PreviewFilter>, PreviewError> {
    filters
        .iter()
        .map(|filter| {
            Ok(PreviewFilter {
                field: filter.field.clone(),
                op: filter.op,
                value: eval_preview_expr_with_functions(&filter.value, scope, env, functions)?,
            })
        })
        .collect()
}

fn build_preview_input_record(
    fields: &[axonyx_core::ax_backend_lowering_prelude::AxFieldPlan],
    input_fields: &BTreeMap<String, String>,
) -> Result<AxValue, PreviewError> {
    let mut record = BTreeMap::new();
    for field in fields {
        let Some(value) = input_fields.get(&field.name).cloned() else {
            if let Some(default) = &field.default {
                record.insert(
                    field.name.clone(),
                    coerce_preview_default_input_value(&field.name, &field.rust_ty, default)?,
                );
                continue;
            }
            if field.optional {
                record.insert(field.name.clone(), AxValue::Null);
                continue;
            }
            if field.rust_ty == "bool" {
                record.insert(field.name.clone(), AxValue::Bool(false));
                continue;
            }
            return Err(PreviewError::Runtime {
                message: format!("missing required input `{}`", field.name),
            });
        };
        record.insert(
            field.name.clone(),
            coerce_preview_input_value(&field.name, &field.rust_ty, value)?,
        );
    }
    Ok(AxValue::Record(record))
}

fn build_preview_loader_input_record(
    fields: &[axonyx_core::ax_backend_lowering_prelude::AxFieldPlan],
    args: &[AxValue],
) -> Result<AxValue, PreviewError> {
    let mut record = BTreeMap::new();
    for (index, field) in fields.iter().enumerate() {
        let Some(value) = args.get(index).cloned() else {
            if let Some(default) = &field.default {
                record.insert(
                    field.name.clone(),
                    coerce_preview_default_input_value(&field.name, &field.rust_ty, default)?,
                );
                continue;
            }
            if field.optional {
                record.insert(field.name.clone(), AxValue::Null);
                continue;
            }
            if field.rust_ty == "bool" {
                record.insert(field.name.clone(), AxValue::Bool(false));
                continue;
            }
            return Err(PreviewError::Runtime {
                message: format!("missing required loader input `{}`", field.name),
            });
        };
        record.insert(
            field.name.clone(),
            coerce_preview_loader_input_value(&field.name, &field.rust_ty, value)?,
        );
    }
    Ok(AxValue::Record(record))
}

fn build_preview_route_input_record(
    fields: &[axonyx_core::ax_backend_lowering_prelude::AxFieldPlan],
    request: &server::AxHttpRequest,
) -> Result<AxValue, PreviewError> {
    let input_fields = fields
        .iter()
        .filter_map(|field| {
            request
                .form_value(&field.name)
                .or_else(|| request.json_field_string(&field.name))
                .map(|value| (field.name.clone(), value))
        })
        .collect::<BTreeMap<_, _>>();

    build_preview_input_record(fields, &input_fields)
}

fn coerce_preview_loader_input_value(
    field_name: &str,
    rust_ty: &str,
    value: AxValue,
) -> Result<AxValue, PreviewError> {
    match (rust_ty, value) {
        ("String", AxValue::String(value)) => Ok(AxValue::String(value)),
        ("String", value @ (AxValue::Number(_) | AxValue::Float(_) | AxValue::Bool(_))) => {
            Ok(AxValue::String(value.as_string()))
        }
        ("bool", AxValue::Bool(value)) => Ok(AxValue::Bool(value)),
        ("i64" | "u64", AxValue::Number(value)) => Ok(AxValue::Number(value)),
        ("f64", AxValue::Number(value)) => Ok(AxValue::Float(
            AxFloat::new(value as f64).expect("i64 is always a finite f64"),
        )),
        ("f64", AxValue::Float(value)) => Ok(AxValue::Float(value)),
        (_, AxValue::Null) => Ok(AxValue::Null),
        (_, value) => Err(PreviewError::Runtime {
            message: format!(
                "loader input `{field_name}` expected {rust_ty} but received {}",
                preview_value_type_name(&value)
            ),
        }),
    }
}

fn coerce_preview_function_input_value(
    field: &AxFieldPlan,
    value: AxValue,
) -> Result<AxValue, PreviewError> {
    match (field.rust_ty.as_str(), value) {
        ("String", AxValue::String(value)) => Ok(AxValue::String(value)),
        ("String", value @ (AxValue::Number(_) | AxValue::Float(_) | AxValue::Bool(_))) => {
            Ok(AxValue::String(value.as_string()))
        }
        ("bool", AxValue::Bool(value)) => Ok(AxValue::Bool(value)),
        ("i64" | "u64", AxValue::Number(value)) => Ok(AxValue::Number(value)),
        ("f64", AxValue::Number(value)) => Ok(AxValue::Float(
            AxFloat::new(value as f64).expect("i64 is always a finite f64"),
        )),
        ("f64", AxValue::Float(value)) => Ok(AxValue::Float(value)),
        (_, AxValue::Null) if field.optional => Ok(AxValue::Null),
        (_, value) => Err(PreviewError::Runtime {
            message: format!(
                "function input `{}` expected {} but received {}",
                field.name,
                field.rust_ty,
                preview_value_type_name(&value)
            ),
        }),
    }
}

fn coerce_preview_default_input_value(
    field_name: &str,
    rust_ty: &str,
    default: &AxRustExpr,
) -> Result<AxValue, PreviewError> {
    let empty_scope = BTreeMap::new();
    let env = backend::AxEnv::from_env();
    let value = eval_preview_expr(default, &empty_scope, &env)?;
    match (rust_ty, value) {
        ("bool", AxValue::Bool(value)) => Ok(AxValue::Bool(value)),
        ("i64" | "u64", AxValue::Number(value)) => Ok(AxValue::Number(value)),
        ("f64", AxValue::Number(value)) => Ok(AxValue::Float(
            AxFloat::new(value as f64).expect("i64 is always a finite f64"),
        )),
        ("f64", AxValue::Float(value)) => Ok(AxValue::Float(value)),
        ("String", AxValue::String(value)) => Ok(AxValue::String(value)),
        ("String", value) => Ok(AxValue::String(value.as_string())),
        (_, value) => Err(PreviewError::Runtime {
            message: format!(
                "default value for input `{field_name}` does not match expected {rust_ty}: got {}",
                preview_value_type_name(&value)
            ),
        }),
    }
}

fn coerce_preview_input_value(
    field_name: &str,
    rust_ty: &str,
    value: String,
) -> Result<AxValue, PreviewError> {
    match rust_ty {
        "bool" => Ok(AxValue::Bool(matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "on" | "yes"
        ))),
        "i64" => value
            .trim()
            .parse::<i64>()
            .map(AxValue::Number)
            .map_err(|_| PreviewError::Runtime {
                message: format!("input `{field_name}` expected i64 but received `{value}`"),
            }),
        "u64" => {
            let parsed = value
                .trim()
                .parse::<u64>()
                .map_err(|_| PreviewError::Runtime {
                    message: format!("input `{field_name}` expected u64 but received `{value}`"),
                })?;
            i64::try_from(parsed)
                .map(AxValue::Number)
                .map_err(|_| PreviewError::Runtime {
                    message: format!("input `{field_name}` exceeded Axonyx preview number range"),
                })
        }
        "f64" => value
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(AxFloat::new)
            .map(AxValue::Float)
            .ok_or_else(|| PreviewError::Runtime {
                message: format!("input `{field_name}` expected finite f64 but received `{value}`"),
            }),
        _ => Ok(AxValue::String(value)),
    }
}

fn preview_value_type_name(value: &AxValue) -> &'static str {
    match value {
        AxValue::Null => "Null",
        AxValue::String(_) => "String",
        AxValue::Number(_) => "Number",
        AxValue::Float(_) => "Float",
        AxValue::Bool(_) => "Bool",
        AxValue::Record(_) => "Record",
        AxValue::List(_) => "List",
    }
}

fn preview_record_matches_all(item: &AxValue, filters: &[PreviewFilter]) -> bool {
    filters
        .iter()
        .all(|filter| preview_record_matches(item, &filter.field, filter.op, &filter.value))
}

fn apply_preview_fields(item: &mut AxValue, fields: &BTreeMap<String, AxValue>) {
    let AxValue::Record(record) = item else {
        return;
    };

    for (name, value) in fields {
        record.insert(name.clone(), value.clone());
    }
}

fn assign_preview_id(record: &mut BTreeMap<String, AxValue>, existing_len: usize) {
    if record.contains_key("id") {
        return;
    }

    record.insert(
        "id".to_string(),
        AxValue::String((existing_len + 1).to_string()),
    );
}

fn build_preview_query_record(query: &BTreeMap<String, String>) -> AxValue {
    AxValue::Record(
        query
            .iter()
            .map(|(key, value)| (key.clone(), AxValue::String(value.clone())))
            .collect(),
    )
}

fn build_preview_request_record(request: &server::AxHttpRequest) -> AxValue {
    let headers = request
        .headers
        .iter()
        .map(|(key, value)| {
            (
                normalize_preview_header_key(key),
                AxValue::String(value.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let cookies = request
        .header_value("Cookie")
        .map(parse_preview_cookie_fields)
        .unwrap_or_default();
    let form = parse_preview_form_fields(&request.body_text_lossy());
    let json = serde_json::from_slice::<serde_json::Value>(&request.body)
        .ok()
        .map(preview_json_to_value)
        .unwrap_or(AxValue::Null);

    AxValue::Record(BTreeMap::from([
        (
            "method".to_string(),
            AxValue::String(request.method.clone()),
        ),
        (
            "target".to_string(),
            AxValue::String(request.target.clone()),
        ),
        (
            "body".to_string(),
            AxValue::String(request.body_text_lossy()),
        ),
        ("headers".to_string(), AxValue::Record(headers)),
        ("cookies".to_string(), AxValue::Record(cookies)),
        ("form".to_string(), AxValue::Record(form)),
        ("json".to_string(), json),
    ]))
}

fn build_preview_auth_record(request: &server::AxHttpRequest, env: &backend::AxEnv) -> AxValue {
    let signed_session = env
        .secret("session_key")
        .ok()
        .and_then(|secret| server::AxAuth::signed_session(request, &secret).map(str::to_string))
        .unwrap_or_default();

    AxValue::Record(BTreeMap::from([
        (
            "bearer".to_string(),
            AxValue::String(
                server::AxAuth::bearer(request)
                    .unwrap_or_default()
                    .to_string(),
            ),
        ),
        (
            "session".to_string(),
            AxValue::String(
                server::AxAuth::session(request)
                    .unwrap_or_default()
                    .to_string(),
            ),
        ),
        ("signedSession".to_string(), AxValue::String(signed_session)),
    ]))
}

fn parse_preview_cookie_fields(cookies: &str) -> BTreeMap<String, AxValue> {
    cookies
        .split(';')
        .filter_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            Some((
                key.trim().to_string(),
                AxValue::String(value.trim().to_string()),
            ))
        })
        .collect()
}

fn parse_preview_form_fields(body: &str) -> BTreeMap<String, AxValue> {
    body.split('&')
        .filter_map(|pair| {
            if pair.is_empty() {
                return None;
            }
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((url_decode(key), AxValue::String(url_decode(value))))
        })
        .collect()
}

fn normalize_preview_header_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn preview_json_to_value(value: serde_json::Value) -> AxValue {
    match value {
        serde_json::Value::Null => AxValue::Null,
        serde_json::Value::Bool(value) => AxValue::Bool(value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(AxValue::Number)
            .unwrap_or_else(|| AxValue::String(value.to_string())),
        serde_json::Value::String(value) => AxValue::String(value),
        serde_json::Value::Array(items) => {
            AxValue::List(items.into_iter().map(preview_json_to_value).collect())
        }
        serde_json::Value::Object(fields) => AxValue::Record(
            fields
                .into_iter()
                .map(|(key, value)| (key, preview_json_to_value(value)))
                .collect(),
        ),
    }
}

fn build_preview_route_scope(
    request_target: &str,
    route_params: &BTreeMap<String, String>,
    query: &BTreeMap<String, String>,
) -> BTreeMap<String, AxValue> {
    let params = AxValue::Record(
        route_params
            .iter()
            .map(|(key, value)| (key.clone(), AxValue::String(value.clone())))
            .collect(),
    );
    let path = request_target
        .split(['?', '#'])
        .next()
        .filter(|path| !path.is_empty())
        .unwrap_or("/");
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let section = segments.next().unwrap_or_default();
    let subsection = segments.next().unwrap_or_default();

    BTreeMap::from([
        ("params".to_string(), params.clone()),
        ("query".to_string(), build_preview_query_record(query)),
        (
            "route".to_string(),
            AxValue::Record(BTreeMap::from([
                ("path".to_string(), AxValue::String(path.to_string())),
                ("section".to_string(), AxValue::String(section.to_string())),
                (
                    "subsection".to_string(),
                    AxValue::String(subsection.to_string()),
                ),
                ("params".to_string(), params),
            ])),
        ),
    ])
}

fn match_preview_route<'a>(
    routes: &'a [AxHandlerPlan],
    method: &str,
    request_path: &str,
) -> Option<PreviewRouteMatch<'a>> {
    let method = normalize_preview_method(method);
    let mut best_match = None;
    let mut best_score = None;

    for route in routes {
        let AxHandlerKind::Route {
            method: route_method,
            path,
            ..
        } = &route.kind
        else {
            continue;
        };

        if normalize_preview_method(route_method) != method {
            continue;
        }

        let Some((params, static_segments)) = match_preview_route_pattern(path, request_path)
        else {
            continue;
        };
        let score = (static_segments, usize::MAX - path_segments(path).len());

        if best_score.is_some_and(|current| current >= score) {
            continue;
        }

        best_score = Some(score);
        best_match = Some(PreviewRouteMatch {
            handler: route,
            params,
        });
    }

    best_match
}

fn match_preview_route_pattern(
    pattern: &str,
    request_path: &str,
) -> Option<(BTreeMap<String, AxValue>, usize)> {
    let pattern_segments = path_segments(pattern);
    let request_segments = path_segments(request_path);
    if pattern_segments.len() != request_segments.len() {
        return None;
    }

    let mut params = BTreeMap::new();
    let mut static_segments = 0;

    for (pattern_segment, request_segment) in pattern_segments.iter().zip(request_segments.iter()) {
        if let Some(param_name) = pattern_segment.strip_prefix(':') {
            if param_name.is_empty() {
                return None;
            }

            params.insert(
                param_name.to_string(),
                AxValue::String(request_segment.clone()),
            );
            continue;
        }

        if pattern_segment != request_segment {
            return None;
        }

        static_segments += 1;
    }

    Some((params, static_segments))
}

fn normalize_preview_request_path(request_target: &str) -> Result<String, PreviewError> {
    let raw_path = request_target
        .split(['?', '#'])
        .next()
        .unwrap_or("/")
        .trim();
    let raw_path = if raw_path.is_empty() { "/" } else { raw_path };
    let mut segments = Vec::new();

    for segment in raw_path.trim_start_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        if segment == "." || segment == ".." || segment.contains('\\') {
            return Err(PreviewError::Runtime {
                message: format!("invalid route path `{request_target}`"),
            });
        }
        segments.push(segment.to_string());
    }

    if segments.is_empty() {
        Ok("/".to_string())
    } else {
        Ok(format!("/{}", segments.join("/")))
    }
}

fn parse_preview_query_fields(request_target: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let Some((_, query)) = request_target.split_once('?') else {
        return fields;
    };
    let query = query.split('#').next().unwrap_or_default();

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }

        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        fields.insert(url_decode(key), url_decode(value));
    }

    fields
}

fn path_segments(path: &str) -> Vec<String> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn render_preview_json_response(value: &AxValue) -> Result<AxPreviewHttpResponse, PreviewError> {
    let body = serde_json::to_vec(&preview_value_to_json(value)).map_err(|error| {
        PreviewError::Runtime {
            message: format!("failed to serialize preview JSON response: {error}"),
        }
    })?;

    Ok(AxPreviewHttpResponse {
        status: 200,
        content_type: "application/json; charset=utf-8".to_string(),
        headers: BTreeMap::new(),
        set_cookies: Vec::new(),
        body,
    })
}

fn preview_value_to_json(value: &AxValue) -> serde_json::Value {
    match value {
        AxValue::Null => serde_json::Value::Null,
        AxValue::String(value) => serde_json::Value::String(value.clone()),
        AxValue::Number(value) => serde_json::Value::Number((*value).into()),
        AxValue::Float(value) => serde_json::Number::from_f64(value.get())
            .map(serde_json::Value::Number)
            .expect("AxFloat always contains a finite value"),
        AxValue::Bool(value) => serde_json::Value::Bool(*value),
        AxValue::Record(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), preview_value_to_json(value)))
                .collect(),
        ),
        AxValue::List(items) => {
            serde_json::Value::Array(items.iter().map(preview_value_to_json).collect())
        }
    }
}

fn normalize_preview_method(method: &str) -> String {
    method.trim().to_ascii_uppercase()
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

fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }

    out
}

fn compose_layout_with_page(mut layout: AxDocument, page: AxDocument) -> AxDocument {
    let page_name = page.page.name;
    let page_head = page.head;
    let page_imports = page.imports;
    let page_body = page.page.body;

    if !inject_slot_statements(&mut layout.page.body, &page_body) {
        layout.page.body.extend(page_body);
    }

    layout.page.name = page_name;
    layout.imports.extend(page_imports);
    layout.head.merge(page_head);
    layout
}

fn inject_slot_statements(statements: &mut Vec<AxStatement>, page_body: &[AxStatement]) -> bool {
    let mut found_slot = false;
    let mut composed = Vec::with_capacity(statements.len() + page_body.len());

    for statement in statements.drain(..) {
        match statement {
            AxStatement::Component(component) if is_slot_component(&component) => {
                composed.extend(page_body.iter().cloned());
                found_slot = true;
            }
            AxStatement::Component(mut component) => {
                if let AxBody::Block(body) = &mut component.body {
                    found_slot |= inject_slot_statements(body, page_body);
                }
                composed.push(AxStatement::Component(component));
            }
            AxStatement::Each(mut each) => {
                found_slot |= inject_slot_statements(&mut each.body, page_body);
                found_slot |= inject_slot_statements(&mut each.empty_body, page_body);
                composed.push(AxStatement::Each(each));
            }
            AxStatement::If(mut if_block) => {
                found_slot |= inject_slot_statements(&mut if_block.body, page_body);
                found_slot |= inject_slot_statements(&mut if_block.else_body, page_body);
                composed.push(AxStatement::If(if_block));
            }
            AxStatement::Match(mut match_block) => {
                for case in &mut match_block.cases {
                    found_slot |= inject_slot_statements(&mut case.body, page_body);
                }
                if let Some(default_body) = &mut match_block.default_body {
                    found_slot |= inject_slot_statements(default_body, page_body);
                }
                composed.push(AxStatement::Match(match_block));
            }
            AxStatement::Pipeline(mut pipeline) => {
                found_slot |= inject_slot_pipeline(&mut pipeline, page_body);
                composed.push(AxStatement::Pipeline(pipeline));
            }
            other => composed.push(other),
        }
    }

    *statements = composed;
    found_slot
}

fn inject_slot_pipeline(pipeline: &mut AxPipeline, page_body: &[AxStatement]) -> bool {
    let mut found_slot = false;

    for stage in &mut pipeline.stages {
        if let AxPipelineStage::Component(component) = stage {
            if is_slot_component(component) {
                *component = AxComponent::new("Fragment").block(page_body.iter().cloned());
                found_slot = true;
                continue;
            }

            if let AxBody::Block(body) = &mut component.body {
                found_slot |= inject_slot_statements(body, page_body);
            }
        }
    }

    found_slot
}

fn is_slot_component(component: &AxComponent) -> bool {
    component.name == "Slot"
}

fn render_preview_document(document: &AxDocument, root: &AxNode) -> String {
    String::from_utf8(render_preview_document_chunks(document, root).concat())
        .expect("preview renderer only emits UTF-8 HTML")
}

fn render_preview_document_response(
    document: &AxDocument,
    root: &AxNode,
) -> server::AxHttpResponse {
    server::AxHttpResponse::html_stream(200, render_preview_document_chunks(document, root))
}

fn render_preview_document_chunks(document: &AxDocument, root: &AxNode) -> Vec<Vec<u8>> {
    let mut body = String::new();
    render_node(root, &mut body);
    let head = render_head_html(&document.head);
    let html_attrs = render_html_attrs(&document.head);
    let behavior_script = if body.contains("data-ax-behavior=") {
        ax_behavior_script()
    } else {
        ""
    };
    let state_bridge_script = if body.contains("data-ax-signal=")
        || body.contains("data-ax-state-name=")
        || body.contains("data-ax-state-if-signal=")
        || body.contains("data-ax-state-match-signal=")
        || body.contains("data-ax-expression-protocol=")
        || body.contains("data-ax-each-signal=")
    {
        ax_state_bridge_script()
    } else {
        ""
    };
    let action_script = if body.contains("/__axonyx/action?") {
        ax_action_script()
    } else {
        ""
    };

    [
        format!(
            "<!DOCTYPE html><html{}><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style>{}</style>",
            html_attrs,
            preview_styles()
        ),
        head,
        "</head><body>".to_string(),
        body,
        state_bridge_script.to_string(),
        action_script.to_string(),
        behavior_script.to_string(),
        "</body></html>".to_string(),
    ]
    .into_iter()
    .filter(|chunk| !chunk.is_empty())
    .map(String::into_bytes)
    .collect()
}

fn ax_behavior_script() -> &'static str {
    r##"<script data-ax-runtime="behavior">
(() => {
  if (window.__axonyxBehaviorRuntime) return;
  window.__axonyxBehaviorRuntime = true;

  const setExpanded = (trigger, expanded) => {
    trigger.setAttribute("aria-expanded", expanded ? "true" : "false");
  };

  const targetFor = (trigger) => {
    const selector = trigger.getAttribute("data-ax-behavior-target");
    if (!selector) return null;
    return document.querySelector(selector);
  };

  const initToggle = (trigger) => {
    const selector = trigger.getAttribute("data-ax-behavior-target");
    if (selector && selector.startsWith("#") && !trigger.hasAttribute("aria-controls")) {
      trigger.setAttribute("aria-controls", selector.slice(1));
    }
    const target = targetFor(trigger);
    if (target) setExpanded(trigger, !target.hidden);
  };

  const allowedThemes = new Set(["silver", "bronze", "gold"]);

  const applyTheme = (theme) => {
    const next = allowedThemes.has(theme) ? theme : "silver";
    document.documentElement.setAttribute("data-theme", next);
    return next;
  };

  const initTheme = (control) => {
    const storageKey = control.getAttribute("data-ax-theme-storage-key") || "axonyx-theme";
    const stored = window.localStorage ? window.localStorage.getItem(storageKey) : null;
    const current = document.documentElement.getAttribute("data-theme");
    const initial = applyTheme(stored || current || control.value || "silver");
    control.value = initial;
  };

  const init = () => {
    document.querySelectorAll('[data-ax-behavior="toggle"]').forEach(initToggle);
    document.querySelectorAll('[data-ax-behavior="theme"]').forEach(initTheme);
  };

  const toggleTarget = (trigger) => {
    const target = targetFor(trigger);
    if (!target) return;
    const nextHidden = !target.hidden;
    target.hidden = nextHidden;
    setExpanded(trigger, !nextHidden);
  };

  document.addEventListener("click", (event) => {
    const trigger = event.target.closest("[data-ax-behavior]");
    if (!trigger) return;
    const behavior = trigger.getAttribute("data-ax-behavior");
    if (behavior === "toggle") {
      toggleTarget(trigger);
    }
  });

  document.addEventListener("change", (event) => {
    const control = event.target.closest('[data-ax-behavior="theme"]');
    if (!control) return;
    const storageKey = control.getAttribute("data-ax-theme-storage-key") || "axonyx-theme";
    const next = applyTheme(control.value);
    if (window.localStorage) window.localStorage.setItem(storageKey, next);
  });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();
</script>"##
}

fn ax_action_script() -> &'static str {
    r##"<script data-ax-runtime="actions">
(() => {
  if (window.__axonyxActionRuntime) return;
  window.__axonyxActionRuntime = true;

  const isAxonyxActionForm = (form) => {
    if (!form || !form.action) return false;
    try {
      return new URL(form.action, window.location.href).pathname === "/__axonyx/action";
    } catch (_error) {
      return form.getAttribute("action")?.startsWith("/__axonyx/action");
    }
  };

  const actionStatuses = (form) => Array.from(form.querySelectorAll(".ax-action-status[data-state]"));

  const syncActionStatus = (form) => {
    const current = form.getAttribute("data-ax-action-state") || "";
    actionStatuses(form).forEach((status) => {
      const active = current && status.getAttribute("data-state") === current;
      status.hidden = !active;
      status.setAttribute("aria-hidden", active ? "false" : "true");
      if (!status.hasAttribute("aria-live")) status.setAttribute("aria-live", "polite");
    });
  };

  const setActionState = (form, state) => {
    form.setAttribute("data-ax-action-state", state);
    syncActionStatus(form);
  };

  const initActionForms = () => {
    document.querySelectorAll("form").forEach((form) => {
      if (isAxonyxActionForm(form)) syncActionStatus(form);
    });
  };

  const actionRoutePath = (payload) => {
    const redirect = typeof payload?.redirect === "string" && payload.redirect ? payload.redirect : window.location.pathname;
    try {
      return new URL(redirect, window.location.href).pathname;
    } catch (_error) {
      return window.location.pathname;
    }
  };

  const refreshDataBindings = async (payload, form, refreshes) => {
    if (!refreshes.length || typeof fetch !== "function") return false;
    const routePath = actionRoutePath(payload);
    const results = await Promise.all(refreshes.map(async (refresh) => {
      if (!refresh || typeof refresh.name !== "string" || !refresh.name) return false;
      const url = new URL("/__axonyx/data", window.location.href);
      url.searchParams.set("path", routePath);
      url.searchParams.set("name", refresh.name);
      try {
        const response = await fetch(url, {
          headers: {
            Accept: "application/ax-data+json",
            "X-Axonyx-State-Protocol": "ax-state/1",
            "X-Axonyx-Tab": getTabId(),
          },
          cache: "no-store",
        });
        const contentType = response.headers.get("content-type") || "";
        const data = contentType.includes("application/ax-data+json")
          ? await response.json()
          : { ok: false, status: response.status };
        const applied = applyDataRefresh(data);
        window.dispatchEvent(new CustomEvent("axonyx:data-refresh", {
          detail: { form, payload, refresh, data },
        }));
        return applied;
      } catch (error) {
        window.dispatchEvent(new CustomEvent("axonyx:data-refresh-error", {
          detail: { form, payload, refresh, error },
        }));
        return false;
      }
    }));
    return results.some(Boolean);
  };

  const applyDataRefresh = (data) => {
    if (!data?.ok || typeof data.html !== "string" || !data.html.trim()) return false;
    const template = document.createElement("template");
    template.innerHTML = data.html.trim();
    const nextRoot = template.content.querySelector('[data-ax-root="page"]');
    const currentRoot = document.querySelector('[data-ax-root="page"]');
    if (!nextRoot || !currentRoot) return false;
    currentRoot.replaceWith(nextRoot);
    initActionForms();
    window.dispatchEvent(new CustomEvent("axonyx:dom-refresh", {
      detail: { data },
    }));
    return true;
  };

  const applyPatchResponse = async (payload, form) => {
    const patches = Array.isArray(payload?.patches) ? payload.patches : [];
    const invalidations = Array.isArray(payload?.invalidations) ? payload.invalidations : [];
    const refreshes = Array.isArray(payload?.refreshes) ? payload.refreshes : [];
    const applyPatch = window.__axonyx?.state?.applyPatch;
    const canApplyPatches = typeof applyPatch === "function";
    if (canApplyPatches) patches.forEach((patch) => applyPatch(patch));
    if (invalidations.length || refreshes.length) {
      window.dispatchEvent(new CustomEvent("axonyx:query-refresh", {
        detail: { form, payload, invalidations, refreshes },
      }));
      invalidations.forEach((invalidation) => {
        window.dispatchEvent(new CustomEvent("axonyx:query-invalidate", {
          detail: { form, payload, invalidation, refreshes },
        }));
      });
    }
    const refreshed = await refreshDataBindings(payload, form, refreshes);
    window.dispatchEvent(new CustomEvent("axonyx:action-complete", {
      detail: { form, payload, patches, invalidations, refreshes },
    }));
    if (!refreshed && (patches.length === 0 || !canApplyPatches) && payload?.redirect) {
      window.location.assign(payload.redirect);
    }
  };

  const getTabId = () => {
    const existing = window.__axonyx?.state?.tabId;
    if (existing) return existing;
    try {
      const key = "axonyx:tab-id";
      let value = window.sessionStorage && window.sessionStorage.getItem(key);
      if (!value) {
        value = "tab-" + Math.random().toString(36).slice(2) + Date.now().toString(36);
        if (window.sessionStorage) window.sessionStorage.setItem(key, value);
      }
      return value;
    } catch (_) {
      return "tab-" + Math.random().toString(36).slice(2);
    }
  };

  document.addEventListener("submit", async (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !isAxonyxActionForm(form)) return;
    event.preventDefault();

    const formData = new FormData(form);
    if (!formData.has("__ax_patch")) formData.append("__ax_patch", "1");
    if (!formData.has("__ax_protocol")) formData.append("__ax_protocol", "ax-state/1");
    if (!formData.has("__ax_tab")) formData.append("__ax_tab", getTabId());
    const hasFile = Array.from(formData.values()).some((value) => value instanceof File);
    const body = hasFile ? formData : new URLSearchParams(formData);
    const contentHeaders = hasFile ? {} : {
      "Content-Type": "application/x-www-form-urlencoded;charset=UTF-8",
    };
    setActionState(form, "pending");

    try {
      const response = await fetch(form.action, {
        method: form.method || "POST",
        headers: {
          Accept: "application/ax-patch+json",
          "X-Axonyx-State-Protocol": "ax-state/1",
          "X-Axonyx-Tab": getTabId(),
          ...contentHeaders,
        },
        body,
        cache: "no-store",
      });
      const contentType = response.headers.get("content-type") || "";
      if (contentType.includes("application/ax-patch+json")) {
        await applyPatchResponse(await response.json(), form);
        setActionState(form, "complete");
        return;
      }
      if (contentType.includes("application/ax-error+json")) {
        const payload = await response.json();
        setActionState(form, "error");
        window.dispatchEvent(new CustomEvent("axonyx:action-error", {
          detail: { form, payload, error: payload?.error },
        }));
        return;
      }
      if (response.redirected) {
        window.location.assign(response.url);
        return;
      }
      window.location.reload();
    } catch (error) {
      setActionState(form, "error");
      window.dispatchEvent(new CustomEvent("axonyx:action-error", {
        detail: { form, error },
      }));
    }
  });

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", initActionForms, { once: true });
  } else {
    initActionForms();
  }
})();
</script>"##
}

fn parse_preview_call_args(code: &str, name: &str) -> Option<Vec<String>> {
    let prefix = format!("{name}(");
    let inner = code.strip_prefix(&prefix)?.strip_suffix(')')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }

    Some(
        split_preview_args(inner)
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
}

fn parse_preview_vec_args(code: &str) -> Option<Vec<String>> {
    let inner = code.strip_prefix("vec![")?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }

    Some(
        split_preview_args(inner)
            .into_iter()
            .map(str::to_string)
            .collect(),
    )
}

fn parse_preview_named_call(code: &str) -> Option<(String, Vec<String>)> {
    let open = code.find('(')?;
    let name = code[..open].trim();
    if !is_preview_identifier(name) || !code.ends_with(')') {
        return None;
    }
    let inner = &code[open + 1..code.len() - 1];
    let args = if inner.trim().is_empty() {
        Vec::new()
    } else {
        split_preview_args(inner)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    Some((name.to_string(), args))
}

fn split_preview_args(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut in_string: Option<char> = None;

    for (index, ch) in input.char_indices() {
        match in_string {
            Some(quote) => {
                if ch == quote {
                    in_string = None;
                }
            }
            None => match ch {
                '"' | '\'' => in_string = Some(ch),
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    result.push(input[start..index].trim());
                    start = index + 1;
                }
                _ => {}
            },
        }
    }

    result.push(input[start..].trim());
    result
}

fn ax_state_bridge_script() -> &'static str {
    r##"<script data-ax-runtime="state-bridge">
(() => {
  if (window.__axonyxStateBridge) return;

  const state = new Map();
  const bindings = new Map();
  const types = new Map();
  const metadata = new Map();
  const metadataByName = new Map();
  const typeSchemas = new Map();
  const aliases = new Map();
  const readBindings = new Map();
  const subscribers = new Map();
  const conditions = new Map();
  const matches = new Map();
  const expressions = new Map();
  const expressionEntries = [];
  const eachBindings = new Map();
  const domCapabilities = new WeakMap();
  const validDomCapabilities = new WeakSet();
  const domCapabilityList = [];
  const storageCapabilities = new Map();
  const storageCapabilityKeys = new Map();
  const validStorageCapabilities = new WeakSet();
  const storageCapabilityList = [];
  const hydratedStorageSignals = new Set();
  const pendingStorageWrites = new Map();
  let storageManifestReady = false;
  let wasmExecutor;
  let wasmTextEncoder = typeof TextEncoder === "function" ? new TextEncoder() : undefined;
  let wasmTextDecoder = typeof TextDecoder === "function"
    ? new TextDecoder("utf-8", { fatal: true })
    : undefined;
  let executorMode = "js-fallback";

  const stateEventProtocol = "ax-state-event/1";
  const domCapabilityProtocol = "ax-dom-capability/1";
  const storageCapabilityProtocol = "ax-storage-capability/1";
  const storageValueProtocol = "ax-storage-value/1";
  const domWriteTargets = new Set(["value", "checked", "text"]);
  const expressionBooleanTargets = new Set([
    "disabled", "checked", "selected", "hidden", "required", "readonly", "multiple", "open",
  ]);
  const storageScopes = new Set(["local", "session"]);
  const maxStorageKeyBytes = 128;
  const maxStateEventSignalLength = 512;
  const maxStateEventStringBytes = 4096;
  const executorStats = {
    wasmOperations: 0,
    fallbackOperations: 0,
    rejectedEvents: 0,
    dedupedEvents: 0,
    registeredDomCapabilities: 0,
    appliedDomWrites: 0,
    rejectedDomCapabilities: 0,
    rejectedDomWrites: 0,
    registeredStorageCapabilities: 0,
    restoredStorageValues: 0,
    persistedStorageValues: 0,
    rejectedStorageCapabilities: 0,
    rejectedStorageReads: 0,
    rejectedStorageWrites: 0,
    queuedStorageWrites: 0,
    flushedStorageWrites: 0,
    expressionEvaluations: 0,
    rejectedExpressions: 0,
    reconciledEachLists: 0,
    rejectedEachLists: 0,
    eachRefreshesRequired: 0,
  };
  const localOperationCode = Object.freeze({ set: 0, add: 1, sub: 2, toggle: 3 });
  const stringLikeTypes = new Set(["String", "DateTime", "Date", "Time", "Uuid"]);
  const numericTypes = new Set(["Number", "Int", "Float"]);
  const rejectedClientTypes = new Set(["Never", "Void"]);
  const maxStateValueDepth = 32;
  const maxStateValueBytes = 64 * 1024;
  const valueFrameVersion = 1;

  const validDateValue = (value) => {
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!match) return false;
    const year = Number(match[1]);
    const month = Number(match[2]);
    const day = Number(match[3]);
    if (year === 0 || month < 1 || month > 12) return false;
    const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
    const maxDay = month === 2 ? (leap ? 29 : 28) : [4, 6, 9, 11].includes(month) ? 30 : 31;
    return day >= 1 && day <= maxDay;
  };

  const validTimeValue = (value) => {
    const match = /^(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?$/.exec(value);
    return !!match
      && Number(match[1]) <= 23
      && Number(match[2]) <= 59
      && Number(match[3]) <= 59;
  };

  const validDateTimeValue = (value) => {
    const match = /^(\d{4}-\d{2}-\d{2})T(.+)(Z|[+-]\d{2}:\d{2})$/.exec(value);
    if (!match || !validDateValue(match[1]) || !validTimeValue(match[2])) return false;
    if (match[3] === "Z") return true;
    return Number(match[3].slice(1, 3)) <= 23 && Number(match[3].slice(4, 6)) <= 59;
  };

  const validStringLikeValue = (value, type) => {
    if (typeof value !== "string") return false;
    if (type === "String") return true;
    if (type === "Date") return validDateValue(value);
    if (type === "Time") return validTimeValue(value);
    if (type === "DateTime") return validDateTimeValue(value);
    if (type === "Uuid") {
      return /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(value);
    }
    return false;
  };

  const unwrapPublicType = (type) => {
    let current = String(type || "Unknown").replace(/\s+/g, "");
    while (current.startsWith("Public<") && current.endsWith(">")) {
      current = current.slice(7, -1);
    }
    return current;
  };

  const valueTypeCode = (type) => {
    type = unwrapPublicType(type);
    if (stringLikeTypes.has(type)) return 0;
    if (numericTypes.has(type)) return 1;
    if (type === "Bool") return 2;
    if (rejectedClientTypes.has(type)
      || type.startsWith("Secret<")
      || type.startsWith("Signal<")
      || type.startsWith("Resource<")) return undefined;
    return 3;
  };

  const operationsForType = (type) => {
    const code = valueTypeCode(type);
    if (code === 0 || code === 3) return new Set(["set"]);
    if (code === 1) return new Set(["set", "add", "sub"]);
    if (code === 2) return new Set(["set", "toggle"]);
    return undefined;
  };

  const genericType = (type, name) => {
    type = unwrapPublicType(type);
    const prefix = `${name}<`;
    if (!type.startsWith(prefix) || !type.endsWith(">")) return undefined;
    const source = type.slice(prefix.length, -1);
    const args = [];
    let depth = 0;
    let start = 0;
    for (let index = 0; index < source.length; index += 1) {
      if (source[index] === "<") depth += 1;
      else if (source[index] === ">") depth -= 1;
      else if (source[index] === "," && depth === 0) {
        args.push(source.slice(start, index));
        start = index + 1;
      }
      if (depth < 0) return undefined;
    }
    if (depth !== 0) return undefined;
    args.push(source.slice(start));
    return args.filter(Boolean);
  };

  const validateStateValueForType = (value, type, depth = 0) => {
    if (depth > maxStateValueDepth) return false;
    type = unwrapPublicType(type);
    if (type === "Unknown" || type === "Json") return !!encodeStateValue(value, type);
    if (stringLikeTypes.has(type)) return validStringLikeValue(value, type);
    if (type === "Number" || type === "Float") return typeof value === "number" && Number.isFinite(value);
    if (type === "Int") return typeof value === "number" && Number.isSafeInteger(value);
    if (type === "Bool") return typeof value === "boolean";
    if (type === "Bytes") {
      return (Array.isArray(value) || value instanceof Uint8Array)
        && Array.from(value).every((item) => Number.isInteger(item) && item >= 0 && item <= 255);
    }
    const optional = genericType(type, "Optional");
    if (optional?.length === 1) {
      return value === null || validateStateValueForType(value, optional[0], depth + 1);
    }
    const list = genericType(type, "List");
    if (list?.length === 1) {
      return Array.isArray(value)
        && value.every((item) => validateStateValueForType(item, list[0], depth + 1));
    }
    const set = genericType(type, "Set");
    if (set?.length === 1 && Array.isArray(value)) {
      const encoded = value.map((item) => encodeStateValue(item, set[0]));
      return encoded.every(Boolean)
        && encoded.every((item, index) => encoded.findIndex((candidate) => (
          candidate.length === item.length
            && candidate.every((byte, byteIndex) => byte === item[byteIndex])
        )) === index)
        && value.every((item) => validateStateValueForType(item, set[0], depth + 1));
    }
    if (type.endsWith("[]")) {
      return Array.isArray(value)
        && value.every((item) => validateStateValueForType(item, type.slice(0, -2), depth + 1));
    }
    const map = genericType(type, "Map");
    if (map?.length === 2) {
      return !!value
        && typeof value === "object"
        && !Array.isArray(value)
        && Object.entries(value).every(([key, item]) => {
          const keyValid = stringLikeTypes.has(map[0]) && validStringLikeValue(key, map[0])
            || map[0] === "Int" && /^-?\d+$/.test(key)
            || map[0] === "Bool" && /^(true|false)$/.test(key);
          return keyValid && validateStateValueForType(item, map[1], depth + 1);
        });
    }
    const result = genericType(type, "Result");
    if (result?.length === 2 && value && typeof value === "object" && !Array.isArray(value)) {
      const keys = Object.keys(value);
      return keys.length === 1
        && (keys[0] === "Ok"
          ? validateStateValueForType(value.Ok, result[0], depth + 1)
          : keys[0] === "Err" && validateStateValueForType(value.Err, result[1], depth + 1));
    }
    const schema = typeSchemas.get(type);
    if (!schema) return false;
    if (Array.isArray(schema.literals) && schema.literals.length > 0) {
      return typeof value === "string" && schema.literals.includes(value);
    }
    if (!value || typeof value !== "object" || Array.isArray(value)) return false;
    const fields = new Map((schema.fields || []).map((field) => [field.name, field]));
    if (Object.keys(value).some((key) => !fields.has(key))) return false;
    return (schema.fields || []).every((field) => {
      if (!Object.hasOwn(value, field.name)) return !!field.optional;
      const fieldType = field.optional ? `Optional<${field.ty}>` : field.ty;
      return validateStateValueForType(value[field.name], fieldType, depth + 1);
    });
  };

  const loadWasmExecutor = async (url = "/_ax/runtime/axonyx-state-v2.wasm") => {
    if (!window.WebAssembly || !window.fetch) return false;
    try {
      const response = await fetch(url, { cache: "force-cache" });
      if (!response.ok) return false;
      const module = await WebAssembly.instantiate(await response.arrayBuffer(), {});
      const exports = module.instance?.exports;
      if (!exports || exports.ax_state_abi_version?.() !== 3) return false;
      if (typeof exports.ax_state_supports_operation !== "function") return false;
      if (typeof exports.ax_state_apply_number !== "function") return false;
      if (typeof exports.ax_state_apply_bool !== "function") return false;
      if (typeof exports.ax_state_apply_string !== "function") return false;
      if (typeof exports.ax_state_string_buffer_ptr !== "function") return false;
      if (typeof exports.ax_state_string_buffer_capacity !== "function") return false;
      if (typeof exports.ax_state_apply_value !== "function") return false;
      if (typeof exports.ax_state_evaluate_expression !== "function") return false;
      if (typeof exports.ax_state_reconcile_keys !== "function") return false;
      if (typeof exports.ax_state_render_each !== "function") return false;
      if (typeof exports.ax_state_value_buffer_ptr !== "function") return false;
      if (typeof exports.ax_state_value_buffer_capacity !== "function") return false;
      if (!(exports.memory instanceof WebAssembly.Memory)) return false;
      wasmExecutor = exports;
      wasmTextEncoder = new TextEncoder();
      wasmTextDecoder = new TextDecoder("utf-8", { fatal: true });
      executorMode = "wasm";
      window.dispatchEvent(new CustomEvent("axonyx:state-runtime", {
        detail: { protocol: "ax-state/1", executor: executorMode, url },
      }));
      expressionEntries.forEach(updateExpression);
      return true;
    } catch (_) {
      return false;
    }
  };

  const readValue = (node, target) => {
    if (target === "checked") return !!node.checked;
    if (target === "text") return node.textContent || "";
    return node.value ?? node.getAttribute("value") ?? "";
  };

  const castValue = (value, type) => {
    type = unwrapPublicType(type);
    if (type === "Bool") {
      return value === true || value === "true" || value === "on";
    }
    if (numericTypes.has(type)) {
      const next = Number(value);
      if (!Number.isFinite(next)) return value;
      return type === "Int" && !Number.isSafeInteger(next) ? value : next;
    }
    if (stringLikeTypes.has(type)) return value == null ? "" : String(value);
    if (typeof value === "string") {
      try { return JSON.parse(value); } catch (_) { return value; }
    }
    return value;
  };

  const encodeStateValue = (value, type = "Unknown") => {
    if (!wasmTextEncoder) return undefined;
    const seen = new WeakSet();
    const frame = (tag, payload = []) => Uint8Array.from([65, 88, valueFrameVersion, tag, ...payload]);
    const u32 = (value) => {
      const bytes = new Uint8Array(4);
      new DataView(bytes.buffer).setUint32(0, value, true);
      return Array.from(bytes);
    };
    const f64 = (value) => {
      const bytes = new Uint8Array(8);
      new DataView(bytes.buffer).setFloat64(0, value, true);
      return Array.from(bytes);
    };
    const i64 = (value) => {
      const bytes = new Uint8Array(8);
      new DataView(bytes.buffer).setBigInt64(0, BigInt(value), true);
      return Array.from(bytes);
    };
    const visit = (current, depth, typeHint) => {
      if (depth > maxStateValueDepth) throw new Error("state-value-too-deep");
      if (current === null) return frame(0);
      if (current === undefined) throw new Error("undefined-state-value");
      if (typeof current === "string") {
        const bytes = wasmTextEncoder.encode(current);
        return frame(1, [...u32(bytes.length), ...bytes]);
      }
      if (typeof current === "boolean") return frame(2, [current ? 1 : 0]);
      if (typeof current === "number") {
        if (!Number.isFinite(current)) throw new Error("invalid-state-number");
        return Number.isSafeInteger(current)
          ? frame(4, i64(current))
          : frame(3, f64(current));
      }
      if (current instanceof Uint8Array || unwrapPublicType(typeHint) === "Bytes") {
        const bytes = current instanceof Uint8Array ? current : Uint8Array.from(current);
        return frame(5, [...u32(bytes.length), ...bytes]);
      }
      if (current instanceof Set) current = Array.from(current.values());
      if (current instanceof Map) current = Object.fromEntries(current.entries());
      if (Array.isArray(current)) {
        if (seen.has(current)) throw new Error("cyclic-state-value");
        seen.add(current);
        const values = current.map((item) => visit(item, depth + 1, "Unknown"));
        seen.delete(current);
        return frame(6, [...u32(values.length), ...values.flatMap((value) => Array.from(value))]);
      }
      if (typeof current === "object") {
        if (seen.has(current)) throw new Error("cyclic-state-value");
        seen.add(current);
        const entries = Object.keys(current).sort().map((key) => {
          const keyBytes = wasmTextEncoder.encode(key);
          const encoded = visit(current[key], depth + 1, "Unknown");
          return [...u32(keyBytes.length), ...keyBytes, ...encoded];
        });
        seen.delete(current);
        return frame(7, [...u32(entries.length), ...entries.flat()]);
      }
      throw new Error("unsupported-state-value");
    };

    try {
      const bytes = visit(value, 0, type);
      return bytes.length <= maxStateValueBytes ? bytes : undefined;
    } catch (_) {
      return undefined;
    }
  };

  const decodeStateValue = (bytes) => {
    if (!wasmTextDecoder) return undefined;
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const read = (offset, length) => {
      if (offset < 0 || length < 0 || offset + length > bytes.length) throw new Error("truncated-state-value");
      return bytes.subarray(offset, offset + length);
    };
    const visit = (start, depth) => {
      if (depth > maxStateValueDepth || start + 4 > bytes.length) throw new Error("invalid-state-value");
      if (bytes[start] !== 65 || bytes[start + 1] !== 88 || bytes[start + 2] !== valueFrameVersion) {
        throw new Error("invalid-state-value-frame");
      }
      const tag = bytes[start + 3];
      let cursor = start + 4;
      const readU32 = () => {
        if (cursor + 4 > bytes.length) throw new Error("truncated-state-value");
        const value = view.getUint32(cursor, true);
        cursor += 4;
        return value;
      };
      if (tag === 0) return { value: null, cursor };
      if (tag === 1 || tag === 5) {
        const length = readU32();
        const value = read(cursor, length);
        cursor += length;
        return { value: tag === 1 ? wasmTextDecoder.decode(value) : Array.from(value), cursor };
      }
      if (tag === 2) {
        const value = read(cursor, 1)[0];
        if (value > 1) throw new Error("invalid-state-bool");
        return { value: value === 1, cursor: cursor + 1 };
      }
      if (tag === 3) {
        if (cursor + 8 > bytes.length) throw new Error("truncated-state-value");
        const value = view.getFloat64(cursor, true);
        if (!Number.isFinite(value)) throw new Error("invalid-state-number");
        return { value, cursor: cursor + 8 };
      }
      if (tag === 4) {
        if (cursor + 8 > bytes.length) throw new Error("truncated-state-value");
        const raw = view.getBigInt64(cursor, true);
        const value = Number(raw);
        if (!Number.isSafeInteger(value)) throw new Error("unsafe-state-int");
        return { value, cursor: cursor + 8 };
      }
      if (tag === 6) {
        const count = readU32();
        const value = [];
        for (let index = 0; index < count; index += 1) {
          const nested = visit(cursor, depth + 1);
          value.push(nested.value);
          cursor = nested.cursor;
        }
        return { value, cursor };
      }
      if (tag === 7) {
        const count = readU32();
        const value = {};
        for (let index = 0; index < count; index += 1) {
          const keyLength = readU32();
          const key = wasmTextDecoder.decode(read(cursor, keyLength));
          cursor += keyLength;
          if (["__proto__", "prototype", "constructor"].includes(key) || Object.hasOwn(value, key)) {
            throw new Error("invalid-state-object-key");
          }
          const nested = visit(cursor, depth + 1);
          value[key] = nested.value;
          cursor = nested.cursor;
        }
        return { value, cursor };
      }
      throw new Error("unknown-state-value-tag");
    };

    try {
      const decoded = visit(0, 0);
      return decoded.cursor === bytes.length ? decoded.value : undefined;
    } catch (_) {
      return undefined;
    }
  };

  const stateValuesEqual = (left, right, type) => {
    if (Object.is(left, right)) return true;
    if (valueTypeCode(type) !== 3) return false;
    const leftBytes = encodeStateValue(left, type);
    const rightBytes = encodeStateValue(right, type);
    return !!leftBytes
      && !!rightBytes
      && leftBytes.length === rightBytes.length
      && leftBytes.every((value, index) => value === rightBytes[index]);
  };

  const writeValue = (node, target, value) => {
    if (target.startsWith("boolean:")) {
      const attribute = target.slice("boolean:".length);
      const enabled = value === true;
      node.toggleAttribute(attribute, enabled);
      if (attribute in node) node[attribute] = enabled;
      return;
    }
    const next = value == null ? "" : String(value);
    if (target === "checked") {
      node.checked = value === true || value === "true" || value === "on";
      return;
    }
    if (target === "text") {
      node.textContent = next;
      return;
    }
    if ("value" in node) node.value = next;
    else node.setAttribute("value", next);
  };

  const rejectDomCapability = (node, signal, target, role, reason) => {
    executorStats.rejectedDomCapabilities += 1;
    window.dispatchEvent(new CustomEvent("axonyx:dom-capability-rejected", {
      detail: { protocol: domCapabilityProtocol, signal, target, role, reason },
    }));
    return undefined;
  };

  const registerDomCapability = (node, signal, target, type, role, allowImplicit = false) => {
    let nodeCapabilities = domCapabilities.get(node);
    if (!nodeCapabilities) {
      nodeCapabilities = new Map();
      domCapabilities.set(node, nodeCapabilities);
    }
    const existing = nodeCapabilities.get(target);
    if (existing) {
      if (existing.signal === signal && existing.target === target && existing.role === role) {
        return existing;
      }
      return rejectDomCapability(node, signal, target, role, "capability-conflict");
    }

    const protocol = node.getAttribute("data-ax-dom-protocol");
    const declaredTarget = node.getAttribute("data-ax-dom-write");
    if (!allowImplicit && protocol !== domCapabilityProtocol) {
      return rejectDomCapability(node, signal, target, role, "invalid-protocol");
    }
    if (!allowImplicit && declaredTarget !== target) {
      return rejectDomCapability(node, signal, target, role, "target-mismatch");
    }
    const booleanTarget = target.startsWith("boolean:")
      ? target.slice("boolean:".length)
      : undefined;
    if (!domWriteTargets.has(target) && !expressionBooleanTargets.has(booleanTarget)) {
      return rejectDomCapability(node, signal, target, role, "unsupported-target");
    }
    if (target === "checked" && !("checked" in node)) {
      return rejectDomCapability(node, signal, target, role, "unsupported-node-target");
    }

    const capability = Object.freeze({ node, signal, target, type, role });
    nodeCapabilities.set(target, capability);
    validDomCapabilities.add(capability);
    domCapabilityList.push(capability);
    executorStats.registeredDomCapabilities += 1;
    return capability;
  };

  const writeDomCapability = (capability, value) => {
    if (!capability || !validDomCapabilities.has(capability)) {
      executorStats.rejectedDomWrites += 1;
      return false;
    }
    writeValue(capability.node, capability.target, value);
    executorStats.appliedDomWrites += 1;
    return true;
  };

  const utf8ByteLength = (value) => {
    if (typeof TextEncoder !== "function") return undefined;
    return new TextEncoder().encode(String(value)).length;
  };

  const rejectStorageCapability = (signal, scope, reason) => {
    executorStats.rejectedStorageCapabilities += 1;
    window.dispatchEvent(new CustomEvent("axonyx:storage-capability-rejected", {
      detail: { protocol: storageCapabilityProtocol, signal, scope, reason },
    }));
    return undefined;
  };

  const storageArea = (scope) => {
    try {
      return scope === "local" ? window.localStorage : window.sessionStorage;
    } catch (_) {
      return undefined;
    }
  };

  const registerStorageCapability = (signal, type, persistence) => {
    if (!persistence || typeof persistence !== "object") return undefined;
    const scope = persistence.scope;
    const key = persistence.key;
    if (persistence.protocol !== storageCapabilityProtocol) {
      return rejectStorageCapability(signal, scope, "invalid-protocol");
    }
    if (!storageScopes.has(scope)) {
      return rejectStorageCapability(signal, scope, "unsupported-scope");
    }
    const keyBytes = typeof key === "string" ? utf8ByteLength(key) : undefined;
    if (!key || keyBytes === undefined || keyBytes > maxStorageKeyBytes || /[\u0000-\u001f\u007f]/.test(key)) {
      return rejectStorageCapability(signal, scope, "invalid-key");
    }
    if (!operationsForType(type)) {
      return rejectStorageCapability(signal, scope, "unsupported-type");
    }

    const existing = storageCapabilities.get(signal);
    if (existing) {
      if (existing.scope === scope && existing.key === key && existing.type === type) return existing;
      return rejectStorageCapability(signal, scope, "signal-capability-conflict");
    }
    const storageKey = `${scope}\u0000${key}`;
    if (storageCapabilityKeys.has(storageKey)) {
      return rejectStorageCapability(signal, scope, "storage-key-conflict");
    }

    const capability = Object.freeze({ signal, type, scope, key });
    storageCapabilities.set(signal, capability);
    storageCapabilityKeys.set(storageKey, capability);
    validStorageCapabilities.add(capability);
    storageCapabilityList.push(capability);
    executorStats.registeredStorageCapabilities += 1;
    return capability;
  };

  const validateStoredValue = (capability, envelope) => {
    if (!envelope || typeof envelope !== "object") return { ok: false, reason: "invalid-envelope" };
    if (envelope.protocol !== storageValueProtocol) return { ok: false, reason: "invalid-value-protocol" };
    if (envelope.type !== capability.type) return { ok: false, reason: "stored-type-mismatch" };
    const normalizedType = unwrapPublicType(capability.type);
    if (stringLikeTypes.has(normalizedType)) {
      const bytes = typeof envelope.value === "string" ? utf8ByteLength(envelope.value) : undefined;
      if (bytes === undefined || bytes > maxStateEventStringBytes) {
        return { ok: false, reason: "invalid-string-value" };
      }
    } else if (numericTypes.has(normalizedType)) {
      if (typeof envelope.value !== "number" || !Number.isFinite(envelope.value)) {
        return { ok: false, reason: "invalid-number-value" };
      }
      if (normalizedType === "Int" && !Number.isSafeInteger(envelope.value)) {
        return { ok: false, reason: "invalid-int-value" };
      }
    } else if (normalizedType === "Bool" && typeof envelope.value !== "boolean") {
      return { ok: false, reason: "invalid-bool-value" };
    } else if (!validateStateValueForType(envelope.value, normalizedType)
      || !encodeStateValue(envelope.value, normalizedType)) {
      return { ok: false, reason: "invalid-structured-value" };
    }
    return { ok: true, value: envelope.value };
  };

  const restoreStorageCapability = (capability) => {
    if (!capability || !validStorageCapabilities.has(capability)) {
      executorStats.rejectedStorageReads += 1;
      return false;
    }
    const area = storageArea(capability.scope);
    if (!area) return false;
    try {
      const raw = area.getItem(capability.key);
      if (raw === null) return false;
      const validation = validateStoredValue(capability, JSON.parse(raw));
      if (!validation.ok) {
        executorStats.rejectedStorageReads += 1;
        window.dispatchEvent(new CustomEvent("axonyx:storage-value-rejected", {
          detail: {
            protocol: storageCapabilityProtocol,
            signal: capability.signal,
            scope: capability.scope,
            reason: validation.reason,
          },
        }));
        return false;
      }
      hydratedStorageSignals.add(capability.signal);
      writeSignal(capability.signal, validation.value, `storage:${capability.scope}`, false);
      executorStats.restoredStorageValues += 1;
      return true;
    } catch (_) {
      executorStats.rejectedStorageReads += 1;
      return false;
    }
  };

  const persistStorageCapability = (signal, value, source) => {
    const sourceName = String(source);
    if (sourceName.startsWith("snapshot") || sourceName.startsWith("storage:")) return false;
    const capability = storageCapabilities.get(signal);
    if (!capability) {
      if (!storageManifestReady) {
        pendingStorageWrites.set(signal, { value, source: sourceName });
        executorStats.queuedStorageWrites += 1;
      }
      return false;
    }
    if (!validStorageCapabilities.has(capability)) {
      executorStats.rejectedStorageWrites += 1;
      return false;
    }
    const validation = validateStoredValue(capability, {
      protocol: storageValueProtocol,
      type: capability.type,
      value,
    });
    if (!validation.ok) {
      executorStats.rejectedStorageWrites += 1;
      return false;
    }
    const area = storageArea(capability.scope);
    if (!area) return false;
    try {
      area.setItem(capability.key, JSON.stringify({
        protocol: storageValueProtocol,
        type: capability.type,
        value: validation.value,
      }));
      executorStats.persistedStorageValues += 1;
      return true;
    } catch (_) {
      executorStats.rejectedStorageWrites += 1;
      return false;
    }
  };

  const valueFromSnapshot = (entry) => {
    if (!entry || typeof entry !== "object") return entry;
    const value = entry.value;
    if (value && typeof value === "object" && "kind" in value) {
      if (value.kind === "null") return null;
      if (value.kind === "list" && Array.isArray(value.value)) {
        return value.value.map((item) => valueFromSnapshot({ value: item }));
      }
      if (value.kind === "object" && value.value && typeof value.value === "object") {
        return Object.fromEntries(
          Object.entries(value.value).map(([key, item]) => [key, valueFromSnapshot({ value: item })]),
        );
      }
      if (value.kind === "bytes" && Array.isArray(value.value)) return value.value.slice();
      return value.value;
    }
    return value;
  };

  const getTabId = () => {
    try {
      const key = "axonyx:tab-id";
      let value = window.sessionStorage && window.sessionStorage.getItem(key);
      if (!value) {
        value = "tab-" + Math.random().toString(36).slice(2) + Date.now().toString(36);
        if (window.sessionStorage) window.sessionStorage.setItem(key, value);
      }
      return value;
    } catch (_) {
      return "tab-" + Math.random().toString(36).slice(2);
    }
  };

  const tabId = getTabId();

  const stateRequestHeaders = () => ({
    "X-Axonyx-State-Protocol": "ax-state/1",
    "X-Axonyx-Tab": tabId,
  });

  const emitPatch = (signal, value, source) => {
    const detail = { op: "set", signal, value, source };
    window.dispatchEvent(new CustomEvent("axonyx:state-patch", { detail }));
  };

  const componentSignalAlias = (signal) => {
    if (!signal || !signal.startsWith("component:")) return undefined;
    const parts = signal.split(":");
    if (parts.length < 5) return undefined;
    const component = parts[1];
    const name = parts[parts.length - 2];
    const index = parts[parts.length - 1] || "1";
    return aliases.get(`${component}.${name}`) || aliases.get(`__ax_component_state__:${component}:${name}:${index}`);
  };

  const canonicalSignal = (signal) => aliases.get(signal) || componentSignalAlias(signal) || signal;

  const bindAlias = (alias, signal) => {
    if (!alias || !signal) return;
    aliases.set(alias, signal);
  };

  const moveSignalBucket = (bucket, from, to) => {
    if (!from || !to || from === to || !bucket.has(from)) return;
    const current = bucket.get(from) || [];
    if (!bucket.has(to)) bucket.set(to, []);
    bucket.get(to).push(...current);
    bucket.delete(from);
  };

  const indexExpressionEntry = (entry) => {
    entry.signals.forEach((signal) => {
      if (!expressions.has(signal)) expressions.set(signal, []);
      if (!expressions.get(signal).includes(entry)) expressions.get(signal).push(entry);
    });
  };

  const rebindExpressions = () => {
    expressions.clear();
    expressionEntries.forEach((entry) => {
      entry.signals = entry.signals.map(canonicalSignal);
      indexExpressionEntry(entry);
    });
  };

  const rebindAliasedSignals = () => {
    aliases.forEach((signal, alias) => {
      if (alias === signal) return;
      moveSignalBucket(bindings, alias, signal);
      moveSignalBucket(readBindings, alias, signal);
      moveSignalBucket(subscribers, alias, signal);
      moveSignalBucket(conditions, alias, signal);
      moveSignalBucket(matches, alias, signal);
      moveSignalBucket(eachBindings, alias, signal);
      (eachBindings.get(signal) || []).forEach((entry) => { entry.signal = signal; });
      if (state.has(alias) && !state.has(signal)) state.set(signal, state.get(alias));
      if (types.has(alias) && !types.has(signal)) types.set(signal, types.get(alias));
      document.querySelectorAll(`[data-ax-signal="${alias}"]`).forEach((node) => {
        node.setAttribute("data-ax-signal", signal);
      });
      document.querySelectorAll(`[data-ax-state-key="${alias}"]`).forEach((node) => {
        node.setAttribute("data-ax-state-key", signal);
      });
      document.querySelectorAll(`[data-ax-state-if-signal="${alias}"]`).forEach((node) => {
        node.setAttribute("data-ax-state-if-signal", signal);
      });
      document.querySelectorAll(`[data-ax-state-match-signal="${alias}"]`).forEach((node) => {
        node.setAttribute("data-ax-state-match-signal", signal);
      });
    });
    rebindExpressions();
  };

  const rebindPendingStorageWrites = () => {
    Array.from(pendingStorageWrites.entries()).forEach(([signal, pending]) => {
      const canonical = canonicalSignal(signal);
      if (canonical === signal) return;
      pendingStorageWrites.delete(signal);
      pendingStorageWrites.set(canonical, pending);
    });
  };

  const hydrateStorageCapability = (capability) => {
    const pending = pendingStorageWrites.get(capability.signal);
    if (!pending) return restoreStorageCapability(capability);
    pendingStorageWrites.delete(capability.signal);
    if (!persistStorageCapability(capability.signal, pending.value, pending.source)) return false;
    hydratedStorageSignals.add(capability.signal);
    executorStats.flushedStorageWrites += 1;
    return true;
  };

  const settleStorageManifest = () => {
    storageManifestReady = true;
    pendingStorageWrites.clear();
  };

  const notifySubscribers = (signal, value, source) => {
    const detail = { signal, value, source };
    window.dispatchEvent(new CustomEvent("axonyx:state-change", { detail }));
    (subscribers.get(signal) || []).forEach((listener) => listener(value, detail));
  };

  const register = (node) => {
    const rawSignal = node.getAttribute("data-ax-signal");
    const signal = canonicalSignal(rawSignal);
    if (!signal) return;
    if (rawSignal !== signal) node.setAttribute("data-ax-signal", signal);
    const target = node.getAttribute("data-ax-bind") || "value";
    const type = node.getAttribute("data-ax-state-type") || "String";
    const capability = registerDomCapability(node, signal, target, type, "binding");
    if (!capability) return;
    const initial = castValue(readValue(node, target), type);
    if (!types.has(signal)) types.set(signal, type);
    if (!state.has(signal)) state.set(signal, initial);
    writeDomCapability(capability, state.get(signal));
    if (!bindings.has(signal)) bindings.set(signal, []);
    bindings.get(signal).push(capability);
  };

  const registerRead = (node) => {
    const name = node.getAttribute("data-ax-state-name");
    if (!name) return;
    const target = node.getAttribute("data-ax-state-target") || "text";
    const signal = canonicalSignal(metadataByName.get(name) || node.getAttribute("data-ax-state-key"));
    if (!signal) return;
    const type = types.get(signal) || "String";
    const capability = registerDomCapability(node, signal, target, type, "named-read", true);
    if (!capability) return;
    node.setAttribute("data-ax-state-key", signal);
    if (!readBindings.has(signal)) readBindings.set(signal, []);
    if (!readBindings.get(signal).includes(capability)) readBindings.get(signal).push(capability);
    if (state.has(signal)) {
      writeDomCapability(capability, state.get(signal));
      node.setAttribute("data-ax-state-source", "state");
    }
  };

  const registerReads = () => {
    document.querySelectorAll("[data-ax-state-name]").forEach(registerRead);
  };

  const evaluateCondition = (entry, current) => {
    const expected = castValue(entry.node.getAttribute("data-ax-state-if-value"), entry.type);
    if (entry.op === "truthy") return !!current;
    if (entry.op === "falsy") return !current;
    if (entry.op === "eq") return current === expected;
    if (entry.op === "ne") return current !== expected;
    if (entry.op === "gt") return current > expected;
    if (entry.op === "ge") return current >= expected;
    if (entry.op === "lt") return current < expected;
    if (entry.op === "le") return current <= expected;
    return false;
  };

  const updateCondition = (entry, current) => {
    const active = evaluateCondition(entry, current);
    entry.node.querySelectorAll(":scope > [data-ax-state-if-branch]").forEach((branch) => {
      const show = branch.getAttribute("data-ax-state-if-branch") === (active ? "then" : "else");
      branch.hidden = !show;
      branch.style.display = show ? "contents" : "none";
    });
  };

  const registerCondition = (node) => {
    const rawSignal = node.getAttribute("data-ax-state-if-signal");
    const signal = canonicalSignal(rawSignal);
    if (!signal) return;
    if (rawSignal !== signal) node.setAttribute("data-ax-state-if-signal", signal);
    const type = node.getAttribute("data-ax-state-if-type") || "String";
    const initial = castValue(node.getAttribute("data-ax-state-if-initial"), type);
    const entry = {
      node,
      type,
      op: node.getAttribute("data-ax-state-if-op") || "truthy",
    };
    if (!types.has(signal)) types.set(signal, type);
    if (!state.has(signal)) state.set(signal, initial);
    if (!conditions.has(signal)) conditions.set(signal, []);
    conditions.get(signal).push(entry);
    updateCondition(entry, state.get(signal));
  };

  const updateMatch = (entry, current) => {
    const value = current == null ? "" : String(current);
    let matched = false;
    const branches = Array.from(
      entry.node.querySelectorAll(":scope > [data-ax-state-match-branch]"),
    );
    branches.forEach((branch) => {
      if (branch.getAttribute("data-ax-state-match-branch") !== "case") return;
      const show = !matched && branch.getAttribute("data-ax-state-match-value") === value;
      if (show) matched = true;
      branch.hidden = !show;
      branch.style.display = show ? "contents" : "none";
    });
    branches.forEach((branch) => {
      if (branch.getAttribute("data-ax-state-match-branch") !== "default") return;
      branch.hidden = matched;
      branch.style.display = matched ? "none" : "contents";
    });
  };

  const registerMatch = (node) => {
    const rawSignal = node.getAttribute("data-ax-state-match-signal");
    const signal = canonicalSignal(rawSignal);
    if (!signal) return;
    if (rawSignal !== signal) node.setAttribute("data-ax-state-match-signal", signal);
    const type = node.getAttribute("data-ax-state-match-type") || "String";
    const literalSource = node.getAttribute("data-ax-state-match-literals");
    if (literalSource && !typeSchemas.has(type)) {
      try {
        const literals = JSON.parse(literalSource);
        if (Array.isArray(literals)
          && literals.length > 0
          && literals.every((literal) => typeof literal === "string")
          && new Set(literals).size === literals.length) {
          typeSchemas.set(type, { name: type, literals });
        }
      } catch (_) {}
    }
    const initial = castValue(node.getAttribute("data-ax-state-match-initial"), type);
    const entry = { node, type };
    if (!types.has(signal)) types.set(signal, type);
    if (!state.has(signal)) state.set(signal, initial);
    if (!matches.has(signal)) matches.set(signal, []);
    matches.get(signal).push(entry);
    updateMatch(entry, state.get(signal));
  };

  const rejectExpression = (node, index, reason) => {
    executorStats.rejectedExpressions += 1;
    window.dispatchEvent(new CustomEvent("axonyx:expression-rejected", {
      detail: { protocol: "ax-expression/1", index, reason },
    }));
    return undefined;
  };

  const decodeHex = (source) => {
    if (typeof source !== "string" || source.length === 0 || source.length % 2 !== 0) {
      return undefined;
    }
    const bytes = new Uint8Array(source.length / 2);
    for (let index = 0; index < bytes.length; index += 1) {
      const value = Number.parseInt(source.slice(index * 2, index * 2 + 2), 16);
      if (!Number.isInteger(value)) return undefined;
      bytes[index] = value;
    }
    return bytes;
  };

  const expressionRequest = (entry) => {
    const values = entry.signals.map((signal, index) => {
      if (!state.has(signal)) return undefined;
      return encodeStateValue(state.get(signal), entry.types[index]);
    });
    if (values.some((value) => !value)) return undefined;
    const length = 4 + entry.program.length + 4
      + values.reduce((total, value) => total + value.length, 0);
    if (length > maxStateValueBytes) return undefined;
    const request = new Uint8Array(length);
    const view = new DataView(request.buffer);
    let cursor = 0;
    view.setUint32(cursor, entry.program.length, true);
    cursor += 4;
    request.set(entry.program, cursor);
    cursor += entry.program.length;
    view.setUint32(cursor, values.length, true);
    cursor += 4;
    values.forEach((value) => {
      request.set(value, cursor);
      cursor += value.length;
    });
    return request;
  };

  const evaluateWasmExpression = (entry) => {
    if (!wasmExecutor || typeof wasmExecutor.ax_state_evaluate_expression !== "function") {
      return undefined;
    }
    const request = expressionRequest(entry);
    if (!request) return undefined;
    const capacity = wasmExecutor.ax_state_value_buffer_capacity();
    const pointer = wasmExecutor.ax_state_value_buffer_ptr();
    if (request.length > capacity
      || pointer + request.length > wasmExecutor.memory.buffer.byteLength) return undefined;
    new Uint8Array(wasmExecutor.memory.buffer, pointer, request.length).set(request);
    const resultLength = wasmExecutor.ax_state_evaluate_expression(request.length) >>> 0;
    if (resultLength === 0xffffffff || resultLength > capacity
      || pointer + resultLength > wasmExecutor.memory.buffer.byteLength) return undefined;
    return decodeStateValue(
      new Uint8Array(wasmExecutor.memory.buffer, pointer, resultLength),
    );
  };

  const updateExpression = (entry) => {
    const value = evaluateWasmExpression(entry);
    if (value === undefined) return false;
    if (entry.target.startsWith("boolean:") && typeof value !== "boolean") {
      rejectExpression(entry.node, entry.index, "boolean-target-type-mismatch");
      return false;
    }
    if (!writeDomCapability(entry.capability, value)) return false;
    executorStats.expressionEvaluations += 1;
    return true;
  };

  const registerExpressions = (node) => {
    if (node.getAttribute("data-ax-expression-protocol") !== "ax-expression/1") {
      rejectExpression(node, -1, "invalid-protocol");
      return;
    }
    const count = Number(node.getAttribute("data-ax-expression-count"));
    if (!Number.isInteger(count) || count < 1 || count > 16) {
      rejectExpression(node, -1, "invalid-count");
      return;
    }
    for (let index = 0; index < count; index += 1) {
      const prefix = `data-ax-expression-${index}`;
      const program = decodeHex(node.getAttribute(`${prefix}-program`));
      let signals;
      let dependencyTypes;
      let initials;
      try {
        signals = JSON.parse(node.getAttribute(`${prefix}-signals`) || "null");
        dependencyTypes = JSON.parse(node.getAttribute(`${prefix}-types`) || "null");
        initials = JSON.parse(node.getAttribute(`${prefix}-initials`) || "null");
      } catch (_) {
        rejectExpression(node, index, "invalid-metadata");
        continue;
      }
      const target = node.getAttribute(`${prefix}-target`);
      if (!program || program.length < 4 || program[0] !== 65 || program[1] !== 88
        || program[2] !== 69 || program[3] !== 1) {
        rejectExpression(node, index, "invalid-program");
        continue;
      }
      if (!Array.isArray(signals) || !Array.isArray(dependencyTypes) || !Array.isArray(initials)
        || signals.length === 0 || signals.length !== dependencyTypes.length
        || signals.length !== initials.length
        || signals.some((signal) => typeof signal !== "string" || !signal)
        || dependencyTypes.some((type) => typeof type !== "string" || !type)
        || initials.some((initial) => typeof initial !== "string")) {
        rejectExpression(node, index, "invalid-dependencies");
        continue;
      }
      const validTarget = target === "text"
        || target?.startsWith("boolean:")
          && expressionBooleanTargets.has(target.slice("boolean:".length));
      if (!validTarget) {
        rejectExpression(node, index, "unsupported-target");
        continue;
      }
      signals = signals.map(canonicalSignal);
      signals.forEach((signal, dependencyIndex) => {
        const type = dependencyTypes[dependencyIndex];
        if (!types.has(signal)) types.set(signal, type);
        if (!state.has(signal)) state.set(signal, castValue(initials[dependencyIndex], type));
      });
      const expressionId = `expression:${index}:${signals.join("|")}`;
      const capability = registerDomCapability(
        node,
        expressionId,
        target,
        target === "text" ? "String" : "Bool",
        "expression",
        true,
      );
      if (!capability) {
        rejectExpression(node, index, "capability-rejected");
        continue;
      }
      const entry = {
        node,
        index,
        program,
        signals,
        types: dependencyTypes,
        target,
        capability,
      };
      expressionEntries.push(entry);
      indexExpressionEntry(entry);
      updateExpression(entry);
    }
  };

  const eachKeyIdentity = (value, expectedKind = "") => {
    if (typeof value === "string" && (!expectedKind || expectedKind === "string")) {
      return `string:${value}`;
    }
    if (typeof value === "boolean" && (!expectedKind || expectedKind === "bool")) {
      return `bool:${value}`;
    }
    if (typeof value === "number" && Number.isFinite(value)) {
      const kind = expectedKind || (Number.isSafeInteger(value) ? "number" : "float");
      if (kind === "number" && Number.isSafeInteger(value) || kind === "float") {
        return `${kind}:${value}`;
      }
    }
    return undefined;
  };

  const eachItemKey = (item, path, keyKind) => {
    if (!path) return eachKeyIdentity(item, keyKind);
    let value = item;
    for (const part of path.split(".")) {
      if (!value || typeof value !== "object" || !Object.hasOwn(value, part)) return undefined;
      value = value[part];
    }
    return eachKeyIdentity(value, keyKind);
  };

  const eachKeys = (items, path, keyKind) => {
    if (!Array.isArray(items)) return undefined;
    const keys = items.map((item) => eachItemKey(item, path, keyKind));
    if (keys.some((key) => !key) || new Set(keys).size !== keys.length) return undefined;
    return keys;
  };

  const fallbackEachPlan = (oldKeys, nextKeys) => {
    const oldSet = new Set(oldKeys);
    const nextSet = new Set(nextKeys);
    return {
      removed: oldKeys.filter((key) => !nextSet.has(key)),
      inserted: nextKeys.filter((key) => !oldSet.has(key)),
      order: nextKeys.filter((key) => oldSet.has(key)),
    };
  };

  const planEachReconciliation = (oldKeys, nextKeys) => {
    if (!wasmExecutor || typeof wasmExecutor.ax_state_reconcile_keys !== "function") {
      return fallbackEachPlan(oldKeys, nextKeys);
    }
    const bytes = encodeStateValue({ old: oldKeys, next: nextKeys }, "Unknown");
    const capacity = wasmExecutor.ax_state_value_buffer_capacity();
    const pointer = wasmExecutor.ax_state_value_buffer_ptr();
    if (!bytes || bytes.length > capacity
      || pointer + bytes.length > wasmExecutor.memory.buffer.byteLength) {
      return fallbackEachPlan(oldKeys, nextKeys);
    }
    new Uint8Array(wasmExecutor.memory.buffer, pointer, bytes.length).set(bytes);
    const resultLength = wasmExecutor.ax_state_reconcile_keys(bytes.length) >>> 0;
    if (resultLength === 0xffffffff || resultLength > capacity) {
      return fallbackEachPlan(oldKeys, nextKeys);
    }
    const plan = decodeStateValue(
      new Uint8Array(wasmExecutor.memory.buffer, pointer, resultLength),
    );
    return plan && Array.isArray(plan.removed) && Array.isArray(plan.inserted)
      && Array.isArray(plan.order)
      ? plan
      : fallbackEachPlan(oldKeys, nextKeys);
  };

  const rejectEach = (entry, reason, nextValue) => {
    executorStats.rejectedEachLists += 1;
    entry.node.setAttribute("data-ax-each-status", "rejected");
    window.dispatchEvent(new CustomEvent("axonyx:each-rejected", {
      detail: { protocol: "ax-each/1", signal: entry.signal, reason, value: nextValue },
    }));
    return false;
  };

  const requireEachRefresh = (entry, reason, nextValue, plan) => {
    executorStats.eachRefreshesRequired += 1;
    entry.node.setAttribute("data-ax-each-status", "refresh-required");
    window.dispatchEvent(new CustomEvent("axonyx:each-refresh-required", {
      detail: { protocol: "ax-each/1", signal: entry.signal, reason, value: nextValue, plan },
    }));
    return false;
  };

  const eachRenderValue = (item, path) => {
    let value = item;
    if (!path) return { ok: true, value };
    for (const part of path.split(".")) {
      if (!value || typeof value !== "object" || !Object.hasOwn(value, part)) {
        return { ok: false };
      }
      value = value[part];
    }
    return { ok: true, value };
  };

  const eachRenderScalar = (value) => {
    if (value === null || value === undefined) return { ok: true, value: "" };
    if (["string", "number", "boolean"].includes(typeof value)) {
      return { ok: true, value: String(value) };
    }
    return { ok: false };
  };

  const safeEachUrl = (target, value) => {
    if (!["href", "src", "action", "formaction"].includes(target)) return true;
    const normalized = value.trim().toLowerCase();
    return normalized.startsWith("/") || normalized.startsWith("./")
      || normalized.startsWith("../") || normalized.startsWith("#")
      || normalized.startsWith("https://") || normalized.startsWith("http://")
      || normalized.startsWith("mailto:") || normalized.startsWith("tel:");
  };

  const eachRenderNodes = (root) => {
    const nodes = [];
    if (root?.matches?.("[data-ax-each-render-target], [data-ax-each-render-attrs]")) {
      nodes.push(root);
    }
    if (typeof root?.querySelectorAll === "function") {
      nodes.push(...root.querySelectorAll(
        "[data-ax-each-render-target], [data-ax-each-render-attrs]",
      ));
    }
    return nodes;
  };

  const resolveEachRenderValues = (item, paths) => {
    if (!wasmExecutor || typeof wasmExecutor.ax_state_render_each !== "function") {
      const resolved = paths.map((path) => eachRenderValue(item, path));
      return resolved.every((entry) => entry.ok)
        ? resolved.map((entry) => entry.value)
        : undefined;
    }
    const bytes = encodeStateValue({ item, paths }, "Unknown");
    const capacity = wasmExecutor.ax_state_value_buffer_capacity();
    const pointer = wasmExecutor.ax_state_value_buffer_ptr();
    if (!bytes || bytes.length > capacity
      || pointer + bytes.length > wasmExecutor.memory.buffer.byteLength) {
      return undefined;
    }
    new Uint8Array(wasmExecutor.memory.buffer, pointer, bytes.length).set(bytes);
    const resultLength = wasmExecutor.ax_state_render_each(bytes.length) >>> 0;
    if (resultLength === 0xffffffff || resultLength > capacity) return undefined;
    const values = decodeStateValue(
      new Uint8Array(wasmExecutor.memory.buffer, pointer, resultLength),
    );
    return Array.isArray(values) && values.length === paths.length ? values : undefined;
  };

  const planEachRenderWrites = (root, item) => {
    const capabilities = [];
    for (const node of eachRenderNodes(root)) {
      if (node.getAttribute("data-ax-each-render-target") === "text") {
        capabilities.push({
          node,
          mode: "text",
          path: node.getAttribute("data-ax-each-render-path") || "",
        });
      }

      const rawAttrs = node.getAttribute("data-ax-each-render-attrs");
      if (!rawAttrs) continue;
      let bindings;
      try {
        bindings = JSON.parse(rawAttrs);
      } catch {
        return undefined;
      }
      if (!Array.isArray(bindings)) return undefined;
      for (const binding of bindings) {
        if (!binding || typeof binding.target !== "string"
          || typeof binding.path !== "string"
          || !["attribute", "boolean"].includes(binding.mode)
          || binding.target.toLowerCase().startsWith("on")) {
          return undefined;
        }
        capabilities.push({
          node,
          mode: binding.mode,
          target: binding.target,
          path: binding.path,
        });
      }
    }
    const values = resolveEachRenderValues(item, capabilities.map((entry) => entry.path));
    if (!values) return undefined;

    const writes = [];
    for (let index = 0; index < capabilities.length; index += 1) {
      const capability = capabilities[index];
      const value = values[index];
      if (capability.mode === "boolean") {
        if (typeof value !== "boolean") return undefined;
        writes.push({ ...capability, value });
        continue;
      }
      const scalar = eachRenderScalar(value);
      if (!scalar.ok
        || capability.mode === "attribute"
          && !safeEachUrl(capability.target, scalar.value)) {
        return undefined;
      }
      writes.push({ ...capability, value: scalar.value });
    }
    return writes;
  };

  const commitEachRenderWrites = (writes) => {
    writes.forEach((write) => {
      if (write.mode === "text") {
        write.node.textContent = write.value;
      } else if (write.mode === "boolean") {
        if (write.value) write.node.setAttribute(write.target, "");
        else write.node.removeAttribute(write.target);
      } else {
        write.node.setAttribute(write.target, write.value);
        if (write.target === "value" && "value" in write.node) write.node.value = write.value;
      }
    });
  };

  const createEachItem = (entry, key, item) => {
    if (!entry.template?.content || typeof document.createElement !== "function") return undefined;
    const fragment = entry.template.content.cloneNode(true);
    const writes = planEachRenderWrites(fragment, item);
    if (!writes) return undefined;
    const node = document.createElement("ax-each-item");
    node.setAttribute("style", "display: contents");
    node.setAttribute("data-ax-each-key-id", key);
    node.setAttribute("data-ax-each-key", key.slice(key.indexOf(":") + 1));
    node.append(fragment);
    commitEachRenderWrites(writes);
    return node;
  };

  const reconcileEach = (entry, nextValue) => {
    const oldKeys = eachKeys(entry.items, entry.keyPath, entry.keyKind);
    const nextKeys = eachKeys(nextValue, entry.keyPath, entry.keyKind);
    if (!oldKeys || !nextKeys) return rejectEach(entry, "invalid-or-duplicate-key", nextValue);
    const plan = planEachReconciliation(oldKeys, nextKeys);
    if (nextKeys.length === 0 && !entry.emptyNode) {
      return requireEachRefresh(entry, "empty-render-program-required", nextValue, plan);
    }

    const oldItems = new Map(oldKeys.map((key, index) => [key, entry.items[index]]));
    const nextItems = new Map(nextKeys.map((key, index) => [key, nextValue[index]]));
    const changed = plan.order.some((key) => (
      !stateValuesEqual(oldItems.get(key), nextItems.get(key), "Unknown")
    ));
    if (!entry.renderReady && (changed || plan.inserted.length > 0)) {
      return requireEachRefresh(
        entry,
        plan.inserted.length > 0
          ? "item-render-program-required"
          : "item-update-program-required",
        nextValue,
        plan,
      );
    }
    const nodes = new Map(entry.itemsNodes.map((node) => [node.getAttribute("data-ax-each-key-id"), node]));
    const pendingWrites = [];
    for (const key of plan.order) {
      if (stateValuesEqual(oldItems.get(key), nextItems.get(key), "Unknown")) continue;
      const writes = planEachRenderWrites(nodes.get(key), nextItems.get(key));
      if (!writes) return requireEachRefresh(entry, "item-update-program-required", nextValue, plan);
      pendingWrites.push(writes);
    }
    for (const key of plan.inserted) {
      const node = createEachItem(entry, key, nextItems.get(key));
      if (!node) return requireEachRefresh(entry, "item-render-program-required", nextValue, plan);
      nodes.set(key, node);
    }

    plan.removed.forEach((key) => nodes.get(key)?.remove());
    pendingWrites.forEach(commitEachRenderWrites);
    let anchor = entry.emptyNode || entry.template || null;
    [...nextKeys].reverse().forEach((key) => {
      const node = nodes.get(key);
      if (node) {
        if (node.nextSibling !== anchor) entry.node.insertBefore(node, anchor);
        anchor = node;
      }
    });
    if (entry.emptyNode) entry.emptyNode.hidden = nextKeys.length !== 0;
    entry.items = nextValue;
    entry.itemsNodes = Array.from(entry.node.children)
      .filter((node) => node.matches("ax-each-item[data-ax-each-key-id]"));
    entry.node.setAttribute("data-ax-each-status", "reconciled");
    executorStats.reconciledEachLists += 1;
    window.dispatchEvent(new CustomEvent("axonyx:each-reconciled", {
      detail: { protocol: "ax-each/1", signal: entry.signal, plan },
    }));
    return true;
  };

  const registerEach = (node) => {
    if (node.getAttribute("data-ax-each-protocol") !== "ax-each/1") return;
    const rawSignal = node.getAttribute("data-ax-each-signal");
    if (!rawSignal) return;
    const signal = canonicalSignal(rawSignal);
    const type = node.getAttribute("data-ax-each-type") || "Unknown";
    const items = castValue(node.getAttribute("data-ax-each-initial") || "[]", type);
    const itemsNodes = Array.from(node.children)
      .filter((child) => child.matches("ax-each-item[data-ax-each-key-id]"));
    const entry = {
      node,
      signal,
      type,
      keyPath: node.getAttribute("data-ax-each-key-path") || "",
      keyKind: node.getAttribute("data-ax-each-key-kind") || "",
      renderReady: node.getAttribute("data-ax-each-render-status") === "ready",
      items: Array.isArray(items) ? items : [],
      itemsNodes,
      emptyNode: Array.from(node.children).find((child) => child.matches("ax-each-empty")),
      template: Array.from(node.children).find((child) => (
        child.matches("template[data-ax-each-render-protocol='ax-each-render/1']")
      )),
    };
    const domKeys = itemsNodes.map((child) => child.getAttribute("data-ax-each-key-id"));
    const initialKeys = eachKeys(entry.items, entry.keyPath, entry.keyKind);
    if (!initialKeys || domKeys.length !== initialKeys.length
      || domKeys.some((key, index) => key !== initialKeys[index])) {
      rejectEach(entry, "initial-dom-key-mismatch", entry.items);
      return;
    }
    if (!types.has(signal)) types.set(signal, type);
    if (!state.has(signal)) state.set(signal, entry.items);
    if (!eachBindings.has(signal)) eachBindings.set(signal, []);
    eachBindings.get(signal).push(entry);
  };

  const writeSignal = (signal, value, source = "client", emit = true) => {
    signal = canonicalSignal(signal);
    const type = types.get(signal) || "String";
    const nextValue = castValue(value, type);
    if (!validateStateValueForType(nextValue, type)) {
      executorStats.rejectedEvents += 1;
      window.dispatchEvent(new CustomEvent("axonyx:state-value-rejected", {
        detail: { protocol: stateEventProtocol, signal, type, source, reason: "type-mismatch" },
      }));
      return state.get(signal);
    }
    state.set(signal, nextValue);
    (eachBindings.get(signal) || []).forEach((entry) => reconcileEach(entry, nextValue));
    (bindings.get(signal) || []).forEach((capability) => {
      writeDomCapability(capability, nextValue);
    });
    (readBindings.get(signal) || []).forEach((capability) => {
      writeDomCapability(capability, nextValue);
      capability.node.setAttribute("data-ax-state-source", source);
    });
    (conditions.get(signal) || []).forEach((entry) => updateCondition(entry, nextValue));
    (matches.get(signal) || []).forEach((entry) => updateMatch(entry, nextValue));
    new Set(expressions.get(signal) || []).forEach(updateExpression);
    persistStorageCapability(signal, nextValue, source);
    notifySubscribers(signal, nextValue, source);
    if (emit) emitPatch(signal, nextValue, source);
    return nextValue;
  };

  const setSignal = (signal, value, source = "client") => {
    return writeSignal(signal, value, source, true);
  };

  const applyLocalOperation = (op, type, current, operand) => {
    const operation = localOperationCode[op];
    const normalizedType = unwrapPublicType(type);
    const valueType = valueTypeCode(normalizedType);
    const wasmSupportsOperation = wasmExecutor
      && operation !== undefined
      && valueType !== undefined
      && wasmExecutor.ax_state_supports_operation(valueType, operation) === 1;
    if (wasmSupportsOperation) {
      if (numericTypes.has(normalizedType) && op !== "toggle") {
        const next = wasmExecutor.ax_state_apply_number(operation, Number(current), Number(operand));
        if (Number.isFinite(next) && (normalizedType !== "Int" || Number.isSafeInteger(next))) {
          executorStats.wasmOperations += 1;
          return next;
        }
      }
      if (normalizedType === "Bool" && (op === "set" || op === "toggle")) {
        const next = wasmExecutor.ax_state_apply_bool(
          operation,
          current ? 1 : 0,
          operand ? 1 : 0,
        ) >>> 0;
        if (next !== 0xffffffff) {
          executorStats.wasmOperations += 1;
          return next !== 0;
        }
      }
      if (stringLikeTypes.has(normalizedType) && op === "set" && wasmTextEncoder && wasmTextDecoder) {
        const bytes = wasmTextEncoder.encode(String(operand));
        const capacity = wasmExecutor.ax_state_string_buffer_capacity();
        const pointer = wasmExecutor.ax_state_string_buffer_ptr();
        if (bytes.length <= capacity && pointer + bytes.length <= wasmExecutor.memory.buffer.byteLength) {
          new Uint8Array(wasmExecutor.memory.buffer, pointer, bytes.length).set(bytes);
          const resultLength = wasmExecutor.ax_state_apply_string(operation, bytes.length) >>> 0;
          if (resultLength !== 0xffffffff && resultLength <= capacity) {
            const next = wasmTextDecoder.decode(
              new Uint8Array(wasmExecutor.memory.buffer, pointer, resultLength),
            );
            executorStats.wasmOperations += 1;
            return next;
          }
        }
      }
      if (valueType === 3 && op === "set") {
        const bytes = encodeStateValue(operand, normalizedType);
        const capacity = wasmExecutor.ax_state_value_buffer_capacity();
        const pointer = wasmExecutor.ax_state_value_buffer_ptr();
        if (bytes && bytes.length <= capacity
          && pointer + bytes.length <= wasmExecutor.memory.buffer.byteLength) {
          new Uint8Array(wasmExecutor.memory.buffer, pointer, bytes.length).set(bytes);
          const resultLength = wasmExecutor.ax_state_apply_value(operation, bytes.length) >>> 0;
          if (resultLength !== 0xffffffff && resultLength <= capacity) {
            const next = decodeStateValue(
              new Uint8Array(wasmExecutor.memory.buffer, pointer, resultLength),
            );
            if (next !== undefined) {
              executorStats.wasmOperations += 1;
              return next;
            }
          }
        }
      }
    }

    if (op === "set") {
      executorStats.fallbackOperations += 1;
      return operand;
    }
    if (op === "add") {
      executorStats.fallbackOperations += 1;
      return Number(current) + Number(operand);
    }
    if (op === "sub") {
      executorStats.fallbackOperations += 1;
      return Number(current) - Number(operand);
    }
    if (op === "toggle") {
      executorStats.fallbackOperations += 1;
      return !castValue(current, "Bool");
    }
    return undefined;
  };

  const stringByteLength = (value) => {
    if (wasmTextEncoder) return wasmTextEncoder.encode(value).length;
    if (typeof TextEncoder === "function") return new TextEncoder().encode(value).length;
    return undefined;
  };

  const validateStateEventPayload = (payload) => {
    if (!payload || typeof payload !== "object") return { ok: false, reason: "invalid-payload" };
    if (payload.protocol !== stateEventProtocol) return { ok: false, reason: "invalid-protocol" };
    if (!["click", "input", "change"].includes(payload.event)) {
      return { ok: false, reason: "unsupported-event" };
    }
    if (typeof payload.signal !== "string"
      || payload.signal.length === 0
      || payload.signal.length > maxStateEventSignalLength
      || /[\u0000-\u001f\u007f]/.test(payload.signal)) {
      return { ok: false, reason: "invalid-signal" };
    }
    const operations = operationsForType(payload.type);
    if (!operations) {
      return { ok: false, reason: "unsupported-type" };
    }
    if (!operations.has(payload.op)) {
      return { ok: false, reason: "unsupported-operation" };
    }
    const registeredType = types.get(payload.signal);
    if (registeredType && unwrapPublicType(registeredType) !== unwrapPublicType(payload.type)) {
      return { ok: false, reason: "signal-type-mismatch" };
    }
    if (!["literal", "value", "checked"].includes(payload.valueSource)) {
      return { ok: false, reason: "unsupported-value-source" };
    }
    if (payload.valueSource !== "literal" && !["input", "change"].includes(payload.event)) {
      return { ok: false, reason: "event-source-mismatch" };
    }
    if (payload.valueSource === "checked"
      && (unwrapPublicType(payload.type) !== "Bool" || typeof payload.value !== "boolean")) {
      return { ok: false, reason: "checked-type-mismatch" };
    }
    if (payload.valueSource === "value"
      && !(stringLikeTypes.has(unwrapPublicType(payload.type))
        || numericTypes.has(unwrapPublicType(payload.type)))) {
      return { ok: false, reason: "value-type-mismatch" };
    }

    const operand = castValue(payload.value, payload.type);
    const normalizedType = unwrapPublicType(payload.type);
    if (numericTypes.has(normalizedType) && !Number.isFinite(operand)) {
      return { ok: false, reason: "invalid-number" };
    }
    if (normalizedType === "Int" && !Number.isSafeInteger(operand)) {
      return { ok: false, reason: "invalid-int" };
    }
    if (stringLikeTypes.has(normalizedType)) {
      const bytes = stringByteLength(operand);
      if (bytes === undefined || bytes > maxStateEventStringBytes) {
        return { ok: false, reason: "string-too-large" };
      }
    } else if (valueTypeCode(normalizedType) === 3
      && (!validateStateValueForType(operand, normalizedType)
        || !encodeStateValue(operand, normalizedType))) {
      return { ok: false, reason: "invalid-structured-value" };
    }
    return { ok: true, operand };
  };

  const rejectStateEvent = (payload, reason) => {
    executorStats.rejectedEvents += 1;
    window.dispatchEvent(new CustomEvent("axonyx:state-event-rejected", {
      detail: {
        protocol: stateEventProtocol,
        reason,
        event: payload?.event,
        signal: payload?.signal,
        op: payload?.op,
        type: payload?.type,
      },
    }));
    return false;
  };

  const executeStateEventPayload = (payload) => {
    const validation = validateStateEventPayload(payload);
    if (!validation.ok) return rejectStateEvent(payload, validation.reason);
    const initial = castValue(payload.initial, payload.type);
    const current = state.has(payload.signal) ? state.get(payload.signal) : initial;
    if (payload.op === "set" && stateValuesEqual(current, validation.operand, payload.type)) {
      executorStats.dedupedEvents += 1;
      return true;
    }
    const next = applyLocalOperation(
      payload.op,
      payload.type,
      current,
      validation.operand,
    );
    if (next === undefined) return rejectStateEvent(payload, "executor-rejected");
    if (!types.has(payload.signal)) types.set(payload.signal, payload.type);
    setSignal(payload.signal, next, `event:${payload.event}`);
    return true;
  };

  const executeLocalEvent = (node, eventName, event) => {
    const prefix = `data-ax-on-${eventName}`;
    const rawSignal = node.getAttribute(`${prefix}-signal`);
    const op = node.getAttribute(`${prefix}-op`);
    if (!rawSignal || !op) return false;

    const signal = canonicalSignal(rawSignal);
    const type = node.getAttribute(`${prefix}-type`) || types.get(signal) || "String";
    const valueSource = node.getAttribute(`${prefix}-value-source`) || "literal";
    const target = event?.target;
    if (valueSource === "checked" && (!target || !("checked" in target))) {
      return rejectStateEvent({ event: eventName, signal, op, type }, "missing-checked-target");
    }
    if (valueSource === "value" && (!target || !("value" in target))) {
      return rejectStateEvent({ event: eventName, signal, op, type }, "missing-value-target");
    }
    const value = valueSource === "checked"
      ? !!target.checked
      : valueSource === "value"
        ? target.value
        : node.getAttribute(`${prefix}-value`);
    return executeStateEventPayload({
      protocol: node.getAttribute(`${prefix}-protocol`),
      event: eventName,
      signal,
      op,
      type,
      initial: node.getAttribute(`${prefix}-initial`),
      valueSource,
      value,
    });
  };

  const executeBoundEvent = (node, eventName) => {
    const signal = canonicalSignal(node.getAttribute("data-ax-signal"));
    const type = node.getAttribute("data-ax-state-type") || types.get(signal) || "String";
    const target = node.getAttribute("data-ax-bind") || "value";
    const valueSource = target === "checked" ? "checked" : "value";
    return executeStateEventPayload({
      protocol: node.getAttribute("data-ax-bind-protocol"),
      event: eventName,
      signal,
      op: "set",
      type,
      initial: readValue(node, target),
      valueSource,
      value: readValue(node, target),
    });
  };

  const applyPatch = (patch) => {
    if (!patch || !patch.signal) return undefined;
    const op = patch.op || "set";
    if (op !== "set") return undefined;
    return writeSignal(patch.signal, patch.value, patch.source || "patch", false);
  };

  const hydrateManifest = (manifest, source = "manifest") => {
    if (!manifest || !Array.isArray(manifest.files)) return 0;
    (manifest.types || []).forEach((schema) => {
      const hasFields = Array.isArray(schema?.fields);
      const hasLiterals = Array.isArray(schema?.literals)
        && schema.literals.length > 0
        && schema.literals.every((literal) => typeof literal === "string")
        && new Set(schema.literals).size === schema.literals.length;
      if (!schema || typeof schema.name !== "string" || (!hasFields && !hasLiterals)) return;
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(schema.name)) return;
      typeSchemas.set(schema.name, schema);
    });
    let count = 0;
    const pendingStorageCapabilities = [];
    manifest.files.forEach((file) => {
      (file.signals || []).forEach((signal) => {
        if (!signal || !signal.key) return;
        const meta = {
          key: signal.key,
          name: signal.name || "",
          scope: signal.scope || "",
          owner: signal.owner || "",
          ty: signal.ty || "String",
          file: file.file || "",
        };
        metadata.set(signal.key, meta);
        if (meta.name && !metadataByName.has(meta.name)) metadataByName.set(meta.name, signal.key);
        if (meta.ty && !types.has(signal.key)) types.set(signal.key, meta.ty);
        bindAlias(signal.key, signal.key);
        bindAlias(meta.name, signal.key);
        const component = meta.owner.startsWith("component:") ? meta.owner.slice("component:".length) : "";
        if (component && meta.name) bindAlias(`${component}.${meta.name}`, signal.key);
        const keyParts = String(signal.key).split(":");
        const index = keyParts[keyParts.length - 1] || "1";
        bindAlias(`root:${meta.name}:${index}`, signal.key);
        if (signal.persistence) {
          const capability = registerStorageCapability(signal.key, meta.ty, signal.persistence);
          if (capability) pendingStorageCapabilities.push(capability);
        }
        count += 1;
      });
    });
    rebindAliasedSignals();
    rebindPendingStorageWrites();
    registerReads();
    pendingStorageCapabilities.forEach(hydrateStorageCapability);
    settleStorageManifest();
    if (count > 0) {
      window.dispatchEvent(new CustomEvent("axonyx:state-manifest", {
        detail: { count, manifest, source },
      }));
    }
    return count;
  };

  const loadManifest = async (url = "/_ax/state/manifest.json") => {
    if (!window.fetch) {
      settleStorageManifest();
      return false;
    }
    try {
      const response = await fetch(url, {
        cache: "no-store",
        headers: stateRequestHeaders(),
      });
      if (!response.ok) {
        settleStorageManifest();
        return false;
      }
      const manifest = await response.json();
      return hydrateManifest(manifest, "manifest") > 0;
    } catch (_) {
      settleStorageManifest();
      return false;
    }
  };

  const hydrateSnapshot = (snapshot, source = "snapshot") => {
    if (!snapshot || !Array.isArray(snapshot.signals)) return 0;
    let count = 0;
    snapshot.signals.forEach((entry) => {
      if (!entry || !entry.key) return;
      const signal = canonicalSignal(entry.key);
      if (entry.ty && !types.has(signal)) types.set(signal, entry.ty);
      if (!hydratedStorageSignals.has(signal)) {
        writeSignal(signal, valueFromSnapshot(entry), source, false);
      }
      count += 1;
    });
    return count;
  };

  const loadSnapshot = async (url = "/_ax/state/snapshot.json") => {
    if (!window.fetch) return false;
    try {
      const response = await fetch(url, {
        cache: "no-store",
        headers: stateRequestHeaders(),
      });
      if (!response.ok) return false;
      const snapshot = await response.json();
      const count = hydrateSnapshot(snapshot, "snapshot");
      if (count > 0) {
        window.dispatchEvent(new CustomEvent("axonyx:state-snapshot", {
          detail: { url, count, snapshot },
        }));
      }
      return count > 0;
    } catch (_) {
      return false;
    }
  };

  const subscribe = (signal, listener) => {
    signal = canonicalSignal(signal);
    if (typeof listener !== "function") return () => {};
    if (!subscribers.has(signal)) subscribers.set(signal, new Set());
    subscribers.get(signal).add(listener);
    return () => subscribers.get(signal)?.delete(listener);
  };

  const describe = (signal) => {
    if (!signal) return undefined;
    signal = canonicalSignal(signal);
    const meta = metadata.get(signal);
    return {
      key: signal,
      value: state.get(signal),
      ty: types.get(signal) || meta?.ty || "String",
      meta,
      bindings: (bindings.get(signal) || []).length,
    };
  };

  const init = () => {
    document.querySelectorAll("[data-ax-signal]").forEach(register);
    registerReads();
    document.querySelectorAll("[data-ax-state-if-signal]").forEach(registerCondition);
    document.querySelectorAll("[data-ax-state-match-signal]").forEach(registerMatch);
    document.querySelectorAll("[data-ax-expression-protocol]").forEach(registerExpressions);
    document.querySelectorAll("ax-state-each[data-ax-each-protocol]").forEach(registerEach);
  };

  document.addEventListener("input", (event) => {
    const node = event.target.closest("[data-ax-signal]");
    if (!node) return;
    if (node.hasAttribute("data-ax-on-input-signal")) return;
    executeBoundEvent(node, "input");
  });

  document.addEventListener("change", (event) => {
    const node = event.target.closest("[data-ax-signal]");
    if (!node) return;
    if (node.hasAttribute("data-ax-on-change-signal")) return;
    executeBoundEvent(node, "change");
  });

  ["click", "input", "change"].forEach((eventName) => {
    document.addEventListener(eventName, (event) => {
      const node = event.target.closest(`[data-ax-on-${eventName}-signal]`);
      if (node) executeLocalEvent(node, eventName, event);
    });
  });

  window.__axonyx = window.__axonyx || {};
  window.__axonyx.state = {
    version: 1,
    protocol: "ax-state/1",
    tabId,
    get: (signal) => state.get(canonicalSignal(signal)),
    set: setSignal,
    subscribe,
    applyPatch,
    hydrateManifest,
    loadManifest,
    hydrateSnapshot,
    loadSnapshot,
    meta: (signal) => metadata.get(canonicalSignal(signal)),
    manifest: () => Array.from(metadata.values()),
    types: () => Array.from(typeSchemas.values()),
    describe,
    capabilities: () => domCapabilityList.map(({ signal, target, type, role }) => ({
      protocol: domCapabilityProtocol,
      signal,
      target,
      type,
      role,
    })),
    storageCapabilities: () => storageCapabilityList.map(({ signal, type, scope, key }) => ({
      protocol: storageCapabilityProtocol,
      signal,
      type,
      scope,
      key,
    })),
    snapshot: () => Object.fromEntries(state.entries()),
    runtime: () => executorMode,
    eventProtocol: stateEventProtocol,
    dispatch: executeStateEventPayload,
    validateValue: validateStateValueForType,
    validateEventPayload: validateStateEventPayload,
    diagnostics: () => ({
      protocol: stateEventProtocol,
      executor: executorMode,
      wasmOperations: executorStats.wasmOperations,
      fallbackOperations: executorStats.fallbackOperations,
      rejectedEvents: executorStats.rejectedEvents,
      dedupedEvents: executorStats.dedupedEvents,
      domProtocol: domCapabilityProtocol,
      registeredDomCapabilities: executorStats.registeredDomCapabilities,
      appliedDomWrites: executorStats.appliedDomWrites,
      rejectedDomCapabilities: executorStats.rejectedDomCapabilities,
      rejectedDomWrites: executorStats.rejectedDomWrites,
      storageProtocol: storageCapabilityProtocol,
      registeredStorageCapabilities: executorStats.registeredStorageCapabilities,
      restoredStorageValues: executorStats.restoredStorageValues,
      persistedStorageValues: executorStats.persistedStorageValues,
      rejectedStorageCapabilities: executorStats.rejectedStorageCapabilities,
      rejectedStorageReads: executorStats.rejectedStorageReads,
      rejectedStorageWrites: executorStats.rejectedStorageWrites,
      queuedStorageWrites: executorStats.queuedStorageWrites,
      flushedStorageWrites: executorStats.flushedStorageWrites,
      expressionProtocol: "ax-expression/1",
      expressionEvaluations: executorStats.expressionEvaluations,
      rejectedExpressions: executorStats.rejectedExpressions,
      eachProtocol: "ax-each/1",
      reconciledEachLists: executorStats.reconciledEachLists,
      rejectedEachLists: executorStats.rejectedEachLists,
      eachRefreshesRequired: executorStats.eachRefreshesRequired,
    }),
    loadWasm: loadWasmExecutor,
  };
  window.__axonyx.applyPatch = applyPatch;
  window.__axonyxStateBridge = window.__axonyx.state;

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
  (async () => {
    await loadWasmExecutor();
    await loadManifest();
    await loadSnapshot();
  })();
})();
</script>"##
}

fn render_html_attrs(head: &AxHead) -> String {
    let mut attrs = String::from(" lang=\"en\"");
    if let Some(theme) = &head.theme {
        attrs.push_str(" data-theme=\"");
        attrs.push_str(&escape_html(&head_expr_to_string(theme)));
        attrs.push('"');
    }
    attrs
}

fn render_head_html(head: &AxHead) -> String {
    let mut html = String::new();

    match &head.title {
        Some(title) => {
            html.push_str("<title>");
            html.push_str(&escape_html(&head_expr_to_string(title)));
            html.push_str("</title>");
        }
        None => html.push_str("<title>Axonyx Preview</title>"),
    }

    if head.theme_preflight {
        html.push_str(&render_theme_preflight_script(head));
    }

    for tag in &head.metas {
        html.push_str(&render_head_void_tag("meta", tag));
    }

    for tag in &head.links {
        html.push_str(&render_head_void_tag("link", tag));
    }

    for tag in &head.scripts {
        html.push_str(&render_head_script_tag(tag));
    }

    html
}

fn render_theme_preflight_script(head: &AxHead) -> String {
    let storage_key = head
        .theme_storage_key
        .as_ref()
        .map(head_expr_to_string)
        .unwrap_or_else(|| "axonyx-theme".to_string());
    let fallback_theme = head
        .theme
        .as_ref()
        .map(head_expr_to_string)
        .unwrap_or_else(|| "silver".to_string());

    format!(
        "<script>(function(){{try{{var k=\"{}\";var t=window.localStorage&&window.localStorage.getItem(k);if(!t)t=\"{}\";if(t)document.documentElement.setAttribute(\"data-theme\",t);}}catch(e){{}}}})();</script>",
        escape_js_string(&storage_key),
        escape_js_string(&fallback_theme)
    )
}

fn render_head_void_tag(tag: &str, head_tag: &AxHeadTag) -> String {
    let mut out = String::new();
    out.push('<');
    out.push_str(tag);
    push_head_attrs(head_tag, &mut out);
    out.push('>');
    out
}

fn render_head_script_tag(head_tag: &AxHeadTag) -> String {
    let mut out = String::new();
    out.push_str("<script");
    push_head_attrs(head_tag, &mut out);
    out.push_str("></script>");
    out
}

fn push_head_attrs(head_tag: &AxHeadTag, out: &mut String) {
    for attr in &head_tag.attrs {
        push_head_attr(attr, out);
    }
}

fn push_head_attr(attr: &AxProp, out: &mut String) {
    out.push(' ');
    out.push_str(&attr.name);
    out.push_str("=\"");
    out.push_str(&escape_html(&head_expr_to_string(&attr.value)));
    out.push('"');
}

fn head_expr_to_string(expr: &AxExpr) -> String {
    match expr {
        AxExpr::String(value) => value.clone(),
        AxExpr::Number(value) => value.to_string(),
        AxExpr::Float(value) => value.get().to_string(),
        AxExpr::Bool(value) => value.to_string(),
        AxExpr::List(items) => {
            let items = items
                .iter()
                .map(head_expr_to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{items}]")
        }
        AxExpr::Object(fields) => {
            let fields = fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", head_expr_to_string(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{fields}}}")
        }
        AxExpr::Identifier(value) => value.clone(),
        AxExpr::Unary { op, expr } => {
            format!(
                "{}{}",
                head_unary_op_to_string(*op),
                head_expr_to_string(expr)
            )
        }
        AxExpr::Binary { op, left, right } => format!(
            "{} {} {}",
            head_expr_to_string(left),
            head_binary_op_to_string(*op),
            head_expr_to_string(right)
        ),
        AxExpr::Index { object, index } => format!(
            "{}[{}]",
            head_index_object_expr_to_string(object),
            head_expr_to_string(index)
        ),
        AxExpr::Member { object, property } => {
            format!("{}.{}", head_expr_to_string(object), property)
        }
        AxExpr::OptionalMember { object, property } => {
            format!("{}?.{}", head_expr_to_string(object), property)
        }
        AxExpr::Call { path, args } => {
            let args = args
                .iter()
                .map(head_expr_to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({args})", path.join("."))
        }
    }
}

fn head_index_object_expr_to_string(expr: &AxExpr) -> String {
    let value = head_expr_to_string(expr);
    if head_index_object_needs_grouping(expr) {
        format!("({value})")
    } else {
        value
    }
}

fn head_index_object_needs_grouping(expr: &AxExpr) -> bool {
    matches!(expr, AxExpr::Binary { .. } | AxExpr::Unary { .. })
}

fn head_unary_op_to_string(op: AxUnaryOp) -> &'static str {
    match op {
        AxUnaryOp::Not => "!",
        AxUnaryOp::Neg => "-",
    }
}

fn head_binary_op_to_string(op: AxBinaryOp) -> &'static str {
    match op {
        AxBinaryOp::Add => "+",
        AxBinaryOp::Sub => "-",
        AxBinaryOp::Mul => "*",
        AxBinaryOp::Div => "/",
        AxBinaryOp::Rem => "%",
        AxBinaryOp::Eq => "==",
        AxBinaryOp::Ne => "!=",
        AxBinaryOp::Gt => ">",
        AxBinaryOp::Ge => ">=",
        AxBinaryOp::Lt => "<",
        AxBinaryOp::Le => "<=",
        AxBinaryOp::In => "in",
        AxBinaryOp::And => "&&",
        AxBinaryOp::Or => "||",
        AxBinaryOp::Fallback => "??",
    }
}

fn render_node(node: &AxNode, out: &mut String) {
    match node {
        AxNode::Text(text) => out.push_str(&escape_html(text)),
        AxNode::RawHtml(html) => out.push_str(html),
        AxNode::Element {
            tag,
            attrs,
            children,
        } => {
            out.push('<');
            out.push_str(tag);
            for attr in attrs {
                push_attr(attr, out);
            }
            out.push('>');
            for child in children {
                render_node(child, out);
            }
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
    }
}

fn push_attr(attr: &Attribute, out: &mut String) {
    out.push(' ');
    out.push_str(attr.name);
    out.push_str("=\"");
    out.push_str(&escape_html(&attr.value));
    out.push('"');
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_js_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('<', "\\u003c")
}

fn preview_styles() -> &'static str {
    r#"
        :root {
            color-scheme: dark;
            --ax-bg: #091019;
            --ax-surface: #0f1826;
            --ax-surface-2: #142132;
            --ax-border: rgba(163, 182, 207, 0.18);
            --ax-border-strong: rgba(163, 182, 207, 0.28);
            --ax-text: #f3f6fb;
            --ax-text-soft: #d7dfeb;
            --ax-text-muted: #99aabc;
            --ax-link: #c8a8ff;
            --ax-cyan: #88d5ff;
            --ax-card-shadow:
                0 0 0 1px rgba(163, 182, 207, 0.08),
                inset 0 1px 0 rgba(255, 255, 255, 0.03),
                0 14px 30px rgba(0, 0, 0, 0.28);
            --ax-card-surface:
                radial-gradient(circle at top left, rgba(120, 155, 220, 0.1), transparent 42%),
                linear-gradient(180deg, rgba(16, 27, 42, 0.98), rgba(7, 16, 25, 0.98));
        }

        * { box-sizing: border-box; }

        body {
            margin: 0;
            min-height: 100vh;
            font-family: "Segoe UI", Inter, sans-serif;
            background: var(--ax-bg);
            color: var(--ax-text);
        }

        [data-ax-root="page"] {
            min-height: 100vh;
            padding: 48px 20px 72px;
        }

        .ax-container {
            width: min(100% - 2rem, 88rem);
            margin: 0 auto;
        }

        .ax-container[data-max="sm"] { max-width: 42rem; }
        .ax-container[data-max="md"] { max-width: 56rem; }
        .ax-container[data-max="lg"] { max-width: 72rem; }
        .ax-container[data-max="xl"] { max-width: 88rem; }

        .ax-grid {
            display: grid;
        }

        .ax-grid[data-cols="1"] { grid-template-columns: 1fr; }
        .ax-grid[data-cols="2"] { grid-template-columns: repeat(2, minmax(0, 1fr)); }
        .ax-grid[data-cols="3"] { grid-template-columns: repeat(3, minmax(0, 1fr)); }
        .ax-grid[data-cols="4"] { grid-template-columns: repeat(4, minmax(0, 1fr)); }
        .ax-grid[data-cols="5"] { grid-template-columns: repeat(5, minmax(0, 1fr)); }
        .ax-grid[data-cols="6"] { grid-template-columns: repeat(6, minmax(0, 1fr)); }

        .ax-grid[data-gap="sm"] { gap: 0.75rem; }
        .ax-grid[data-gap="md"] { gap: 1rem; }
        .ax-grid[data-gap="lg"] { gap: 1.5rem; }
        .ax-grid[data-gap="xl"] { gap: 2rem; }

        .ax-card {
            padding: 24px;
            border-radius: 24px;
            border: 1px solid var(--ax-border);
            background: var(--ax-card-surface);
            box-shadow: var(--ax-card-shadow);
        }

        .ax-card[data-recipe="hero-card"] {
            padding: 32px;
        }

        .ax-card__title {
            margin: 0 0 0.85rem;
            color: var(--ax-text);
            font-size: clamp(1.5rem, 2vw, 2.15rem);
            line-height: 1.05;
            font-weight: 800;
            letter-spacing: -0.03em;
        }

        .ax-copy {
            margin: 0 0 0.85rem;
            color: var(--ax-text-soft);
            font-size: 1rem;
            line-height: 1.55;
        }

        .ax-copy[data-tone="lead"] {
            font-size: 1.06rem;
            line-height: 1.6;
            color: var(--ax-text);
        }

        .ax-copy[data-tone="eyebrow"] {
            color: var(--ax-cyan);
            font-size: 0.82rem;
            font-weight: 700;
            text-transform: uppercase;
            letter-spacing: 0.14em;
        }

        .ax-copy[data-tone="muted"] {
            color: var(--ax-text-muted);
            font-size: 0.95rem;
        }

        a {
            color: var(--ax-link);
            text-decoration: underline;
            text-underline-offset: 0.14em;
        }

        .ax-button {
            display: inline-flex;
            align-items: center;
            justify-content: center;
            min-height: 44px;
            padding: 0 16px;
            border: 0;
            border-radius: 999px;
            background: color-mix(in srgb, var(--ax-cyan) 80%, white);
            color: #082032;
            font-weight: 700;
        }

        .docs-nav {
            display: flex;
            flex-wrap: wrap;
            gap: 0.75rem 1rem;
        }

        [data-recipe="app-shell"] {
            display: grid;
            gap: 1.25rem;
        }

        [data-recipe="hello-shell"] {
            gap: 1.5rem;
        }

        [data-recipe="app-frame"] {
            gap: 1.25rem;
        }

        img {
            max-width: 100%;
            height: auto;
        }

        .ax-card > *:last-child,
        .ax-copy:last-child {
            margin-bottom: 0;
        }

        @media (max-width: 900px) {
            .ax-grid[data-cols="2"],
            .ax-grid[data-cols="3"],
            .ax-grid[data-cols="4"],
            .ax-grid[data-cols="5"],
            .ax-grid[data-cols="6"] {
                grid-template-columns: 1fr;
            }
        }

        @media (prefers-reduced-motion: no-preference) {
            .ax-card,
            .ax-copy,
            .ax-button,
            a {
                transition:
                    transform 180ms ease,
                    border-color 180ms ease,
                    color 180ms ease,
                    background-color 180ms ease;
            }
        }

        .ax-card__title,
        .ax-copy[data-tone="lead"] {
            margin-bottom: 14px;
        }

        .ax-form {
            display: grid;
            gap: 12px;
            margin-top: 12px;
        }

        .ax-input,
        .ax-textarea {
            width: 100%;
            padding: 14px 16px;
            border-radius: 16px;
            border: 1px solid var(--ax-border);
            background: rgba(15, 23, 42, 0.72);
            color: var(--ax-text);
            font: inherit;
        }

        .ax-textarea {
            min-height: 128px;
            resize: vertical;
        }
    "#
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axonyx_core::compile_pipeline;

    use super::*;

    #[test]
    fn preview_inputs_preserve_finite_float_values() {
        let from_text = coerce_preview_input_value("ratio", "f64", "0.625".to_string())
            .expect("finite decimal input should coerce");
        let from_default =
            coerce_preview_default_input_value("ratio", "f64", &AxRustExpr::new("0.625_f64"))
                .expect("lowered Float default should evaluate");

        assert!(matches!(from_text, AxValue::Float(value) if value.get() == 0.625));
        assert!(matches!(from_default, AxValue::Float(value) if value.get() == 0.625));
        assert!(coerce_preview_input_value("ratio", "f64", "NaN".to_string()).is_err());
    }

    #[test]
    fn renders_compiled_page_ast_with_fresh_loader_values() {
        let document = parse_ax_auto(
            r#"page Posts() {
data posts = loadPosts()
return ASX {
  <Each items={posts} as="post">
    <article>{post.title}</article>
  </Each>
}
}"#,
        )
        .expect("page should parse");
        let document_json = serde_json::to_string(&document).expect("document should serialize");
        let loader_values = BTreeMap::from([(
            compiled_loader_call_key("loadPosts", &[]),
            json!([{ "title": "Fresh from compiled loader" }]),
        )]);

        let html = render_compiled_page_fragment(
            &document_json,
            &[],
            "/posts",
            &BTreeMap::new(),
            &loader_values,
        )
        .expect("compiled page should render");

        assert!(html.starts_with("<main"));
        assert!(html.contains("data-ax-root=\"page\""));
        assert!(html.contains("Fresh from compiled loader"));
    }

    #[test]
    fn renders_compiled_page_ast_with_distinct_parameterized_loader_calls() {
        let document = parse_ax_auto(
            r#"page Posts() {
data first = loadPost(params.first)
data second = loadPost("second")
return ASX { <><Copy>{first.title}</Copy><Copy>{second.title}</Copy></> }
}"#,
        )
        .expect("page should parse");
        let document_json = serde_json::to_string(&document).expect("document should serialize");
        let loader_values = BTreeMap::from([
            (
                compiled_loader_call_key("loadPost", &[json!("first")]),
                json!({ "title": "First result" }),
            ),
            (
                compiled_loader_call_key("loadPost", &[json!("second")]),
                json!({ "title": "Second result" }),
            ),
        ]);

        let html = render_compiled_page_fragment(
            &document_json,
            &[],
            "/posts/first",
            &BTreeMap::from([("first".to_string(), "first".to_string())]),
            &loader_values,
        )
        .expect("compiled parameterized page should render");

        assert!(html.contains("First result"));
        assert!(html.contains("Second result"));
    }

    #[test]
    fn builds_render_plan_from_ir() {
        let ir = compile_pipeline(r#"db.posts.all() |> layout.Grid(3) |> Card()"#)
            .expect("pipeline should compile");
        let plan = execute(&ir);

        assert_eq!(plan.source, "posts");
        assert_eq!(plan.layout.kind, "grid");
        assert_eq!(plan.layout.columns, 3);
        assert_eq!(plan.view.component, "Card");
    }

    #[test]
    fn builds_render_plan_from_json() {
        let ir = compile_pipeline(r#"db.users.all() |> layout.Grid(2) |> ProfileCard()"#)
            .expect("pipeline should compile");
        let ir_json = serde_json::to_string(&ir).expect("serialize");
        let plan = execute_json(&ir_json).expect("json execution should work");

        assert_eq!(plan.source, "users");
        assert_eq!(plan.layout.columns, 2);
        assert_eq!(plan.view.component, "ProfileCard");
    }

    #[test]
    fn extracts_route_hooks_from_backend_handler_plan() {
        let document = parse_backend_ax(
            r#"
route GET "/api/admin"
  before Auth.session
  before Security.headers
  after Cache.noStore
  return json("ok")
"#,
        )
        .expect("backend source should parse");
        let plan = lower_backend_document(&document).expect("backend source should lower");

        let hooks = route_hooks_from_handler_plan(&plan.handlers[0]);

        assert_eq!(
            hooks,
            vec![
                server::AxRouteHook::new(server::AxMiddlewarePhase::Before, "Auth.session"),
                server::AxRouteHook::new(server::AxMiddlewarePhase::Before, "Security.headers"),
                server::AxRouteHook::new(server::AxMiddlewarePhase::After, "Cache.noStore"),
            ]
        );
    }

    #[test]
    fn previews_static_ax_page_as_html_document() {
        let html = preview_ax_page(
            r#"
page Home
  Container max: "xl", recipe: "hello-shell"
    Card title: "Hello Axonyx", recipe: "hero-card"
      Copy tone: "lead" -> "A Rust-first page preview."
      Button tone: "primary" -> "Edit app/page.ax"
"#,
        )
        .expect("preview should render");

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("data-recipe=\"hero-card\""));
        assert!(html.contains("Edit app/page.ax"));
        assert!(html.contains("class=\"ax-container\""));
        assert!(html.contains("class=\"ax-card__title\""));
    }

    #[test]
    fn previews_static_ax_page_as_streaming_html_response() {
        let response = preview_ax_page_stream_response(
            r#"
page Home
<Container max="xl">
  <Card title="Stream Ready">
    <Copy>Chunked preview path</Copy>
  </Card>
</Container>
"#,
        )
        .expect("streaming preview should render");

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "text/html; charset=utf-8");
        assert!(response.body.is_streaming());

        let chunks = response.body.chunks_iter().collect::<Vec<_>>();
        assert!(chunks.len() >= 4);

        let html = chunks
            .iter()
            .map(|chunk| String::from_utf8_lossy(chunk))
            .collect::<String>();

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Stream Ready"));
        assert!(html.contains("Chunked preview path"));
        assert!(html.contains("</body></html>"));
    }

    #[test]
    fn builds_route_definition_from_ax_page_source() {
        let route = ax_page_route_definition(
            "GET",
            "/",
            r#"
page Home
<Container max="xl">
  <Card title="Route Page">
    <Copy>Served through AxRouteDefinition</Copy>
  </Card>
</Container>
"#,
        )
        .expect("page route should build");

        assert!(route.matches(&server::AxHttpRequest::new("GET", "/")));

        let response = route.handle(server::AxHttpRequest::new("GET", "/"), None);

        assert_eq!(response.status, 200);
        assert!(response.body.is_streaming());

        let html = String::from_utf8(response.body.into_bytes()).expect("HTML should be UTF-8");
        assert!(html.contains("Route Page"));
        assert!(html.contains("Served through AxRouteDefinition"));
    }

    #[test]
    fn builds_route_definition_from_ax_page_with_layout_and_loaders() {
        let store = AxPreviewStore::default();
        let route = ax_page_route_definition_with_backend(
            "GET",
            "/blog",
            &[r#"
page Docs
  Container max: "xl", recipe: "docs-shell"
    Copy tone: "eyebrow" -> "Docs Layout"
    Slot
"#],
            &[r#"
loader PostsList
  data posts = db.posts.all()
    where status = "published"
    limit 1
  return posts
"#],
            &[],
            r#"
page Blog
<Each item="post" in={load PostsList}>
  <Card title={post.title}>
    <Copy>{post.excerpt}</Copy>
  </Card>
</Each>
"#,
            "/blog",
            &store,
        )
        .expect("page route with backend context should build");

        let response = route.handle(server::AxHttpRequest::new("GET", "/blog"), None);

        assert_eq!(response.status, 200);
        assert!(response.body.is_streaming());

        let html = String::from_utf8(response.body.into_bytes()).expect("HTML should be UTF-8");
        assert!(html.contains("Docs Layout"));
        assert!(html.contains("data-recipe=\"docs-shell\""));
        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("A fast page rendered from .ax"));
    }

    #[test]
    fn previews_jsx_like_ax_page_as_html_document() {
        let html = preview_ax_page(
            r#"
page Home
<Head>
  <Title>{"Hello Axonyx"}</Title>
  <Theme>silver</Theme>
  <Meta name="description" content="Docs without bloat" />
</Head>
<Container max="xl">
  <Card title="Runtime V2">
    <Copy>Body from JSX-like .ax</Copy>
  </Card>
</Container>
"#,
        )
        .expect("jsx-like preview should render");

        assert!(html.contains("<title>Hello Axonyx</title>"));
        assert!(html.contains("data-theme=\"silver\""));
        assert!(html.contains("<meta name=\"description\" content=\"Docs without bloat\">"));
        assert!(html.contains("Runtime V2"));
        assert!(html.contains("Body from JSX-like .ax"));
    }

    #[test]
    fn theme_preflight_script_renders_before_head_links() {
        let html = preview_ax_page(
            r#"
page Home
<Head>
  <Title>Theme Test</Title>
  <Theme default="silver" storageKey="axonyx-site-theme" preflight />
  <Link rel="stylesheet" href="/css/site.css" />
</Head>
<Copy>Body</Copy>
"#,
        )
        .expect("theme preflight preview should render");

        let script_index = html
            .find("localStorage.getItem(k)")
            .expect("preflight script should read storage");
        let link_index = html
            .find("<link rel=\"stylesheet\" href=\"/css/site.css\">")
            .expect("stylesheet link should render");

        assert!(html.contains("var k=\"axonyx-site-theme\""));
        assert!(html.contains("if(!t)t=\"silver\""));
        assert!(script_index < link_index);
    }

    #[test]
    fn preview_injects_behavior_runtime_only_when_behavior_hooks_exist() {
        let static_html = preview_ax_page(
            r#"
page Home
<Button>Plain</Button>
"#,
        )
        .expect("static preview should render");

        assert!(!static_html.contains("data-ax-runtime=\"behavior\""));

        let interactive_html = preview_ax_page(
            r##"
page Home
<Button behavior="toggle" behaviorTarget="#menu">Menu</Button>
<nav id="menu" hidden="true">Links</nav>
"##,
        )
        .expect("interactive preview should render");

        assert!(interactive_html.contains("data-ax-behavior=\"toggle\""));
        assert!(interactive_html.contains("data-ax-behavior-target=\"#menu\""));
        assert!(interactive_html.contains("data-ax-runtime=\"behavior\""));
        assert!(interactive_html.contains("window.__axonyxBehaviorRuntime"));
        assert!(interactive_html.contains("aria-controls"));
        assert!(interactive_html.contains("DOMContentLoaded"));

        let themed_html = preview_ax_page(
            r##"
page Home
<select data-ax-behavior="theme" data-ax-theme-storage-key="demo-theme">
  <option value="silver">Silver</option>
  <option value="bronze">Bronze</option>
  <option value="gold">Gold</option>
</select>
"##,
        )
        .expect("theme behavior preview should render");

        assert!(themed_html.contains("data-ax-behavior=\"theme\""));
        assert!(themed_html.contains("data-ax-theme-storage-key=\"demo-theme\""));
        assert!(themed_html.contains("allowedThemes"));
        assert!(themed_html.contains("localStorage.setItem"));
    }

    #[test]
    fn preview_injects_state_bridge_only_when_signal_hooks_exist() {
        let static_html = preview_ax_page(
            r#"
page Home
<input value="silver" />
"#,
        )
        .expect("static preview should render");

        assert!(!static_html.contains("data-ax-runtime=\"state-bridge\""));

        let state_html = preview_ax_page(
            r#"
page Home
<input data-ax-signal="root:theme:1" data-ax-bind="value" value="silver" />
<span data-ax-signal="root:theme:1" data-ax-bind="text">silver</span>
"#,
        )
        .expect("state bridge preview should render");

        assert!(state_html.contains("data-ax-signal=\"root:theme:1\""));
        assert!(state_html.contains("data-ax-bind=\"value\""));
        assert!(state_html.contains("data-ax-runtime=\"state-bridge\""));
        assert!(state_html.contains("axonyx:state-patch"));
        assert!(state_html.contains("axonyx:state-change"));
        assert!(state_html.contains("window.__axonyx.state"));
        assert!(state_html.contains("protocol: \"ax-state/1\""));
        assert!(state_html.contains("tabId"));
        assert!(state_html.contains("axonyx:tab-id"));
        assert!(state_html.contains("X-Axonyx-State-Protocol"));
        assert!(state_html.contains("X-Axonyx-Tab"));
        assert!(state_html.contains("applyPatch"));
        assert!(state_html.contains("hydrateManifest"));
        assert!(state_html.contains("validateStateValueForType"));
        assert!(state_html.contains("typeSchemas.set(schema.name, schema)"));
        assert!(state_html.contains("types: () => Array.from(typeSchemas.values())"));
        assert!(state_html.contains("loadManifest"));
        assert!(state_html.contains("/_ax/state/manifest.json"));
        assert!(state_html.contains("axonyx:state-manifest"));
        assert!(state_html.contains("hydrateSnapshot"));
        assert!(state_html.contains("loadSnapshot"));
        assert!(state_html.contains("/_ax/state/snapshot.json"));
        assert!(state_html.contains("axonyx:state-snapshot"));
        assert!(state_html.contains("canonicalSignal"));
        assert!(state_html.contains("componentSignalAlias"));
        assert!(state_html.contains("aliases.get(`${component}.${name}`)"));
        assert!(state_html.contains("rebindAliasedSignals"));
        assert!(state_html.contains("bindAlias(`${component}.${meta.name}`, signal.key)"));
        assert!(state_html.contains("bindAlias(`root:${meta.name}:${index}`, signal.key)"));
        assert!(state_html.contains("meta: (signal) => metadata.get(canonicalSignal(signal))"));
        assert!(state_html.contains("manifest: () => Array.from(metadata.values())"));
        assert!(state_html.contains("describe"));
        assert!(state_html.contains("bindings: (bindings.get(signal) || []).length"));
        assert!(state_html.contains("subscribe"));
        assert!(state_html.contains("window.__axonyxStateBridge"));
        assert!(state_html.contains(AX_STATE_WASM_PATH));
        assert!(state_html.contains("ax_state_apply_number"));
        assert!(state_html.contains("ax_state_apply_bool"));
        assert!(state_html.contains("ax_state_apply_string"));
        assert!(state_html.contains("ax_state_supports_operation"));
        assert!(state_html.contains("exports.ax_state_abi_version?.() !== 3"));
        assert!(state_html.contains("exports.ax_state_apply_value"));
        assert!(state_html.contains("runtime: () => executorMode"));
        assert!(state_html.contains("stateEventProtocol = \"ax-state-event/1\""));
        assert!(state_html.contains("domCapabilityProtocol = \"ax-dom-capability/1\""));
        assert!(state_html.contains("registerDomCapability"));
        assert!(state_html.contains("writeDomCapability"));
        assert!(state_html.contains("axonyx:dom-capability-rejected"));
        assert!(state_html.contains("capabilities: () => domCapabilityList.map"));
        assert!(state_html.contains("storageCapabilityProtocol = \"ax-storage-capability/1\""));
        assert!(state_html.contains("storageValueProtocol = \"ax-storage-value/1\""));
        assert!(state_html.contains("registerStorageCapability"));
        assert!(state_html.contains("restoreStorageCapability"));
        assert!(state_html.contains("persistStorageCapability"));
        assert!(state_html.contains("pendingStorageWrites"));
        assert!(state_html.contains("rebindPendingStorageWrites"));
        assert!(state_html.contains("hydrateStorageCapability"));
        assert!(state_html.contains("queuedStorageWrites: executorStats.queuedStorageWrites"));
        assert!(state_html.contains("flushedStorageWrites: executorStats.flushedStorageWrites"));
        assert!(state_html.contains("axonyx:storage-capability-rejected"));
        assert!(state_html.contains("axonyx:storage-value-rejected"));
        assert!(state_html.contains("storageCapabilities: () => storageCapabilityList.map"));
        assert!(state_html.contains("validateStateEventPayload"));
        assert!(state_html.contains("axonyx:state-event-rejected"));
        assert!(state_html.contains("if (node.hasAttribute(\"data-ax-on-input-signal\")) return"));
        assert!(state_html.contains("diagnostics: () => ({"));
        assert!(state_html.contains("wasmOperations: executorStats.wasmOperations"));
        assert!(state_html.contains("rejectedEvents: executorStats.rejectedEvents"));
        assert!(state_html.contains("dedupedEvents: executorStats.dedupedEvents"));
        assert!(state_html.contains(
            "payload.op === \"set\" && stateValuesEqual(current, validation.operand, payload.type)"
        ));
    }

    #[test]
    fn bundled_state_executor_is_a_wasm_v2_module() {
        let bytes = ax_state_wasm_bytes();

        assert!(bytes.starts_with(b"\0asm"));
        assert_eq!(&bytes[4..8], &[1, 0, 0, 0]);
    }

    #[test]
    fn preview_injects_state_bridge_for_named_state_reads() {
        let html = preview_ax_page(
            r#"
page Home

<span data-ax-state-name="packageVersions">Loading...</span>
"#,
        )
        .expect("named state read preview should render");

        assert!(html.contains("data-ax-state-name=\"packageVersions\""));
        assert!(html.contains("data-ax-runtime=\"state-bridge\""));
        assert!(html.contains("metadataByName"));
        assert!(html.contains("readBindings"));
        assert!(html.contains("registerRead"));
        assert!(html.contains("data-ax-state-key"));
        assert!(html.contains("data-ax-state-source"));
    }

    #[test]
    fn preview_lowers_state_signal_syntax_into_bridge_metadata() {
        let html = preview_ax_page(
            r#"
page Home

state theme = "silver"
state count: Number = 0

<input bind:value={theme} />
<span bind:text={theme}>{theme}</span>
<input bind:value={count} />
"#,
        )
        .expect("state syntax preview should render");

        assert!(html.contains("data-ax-signal=\"root:theme:1\""));
        assert!(html.contains("data-ax-bind=\"value\""));
        assert!(html.contains("data-ax-state-type=\"String\""));
        assert!(html.contains("data-ax-dom-protocol=\"ax-dom-capability/1\""));
        assert!(html.contains("data-ax-dom-write=\"value\""));
        assert!(html.contains("value=\"silver\""));
        assert!(html.contains("data-ax-bind=\"text\""));
        assert!(html.contains("data-ax-dom-write=\"text\""));
        assert!(html.contains(">silver</span>"));
        assert!(html.contains("data-ax-state-type=\"Number\""));
        assert!(html.contains("value=\"0\""));
        assert!(html.contains("data-ax-runtime=\"state-bridge\""));
        assert!(html.contains("castValue"));
    }

    #[test]
    fn preview_lowers_local_state_events_without_inline_javascript() {
        let html = preview_ax_page(
            r#"
page Counter() {
  state count: Number = 0

  return ASX {
    <>
      <Button on:click={count += 1}>Increase</Button>
      <Copy>{count}</Copy>
    </>
  }
}
"#,
        )
        .expect("local state event preview should render");

        assert!(html.contains("data-ax-on-click-signal=\"root:count:1\""));
        assert!(html.contains("data-ax-on-click-op=\"add\""));
        assert!(html.contains("data-ax-on-click-value=\"1\""));
        assert!(html.contains("data-ax-signal=\"root:count:1\""));
        assert!(html.contains("data-ax-bind=\"text\""));
        assert!(html.contains("executeLocalEvent"));
        assert!(!html.contains("onclick="));
    }

    #[test]
    fn preview_compiles_reactive_expressions_for_wasm_evaluation() {
        let html = preview_ax_page(
            r#"
page Counter() {
  state count: Number = 2
  state limit: Number = 5

  return ASX {
    <>
      <button on:click={count += 1}>Increase</button>
      <Copy>{count * 2}</Copy>
      <button disabled={count >= limit}>Locked</button>
    </>
  }
}
"#,
        )
        .expect("reactive expression preview should render");

        assert!(html.contains("<ax-expression"));
        assert!(html.contains("data-ax-expression-protocol=\"ax-expression/1\""));
        assert!(html.contains("data-ax-expression-0-target=\"text\""));
        assert!(html.contains("data-ax-expression-0-target=\"boolean:disabled\""));
        assert!(html.contains("data-ax-expression-0-program="));
        assert!(html.contains("data-ax-expression-0-signals="));
        assert!(html.contains("exports.ax_state_evaluate_expression"));
        assert!(html.contains("registerExpressions"));
        assert!(!html.contains("disabled=\"false\""));
    }

    #[test]
    fn preview_inlines_pure_functions_into_wasm_reactive_expressions() {
        let html = preview_ax_page(
            r#"
page Counter() {
  state count: Int = 2
  state limit: Int = 5
  fn double(value: Int) = value * 2
  fn reached(value: Int, maximum: Int) = value >= maximum

  return ASX {
    <>
      <button on:click={count += 1}>Increase</button>
      <Copy>{double(count)}</Copy>
      <button disabled={reached(count, limit)}>Locked</button>
    </>
  }
}
"#,
        )
        .expect("pure reactive function preview should render");

        assert!(html.contains(">4</ax-expression>"));
        assert!(html.contains("data-ax-expression-0-target=\"text\""));
        assert!(html.contains("data-ax-expression-0-target=\"boolean:disabled\""));
        assert!(html.contains("data-ax-expression-0-signals="));
        assert!(html.contains("exports.ax_state_evaluate_expression"));
        assert!(!html.contains("disabled=\"false\""));
    }

    #[test]
    fn preview_renders_reactive_collection_literals_for_server_and_wasm() {
        let html = preview_ax_page(
            r#"
page CollectionProbe() {
  state count: Int = 2
  state limit: Int = 3
  return ASX {
    <>
      <Copy>{count in [1, 2, 3]}</Copy>
      <button disabled={({active: count >= limit}).active}>Locked</button>
    </>
  }
}
"#,
        )
        .expect("reactive collection preview should render");

        assert!(html.contains(">true</ax-expression>"));
        assert!(html.contains("data-ax-expression-0-target=\"text\""));
        assert!(html.contains("data-ax-expression-0-target=\"boolean:disabled\""));
        assert!(html.contains("3203001f"));
        assert!(html.contains("33010006000000616374697665"));
        assert!(html.contains("exports.ax_state_evaluate_expression"));
        assert!(!html.contains("disabled=\"false\""));
    }

    #[test]
    fn preview_emits_keyed_each_ownership_protocol_for_state_lists() {
        let html = preview_ax_page(
            r#"
page Posts() {
  state posts = [{ id: "first", title: "Hello" }, { id: "second", title: "World" }]
  return ASX {
    <Each items={posts} as="post" key={post.id}>
      <Copy>{post.title}</Copy>
    </Each>
  }
}
"#,
        )
        .expect("keyed state each preview should render");

        assert!(html.contains("<ax-state-each"));
        assert!(html.contains("data-ax-each-protocol=\"ax-each/1\""));
        assert!(html.contains("data-ax-each-signal=\"root:posts:1\""));
        assert!(html.contains("data-ax-each-key-path=\"id\""));
        assert!(html.contains("data-ax-each-key-kind=\"string\""));
        assert!(html.contains("data-ax-each-key=\"first\""));
        assert!(html.contains("data-ax-each-key-id=\"string:first\""));
        assert!(html.contains("data-ax-each-key=\"second\""));
        assert!(html.contains("data-ax-each-render-protocol=\"ax-each-render/1\""));
        assert!(html.contains("data-ax-each-render-status=\"ready\""));
        assert!(html.contains("data-ax-each-render-target=\"text\""));
        assert!(html.contains("data-ax-each-render-path=\"title\""));
        assert!(html.contains("ax_state_reconcile_keys"));
        assert!(html.contains("axonyx:each-refresh-required"));
        assert!(html.contains("Hello"));
        assert!(html.contains("World"));
    }

    #[test]
    fn preview_preserves_state_dependent_if_branches_for_local_updates() {
        let html = preview_ax_page(
            r#"
page Counter() {
  state count: Number = 0
  return ASX {
    <If when={count > 5}>
      <Badge>High value</Badge>
      <Else><Copy>Low value</Copy></Else>
    </If>
  }
}
"#,
        )
        .expect("state condition preview should render");

        assert!(html.contains("data-ax-state-if-signal=\"root:count:1\""));
        assert!(html.contains("data-ax-state-if-op=\"gt\""));
        assert!(html.contains("data-ax-state-if-value=\"5\""));
        assert!(html.contains("data-ax-state-if-branch=\"then\""));
        assert!(html.contains("data-ax-state-if-branch=\"else\""));
        assert!(html.contains("High value"));
        assert!(html.contains("Low value"));
        assert!(html.contains("registerCondition"));
    }

    #[test]
    fn preview_preserves_state_dependent_match_branches_for_local_updates() {
        let html = preview_ax_page(
            r#"
page ThemePreview() {
  type Theme = "silver" | "bronze" | "gold"
  state theme: Theme = "silver"
  return ASX {
    <>
      <Button on:click={theme = "gold"}>Gold</Button>
      <Match value={theme}>
        <Case is="silver"><Copy>Silver preview</Copy></Case>
        <Case is="bronze"><Copy>Bronze preview</Copy></Case>
        <Case is="gold"><Copy>Gold preview</Copy></Case>
        <Default><Copy>Custom preview</Copy></Default>
      </Match>
    </>
  }
}
"#,
        )
        .expect("state match preview should render");

        assert!(html.contains("data-ax-state-match-signal=\"root:theme:1\""));
        assert!(html.contains("data-ax-state-match-type=\"Theme\""));
        assert!(html.contains("data-ax-state-match-literals="));
        assert!(html.contains("data-ax-state-match-value=\"silver\""));
        assert!(html.contains("data-ax-state-match-value=\"gold\""));
        assert!(html.contains("data-ax-state-match-branch=\"default\""));
        assert!(html.contains("Silver preview"));
        assert!(html.contains("Gold preview"));
        assert!(html.contains("updateMatch"));
        assert!(html.contains("registerMatch"));
    }

    #[test]
    fn previews_jsx_like_mixed_children_and_fragment_without_wrapper() {
        let html = preview_ax_page(
            r#"
page Home
<>
  <p>
    Hello <strong>Axonyx</strong>
  </p>
</>
"#,
        )
        .expect("mixed children preview should render");

        assert!(html.contains("<p>Hello<strong>Axonyx</strong></p>"));
        assert!(!html.contains("data-component=\"Fragment\""));
    }

    #[test]
    fn previews_html_primitive_as_raw_content() {
        let html = preview_ax_page(
            r#"
page Home

<Html content={"<h2>Rendered markdown</h2><p>Safe build output</p>"} />
"#,
        )
        .expect("html primitive should render");

        assert!(html.contains(
            "<div class=\"ax-html\"><h2>Rendered markdown</h2><p>Safe build output</p></div>"
        ));
        assert!(!html.contains("&lt;h2&gt;Rendered markdown&lt;/h2&gt;"));
    }

    #[test]
    fn previews_jsx_like_each_and_if_controls() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
loader PostsList
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 2
  return posts
"#],
            r#"
page Posts
<If when={false}>
  <Copy>Hidden</Copy>
</If>
<Each item="post" in={load PostsList}>
  <Card title={post.title}>
    <Copy>{post.excerpt}</Copy>
  </Card>
</Each>
"#,
        )
        .expect("jsx-like control preview should render");

        assert!(!html.contains("Hidden"));
        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("Docs Without Bloat"));
        assert!(!html.contains("Draft Preview"));
    }

    #[test]
    fn previews_jsx_like_else_and_empty_controls() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
loader EmptyPosts
  data posts = db.posts.all()
    where status = "archived"
  return posts
"#],
            r#"
page Posts
<If when={false}>
  <Copy>Visible when true</Copy>
  <Else>
    <Copy>Else branch</Copy>
  </Else>
</If>
<Each item="post" in={load EmptyPosts}>
  <Card title={post.title} />
  <Empty>
    <Copy>No posts yet</Copy>
  </Empty>
</Each>
"#,
        )
        .expect("jsx-like else and empty preview should render");

        assert!(html.contains("Else branch"));
        assert!(html.contains("No posts yet"));
        assert!(!html.contains("Visible when true"));
    }

    #[test]
    fn previews_imported_component_with_resolution_metadata() {
        let html = preview_ax_page(
            r#"
import { Card as SiteCard } from "@/ui"

page Home
<SiteCard title="Hello import" />
"#,
        )
        .expect("imported component preview should render");

        assert!(html.contains("data-import-source=\"@/ui\""));
        assert!(html.contains("data-import-name=\"Card\""));
        assert!(html.contains("data-import-local=\"SiteCard\""));
    }

    #[test]
    fn previews_imported_component_from_resolved_source() {
        let resolver = |source: &str| match source {
            "@axonyx/ui/foundry/SectionCard.ax" => Some(
                r#"
page SectionCard
<Card title={title}>
  <Slot />
</Card>
"#
                .to_string(),
            ),
            _ => None,
        };

        let html = preview_ax_page_with_imports(
            r#"
import { SectionCard } from "@axonyx/ui/foundry/SectionCard.ax"

page Home
<SectionCard title="Imported title">
  <Copy>Imported body</Copy>
</SectionCard>
"#,
            &resolver,
        )
        .expect("resolved imported component preview should render");

        assert!(html.contains("Imported title"));
        assert!(html.contains("Imported body"));
        assert!(!html.contains("data-import-source"));
    }

    #[test]
    fn previews_layout_and_page_as_one_html_document() {
        let html = preview_ax_app(
            Some(
                r#"
page RootLayout
  theme "bronze"
  Container max: "xl", recipe: "app-shell"
    Copy tone: "eyebrow" -> "Axonyx Layout"
    Slot
"#,
            ),
            r#"
page Home
  Card title: "Hello Axonyx"
    Copy -> "Page content"
"#,
        )
        .expect("layout preview should render");

        assert!(html.contains("Axonyx Layout"));
        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("Page content"));
        assert!(html.contains("data-ax-page=\"Home\""));
        assert!(html.contains("<html lang=\"en\" data-theme=\"bronze\">"));
        assert!(!html.contains("data-component=\"Slot\""));
    }

    #[test]
    fn appends_page_when_layout_has_no_slot() {
        let html = preview_ax_app(
            Some(
                r#"
page RootLayout
  Copy -> "Layout only"
"#,
            ),
            r#"
page Home
  Copy -> "Page body"
"#,
        )
        .expect("layout without slot should still render");

        assert!(html.contains("Layout only"));
        assert!(html.contains("Page body"));
    }

    #[test]
    fn previews_route_with_nested_layouts() {
        let html = preview_ax_route(
            &[
                r#"
page RootLayout
  Container max: "xl", recipe: "app-shell"
    Copy tone: "eyebrow" -> "Root Layout"
    Slot
"#,
                r#"
page DocsLayout
  Card title: "Docs Shell"
    Slot
"#,
            ],
            r#"
page DocsHome
  Copy -> "Nested page"
"#,
        )
        .expect("route preview should render");

        assert!(html.contains("Root Layout"));
        assert!(html.contains("Docs Shell"));
        assert!(html.contains("Nested page"));
        assert!(html.contains("data-ax-page=\"DocsHome\""));
    }

    #[test]
    fn previews_route_loader_data_inside_page() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
loader PostsList
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 2
  return posts
"#],
            r#"
page Posts
  data posts = load PostsList
  Grid cols: 2
    each post in posts
      Card title: post.title
        Copy -> post.excerpt
"#,
        )
        .expect("route loader preview should render");

        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("Docs Without Bloat"));
        assert!(!html.contains("Draft Preview"));
    }

    #[test]
    fn previews_query_function_data_inside_page() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
query loadPosts() -> Post[]
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 6
  return posts
"#],
            r#"
page Posts
  data posts = loadPosts()
  Grid cols: 2
    each post in posts
      Card title: post.title
        Copy -> post.excerpt
"#,
        )
        .expect("query function preview should render");

        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("Docs Without Bloat"));
        assert!(!html.contains("Draft Preview"));
    }

    #[test]
    fn previews_query_function_with_call_arguments() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
query loadPosts(status: String) -> Post[]
  data posts = db.posts.all()
    where status = input.status
    order created_at desc
  return posts
"#],
            r#"
page Posts
  data status = "draft"
  data posts = loadPosts(status)
  Grid cols: 2
    each post in posts
      Card title: post.title
        Copy -> post.excerpt
"#,
        )
        .expect("query function with arguments should render");

        assert!(html.contains("Draft Preview"));
        assert!(!html.contains("Hello Axonyx"));
        assert!(!html.contains("Docs Without Bloat"));
    }

    #[test]
    fn previews_query_function_first_result_with_route_params() {
        let store = AxPreviewStore::default();
        let html = preview_ax_route_with_request_context(
            &[],
            &[r#"
query loadPost(slug: String) -> Post? {
  return db.posts.first()
    .where({ slug: input.slug })
}
"#],
            &[],
            r#"
page Post
  data post = loadPost(params.slug)
  Copy -> post.title
  Copy -> post.excerpt
"#,
            "/posts/hello-axonyx",
            &BTreeMap::from([("slug".to_string(), "hello-axonyx".to_string())]),
            &store,
        )
        .expect("single-record query function should render");

        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("A fast page rendered from .ax with almost no JavaScript."));
        assert!(!html.contains("Docs Without Bloat"));
        assert!(!html.contains("Draft Preview"));
    }

    #[test]
    fn previews_pure_backend_function_data_inside_page() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
fn normalizeStatus(status: String) -> String
  return status
"#],
            r#"
page Status
  data status = normalizeStatus("draft")
  Copy -> status
"#,
        )
        .expect("pure function preview should render");

        assert!(html.contains("draft"));
    }

    #[test]
    fn previews_query_function_with_pure_backend_helper() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
fn normalizeStatus(status: String) -> String
  data normalized = status
  return normalized

query loadPosts(status: String) -> Post[]
  data normalized = normalizeStatus(input.status)
  data posts = db.posts.all()
    where status = normalized
    order created_at desc
  return posts
"#],
            r#"
page Posts
  data posts = loadPosts("draft")
  Grid cols: 2
    each post in posts
      Card title: post.title
        Copy -> post.excerpt
"#,
        )
        .expect("query function with pure helper should render");

        assert!(html.contains("Draft Preview"));
        assert!(!html.contains("Hello Axonyx"));
        assert!(!html.contains("Docs Without Bloat"));
    }

    #[test]
    fn previews_function_shaped_page_with_query_data() {
        let html = preview_ax_route_with_loaders(
            &[],
            &[r#"
query loadPosts() -> Post[]
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 6
  return posts
"#],
            r#"
page Posts() {
  type Post {
    title: String
    excerpt?: String
  }

  data posts: List<Post> = loadPosts()

  return ASX {
    <Grid cols={2}>
      <Each items={posts} as="post">
        <Card title={post.title}>
          <Copy>{post?.excerpt}</Copy>
        </Card>
      </Each>
    </Grid>
  }
}
"#,
        )
        .expect("function-shaped page preview should render");

        assert!(html.contains("Hello Axonyx"));
        assert!(html.contains("Docs Without Bloat"));
        assert!(!html.contains("Draft Preview"));
    }

    #[test]
    fn previews_content_collection_loader_data_inside_page() {
        let store = AxPreviewStore::default().with_collection(
            "docs",
            vec![AxValue::record([
                ("slug", AxValue::from("getting-started")),
                ("path", AxValue::from("content/docs/getting-started.md")),
                ("title", AxValue::from("Getting Started")),
            ])],
        );
        let html = preview_ax_route_with_backend(
            &[],
            &[r#"
loader DocsList
  data docs = Content.Collection("docs")
    order slug asc
  return docs
"#],
            &[],
            r#"
page Docs
  data docs = load DocsList
  each doc in docs
    Card title: doc.title
      Copy -> doc.path
"#,
            "/docs",
            &store,
        )
        .expect("content collection preview should render");

        assert!(html.contains("Getting Started"));
        assert!(html.contains("content/docs/getting-started.md"));
    }

    #[test]
    fn previews_route_action_endpoint_inside_form() {
        let store = AxPreviewStore::default();
        let html = preview_ax_route_with_backend(
            &[],
            &[r#"
loader PostsList
  data posts = db.posts.all()
  return posts
"#],
            &[r#"
action CreatePost
  input:
    title: string
    excerpt: string

  insert "posts"
    title: input.title
    excerpt: input.excerpt

  revalidate "/posts"
  return ok
"#],
            r#"
page Posts
  form method: "post", action: action CreatePost
    Button type: "submit", tone: "primary" -> "Create"
"#,
            "/posts",
            &store,
        )
        .expect("action endpoint should render");

        assert!(html.contains("/__axonyx/action?path=%2Fposts&amp;name=CreatePost"));
        assert!(html.contains("type=\"submit\""));
        assert!(html.contains("data-ax-runtime=\"actions\""));
        assert!(html.contains("application/ax-patch+json"));
        assert!(html.contains("__ax_patch"));
        assert!(html.contains("__ax_protocol"));
        assert!(html.contains("__ax_tab"));
        assert!(html.contains("X-Axonyx-State-Protocol"));
        assert!(html.contains("X-Axonyx-Tab"));
        assert!(html.contains("new URLSearchParams(formData)"));
        assert!(html.contains("application/x-www-form-urlencoded;charset=UTF-8"));
        assert!(html.contains("syncActionStatus"));
        assert!(html.contains("setActionState"));
        assert!(html.contains("status.hidden = !active"));
        assert!(html.contains("aria-live"));
        assert!(html.contains("refreshes"));
        assert!(html.contains("/__axonyx/data"));
        assert!(html.contains("application/ax-data+json"));
        assert!(html.contains("applyDataRefresh"));
        assert!(html.contains("const refreshed = await refreshDataBindings"));
        assert!(html.contains("axonyx:query-refresh"));
        assert!(html.contains("axonyx:query-invalidate"));
        assert!(html.contains("axonyx:data-refresh"));
        assert!(html.contains("axonyx:data-refresh-error"));
        assert!(html.contains("axonyx:dom-refresh"));
        assert!(html.contains(
            "if (!refreshed && (patches.length === 0 || !canApplyPatches) && payload?.redirect)"
        ));
        assert!(!html.contains("if (refreshes.length === 0 &&"));
        assert!(html.contains("application/ax-error+json"));
        assert!(html.contains("setActionState(form, \"error\")"));
    }

    #[test]
    fn previews_jsx_action_form_with_patch_hidden_input() {
        let store = AxPreviewStore::default();
        let html = preview_ax_route_with_backend(
            &[],
            &[],
            &[r#"
action SetTheme
  input:
    theme: string

  patch theme = input.theme
  return ok
"#],
            r#"
page Home

<ActionForm name="SetTheme">
  <select name="theme">
    <option value="silver">Silver</option>
    <option value="gold">Gold</option>
  </select>
  <ActionStatus state="pending">Saving theme...</ActionStatus>
  <ActionStatus state="complete">Theme saved.</ActionStatus>
  <ActionStatus state="error">Theme could not be saved.</ActionStatus>
  <Button type="submit">Apply</Button>
</ActionForm>
"#,
            "/",
            &store,
        )
        .expect("jsx action form should render");

        assert!(html.contains("<form"));
        assert!(html.contains("class=\"ax-form\""));
        assert!(html.contains("method=\"post\""));
        assert!(html.contains("/__axonyx/action?path=%2F&amp;name=SetTheme"));
        assert!(html.contains("name=\"__ax_patch\""));
        assert!(html.contains("value=\"1\""));
        assert!(html.contains("class=\"ax-action-status\""));
        assert!(html.contains("data-state=\"pending\""));
        assert!(html.contains("Saving theme..."));
        assert!(html.contains("data-ax-runtime=\"actions\""));
    }

    #[test]
    fn preview_action_mutates_store_and_redirects() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action CreatePost
  input:
    title: string
    excerpt: string

  insert "posts"
    title: input.title
    excerpt: input.excerpt

  revalidate "/posts"
  return ok
"#],
            "CreatePost",
            &BTreeMap::from([
                ("title".to_string(), "Axonyx Forms".to_string()),
                (
                    "excerpt".to_string(),
                    "Route-local actions now mutate preview data.".to_string(),
                ),
            ]),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.redirect_to.as_deref(), Some("/posts"));

        let html = preview_ax_route_with_backend(
            &[],
            &[r#"
loader PostsList
  data posts = db.posts.all()
  return posts
"#],
            &[r#"
action CreatePost
  input:
    title: string
    excerpt: string

  insert "posts"
    title: input.title
    excerpt: input.excerpt

  revalidate "/posts"
  return ok
"#],
            r#"
page Posts
  data posts = load PostsList
  each post in posts
    Copy -> post.title
"#,
            "/posts",
            &store,
        )
        .expect("page should render with mutated store");

        assert!(html.contains("Axonyx Forms"));
    }

    #[test]
    fn production_action_runtime_writes_to_sqlite_instead_of_preview_store() {
        let path = std::env::temp_dir().join(format!(
            "axonyx-action-runtime-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_nanos()
        ));
        let connection = rusqlite::Connection::open(&path).expect("sqlite database should open");
        connection
            .execute(
                "create table posts (id integer primary key, title text not null)",
                [],
            )
            .expect("posts table should create");
        drop(connection);

        let env = backend::AxEnv::new()
            .with_secret("db_dialect", "sqlite")
            .with_secret("db_url", path.to_string_lossy());
        let runtime = backend::runtime_from_env(env).expect("database runtime should initialize");
        let mut store = AxPreviewStore::default();
        let preview_count = store.collection_items("posts").len();

        execute_preview_action_sources_with_runtime(
            &[r#"
action CreatePost
  input:
    title: string

  insert posts
    title: input.title
  return ok
"#],
            "CreatePost",
            &BTreeMap::from([("title".to_string(), "Stored in SQLite".to_string())]),
            &runtime,
            &mut store,
        )
        .expect("production action should execute");

        let connection = rusqlite::Connection::open(&path).expect("sqlite database should reopen");
        let title: String = connection
            .query_row("select title from posts", [], |row| row.get(0))
            .expect("inserted post should exist");
        assert_eq!(title, "Stored in SQLite");
        assert_eq!(store.collection_items("posts").len(), preview_count);

        drop(connection);
        std::fs::remove_file(path).expect("sqlite database should clean up");
    }

    #[test]
    fn preview_action_collects_state_patches() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SetTheme
  input:
    theme: string

  patch theme = input.theme
  return ok
"#],
            "SetTheme",
            &BTreeMap::from([("theme".to_string(), "gold".to_string())]),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.redirect_to, None);
        assert_eq!(result.patches.len(), 1);
        assert_eq!(result.patches[0].op, "set");
        assert_eq!(result.patches[0].signal, "root:theme:1");
        assert_eq!(result.patches[0].value, AxValue::String("gold".to_string()));
        assert_eq!(result.patches[0].source.as_deref(), Some("action"));
    }

    #[test]
    fn preview_action_collects_query_invalidations() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action CreatePost
  invalidate posts
  return ok
"#],
            "CreatePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.redirect_to, None);
        assert!(result.patches.is_empty());
        assert_eq!(
            result.invalidations,
            vec![AxPreviewInvalidation {
                target: "posts".to_string(),
                query_key: vec!["posts".to_string()],
            }]
        );
    }

    #[test]
    fn preview_action_can_use_pure_domain_helpers() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
fn normalizeTitle(title: String) -> String
  return title

fn isSupportedStatus(status: String) -> bool
  data statuses = ["draft", "published"]
  return contains(statuses, status)

action createPost(title: string, status: string) -> Post {
  data normalizedTitle = normalizeTitle(input.title)
  require isSupportedStatus(input.status) else error "Status is not supported."

  insert posts
    title: normalizedTitle
    status: input.status

  revalidate "/posts"
  return ok
}
"#],
            "createPost",
            &BTreeMap::from([
                ("title".to_string(), "Domain Action".to_string()),
                ("status".to_string(), "published".to_string()),
            ]),
            &mut store,
        )
        .expect("action should execute with domain helpers");

        assert!(result.error.is_none());
        assert_eq!(result.redirect_to.as_deref(), Some("/posts"));
        let posts = store.collection_items("posts");
        assert!(posts.iter().any(|post| {
            preview_record_field(post, "title")
                == Some(&AxValue::String("Domain Action".to_string()))
        }));
    }

    #[test]
    fn preview_action_guard_can_use_pure_domain_helper() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
fn isSupportedTheme(theme: String) -> bool
  data themes = ["silver", "bronze", "gold"]
  return contains(themes, theme)

action SetTheme(theme: string) {
  guard(isSupportedTheme(input.theme), "Theme is not supported.")
  patch theme = input.theme
  return ok
}
"#],
            "SetTheme",
            &BTreeMap::from([("theme".to_string(), "gold".to_string())]),
            &mut store,
        )
        .expect("guard action should execute");

        assert!(result.error.is_none());
        assert_eq!(result.patches.len(), 1);
        assert_eq!(result.patches[0].value, AxValue::String("gold".to_string()));
    }

    #[test]
    fn preview_action_query_data_can_use_domain_helpers() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
fn normalizeStatus(status: String) -> String
  return status

action LoadMatchingPosts(status: string) {
  data posts = db.posts.all()
    where status = normalizeStatus(input.status)

  return posts
}
"#],
            "LoadMatchingPosts",
            &BTreeMap::from([("status".to_string(), "published".to_string())]),
            &mut store,
        )
        .expect("query-backed action data should execute with domain helpers");

        let AxValue::List(posts) = result.value else {
            panic!("expected action to return posts");
        };
        assert_eq!(posts.len(), 2);
    }

    #[test]
    fn preview_action_auto_invalidates_mutated_collection() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action CreatePost
  input:
    title: string

  insert posts
    title: input.title

  return ok
"#],
            "CreatePost",
            &BTreeMap::from([("title".to_string(), "Hello".to_string())]),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(
            result.invalidations,
            vec![AxPreviewInvalidation {
                target: "posts".to_string(),
                query_key: vec!["posts".to_string()],
            }]
        );
    }

    #[test]
    fn preview_action_dedupes_manual_and_auto_invalidations() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action CreatePost
  insert posts
    title: "Hello"

  invalidate posts
  return ok
"#],
            "CreatePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.invalidations.len(), 1);
        assert_eq!(result.invalidations[0].query_key, vec!["posts".to_string()]);
    }

    #[test]
    fn preview_action_bare_invalidate_ignores_shadowing_bindings() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action CreatePost
  data posts = db.posts.all()
  invalidate posts
  return ok
"#],
            "CreatePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(
            result.invalidations,
            vec![AxPreviewInvalidation {
                target: "posts".to_string(),
                query_key: vec!["posts".to_string()],
            }]
        );
    }

    #[test]
    fn preview_action_explicit_route_revalidation_wins_over_auto_invalidation() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SavePost
  insert posts
    title: "Hello"

  revalidate "/posts"
  return ok
"#],
            "SavePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.redirect_to.as_deref(), Some("/posts"));
        assert_eq!(
            result.invalidations,
            vec![AxPreviewInvalidation {
                target: "/posts".to_string(),
                query_key: vec!["posts".to_string()],
            }]
        );
    }

    #[test]
    fn preview_action_revalidate_route_keeps_redirect_and_invalidation() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SavePost
  revalidate "/posts"
  return ok
"#],
            "SavePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.redirect_to.as_deref(), Some("/posts"));
        assert_eq!(
            result.invalidations,
            vec![AxPreviewInvalidation {
                target: "/posts".to_string(),
                query_key: vec!["posts".to_string()],
            }]
        );
    }

    #[test]
    fn preview_action_revalidate_evaluates_bound_targets() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SavePost
  data target = "/posts"
  revalidate target
  return ok
"#],
            "SavePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("action should execute");

        assert_eq!(result.redirect_to.as_deref(), Some("/posts"));
        assert_eq!(
            result.invalidations,
            vec![AxPreviewInvalidation {
                target: "/posts".to_string(),
                query_key: vec!["posts".to_string()],
            }]
        );
    }

    #[test]
    fn preview_action_require_in_list_can_return_validation_error() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SetTheme
  input:
    theme: string

  require input.theme in ["silver", "bronze", "gold"] else error "Theme is not supported."
  patch theme = input.theme
  return ok
"#],
            "SetTheme",
            &BTreeMap::from([("theme".to_string(), "blue".to_string())]),
            &mut store,
        )
        .expect("action should execute");

        let error = result.error.expect("invalid theme should return error");
        assert_eq!(error.status, 422);
        assert_eq!(error.message, "Theme is not supported.");
        assert!(result.patches.is_empty());
    }

    #[test]
    fn preview_action_require_in_variable_list_allows_valid_value() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SetTheme
  input:
    theme: string

  data themes = ["silver", "bronze", "gold"]
  require input.theme in themes else error "Theme is not supported."
  patch theme = input.theme
  return ok
"#],
            "SetTheme",
            &BTreeMap::from([("theme".to_string(), "gold".to_string())]),
            &mut store,
        )
        .expect("action should execute");

        assert!(result.error.is_none());
        assert_eq!(result.patches.len(), 1);
        assert_eq!(result.patches[0].value, AxValue::String("gold".to_string()));
    }

    #[test]
    fn preview_action_can_use_backend_root_data() {
        let mut store = AxPreviewStore::default();
        let backend = r#"
backend
  data themes: List<String> = ["silver", "bronze", "gold"]
"#;
        let action = r#"
action SetTheme
  input:
    theme: string

  require input.theme in themes else error "Theme is not supported."
  patch theme = input.theme
  return ok
"#;

        let result = execute_preview_action_sources(
            &[backend, action],
            "SetTheme",
            &BTreeMap::from([("theme".to_string(), "gold".to_string())]),
            &mut store,
        )
        .expect("action should execute");

        assert!(result.error.is_none());
        assert_eq!(result.patches[0].value, AxValue::String("gold".to_string()));
    }

    #[test]
    fn preview_action_coerces_multiple_typed_inputs() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action UpdatePreferences
  input:
    displayName: string
    count: i64
    newsletter: bool

  return input
"#],
            "UpdatePreferences",
            &BTreeMap::from([
                ("displayName".to_string(), "Vladan".to_string()),
                ("count".to_string(), "42".to_string()),
                ("newsletter".to_string(), "on".to_string()),
            ]),
            &mut store,
        )
        .expect("action should execute");

        let AxValue::Record(fields) = result.value else {
            panic!("expected input record");
        };
        assert_eq!(
            fields.get("displayName"),
            Some(&AxValue::String("Vladan".to_string()))
        );
        assert_eq!(fields.get("count"), Some(&AxValue::Number(42)));
        assert_eq!(fields.get("newsletter"), Some(&AxValue::Bool(true)));
    }

    #[test]
    fn preview_action_rejects_invalid_integer_input() {
        let mut store = AxPreviewStore::default();
        let error = execute_preview_action_sources(
            &[r#"
action UpdateCount
  input:
    count: i64

  return input
"#],
            "UpdateCount",
            &BTreeMap::from([("count".to_string(), "many".to_string())]),
            &mut store,
        )
        .expect_err("invalid integer should fail");

        assert!(error
            .to_string()
            .contains("input `count` expected i64 but received `many`"));
    }

    #[test]
    fn preview_action_allows_missing_optional_input() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action CreatePost
  input:
    title: string
    summary?: string

  return input
"#],
            "CreatePost",
            &BTreeMap::from([("title".to_string(), "Hello".to_string())]),
            &mut store,
        )
        .expect("optional input can be missing");

        let AxValue::Record(fields) = result.value else {
            panic!("expected input record");
        };
        assert_eq!(
            fields.get("title"),
            Some(&AxValue::String("Hello".to_string()))
        );
        assert_eq!(fields.get("summary"), Some(&AxValue::Null));
    }

    #[test]
    fn preview_action_rejects_missing_required_input() {
        let mut store = AxPreviewStore::default();
        let error = execute_preview_action_sources(
            &[r#"
action CreatePost
  input:
    title: string

  return input
"#],
            "CreatePost",
            &BTreeMap::new(),
            &mut store,
        )
        .expect_err("missing required input should fail");

        assert!(error.to_string().contains("missing required input `title`"));
    }

    #[test]
    fn preview_action_uses_default_input_values() {
        let mut store = AxPreviewStore::default();
        let result = execute_preview_action_sources(
            &[r#"
action SetLanguage
  input:
    language?: string = "sr"
    count: i64 = 0
    newsletter: bool = true

  return input
"#],
            "SetLanguage",
            &BTreeMap::new(),
            &mut store,
        )
        .expect("default inputs should execute");

        let AxValue::Record(fields) = result.value else {
            panic!("expected input record");
        };
        assert_eq!(
            fields.get("language"),
            Some(&AxValue::String("sr".to_string()))
        );
        assert_eq!(fields.get("count"), Some(&AxValue::Number(0)));
        assert_eq!(fields.get("newsletter"), Some(&AxValue::Bool(true)));
    }

    #[test]
    fn preview_action_rejects_default_input_type_mismatch() {
        let mut store = AxPreviewStore::default();
        let error = execute_preview_action_sources(
            &[r#"
action SetCount
  input:
    count: i64 = "many"

  return input
"#],
            "SetCount",
            &BTreeMap::new(),
            &mut store,
        )
        .expect_err("mismatched default should fail");

        assert!(error
            .to_string()
            .contains("default value for input `count` does not match expected i64"));
    }

    #[test]
    fn preview_route_sources_return_json_payload() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/posts"
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 2
  return posts
"#],
            "GET",
            "/api/posts",
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json; charset=utf-8");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert!(body.contains("Hello Axonyx"));
        assert!(body.contains("Docs Without Bloat"));
        assert!(!body.contains("Draft Preview"));
    }

    #[test]
    fn preview_route_sources_support_http_return_helpers() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/posts"
  data posts = db.posts.all()
  return json(posts)
"#],
            "GET",
            "/api/posts",
            &mut store,
        )
        .expect("json route should execute")
        .expect("json route should match");

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json; charset=utf-8");

        let response = execute_preview_route_sources(
            &[r#"
route GET "/go"
  return redirect("/next")
"#],
            "GET",
            "/go",
            &mut store,
        )
        .expect("redirect route should execute")
        .expect("redirect route should match");

        assert_eq!(response.status, 303);
        assert_eq!(
            response.headers.get("Location").map(String::as_str),
            Some("/next")
        );

        let response = execute_preview_route_sources(
            &[r#"
route DELETE "/api/posts"
  return noContent()
"#],
            "DELETE",
            "/api/posts",
            &mut store,
        )
        .expect("no content route should execute")
        .expect("no content route should match");

        assert_eq!(response.status, 204);
        assert!(response.body.is_empty());
    }

    #[test]
    fn preview_route_sources_apply_headers_and_cookies() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/session"
  header "Cache-Control" = "no-store"
  cookie "theme" = query.theme
  clearCookie "flash"
  return json("ok")
"#],
            "GET",
            "/api/session?theme=gold",
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(
            response.headers.get("Cache-Control").map(String::as_str),
            Some("no-store")
        );
        assert!(response
            .set_cookies
            .iter()
            .any(|cookie| cookie == "theme=gold; Path=/"));
        assert!(response
            .set_cookies
            .iter()
            .any(|cookie| cookie == "flash=; Path=/; Max-Age=0"));
    }

    #[test]
    fn preview_route_sources_apply_production_hooks() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/admin"
  before Security.headers
  after Cache.noStore
  before Auth.session
  return json("ok")
"#],
            "GET",
            "/api/admin",
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 401);
        assert_eq!(
            response
                .headers
                .get("X-Content-Type-Options")
                .map(String::as_str),
            Some("nosniff")
        );
        assert_eq!(
            response.headers.get("Cache-Control").map(String::as_str),
            Some("no-store")
        );

        let request =
            server::AxHttpRequest::new("GET", "/api/admin").with_header("Cookie", "session=abc123");
        let response = execute_preview_route_request_sources(
            &[r#"
route GET "/api/admin"
  before Auth.session
  return json(Auth.session)
"#],
            &request,
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 200);
    }

    #[test]
    fn preview_route_sources_can_read_request_context() {
        let mut store = AxPreviewStore::default();
        let request = server::AxHttpRequest::new("POST", "/api/session?source=form")
            .with_header("Cookie", "theme=gold; session=abc123")
            .with_header("User-Agent", "AxonyxTest")
            .with_body(b"name=Axonyx".to_vec());
        let response = execute_preview_route_request_sources(
            &[r#"
route POST "/api/session"
  data theme = request.cookies.theme
  data session = request.cookies.session
  data agent = request.headers.user_agent
  data body = request.body
  return json(theme)
"#],
            &request,
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "\"gold\"");
    }

    #[test]
    fn preview_route_sources_can_require_request_values() {
        let mut store = AxPreviewStore::default();
        let source = r#"
route GET "/api/admin"
  require request.cookies.session
  return json("ok")
"#;
        let request = server::AxHttpRequest::new("GET", "/api/admin");
        let response = execute_preview_route_request_sources(&[source], &request, &mut store)
            .expect("route should execute")
            .expect("route should match");

        assert_eq!(response.status, 401);
        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert!(body.contains("unauthorized"));

        let request =
            server::AxHttpRequest::new("GET", "/api/admin").with_header("Cookie", "session=abc123");
        let response = execute_preview_route_request_sources(&[source], &request, &mut store)
            .expect("route should execute")
            .expect("route should match");

        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "\"ok\"");
    }

    #[test]
    fn preview_route_sources_can_use_require_fallbacks() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_request_sources(
            &[r#"
route GET "/api/admin"
  require request.cookies.session else redirect("/login")
  return json("ok")
"#],
            &server::AxHttpRequest::new("GET", "/api/admin"),
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 303);
        assert_eq!(
            response.headers.get("Location").map(String::as_str),
            Some("/login")
        );
    }

    #[test]
    fn preview_route_sources_can_use_auth_request_aliases() {
        let mut store = AxPreviewStore::default();
        let source = r#"
route GET "/api/admin"
  require Auth.bearer else redirect("/login")
  data token = Auth.bearer
  data session = Auth.session
  return json(token)
"#;
        let response = execute_preview_route_request_sources(
            &[source],
            &server::AxHttpRequest::new("GET", "/api/admin"),
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 303);
        assert_eq!(
            response.headers.get("Location").map(String::as_str),
            Some("/login")
        );

        let request = server::AxHttpRequest::new("GET", "/api/admin")
            .with_header("Authorization", "Bearer abc")
            .with_header("Cookie", "session=s123");
        let response = execute_preview_route_request_sources(&[source], &request, &mut store)
            .expect("route should execute")
            .expect("route should match");

        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "\"abc\"");
    }

    #[test]
    fn preview_route_sources_can_use_signed_session_alias() {
        let secret_prev = std::env::var("AX_SECRET_SESSION_KEY").ok();
        std::env::set_var("AX_SECRET_SESSION_KEY", "local-secret");

        let mut store = AxPreviewStore::default();
        let source = r#"
route GET "/api/admin"
  require Auth.signedSession else redirect("/login")
  data session = Auth.signedSession
  return json(session)
"#;

        let response = execute_preview_route_request_sources(
            &[source],
            &server::AxHttpRequest::new("GET", "/api/admin"),
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 303);

        let signed = server::AxAuth::sign_session("s123", "local-secret");
        let request = server::AxHttpRequest::new("GET", "/api/admin")
            .with_header("Cookie", format!("session={signed}"));
        let response = execute_preview_route_request_sources(&[source], &request, &mut store)
            .expect("route should execute")
            .expect("route should match");

        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "\"s123\"");

        if let Some(value) = secret_prev {
            std::env::set_var("AX_SECRET_SESSION_KEY", value);
        } else {
            std::env::remove_var("AX_SECRET_SESSION_KEY");
        }
    }

    #[test]
    fn preview_route_sources_can_read_structured_request_body() {
        let mut store = AxPreviewStore::default();
        let request = server::AxHttpRequest::new("POST", "/api/form")
            .with_body(b"title=Hello+Axonyx&excerpt=Fast%20forms".to_vec());
        let response = execute_preview_route_request_sources(
            &[r#"
route POST "/api/form"
  data title = request.form.title
  return json(title)
"#],
            &request,
            &mut store,
        )
        .expect("form route should execute")
        .expect("form route should match");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "\"Hello Axonyx\"");

        let request = server::AxHttpRequest::new("POST", "/api/json")
            .with_body(br#"{"title":"Hello JSON","count":3}"#.to_vec());
        let response = execute_preview_route_request_sources(
            &[r#"
route POST "/api/json"
  data title = request.json.title
  data count = request.json.count
  return json(count)
"#],
            &request,
            &mut store,
        )
        .expect("json route should execute")
        .expect("json route should match");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "3");
    }

    #[test]
    fn preview_route_sources_can_use_typed_request_input() {
        let mut store = AxPreviewStore::default();
        let source = r#"
route POST "/api/posts"
  input:
    title: string
    count: i64
    featured?: bool = false

  return json(input.count)
"#;

        let request = server::AxHttpRequest::new("POST", "/api/posts")
            .with_body(br#"{"title":"Hello","count":3}"#.to_vec());
        let response = execute_preview_route_request_sources(&[source], &request, &mut store)
            .expect("json route should execute")
            .expect("route should match");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "3");

        let request =
            server::AxHttpRequest::new("POST", "/api/posts").with_body(b"title=Hello".to_vec());
        let error = execute_preview_route_request_sources(&[source], &request, &mut store)
            .expect_err("missing typed input should fail");

        assert!(error.to_string().contains("missing required input `count`"));
    }

    #[test]
    fn preview_route_sources_can_use_params_and_query() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/posts/:slug"
  data posts = db.posts.all()
    where slug = params.slug
    where status = query.status
  return posts
"#],
            "GET",
            "/api/posts/draft-preview?status=draft",
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert!(body.contains("Draft Preview"));
        assert!(!body.contains("Hello Axonyx"));
    }

    #[test]
    fn preview_route_sources_can_return_not_found_fallback() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/posts/:slug"
  data post = db.posts.first()
    .where({ slug: params.slug })
  require post else notFound()
  return json(post)
"#],
            "GET",
            "/api/posts/missing-post",
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        assert_eq!(response.status, 404);
        assert_eq!(String::from_utf8(response.body).unwrap(), "not found");
    }

    #[test]
    fn preview_route_sources_prefer_static_path_over_dynamic_match() {
        let mut store = AxPreviewStore::default();
        let response = execute_preview_route_sources(
            &[r#"
route GET "/api/posts/:slug"
  return "dynamic"

route GET "/api/posts/featured"
  return "featured"
"#],
            "GET",
            "/api/posts/featured",
            &mut store,
        )
        .expect("route should execute")
        .expect("route should match");

        let body = String::from_utf8(response.body).expect("json response should be utf-8");
        assert_eq!(body, "\"featured\"");
    }

    #[test]
    fn previews_page_route_with_request_context_in_page_and_loader() {
        let store = AxPreviewStore::default();
        let html = preview_ax_route_with_request_context(
            &[],
            &[r#"
loader PostDetail
  data posts = db.posts.all()
    where slug = params.slug
    where status = query.status
  return posts
"#],
            &[],
            r#"
page Post
  Copy -> params.slug
  data posts = load PostDetail
  each post in posts
    Copy -> post.title
"#,
            "/posts/draft-preview?status=draft",
            &BTreeMap::from([("slug".to_string(), "draft-preview".to_string())]),
            &store,
        )
        .expect("page should render with request context");

        assert!(html.contains("draft-preview"));
        assert!(html.contains("Draft Preview"));
        assert!(!html.contains("Hello Axonyx"));
    }

    #[test]
    fn previews_canonical_route_context_in_modern_asx() {
        let store = AxPreviewStore::default();
        let html = preview_ax_route_with_request_context(
            &[],
            &[],
            &[],
            r#"
page RouteProbe() {
  return ASX {
    <>
      <Copy>{route.path}</Copy>
      <Copy>{route.section}</Copy>
      <Copy>{route.subsection}</Copy>
      <Copy>{route.params.slug}</Copy>
      <Copy>{query.mode}</Copy>
    </>
  }
}
"#,
            "/docs/getting-started?mode=preview#intro",
            &BTreeMap::from([("slug".to_string(), "getting-started".to_string())]),
            &store,
        )
        .expect("page should render with canonical route context");

        assert!(html.contains("/docs/getting-started"));
        assert!(html.contains(">docs</p>"));
        assert!(html.contains(">getting-started</p>"));
        assert!(html.contains(">preview</p>"));
    }

    #[test]
    fn root_route_context_uses_stable_empty_segments() {
        let scope = build_preview_route_scope("/?mode=home", &BTreeMap::new(), &BTreeMap::new());
        let Some(AxValue::Record(route)) = scope.get("route") else {
            panic!("route context should be a record");
        };

        assert_eq!(route.get("path"), Some(&AxValue::String("/".to_string())));
        assert_eq!(route.get("section"), Some(&AxValue::String(String::new())));
        assert_eq!(
            route.get("subsection"),
            Some(&AxValue::String(String::new()))
        );
    }

    #[test]
    fn route_context_drives_props_on_imported_shell_components() {
        let store = AxPreviewStore::default();
        let import_resolver = |source: &str| {
            (source == "@/ActiveLink.asx").then(|| {
                r#"
component ActiveLink(active = false) {
  render ASX {
    <a data-active={active}><Slot /></a>
  }
}
"#
                .to_string()
            })
        };
        let html = preview_ax_route_with_request_context_and_imports(
            &[],
            &[],
            &[],
            r#"
import { ActiveLink } from "@/ActiveLink.asx"

page DocsShell() {
  return ASX {
    <ActiveLink active={route.section == "docs"}>{route.subsection}</ActiveLink>
  }
}
"#,
            "/docs/runtime",
            &BTreeMap::new(),
            &store,
            &import_resolver,
        )
        .expect("route context should flow through imported component props");

        assert!(html.contains("data-active=\"true\""));
        assert!(html.contains(">runtime</a>"));
    }

    #[test]
    fn previews_head_metadata_inside_html_head() {
        let html = preview_ax_page(
            r#"
page Home
  title "Axonyx Site"
  meta name: "description", content: "Fast pages with minimal JS."
  link rel: "icon", href: "/favicon.svg", type: "image/svg+xml"
  script src: "/app.js", defer: true
  Copy -> "Hello"
"#,
        )
        .expect("preview should render");

        assert!(html.contains("<title>Axonyx Site</title>"));
        assert!(
            html.contains("<meta name=\"description\" content=\"Fast pages with minimal JS.\">")
        );
        assert!(html.contains("<link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\">"));
        assert!(html.contains("<script src=\"/app.js\" defer=\"true\"></script>"));
    }

    #[test]
    fn page_title_overrides_layout_title_while_layout_meta_stays() {
        let html = preview_ax_app(
            Some(
                r#"
page RootLayout
  title "Layout Title"
  meta name: "description", content: "Layout description."
  Slot
"#,
            ),
            r#"
page Home
  title "Page Title"
  link rel: "icon", href: "/favicon.svg"
  Copy -> "Body"
"#,
        )
        .expect("layout preview should render");

        assert!(html.contains("<title>Page Title</title>"));
        assert!(html.contains("<meta name=\"description\" content=\"Layout description.\">"));
        assert!(html.contains("<link rel=\"icon\" href=\"/favicon.svg\">"));
    }
}
