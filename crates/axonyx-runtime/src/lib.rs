pub mod backend;
pub mod server;

use std::cell::RefCell;
use std::collections::BTreeMap;

use axonyx_core::ax_ast_prelude::{
    AxBody, AxComponent, AxDocument, AxExpr, AxHead, AxHeadTag, AxPipeline, AxPipelineStage,
    AxProp, AxStatement,
};
use axonyx_core::ax_backend_lowering::AxBackendLowerError;
use axonyx_core::ax_backend_lowering_prelude::{
    lower_backend_document, AxHandlerKind, AxHandlerPlan, AxHookPhasePlan, AxQueryFilterOpPlan,
    AxQueryOrderDirectionPlan, AxQueryPlan, AxQuerySourcePlan, AxReturnPlan, AxRustExpr,
    AxStepPlan, AxValuePlan,
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
pub use server::prelude as server_prelude;

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
        &BTreeMap::new(),
        &parse_preview_query_fields(request_target),
    );
    let resolver_error = RefCell::new(None);
    let resolver = |path: &[String], args: &[AxValue]| -> Option<AxValue> {
        match preview_resolve_call(
            &handlers,
            &cache,
            &env,
            request_target,
            &route_scope,
            store,
            path,
            args,
        ) {
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
    let page_document = parse_ax_auto(page_source)?;
    let mut document = page_document;

    for layout_source in layout_sources.iter().rev() {
        let layout_document = parse_ax_auto(layout_source)?;
        document = compose_layout_with_page(layout_document, document);
    }

    let handlers = collect_preview_handlers(loader_sources, action_sources, &[])?;
    let cache = RefCell::new(BTreeMap::new());
    let env = backend::AxEnv::from_env();
    let route_scope =
        build_preview_route_scope(route_params, &parse_preview_query_fields(request_target));
    let resolver_error = RefCell::new(None);
    let resolver = |path: &[String], args: &[AxValue]| -> Option<AxValue> {
        match preview_resolve_call(
            &handlers,
            &cache,
            &env,
            request_target,
            &route_scope,
            store,
            path,
            args,
        ) {
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
    execute_preview_action(&handlers.actions, action_name, input_fields, &env, store)
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

    for source in route_sources {
        let document = parse_backend_ax(source)?;
        let plan = lower_backend_document(&document)?;

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

        for handler in plan.handlers {
            if matches!(handler.kind, AxHandlerKind::Loader { .. }) {
                loaders.insert(handler.name.clone(), handler);
            }
        }
    }

    for source in action_sources {
        let document = parse_backend_ax(source)?;
        let plan = lower_backend_document(&document)?;

        for handler in plan.handlers {
            if matches!(handler.kind, AxHandlerKind::Action { .. }) {
                actions.insert(handler.name.clone(), handler);
            }
        }
    }

    Ok(PreviewHandlers {
        routes,
        loaders,
        actions,
    })
}

fn preview_resolve_call(
    handlers: &PreviewHandlers,
    cache: &RefCell<BTreeMap<String, AxValue>>,
    env: &backend::AxEnv,
    request_target: &str,
    route_scope: &BTreeMap<String, AxValue>,
    store: &AxPreviewStore,
    path: &[String],
    args: &[AxValue],
) -> Result<Option<AxValue>, PreviewError> {
    if path == ["load".to_string()] {
        let [AxValue::String(loader_name)] = args else {
            return Err(PreviewError::Runtime {
                message: "load(...) expects a single loader name".to_string(),
            });
        };

        if let Some(cached) = cache.borrow().get(loader_name).cloned() {
            return Ok(Some(cached));
        }

        let loader = handlers
            .loaders
            .get(loader_name)
            .ok_or_else(|| PreviewError::Runtime {
                message: format!("loader `{loader_name}` was not found for this route"),
            })?;
        let value = execute_preview_loader(loader, route_scope, env, store)?;
        cache
            .borrow_mut()
            .insert(loader_name.clone(), value.clone());
        return Ok(Some(value));
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

    if path == ["Db".to_string(), "Stream".to_string()] {
        let [AxValue::String(collection)] = args else {
            return Err(PreviewError::Runtime {
                message: "Db.Stream(...) expects a collection name".to_string(),
            });
        };

        return Ok(Some(AxValue::List(store.collection_items(collection))));
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

fn execute_preview_loader(
    loader: &AxHandlerPlan,
    initial_scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    store: &AxPreviewStore,
) -> Result<AxValue, PreviewError> {
    let mut scope = initial_scope.clone();

    for step in &loader.steps {
        match step {
            AxStepPlan::Let { binding, value } => {
                let value = eval_preview_value(value, &scope, env, store)?;
                scope.insert(binding.clone(), value);
            }
            AxStepPlan::Return(value) => return eval_preview_return(value, &scope, env),
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

fn execute_preview_action(
    actions: &BTreeMap<String, AxHandlerPlan>,
    action_name: &str,
    input_fields: &BTreeMap<String, String>,
    env: &backend::AxEnv,
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

    for step in &action.steps {
        match step {
            AxStepPlan::Let {
                binding,
                value: plan,
            } => {
                let evaluated = eval_preview_value(plan, &scope, env, store)?;
                scope.insert(binding.clone(), evaluated);
            }
            AxStepPlan::Insert { collection, fields } => {
                let mut record = eval_preview_fields(fields, &scope, env)?;
                assign_preview_id(&mut record, store.collection_items(collection).len());
                store
                    .ensure_collection(collection)
                    .push(AxValue::Record(record));
            }
            AxStepPlan::Update {
                collection,
                fields,
                filters,
            } => {
                let fields = eval_preview_fields(fields, &scope, env)?;
                let filters = eval_preview_filters(filters, &scope, env)?;
                for item in store.ensure_collection(collection).iter_mut() {
                    if preview_record_matches_all(item, &filters) {
                        apply_preview_fields(item, &fields);
                    }
                }
            }
            AxStepPlan::Delete {
                collection,
                filters,
            } => {
                let filters = eval_preview_filters(filters, &scope, env)?;
                store
                    .ensure_collection(collection)
                    .retain(|item| !preview_record_matches_all(item, &filters));
            }
            AxStepPlan::Revalidate { target } => {
                redirect_to = Some(eval_preview_expr(target, &scope, env)?.as_string());
            }
            AxStepPlan::Patch { signal, value } => {
                let signal = eval_preview_expr(signal, &scope, env)?.as_string();
                let value = eval_preview_expr(value, &scope, env)?;
                patches.push(AxPreviewStatePatch::set(signal, value));
            }
            AxStepPlan::Return(result) => {
                value = eval_preview_return(result, &scope, env)?;
            }
            AxStepPlan::Header { .. }
            | AxStepPlan::Hook { .. }
            | AxStepPlan::Cookie { .. }
            | AxStepPlan::ClearCookie { .. }
            | AxStepPlan::Require { .. }
            | AxStepPlan::Send { .. } => {}
        }
    }

    Ok(AxPreviewActionResult {
        redirect_to,
        value,
        patches,
    })
}

fn execute_preview_route(
    routes: &[AxHandlerPlan],
    request: &server::AxHttpRequest,
    request_path: &str,
    query: &BTreeMap<String, String>,
    env: &backend::AxEnv,
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
                let evaluated = eval_preview_value(plan, &scope, env, store)?;
                scope.insert(binding.clone(), evaluated);
            }
            AxStepPlan::Insert { collection, fields } => {
                let mut record = eval_preview_fields(fields, &scope, env)?;
                assign_preview_id(&mut record, store.collection_items(collection).len());
                store
                    .ensure_collection(collection)
                    .push(AxValue::Record(record));
            }
            AxStepPlan::Update {
                collection,
                fields,
                filters,
            } => {
                let fields = eval_preview_fields(fields, &scope, env)?;
                let filters = eval_preview_filters(filters, &scope, env)?;
                for item in store.ensure_collection(collection).iter_mut() {
                    if preview_record_matches_all(item, &filters) {
                        apply_preview_fields(item, &fields);
                    }
                }
            }
            AxStepPlan::Delete {
                collection,
                filters,
            } => {
                let filters = eval_preview_filters(filters, &scope, env)?;
                store
                    .ensure_collection(collection)
                    .retain(|item| !preview_record_matches_all(item, &filters));
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
                if eval_preview_require_expr(value, &scope, env)?
                    .as_string()
                    .is_empty()
                {
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
            if eval_preview_require_expr(hook, scope, env)?
                .as_string()
                .is_empty()
            {
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
    match eval_preview_expr(expr, scope, env) {
        Ok(value) => Ok(value),
        Err(_error) if expr.code.trim().starts_with("request.") => {
            Ok(AxValue::String(String::new()))
        }
        Err(error) => Err(error),
    }
}

fn eval_preview_value(
    value: &AxValuePlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    store: &AxPreviewStore,
) -> Result<AxValue, PreviewError> {
    match value {
        AxValuePlan::Expr(expr) => eval_preview_expr(expr, scope, env),
        AxValuePlan::Query(query) => eval_preview_query(query, scope, env, store),
    }
}

fn eval_preview_return(
    value: &AxReturnPlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<AxValue, PreviewError> {
    match value {
        AxReturnPlan::Expr(expr) => eval_preview_expr(expr, scope, env),
        AxReturnPlan::Json(expr) => eval_preview_expr(expr, scope, env),
        AxReturnPlan::Redirect { .. } | AxReturnPlan::NoContent => Err(PreviewError::Runtime {
            message: "HTTP response helpers are only supported in route blocks".to_string(),
        }),
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

fn eval_preview_query(
    query: &AxQueryPlan,
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
    store: &AxPreviewStore,
) -> Result<AxValue, PreviewError> {
    let collection = match &query.source {
        AxQuerySourcePlan::Stream { collection } => collection,
        AxQuerySourcePlan::ContentCollection { collection } => collection,
    };
    let mut items = store.collection_items(collection);

    for filter in &query.filters {
        let expected = eval_preview_expr(&filter.value, scope, env)?;
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

    Ok(AxValue::List(items))
}

fn preview_record_matches(
    item: &AxValue,
    field: &str,
    op: AxQueryFilterOpPlan,
    expected: &AxValue,
) -> bool {
    let Some(value) = preview_record_field(item, field) else {
        return false;
    };

    match op {
        AxQueryFilterOpPlan::Eq => value == expected,
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

    if let Some(key) = parse_preview_env_call(code, "public") {
        return Ok(AxValue::String(env.public(&key)?));
    }

    if let Some(key) = parse_preview_env_call(code, "secret") {
        return Ok(AxValue::String(env.secret(&key)?));
    }

    if let Some(value) = lookup_preview_scope(scope, code) {
        return Ok(value);
    }

    Err(PreviewError::Runtime {
        message: format!("preview loader expression `{code}` is not supported yet"),
    })
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
    let mut map = BTreeMap::new();

    for field in fields {
        map.insert(
            field.name.clone(),
            eval_preview_expr(&field.value, scope, env)?,
        );
    }

    Ok(map)
}

fn eval_preview_filters(
    filters: &[axonyx_core::ax_backend_lowering_prelude::AxQueryFilterPlan],
    scope: &BTreeMap<String, AxValue>,
    env: &backend::AxEnv,
) -> Result<Vec<PreviewFilter>, PreviewError> {
    filters
        .iter()
        .map(|filter| {
            Ok(PreviewFilter {
                field: filter.field.clone(),
                op: filter.op,
                value: eval_preview_expr(&filter.value, scope, env)?,
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
        _ => Ok(AxValue::String(value)),
    }
}

fn preview_value_type_name(value: &AxValue) -> &'static str {
    match value {
        AxValue::Null => "Null",
        AxValue::String(_) => "String",
        AxValue::Number(_) => "Number",
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
    route_params: &BTreeMap<String, String>,
    query: &BTreeMap<String, String>,
) -> BTreeMap<String, AxValue> {
    BTreeMap::from([
        (
            "params".to_string(),
            AxValue::Record(
                route_params
                    .iter()
                    .map(|(key, value)| (key.clone(), AxValue::String(value.clone())))
                    .collect(),
            ),
        ),
        ("query".to_string(), build_preview_query_record(query)),
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
    let state_bridge_script = if body.contains("data-ax-signal=") {
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

  const applyPatchResponse = (payload, form) => {
    const patches = Array.isArray(payload?.patches) ? payload.patches : [];
    const applyPatch = window.__axonyx?.state?.applyPatch;
    const canApplyPatches = typeof applyPatch === "function";
    if (canApplyPatches) patches.forEach((patch) => applyPatch(patch));
    window.dispatchEvent(new CustomEvent("axonyx:action-complete", {
      detail: { form, payload, patches },
    }));
    if ((patches.length === 0 || !canApplyPatches) && payload?.redirect) {
      window.location.assign(payload.redirect);
    }
  };

  document.addEventListener("submit", async (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || !isAxonyxActionForm(form)) return;
    event.preventDefault();

    const body = new FormData(form);
    if (!body.has("__ax_patch")) body.append("__ax_patch", "1");
    form.setAttribute("data-ax-action-state", "pending");

    try {
      const response = await fetch(form.action, {
        method: form.method || "POST",
        headers: { Accept: "application/ax-patch+json" },
        body,
        cache: "no-store",
      });
      const contentType = response.headers.get("content-type") || "";
      if (contentType.includes("application/ax-patch+json")) {
        applyPatchResponse(await response.json(), form);
        form.setAttribute("data-ax-action-state", "complete");
        return;
      }
      if (response.redirected) {
        window.location.assign(response.url);
        return;
      }
      window.location.reload();
    } catch (error) {
      form.setAttribute("data-ax-action-state", "error");
      window.dispatchEvent(new CustomEvent("axonyx:action-error", {
        detail: { form, error },
      }));
    }
  });
})();
</script>"##
}

fn ax_state_bridge_script() -> &'static str {
    r##"<script data-ax-runtime="state-bridge">
(() => {
  if (window.__axonyxStateBridge) return;

  const state = new Map();
  const bindings = new Map();
  const types = new Map();
  const subscribers = new Map();

  const readValue = (node, target) => {
    if (target === "checked") return !!node.checked;
    if (target === "text") return node.textContent || "";
    return node.value ?? node.getAttribute("value") ?? "";
  };

  const castValue = (value, type) => {
    if (type === "Bool") {
      return value === true || value === "true" || value === "on";
    }
    if (type === "Number") {
      const next = Number(value);
      return Number.isFinite(next) ? next : value;
    }
    return value == null ? "" : String(value);
  };

  const writeValue = (node, target, value) => {
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

  const emitPatch = (signal, value, source) => {
    const detail = { op: "set", signal, value, source };
    window.dispatchEvent(new CustomEvent("axonyx:state-patch", { detail }));
  };

  const notifySubscribers = (signal, value, source) => {
    const detail = { signal, value, source };
    window.dispatchEvent(new CustomEvent("axonyx:state-change", { detail }));
    (subscribers.get(signal) || []).forEach((listener) => listener(value, detail));
  };

  const register = (node) => {
    const signal = node.getAttribute("data-ax-signal");
    if (!signal) return;
    const target = node.getAttribute("data-ax-bind") || "value";
    const type = node.getAttribute("data-ax-state-type") || "String";
    const initial = castValue(readValue(node, target), type);
    if (!types.has(signal)) types.set(signal, type);
    if (!state.has(signal)) state.set(signal, initial);
    writeValue(node, target, state.get(signal));
    if (!bindings.has(signal)) bindings.set(signal, []);
    bindings.get(signal).push({ node, target, type });
  };

  const writeSignal = (signal, value, source = "client", emit = true) => {
    const type = types.get(signal) || "String";
    const nextValue = castValue(value, type);
    state.set(signal, nextValue);
    (bindings.get(signal) || []).forEach(({ node, target }) => {
      writeValue(node, target, nextValue);
    });
    notifySubscribers(signal, nextValue, source);
    if (emit) emitPatch(signal, nextValue, source);
    return nextValue;
  };

  const setSignal = (signal, value, source = "client") => {
    return writeSignal(signal, value, source, true);
  };

  const applyPatch = (patch) => {
    if (!patch || !patch.signal) return undefined;
    const op = patch.op || "set";
    if (op !== "set") return undefined;
    return writeSignal(patch.signal, patch.value, patch.source || "patch", false);
  };

  const subscribe = (signal, listener) => {
    if (typeof listener !== "function") return () => {};
    if (!subscribers.has(signal)) subscribers.set(signal, new Set());
    subscribers.get(signal).add(listener);
    return () => subscribers.get(signal)?.delete(listener);
  };

  const init = () => {
    document.querySelectorAll("[data-ax-signal]").forEach(register);
  };

  document.addEventListener("input", (event) => {
    const node = event.target.closest("[data-ax-signal]");
    if (!node) return;
    const type = node.getAttribute("data-ax-state-type") || "String";
    setSignal(node.getAttribute("data-ax-signal"), castValue(readValue(node, node.getAttribute("data-ax-bind") || "value"), type));
  });

  document.addEventListener("change", (event) => {
    const node = event.target.closest("[data-ax-signal]");
    if (!node) return;
    const type = node.getAttribute("data-ax-state-type") || "String";
    setSignal(node.getAttribute("data-ax-signal"), castValue(readValue(node, node.getAttribute("data-ax-bind") || "value"), type));
  });

  window.__axonyx = window.__axonyx || {};
  window.__axonyx.state = {
    version: 1,
    get: (signal) => state.get(signal),
    set: setSignal,
    subscribe,
    applyPatch,
    snapshot: () => Object.fromEntries(state.entries()),
  };
  window.__axonyx.applyPatch = applyPatch;
  window.__axonyxStateBridge = window.__axonyx.state;

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
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
        AxExpr::Bool(value) => value.to_string(),
        AxExpr::Identifier(value) => value.clone(),
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
    fn builds_render_plan_from_ir() {
        let ir = compile_pipeline(r#"Db.Stream("posts") |> layout.Grid(3) |> Card()"#)
            .expect("pipeline should compile");
        let plan = execute(&ir);

        assert_eq!(plan.source, "posts");
        assert_eq!(plan.layout.kind, "grid");
        assert_eq!(plan.layout.columns, 3);
        assert_eq!(plan.view.component, "Card");
    }

    #[test]
    fn builds_render_plan_from_json() {
        let ir = compile_pipeline(r#"Db.Stream("users") |> layout.Grid(2) |> ProfileCard()"#)
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
  data posts = Db.Stream("posts")
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
        assert!(state_html.contains("applyPatch"));
        assert!(state_html.contains("subscribe"));
        assert!(state_html.contains("window.__axonyxStateBridge"));
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
        assert!(html.contains("value=\"silver\""));
        assert!(html.contains("data-ax-bind=\"text\""));
        assert!(html.contains(">silver</span>"));
        assert!(html.contains("data-ax-state-type=\"Number\""));
        assert!(html.contains("value=\"0\""));
        assert!(html.contains("data-ax-runtime=\"state-bridge\""));
        assert!(html.contains("castValue"));
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
  data posts = Db.Stream("posts")
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
