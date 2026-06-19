use thiserror::Error;

use crate::ax_ast::prelude::{AxBinaryOp, AxExpr, AxUnaryOp};
use crate::ax_backend_ast::prelude::*;
use crate::ax_query_ast::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxBackendPlan {
    pub envs: Vec<AxEnvPlan>,
    pub globals: Vec<AxStepPlan>,
    pub functions: Vec<AxFunctionPlan>,
    pub handlers: Vec<AxHandlerPlan>,
}

impl AxBackendPlan {
    pub fn new(handlers: impl IntoIterator<Item = AxHandlerPlan>) -> Self {
        Self {
            envs: Vec::new(),
            globals: Vec::new(),
            functions: Vec::new(),
            handlers: handlers.into_iter().collect(),
        }
    }

    pub fn with_globals(
        envs: impl IntoIterator<Item = AxEnvPlan>,
        globals: impl IntoIterator<Item = AxStepPlan>,
        functions: impl IntoIterator<Item = AxFunctionPlan>,
        handlers: impl IntoIterator<Item = AxHandlerPlan>,
    ) -> Self {
        Self {
            envs: envs.into_iter().collect(),
            globals: globals.into_iter().collect(),
            functions: functions.into_iter().collect(),
            handlers: handlers.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxEnvPlan {
    pub name: String,
    pub visibility: AxEnvVisibilityPlan,
    pub ty: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxEnvVisibilityPlan {
    Public,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxFunctionPlan {
    pub name: String,
    pub rust_fn: String,
    pub returns: Option<String>,
    pub input: Vec<AxFieldPlan>,
    pub steps: Vec<AxStepPlan>,
    pub exported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxHandlerPlan {
    pub name: String,
    pub rust_fn: String,
    pub kind: AxHandlerKind,
    pub steps: Vec<AxStepPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxHandlerKind {
    Route {
        method: String,
        path: String,
        returns: Option<String>,
        input: Vec<AxFieldPlan>,
    },
    Loader {
        returns: Option<String>,
        input: Vec<AxFieldPlan>,
    },
    Action {
        returns: Option<String>,
        input: Vec<AxFieldPlan>,
    },
    Job,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxFieldPlan {
    pub name: String,
    pub rust_ty: String,
    pub optional: bool,
    pub default: Option<AxRustExpr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxStepPlan {
    Let {
        binding: String,
        value: AxValuePlan,
    },
    Insert {
        collection: String,
        fields: Vec<AxAssignmentPlan>,
    },
    Update {
        collection: String,
        fields: Vec<AxAssignmentPlan>,
        filters: Vec<AxQueryFilterPlan>,
    },
    Delete {
        collection: String,
        filters: Vec<AxQueryFilterPlan>,
    },
    Revalidate {
        target: AxRustExpr,
        literal: bool,
    },
    Patch {
        signal: AxRustExpr,
        value: AxRustExpr,
    },
    Hook {
        phase: AxHookPhasePlan,
        value: AxRustExpr,
    },
    Header {
        name: AxRustExpr,
        value: AxRustExpr,
    },
    Cookie {
        name: AxRustExpr,
        value: AxRustExpr,
    },
    ClearCookie {
        name: AxRustExpr,
    },
    Require {
        value: AxRustExpr,
        fallback: Option<AxReturnPlan>,
    },
    Return(AxReturnPlan),
    Send {
        target: String,
        payload: AxRustExpr,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxHookPhasePlan {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxValuePlan {
    Expr(AxRustExpr),
    Query(AxQueryPlan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxAssignmentPlan {
    pub name: String,
    pub value: AxRustExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxReturnPlan {
    Expr(AxRustExpr),
    Json(AxRustExpr),
    Redirect {
        target: AxRustExpr,
        status: Option<u16>,
    },
    NoContent,
    Ok,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxQueryPlan {
    pub source: AxQuerySourcePlan,
    pub filters: Vec<AxQueryFilterPlan>,
    pub orders: Vec<AxQueryOrderPlan>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxQuerySourcePlan {
    Stream {
        collection: String,
    },
    ContentCollection {
        collection: String,
    },
    RawSql {
        sql: String,
        params: Vec<AxRustExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxQueryFilterPlan {
    pub field: String,
    pub op: AxQueryFilterOpPlan,
    pub value: AxRustExpr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxQueryFilterOpPlan {
    Eq,
    Ne,
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxQueryOrderPlan {
    pub field: String,
    pub direction: AxQueryOrderDirectionPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxQueryOrderDirectionPlan {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxRustExpr {
    pub code: String,
}

impl AxRustExpr {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxBackendLowerError {
    #[error("handler name cannot be empty")]
    EmptyHandlerName,
    #[error("route path cannot be empty")]
    EmptyRoutePath,
    #[error("route method cannot be empty")]
    EmptyRouteMethod,
    #[error("input field type cannot be empty for `{field}`")]
    EmptyInputType { field: String },
    #[error("invalid runtime env path `{path}`")]
    InvalidRuntimeEnvPath { path: String },
}

pub fn lower_backend_document(
    document: &AxBackendDocument,
) -> Result<AxBackendPlan, AxBackendLowerError> {
    let mut envs = Vec::new();
    let mut globals = Vec::new();
    let mut functions = Vec::new();
    let mut handlers = Vec::new();

    for block in &document.blocks {
        match block {
            AxBackendBlock::Backend(root) => {
                for stmt in &root.body {
                    match stmt {
                        AxBackendStmt::Env(env) => envs.push(lower_env(env)),
                        _ => globals.push(lower_step(stmt)),
                    }
                }
            }
            AxBackendBlock::Function(function) => functions.push(lower_function(function)?),
            _ => handlers.push(lower_backend_block(block)?),
        }
    }

    Ok(AxBackendPlan::with_globals(
        envs, globals, functions, handlers,
    ))
}

fn lower_backend_block(block: &AxBackendBlock) -> Result<AxHandlerPlan, AxBackendLowerError> {
    match block {
        AxBackendBlock::Backend(_) => unreachable!("backend root is lowered at document level"),
        AxBackendBlock::Route(route) => lower_route(route),
        AxBackendBlock::Loader(loader) => lower_loader(loader),
        AxBackendBlock::Action(action) => lower_action(action),
        AxBackendBlock::Function(_) => {
            unreachable!("domain functions are lowered at document level")
        }
        AxBackendBlock::Job(job) => lower_job(job),
    }
}

fn lower_function(function: &AxBackendFunction) -> Result<AxFunctionPlan, AxBackendLowerError> {
    let name = function.name.trim();
    if name.is_empty() {
        return Err(AxBackendLowerError::EmptyHandlerName);
    }

    let input = function
        .input
        .iter()
        .map(lower_input_field)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxFunctionPlan {
        name: function.name.clone(),
        rust_fn: format!("fn_{}", normalize_ident(name)),
        returns: function.returns.clone(),
        input,
        steps: lower_steps(&function.body),
        exported: function.exported,
    })
}

fn lower_route(route: &AxRoute) -> Result<AxHandlerPlan, AxBackendLowerError> {
    if route.method.trim().is_empty() {
        return Err(AxBackendLowerError::EmptyRouteMethod);
    }
    if route.path.trim().is_empty() {
        return Err(AxBackendLowerError::EmptyRoutePath);
    }

    let name = format!("route {} {}", route.method, route.path);
    let rust_fn = format!(
        "route_{}_{}",
        route_method_ident(&route.method),
        route_path_ident(&route.path)
    );

    let input = route
        .input
        .iter()
        .map(lower_input_field)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxHandlerPlan {
        name,
        rust_fn,
        kind: AxHandlerKind::Route {
            method: route.method.clone(),
            path: route.path.clone(),
            returns: route.returns.clone(),
            input,
        },
        steps: lower_steps(&route.body),
    })
}

fn lower_loader(loader: &AxLoader) -> Result<AxHandlerPlan, AxBackendLowerError> {
    let name = loader.name.trim();
    if name.is_empty() {
        return Err(AxBackendLowerError::EmptyHandlerName);
    }

    let input = loader
        .input
        .iter()
        .map(lower_input_field)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxHandlerPlan {
        name: loader.name.clone(),
        rust_fn: format!("loader_{}", normalize_ident(name)),
        kind: AxHandlerKind::Loader {
            returns: loader.returns.clone(),
            input,
        },
        steps: lower_steps(&loader.body),
    })
}

fn lower_action(action: &AxAction) -> Result<AxHandlerPlan, AxBackendLowerError> {
    let name = action.name.trim();
    if name.is_empty() {
        return Err(AxBackendLowerError::EmptyHandlerName);
    }

    let input = action
        .input
        .iter()
        .map(lower_input_field)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(AxHandlerPlan {
        name: action.name.clone(),
        rust_fn: format!("action_{}", normalize_ident(name)),
        kind: AxHandlerKind::Action {
            returns: action.returns.clone(),
            input,
        },
        steps: lower_steps(&action.body),
    })
}

fn lower_job(job: &AxJob) -> Result<AxHandlerPlan, AxBackendLowerError> {
    let name = job.name.trim();
    if name.is_empty() {
        return Err(AxBackendLowerError::EmptyHandlerName);
    }

    Ok(AxHandlerPlan {
        name: job.name.clone(),
        rust_fn: format!("job_{}", normalize_ident(name)),
        kind: AxHandlerKind::Job,
        steps: lower_steps(&job.body),
    })
}

fn lower_input_field(field: &AxField) -> Result<AxFieldPlan, AxBackendLowerError> {
    if field.ty.trim().is_empty() {
        return Err(AxBackendLowerError::EmptyInputType {
            field: field.name.clone(),
        });
    }

    Ok(AxFieldPlan {
        name: field.name.clone(),
        rust_ty: map_input_type(&field.ty),
        optional: field.optional,
        default: field.default.as_ref().map(lower_expr),
    })
}

fn lower_env(env: &AxBackendEnv) -> AxEnvPlan {
    AxEnvPlan {
        name: env.name.clone(),
        visibility: match env.visibility {
            AxBackendEnvVisibility::Public => AxEnvVisibilityPlan::Public,
            AxBackendEnvVisibility::Secret => AxEnvVisibilityPlan::Secret,
        },
        ty: env.ty.clone(),
    }
}

fn lower_steps(steps: &[AxBackendStmt]) -> Vec<AxStepPlan> {
    steps.iter().map(lower_step).collect()
}

fn lower_step(step: &AxBackendStmt) -> AxStepPlan {
    match step {
        AxBackendStmt::Data(data) => AxStepPlan::Let {
            binding: data.name.clone(),
            value: lower_backend_value(&data.value),
        },
        AxBackendStmt::Env(_) => unreachable!("env declarations are lowered at document level"),
        AxBackendStmt::Insert(mutation) => AxStepPlan::Insert {
            collection: mutation.collection.clone(),
            fields: lower_assignments(&mutation.fields),
        },
        AxBackendStmt::Update(mutation) => AxStepPlan::Update {
            collection: mutation.collection.clone(),
            fields: lower_assignments(&mutation.fields),
            filters: mutation
                .filters
                .iter()
                .map(|filter| AxQueryFilterPlan {
                    field: filter.field.clone(),
                    op: lower_query_filter_op(filter.op),
                    value: lower_expr(&filter.value),
                })
                .collect(),
        },
        AxBackendStmt::Delete(mutation) => AxStepPlan::Delete {
            collection: mutation.collection.clone(),
            filters: mutation
                .filters
                .iter()
                .map(|filter| AxQueryFilterPlan {
                    field: filter.field.clone(),
                    op: lower_query_filter_op(filter.op),
                    value: lower_expr(&filter.value),
                })
                .collect(),
        },
        AxBackendStmt::Revalidate(revalidate) => AxStepPlan::Revalidate {
            target: lower_expr(&revalidate.target),
            literal: revalidate.literal,
        },
        AxBackendStmt::Patch(patch) => AxStepPlan::Patch {
            signal: lower_patch_signal(&patch.signal),
            value: lower_expr(&patch.value),
        },
        AxBackendStmt::Hook(hook) => AxStepPlan::Hook {
            phase: match hook.phase {
                AxHookPhase::Before => AxHookPhasePlan::Before,
                AxHookPhase::After => AxHookPhasePlan::After,
            },
            value: lower_expr(&hook.value),
        },
        AxBackendStmt::Header(header) => AxStepPlan::Header {
            name: lower_expr(&header.name),
            value: lower_expr(&header.value),
        },
        AxBackendStmt::Cookie(cookie) => AxStepPlan::Cookie {
            name: lower_expr(&cookie.name),
            value: lower_expr(&cookie.value),
        },
        AxBackendStmt::ClearCookie(name) => AxStepPlan::ClearCookie {
            name: lower_expr(name),
        },
        AxBackendStmt::Require(requirement) => AxStepPlan::Require {
            value: lower_expr(&requirement.value),
            fallback: requirement.fallback.as_ref().map(lower_return),
        },
        AxBackendStmt::Return(value) => AxStepPlan::Return(lower_return(value)),
        AxBackendStmt::Send(send) => AxStepPlan::Send {
            target: send.target.clone(),
            payload: lower_expr(&send.payload),
        },
    }
}

fn lower_backend_value(value: &AxBackendValue) -> AxValuePlan {
    match value {
        AxBackendValue::Expr(expr) => AxValuePlan::Expr(lower_expr(expr)),
        AxBackendValue::Query(query) => AxValuePlan::Query(lower_query(query)),
    }
}

fn lower_assignments(fields: &[AxAssignment]) -> Vec<AxAssignmentPlan> {
    fields
        .iter()
        .map(|field| AxAssignmentPlan {
            name: field.name.clone(),
            value: lower_expr(&field.value),
        })
        .collect()
}

fn lower_return(value: &AxReturn) -> AxReturnPlan {
    match value {
        AxReturn::Expr(expr) => lower_return_expr(expr),
        AxReturn::Ok => AxReturnPlan::Ok,
    }
}

fn lower_return_expr(expr: &AxExpr) -> AxReturnPlan {
    let AxExpr::Call { path, args } = expr else {
        return AxReturnPlan::Expr(lower_expr(expr));
    };

    let Some(name) = path.last().map(String::as_str) else {
        return AxReturnPlan::Expr(lower_expr(expr));
    };

    match name {
        "json" if args.len() == 1 => AxReturnPlan::Json(lower_expr(&args[0])),
        "redirect" if args.len() == 1 => AxReturnPlan::Redirect {
            target: lower_expr(&args[0]),
            status: None,
        },
        "redirect" if args.len() == 2 => {
            let status = match &args[0] {
                AxExpr::Number(value) => u16::try_from(*value).ok(),
                _ => None,
            };
            AxReturnPlan::Redirect {
                target: lower_expr(&args[1]),
                status,
            }
        }
        "noContent" | "no_content" if args.is_empty() => AxReturnPlan::NoContent,
        _ => AxReturnPlan::Expr(lower_expr(expr)),
    }
}

fn lower_query(query: &AxQuerySpec) -> AxQueryPlan {
    AxQueryPlan {
        source: match &query.source {
            AxQuerySource::Stream { collection } => AxQuerySourcePlan::Stream {
                collection: collection.clone(),
            },
            AxQuerySource::ContentCollection { collection } => {
                AxQuerySourcePlan::ContentCollection {
                    collection: collection.clone(),
                }
            }
            AxQuerySource::RawSql { sql, params } => AxQuerySourcePlan::RawSql {
                sql: sql.clone(),
                params: params.iter().map(lower_expr).collect(),
            },
        },
        filters: query
            .filters
            .iter()
            .map(|filter| AxQueryFilterPlan {
                field: filter.field.clone(),
                op: lower_query_filter_op(filter.op),
                value: lower_expr(&filter.value),
            })
            .collect(),
        orders: query
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
        limit: query.limit,
        offset: query.offset,
    }
}

fn lower_query_filter_op(op: AxQueryFilterOp) -> AxQueryFilterOpPlan {
    match op {
        AxQueryFilterOp::Eq => AxQueryFilterOpPlan::Eq,
        AxQueryFilterOp::Ne => AxQueryFilterOpPlan::Ne,
        AxQueryFilterOp::In => AxQueryFilterOpPlan::In,
        AxQueryFilterOp::NotIn => AxQueryFilterOpPlan::NotIn,
        AxQueryFilterOp::IsNull => AxQueryFilterOpPlan::IsNull,
        AxQueryFilterOp::IsNotNull => AxQueryFilterOpPlan::IsNotNull,
    }
}

fn lower_expr(expr: &AxExpr) -> AxRustExpr {
    AxRustExpr::new(render_expr(expr))
}

fn lower_patch_signal(expr: &AxExpr) -> AxRustExpr {
    match expr {
        AxExpr::Identifier(name) => {
            let signal = format!("root:{name}:1");
            AxRustExpr::new(format!("{signal:?}.to_string()"))
        }
        _ => lower_expr(expr),
    }
}

fn render_expr(expr: &AxExpr) -> String {
    if let Some(env_expr) = try_render_runtime_env(expr) {
        return env_expr;
    }
    if let Some(auth_expr) = try_render_auth(expr) {
        return auth_expr;
    }

    match expr {
        AxExpr::String(value) => format!("{value:?}.to_string()"),
        AxExpr::Number(value) => value.to_string(),
        AxExpr::Bool(value) => value.to_string(),
        AxExpr::List(items) => {
            let items = items.iter().map(render_expr).collect::<Vec<_>>().join(", ");
            format!("vec![{items}]")
        }
        AxExpr::Identifier(name) => name.clone(),
        AxExpr::Unary { op, expr } => format!("({}{})", render_unary_op(*op), render_expr(expr)),
        AxExpr::Binary { op, left, right } => {
            if *op == AxBinaryOp::Fallback {
                return format!("({}).unwrap_or({})", render_expr(left), render_expr(right));
            }
            if *op == AxBinaryOp::In {
                return format!("({}).contains(&{})", render_expr(right), render_expr(left));
            }
            format!(
                "({} {} {})",
                render_expr(left),
                render_binary_op(*op),
                render_expr(right)
            )
        }
        AxExpr::Index { object, index } => {
            format!(
                "{}[{}]",
                render_index_object_expr(object),
                render_expr(index)
            )
        }
        AxExpr::Member { object, property } => format!("{}.{}", render_expr(object), property),
        AxExpr::OptionalMember { object, property } => {
            format!("{}.{}", render_expr(object), property)
        }
        AxExpr::Call { path, args } => {
            if path.as_slice() == ["list"] {
                let args = args.iter().map(render_expr).collect::<Vec<_>>().join(", ");
                return format!("vec![{args}]");
            }
            let fn_name = path.join("::");
            let args = args.iter().map(render_expr).collect::<Vec<_>>().join(", ");
            format!("{fn_name}({args})")
        }
    }
}

fn render_index_object_expr(expr: &AxExpr) -> String {
    let value = render_expr(expr);
    if index_object_needs_grouping(expr) {
        format!("({value})")
    } else {
        value
    }
}

fn index_object_needs_grouping(expr: &AxExpr) -> bool {
    matches!(expr, AxExpr::Binary { .. } | AxExpr::Unary { .. })
}

fn render_unary_op(op: AxUnaryOp) -> &'static str {
    match op {
        AxUnaryOp::Not => "!",
        AxUnaryOp::Neg => "-",
    }
}

fn render_binary_op(op: AxBinaryOp) -> &'static str {
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

fn try_render_auth(expr: &AxExpr) -> Option<String> {
    let path = expr_member_path(expr)?;
    let normalized = path
        .iter()
        .map(|segment| segment.as_str())
        .collect::<Vec<_>>();

    match normalized.as_slice() {
        ["Auth", "bearer"] => Some("Auth.bearer".to_string()),
        ["Auth", "session"] => Some("Auth.session".to_string()),
        ["Auth", "signedSession"] => Some("Auth.signedSession".to_string()),
        _ => None,
    }
}

fn try_render_runtime_env(expr: &AxExpr) -> Option<String> {
    let path = expr_member_path(expr)?;
    let normalized = path
        .iter()
        .map(|segment| segment.as_str())
        .collect::<Vec<_>>();

    match normalized.as_slice() {
        ["env", key] => Some(format!("runtime.env().value({key:?})?")),
        ["Runtime", "Env", "public", key] => Some(format!("runtime.env().public({key:?})?")),
        ["Runtime", "Env", "secret", key] => Some(format!("runtime.env().secret({key:?})?")),
        _ => None,
    }
}

fn expr_member_path(expr: &AxExpr) -> Option<Vec<String>> {
    match expr {
        AxExpr::Identifier(name) => Some(vec![name.clone()]),
        AxExpr::Member { object, property } => {
            let mut path = expr_member_path(object)?;
            path.push(property.clone());
            Some(path)
        }
        AxExpr::OptionalMember { object, property } => {
            let mut path = expr_member_path(object)?;
            path.push(property.clone());
            Some(path)
        }
        _ => None,
    }
}

fn map_input_type(ty: &str) -> String {
    match ty.trim() {
        "string" => "String".to_string(),
        "bool" | "boolean" => "bool".to_string(),
        "i64" | "int" | "integer" => "i64".to_string(),
        "u64" => "u64".to_string(),
        "f64" | "float" | "number" => "f64".to_string(),
        other => other.to_string(),
    }
}

fn normalize_ident(input: &str) -> String {
    let mut out = String::new();
    let mut previous_was_sep = true;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && !out.is_empty() && !previous_was_sep {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_was_sep = false;
        } else if !previous_was_sep {
            out.push('_');
            previous_was_sep = true;
        }
    }

    out.trim_matches('_').to_string()
}

fn route_path_ident(path: &str) -> String {
    let normalized = normalize_ident(path);
    if normalized.is_empty() {
        "root".to_string()
    } else {
        normalized
    }
}

fn route_method_ident(method: &str) -> String {
    let method = method.trim();
    if method
        .chars()
        .all(|ch| !ch.is_ascii_alphabetic() || ch.is_ascii_uppercase())
    {
        method.to_ascii_lowercase()
    } else {
        normalize_ident(method)
    }
}

pub mod prelude {
    pub use super::lower_backend_document;
    pub use super::AxAssignmentPlan;
    pub use super::AxBackendLowerError;
    pub use super::AxBackendPlan;
    pub use super::AxEnvPlan;
    pub use super::AxEnvVisibilityPlan;
    pub use super::AxFieldPlan;
    pub use super::AxFunctionPlan;
    pub use super::AxHandlerKind;
    pub use super::AxHandlerPlan;
    pub use super::AxHookPhasePlan;
    pub use super::AxQueryFilterOpPlan;
    pub use super::AxQueryFilterPlan;
    pub use super::AxQueryOrderDirectionPlan;
    pub use super::AxQueryOrderPlan;
    pub use super::AxQueryPlan;
    pub use super::AxQuerySourcePlan;
    pub use super::AxReturnPlan;
    pub use super::AxRustExpr;
    pub use super::AxStepPlan;
    pub use super::AxValuePlan;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax_backend_parser::parse_backend_ax;

    #[test]
    fn lowers_exported_domain_function_into_function_plan() {
        let document = parse_backend_ax(
            r#"
export fn normalizeStatus(status?: String) -> String {
  return status ?? "published"
}
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert!(plan.handlers.is_empty());
        assert_eq!(plan.functions.len(), 1);
        let function = &plan.functions[0];
        assert_eq!(function.name, "normalizeStatus");
        assert_eq!(function.rust_fn, "fn_normalize_status");
        assert_eq!(function.returns.as_deref(), Some("String"));
        assert!(function.exported);
        assert_eq!(function.input.len(), 1);
        assert_eq!(function.input[0].name, "status");
        assert!(function.input[0].optional);
        assert_eq!(
            function.steps[0],
            AxStepPlan::Return(AxReturnPlan::Expr(AxRustExpr::new(
                r#"status ?? "published""#
            )))
        );
    }

    #[test]
    fn lowers_loader_query_into_backend_plan() {
        let document = parse_backend_ax(
            r#"
loader PostsList
  data posts = db.posts.all()
    where status = "published"
    order created_at desc
    limit 20
    offset 40
  return posts
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(plan.handlers.len(), 1);
        let handler = &plan.handlers[0];
        assert_eq!(handler.name, "PostsList");
        assert_eq!(handler.rust_fn, "loader_posts_list");
        assert_eq!(
            handler.kind,
            AxHandlerKind::Loader {
                returns: None,
                input: Vec::new(),
            }
        );

        let AxStepPlan::Let { binding, value } = &handler.steps[0] else {
            panic!("expected let step");
        };
        assert_eq!(binding, "posts");
        assert_eq!(
            value,
            &AxValuePlan::Query(AxQueryPlan {
                source: AxQuerySourcePlan::Stream {
                    collection: "posts".to_string(),
                },
                filters: vec![AxQueryFilterPlan {
                    field: "status".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new(r#""published".to_string()"#),
                }],
                orders: vec![AxQueryOrderPlan {
                    field: "created_at".to_string(),
                    direction: AxQueryOrderDirectionPlan::Desc,
                }],
                limit: Some(20),
                offset: Some(40),
            })
        );

        assert_eq!(
            handler.steps[1],
            AxStepPlan::Return(AxReturnPlan::Expr(AxRustExpr::new("posts")))
        );
    }

    #[test]
    fn lowers_db_all_query_into_backend_plan() {
        let document = parse_backend_ax(
            r#"
loader PostsList
  data posts = db.posts.all()
  return posts
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        let AxStepPlan::Let { value, .. } = &handler.steps[0] else {
            panic!("expected let step");
        };
        assert_eq!(
            value,
            &AxValuePlan::Query(AxQueryPlan {
                source: AxQuerySourcePlan::Stream {
                    collection: "posts".to_string(),
                },
                filters: Vec::new(),
                orders: Vec::new(),
                limit: None,
                offset: None,
            })
        );
    }

    #[test]
    fn lowers_action_input_and_mutation_into_rust_shaped_plan() {
        let document = parse_backend_ax(
            r#"
action CreatePost
  input:
    title: string
    featured: bool

  insert "posts"
    title: input.title
    featured: input.featured

  revalidate "/posts"
  return ok
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        let handler = &plan.handlers[0];
        assert_eq!(handler.rust_fn, "action_create_post");
        assert_eq!(
            handler.kind,
            AxHandlerKind::Action {
                returns: None,
                input: vec![
                    AxFieldPlan {
                        name: "title".to_string(),
                        rust_ty: "String".to_string(),
                        optional: false,
                        default: None,
                    },
                    AxFieldPlan {
                        name: "featured".to_string(),
                        rust_ty: "bool".to_string(),
                        optional: false,
                        default: None,
                    },
                ],
            }
        );
        assert_eq!(
            handler.steps[0],
            AxStepPlan::Insert {
                collection: "posts".to_string(),
                fields: vec![
                    AxAssignmentPlan {
                        name: "title".to_string(),
                        value: AxRustExpr::new("input.title"),
                    },
                    AxAssignmentPlan {
                        name: "featured".to_string(),
                        value: AxRustExpr::new("input.featured"),
                    },
                ],
            }
        );
        assert_eq!(
            handler.steps[1],
            AxStepPlan::Revalidate {
                target: AxRustExpr::new(r#""/posts".to_string()"#),
                literal: false,
            }
        );
        assert_eq!(handler.steps[2], AxStepPlan::Return(AxReturnPlan::Ok));
    }

    #[test]
    fn lowers_action_patch_step_into_plan() {
        let document = parse_backend_ax(
            r#"
action SetTheme
  input:
    theme: string

  patch theme = input.theme
  return ok
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        assert_eq!(
            handler.steps[0],
            AxStepPlan::Patch {
                signal: AxRustExpr::new(r#""root:theme:1".to_string()"#),
                value: AxRustExpr::new("input.theme"),
            }
        );
    }

    #[test]
    fn keeps_explicit_patch_signal_strings() {
        let document = parse_backend_ax(
            r#"
action SetTheme
  input:
    theme: string

  patch "root:theme:2" = input.theme
  return ok
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        assert_eq!(
            handler.steps[0],
            AxStepPlan::Patch {
                signal: AxRustExpr::new(r#""root:theme:2".to_string()"#),
                value: AxRustExpr::new("input.theme"),
            }
        );
    }

    #[test]
    fn lowers_update_where_clause_into_runtime_filters() {
        let document = parse_backend_ax(
            r#"
action PublishPost
  input:
    id: i64
    title: string

  update "posts"
    title: input.title
    where id = input.id

  return ok
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        assert_eq!(
            handler.steps[0],
            AxStepPlan::Update {
                collection: "posts".to_string(),
                fields: vec![AxAssignmentPlan {
                    name: "title".to_string(),
                    value: AxRustExpr::new("input.title"),
                }],
                filters: vec![AxQueryFilterPlan {
                    field: "id".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new("input.id"),
                }],
            }
        );
    }

    #[test]
    fn lowers_delete_where_clause_into_runtime_filters() {
        let document = parse_backend_ax(
            r#"
action RemovePost
  input:
    id: i64

  delete "posts"
    where id = input.id

  return ok
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        assert_eq!(
            handler.steps[0],
            AxStepPlan::Delete {
                collection: "posts".to_string(),
                filters: vec![AxQueryFilterPlan {
                    field: "id".to_string(),
                    op: AxQueryFilterOpPlan::Eq,
                    value: AxRustExpr::new("input.id"),
                }],
            }
        );
    }

    #[test]
    fn lowers_route_name_into_stable_rust_fn() {
        let document = AxBackendDocument::new([AxBackendBlock::Route(AxRoute::new(
            "GET",
            "/api/posts",
            [AxBackendStmt::r#return(AxExpr::ident("posts"))],
        ))]);

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(plan.handlers[0].rust_fn, "route_get_api_posts");
        assert_eq!(
            plan.handlers[0].kind,
            AxHandlerKind::Route {
                method: "GET".to_string(),
                path: "/api/posts".to_string(),
                returns: None,
                input: Vec::new(),
            }
        );
    }

    #[test]
    fn lowers_backend_return_contracts() {
        let document = parse_backend_ax(
            r#"
loader PostsList -> Post[]
  return posts

route GET "/api/posts" -> Post[]
  return json(posts)

action CreatePost -> Post
  input:
    title: string

  return json(input.title)
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].kind,
            AxHandlerKind::Loader {
                returns: Some("Post[]".to_string()),
                input: Vec::new(),
            }
        );
        assert_eq!(
            plan.handlers[1].kind,
            AxHandlerKind::Route {
                method: "GET".to_string(),
                path: "/api/posts".to_string(),
                returns: Some("Post[]".to_string()),
                input: Vec::new(),
            }
        );
        assert_eq!(
            plan.handlers[2].kind,
            AxHandlerKind::Action {
                returns: Some("Post".to_string()),
                input: vec![AxFieldPlan {
                    name: "title".to_string(),
                    rust_ty: "String".to_string(),
                    optional: false,
                    default: None,
                }],
            }
        );
    }

    #[test]
    fn lowers_query_function_inputs_into_loader_plan() {
        let document = parse_backend_ax(
            r#"
query loadPosts(status: String, limit: i64 = 6) -> Post[]
  data posts = db.posts.all()
    where status = input.status
  return posts
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].kind,
            AxHandlerKind::Loader {
                returns: Some("Post[]".to_string()),
                input: vec![
                    AxFieldPlan {
                        name: "status".to_string(),
                        rust_ty: "String".to_string(),
                        optional: false,
                        default: None,
                    },
                    AxFieldPlan {
                        name: "limit".to_string(),
                        rust_ty: "i64".to_string(),
                        optional: false,
                        default: Some(AxRustExpr::new("6")),
                    },
                ],
            }
        );
    }

    #[test]
    fn lowers_runtime_env_access_into_runtime_calls() {
        let document = parse_backend_ax(
            r#"
loader PostsList
  data db_url = Runtime.Env.secret.db_url
  data app_name = Runtime.Env.public.app_name
  return app_name
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].steps[0],
            AxStepPlan::Let {
                binding: "db_url".to_string(),
                value: AxValuePlan::Expr(AxRustExpr::new(r#"runtime.env().secret("db_url")?"#,)),
            }
        );
        assert_eq!(
            plan.handlers[0].steps[1],
            AxStepPlan::Let {
                binding: "app_name".to_string(),
                value: AxValuePlan::Expr(AxRustExpr::new(r#"runtime.env().public("app_name")?"#,)),
            }
        );
    }

    #[test]
    fn lowers_route_http_return_helpers() {
        let document = parse_backend_ax(
            r#"
route GET "/api/posts"
  data posts = db.posts.all()
  return json(posts)

route GET "/go"
  return redirect("/next")

route DELETE "/api/posts"
  return noContent()
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].steps[1],
            AxStepPlan::Return(AxReturnPlan::Json(AxRustExpr::new("posts")))
        );
        assert_eq!(
            plan.handlers[1].steps[0],
            AxStepPlan::Return(AxReturnPlan::Redirect {
                target: AxRustExpr::new(r#""/next".to_string()"#),
                status: None,
            })
        );
        assert_eq!(
            plan.handlers[2].steps[0],
            AxStepPlan::Return(AxReturnPlan::NoContent)
        );
    }

    #[test]
    fn lowers_route_response_metadata_steps() {
        let document = parse_backend_ax(
            r#"
route GET "/api/session"
  require request.cookies.session
  header "Cache-Control" = "no-store"
  cookie "theme" = query.theme
  clearCookie "flash"
  return json("ok")
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        assert_eq!(
            handler.steps[0],
            AxStepPlan::Require {
                value: AxRustExpr::new("request.cookies.session"),
                fallback: None,
            }
        );
        assert_eq!(
            handler.steps[1],
            AxStepPlan::Header {
                name: AxRustExpr::new(r#""Cache-Control".to_string()"#),
                value: AxRustExpr::new(r#""no-store".to_string()"#),
            }
        );
        assert_eq!(
            handler.steps[2],
            AxStepPlan::Cookie {
                name: AxRustExpr::new(r#""theme".to_string()"#),
                value: AxRustExpr::new("query.theme"),
            }
        );
        assert_eq!(
            handler.steps[3],
            AxStepPlan::ClearCookie {
                name: AxRustExpr::new(r#""flash".to_string()"#),
            }
        );
    }

    #[test]
    fn lowers_require_fallback_step() {
        let document = parse_backend_ax(
            r#"
route GET "/api/admin"
  require request.cookies.session else redirect("/login")
  return json("ok")
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].steps[0],
            AxStepPlan::Require {
                value: AxRustExpr::new("request.cookies.session"),
                fallback: Some(AxReturnPlan::Redirect {
                    target: AxRustExpr::new(r#""/login".to_string()"#),
                    status: None,
                }),
            }
        );
    }

    #[test]
    fn lowers_route_hooks_into_plan() {
        let document = parse_backend_ax(
            r#"
route GET "/api/admin"
  before Auth.session
  before Security.headers
  after Cache.noStore
  return json("ok")
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");
        let handler = &plan.handlers[0];

        assert_eq!(
            handler.steps[0],
            AxStepPlan::Hook {
                phase: AxHookPhasePlan::Before,
                value: AxRustExpr::new("Auth.session"),
            }
        );
        assert_eq!(
            handler.steps[1],
            AxStepPlan::Hook {
                phase: AxHookPhasePlan::Before,
                value: AxRustExpr::new("Security.headers"),
            }
        );
        assert_eq!(
            handler.steps[2],
            AxStepPlan::Hook {
                phase: AxHookPhasePlan::After,
                value: AxRustExpr::new("Cache.noStore"),
            }
        );
    }

    #[test]
    fn lowers_route_input_fields() {
        let document = parse_backend_ax(
            r#"
route POST "/api/posts"
  input:
    title: string
    featured?: bool = false

  return json(input.title)
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].kind,
            AxHandlerKind::Route {
                method: "POST".to_string(),
                path: "/api/posts".to_string(),
                returns: None,
                input: vec![
                    AxFieldPlan {
                        name: "title".to_string(),
                        rust_ty: "String".to_string(),
                        optional: false,
                        default: None,
                    },
                    AxFieldPlan {
                        name: "featured".to_string(),
                        rust_ty: "bool".to_string(),
                        optional: true,
                        default: Some(AxRustExpr::new("false")),
                    },
                ],
            }
        );
    }

    #[test]
    fn lowers_auth_signed_session_alias() {
        let document = parse_backend_ax(
            r#"
route GET "/api/admin"
  require Auth.signedSession else redirect("/login")
  data session = Auth.signedSession
  return json(session)
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].steps[0],
            AxStepPlan::Require {
                value: AxRustExpr::new("Auth.signedSession"),
                fallback: Some(AxReturnPlan::Redirect {
                    target: AxRustExpr::new(r#""/login".to_string()"#),
                    status: None,
                }),
            }
        );
        assert_eq!(
            plan.handlers[0].steps[1],
            AxStepPlan::Let {
                binding: "session".to_string(),
                value: AxValuePlan::Expr(AxRustExpr::new("Auth.signedSession")),
            }
        );
    }

    #[test]
    fn lowers_backend_root_data_as_global_steps() {
        let document = parse_backend_ax(
            r#"
backend
  data themes = ["silver", "bronze", "gold"]
  env PUBLIC_SITE_URL: Public<String>

action SetTheme
  input:
    theme: string

  require input.theme in themes else error "Theme is not supported."
  return ok
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(plan.globals.len(), 1);
        assert_eq!(plan.envs.len(), 1);
        assert_eq!(plan.envs[0].name, "PUBLIC_SITE_URL");
        assert_eq!(plan.envs[0].visibility, AxEnvVisibilityPlan::Public);
        assert_eq!(
            plan.globals[0],
            AxStepPlan::Let {
                binding: "themes".to_string(),
                value: AxValuePlan::Expr(AxRustExpr::new(
                    r#"vec!["silver".to_string(), "bronze".to_string(), "gold".to_string()]"#,
                )),
            }
        );
        assert_eq!(plan.handlers.len(), 1);
        assert_eq!(plan.handlers[0].name, "SetTheme");
    }

    #[test]
    fn lowers_env_scope_into_runtime_env_lookup() {
        let document = parse_backend_ax(
            r#"
backend
  env PUBLIC_SITE_URL: Public<String>

loader SiteConfig
  data siteUrl = env.PUBLIC_SITE_URL
  return siteUrl
"#,
        )
        .expect("document should parse");

        let plan = lower_backend_document(&document).expect("document should lower");

        assert_eq!(
            plan.handlers[0].steps[0],
            AxStepPlan::Let {
                binding: "siteUrl".to_string(),
                value: AxValuePlan::Expr(AxRustExpr::new(
                    r#"runtime.env().value("PUBLIC_SITE_URL")?"#
                )),
            }
        );
    }
}
