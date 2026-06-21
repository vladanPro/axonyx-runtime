use serde::{Deserialize, Serialize};

use crate::ax_ast::prelude::AxExpr;
use crate::ax_query_ast::prelude::{AxQueryFilter, AxQuerySpec};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendDocument {
    pub imports: Vec<AxBackendImport>,
    pub blocks: Vec<AxBackendBlock>,
}

impl AxBackendDocument {
    pub fn new(blocks: impl IntoIterator<Item = AxBackendBlock>) -> Self {
        Self {
            imports: Vec::new(),
            blocks: blocks.into_iter().collect(),
        }
    }

    pub fn with_imports(
        imports: impl IntoIterator<Item = AxBackendImport>,
        blocks: impl IntoIterator<Item = AxBackendBlock>,
    ) -> Self {
        Self {
            imports: imports.into_iter().collect(),
            blocks: blocks.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendImport {
    pub bindings: Vec<AxBackendImportBinding>,
    pub source: String,
}

impl AxBackendImport {
    pub fn new(
        bindings: impl IntoIterator<Item = AxBackendImportBinding>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
            source: source.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendImportBinding {
    pub imported: String,
    pub local: String,
}

impl AxBackendImportBinding {
    pub fn new(imported: impl Into<String>, local: impl Into<String>) -> Self {
        Self {
            imported: imported.into(),
            local: local.into(),
        }
    }

    pub fn namespace(local: impl Into<String>) -> Self {
        Self {
            imported: "*".to_string(),
            local: local.into(),
        }
    }

    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            imported: name.clone(),
            local: name,
        }
    }

    pub fn is_namespace(&self) -> bool {
        self.imported == "*"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxBackendBlock {
    Backend(AxBackendRoot),
    Route(AxRoute),
    Loader(AxLoader),
    Action(AxAction),
    Function(AxBackendFunction),
    Job(AxJob),
    Scope(AxScope),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendRoot {
    pub body: Vec<AxBackendStmt>,
}

impl AxBackendRoot {
    pub fn new(body: impl IntoIterator<Item = AxBackendStmt>) -> Self {
        Self {
            body: body.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxRoute {
    pub method: String,
    pub path: String,
    pub returns: Option<String>,
    pub input: Vec<AxField>,
    pub body: Vec<AxBackendStmt>,
}

impl AxRoute {
    pub fn new(
        method: impl Into<String>,
        path: impl Into<String>,
        body: impl IntoIterator<Item = AxBackendStmt>,
    ) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            returns: None,
            input: Vec::new(),
            body: body.into_iter().collect(),
        }
    }

    pub fn returns(mut self, ty: impl Into<String>) -> Self {
        self.returns = Some(ty.into());
        self
    }

    pub fn input(mut self, fields: impl IntoIterator<Item = AxField>) -> Self {
        self.input = fields.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxLoader {
    pub name: String,
    pub returns: Option<String>,
    pub input: Vec<AxField>,
    pub body: Vec<AxBackendStmt>,
    pub exported: bool,
}

impl AxLoader {
    pub fn new(name: impl Into<String>, body: impl IntoIterator<Item = AxBackendStmt>) -> Self {
        Self {
            name: name.into(),
            returns: None,
            input: Vec::new(),
            body: body.into_iter().collect(),
            exported: false,
        }
    }

    pub fn returns(mut self, ty: impl Into<String>) -> Self {
        self.returns = Some(ty.into());
        self
    }

    pub fn input(mut self, fields: impl IntoIterator<Item = AxField>) -> Self {
        self.input = fields.into_iter().collect();
        self
    }

    pub fn exported(mut self, exported: bool) -> Self {
        self.exported = exported;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxAction {
    pub name: String,
    pub returns: Option<String>,
    pub input: Vec<AxField>,
    pub body: Vec<AxBackendStmt>,
    pub exported: bool,
}

impl AxAction {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            returns: None,
            input: Vec::new(),
            body: Vec::new(),
            exported: false,
        }
    }

    pub fn returns(mut self, ty: impl Into<String>) -> Self {
        self.returns = Some(ty.into());
        self
    }

    pub fn input(mut self, fields: impl IntoIterator<Item = AxField>) -> Self {
        self.input = fields.into_iter().collect();
        self
    }

    pub fn body(mut self, body: impl IntoIterator<Item = AxBackendStmt>) -> Self {
        self.body = body.into_iter().collect();
        self
    }

    pub fn exported(mut self, exported: bool) -> Self {
        self.exported = exported;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendFunction {
    pub name: String,
    pub returns: Option<String>,
    pub input: Vec<AxField>,
    pub body: Vec<AxBackendStmt>,
    pub exported: bool,
}

impl AxBackendFunction {
    pub fn new(name: impl Into<String>, body: impl IntoIterator<Item = AxBackendStmt>) -> Self {
        Self {
            name: name.into(),
            returns: None,
            input: Vec::new(),
            body: body.into_iter().collect(),
            exported: false,
        }
    }

    pub fn returns(mut self, ty: impl Into<String>) -> Self {
        self.returns = Some(ty.into());
        self
    }

    pub fn input(mut self, fields: impl IntoIterator<Item = AxField>) -> Self {
        self.input = fields.into_iter().collect();
        self
    }

    pub fn exported(mut self, exported: bool) -> Self {
        self.exported = exported;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxJob {
    pub name: String,
    pub body: Vec<AxBackendStmt>,
}

impl AxJob {
    pub fn new(name: impl Into<String>, body: impl IntoIterator<Item = AxBackendStmt>) -> Self {
        Self {
            name: name.into(),
            body: body.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxScope {
    pub name: String,
    pub members: Vec<String>,
    pub body: Vec<AxScopeStmt>,
}

impl AxScope {
    pub fn new<S>(
        name: impl Into<String>,
        members: impl IntoIterator<Item = S>,
        body: impl IntoIterator<Item = AxScopeStmt>,
    ) -> Self
    where
        S: Into<String>,
    {
        Self {
            name: name.into(),
            members: members.into_iter().map(Into::into).collect(),
            body: body.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxScopeStmt {
    State(AxScopeState),
    Render(AxScopeRender),
}

impl AxScopeStmt {
    pub fn state(
        name: impl Into<String>,
        ty: impl Into<String>,
        default: impl Into<AxExpr>,
    ) -> Self {
        Self::State(AxScopeState::new(name, ty).default(default))
    }

    pub fn render(call: impl Into<AxExpr>) -> Self {
        Self::Render(AxScopeRender::new(call))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxScopeState {
    pub name: String,
    pub ty: String,
    pub default: Option<AxExpr>,
}

impl AxScopeState {
    pub fn new(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            default: None,
        }
    }

    pub fn default(mut self, default: impl Into<AxExpr>) -> Self {
        self.default = Some(default.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxScopeRender {
    pub call: AxExpr,
}

impl AxScopeRender {
    pub fn new(call: impl Into<AxExpr>) -> Self {
        Self { call: call.into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxBackendStmt {
    Data(AxBackendData),
    Env(AxBackendEnv),
    Insert(AxMutation),
    Update(AxMutation),
    Delete(AxMutation),
    Patch(AxPatch),
    Hook(AxHook),
    Header(AxResponseHeader),
    Cookie(AxResponseCookie),
    ClearCookie(AxExpr),
    Require(AxRequirement),
    Revalidate(AxRevalidate),
    Return(AxReturn),
    Send(AxSend),
}

impl AxBackendStmt {
    pub fn data(name: impl Into<String>, value: impl Into<AxBackendValue>) -> Self {
        Self::Data(AxBackendData::new(name, value))
    }

    pub fn env(
        name: impl Into<String>,
        visibility: AxBackendEnvVisibility,
        ty: impl Into<String>,
    ) -> Self {
        Self::Env(AxBackendEnv::new(name, visibility, ty))
    }

    pub fn insert(
        collection: impl Into<String>,
        fields: impl IntoIterator<Item = AxAssignment>,
    ) -> Self {
        Self::Insert(AxMutation::new(collection, fields))
    }

    pub fn update(
        collection: impl Into<String>,
        fields: impl IntoIterator<Item = AxAssignment>,
    ) -> Self {
        Self::Update(AxMutation::new(collection, fields))
    }

    pub fn delete(collection: impl Into<String>) -> Self {
        Self::Delete(AxMutation::new(collection, []))
    }

    pub fn revalidate(value: impl Into<AxExpr>) -> Self {
        Self::Revalidate(AxRevalidate::expression(value))
    }

    pub fn invalidate(value: impl Into<AxExpr>) -> Self {
        Self::Revalidate(AxRevalidate::literal(value))
    }

    pub fn patch(signal: impl Into<AxExpr>, value: impl Into<AxExpr>) -> Self {
        Self::Patch(AxPatch::new(signal, value))
    }

    pub fn before(value: impl Into<AxExpr>) -> Self {
        Self::Hook(AxHook::before(value))
    }

    pub fn after(value: impl Into<AxExpr>) -> Self {
        Self::Hook(AxHook::after(value))
    }

    pub fn header(name: impl Into<AxExpr>, value: impl Into<AxExpr>) -> Self {
        Self::Header(AxResponseHeader::new(name, value))
    }

    pub fn cookie(name: impl Into<AxExpr>, value: impl Into<AxExpr>) -> Self {
        Self::Cookie(AxResponseCookie::new(name, value))
    }

    pub fn clear_cookie(name: impl Into<AxExpr>) -> Self {
        Self::ClearCookie(name.into())
    }

    pub fn require(value: impl Into<AxExpr>) -> Self {
        Self::Require(AxRequirement::new(value))
    }

    pub fn require_with_fallback(value: impl Into<AxExpr>, fallback: impl Into<AxReturn>) -> Self {
        Self::Require(AxRequirement::new(value).fallback(fallback))
    }

    pub fn r#return(value: impl Into<AxReturn>) -> Self {
        Self::Return(value.into())
    }

    pub fn send(target: impl Into<String>, payload: impl Into<AxExpr>) -> Self {
        Self::Send(AxSend::new(target, payload))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxRevalidate {
    pub target: AxExpr,
    pub literal: bool,
}

impl AxRevalidate {
    pub fn expression(value: impl Into<AxExpr>) -> Self {
        Self {
            target: value.into(),
            literal: false,
        }
    }

    pub fn literal(value: impl Into<AxExpr>) -> Self {
        Self {
            target: value.into(),
            literal: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxHook {
    pub phase: AxHookPhase,
    pub value: AxExpr,
}

impl AxHook {
    pub fn before(value: impl Into<AxExpr>) -> Self {
        Self {
            phase: AxHookPhase::Before,
            value: value.into(),
        }
    }

    pub fn after(value: impl Into<AxExpr>) -> Self {
        Self {
            phase: AxHookPhase::After,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxHookPhase {
    Before,
    After,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendData {
    pub name: String,
    pub value: AxBackendValue,
}

impl AxBackendData {
    pub fn new(name: impl Into<String>, value: impl Into<AxBackendValue>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxBackendValue {
    Expr(AxExpr),
    Query(AxQuerySpec),
}

impl From<AxExpr> for AxBackendValue {
    fn from(value: AxExpr) -> Self {
        Self::Expr(value)
    }
}

impl From<AxQuerySpec> for AxBackendValue {
    fn from(value: AxQuerySpec) -> Self {
        Self::Query(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxMutation {
    pub collection: String,
    pub fields: Vec<AxAssignment>,
    pub filters: Vec<AxQueryFilter>,
}

impl AxMutation {
    pub fn new(
        collection: impl Into<String>,
        fields: impl IntoIterator<Item = AxAssignment>,
    ) -> Self {
        Self {
            collection: collection.into(),
            fields: fields.into_iter().collect(),
            filters: Vec::new(),
        }
    }

    pub fn filter(mut self, filter: AxQueryFilter) -> Self {
        self.filters.push(filter);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxAssignment {
    pub name: String,
    pub value: AxExpr,
}

impl AxAssignment {
    pub fn new(name: impl Into<String>, value: impl Into<AxExpr>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxField {
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub default: Option<AxExpr>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxBackendEnv {
    pub name: String,
    pub visibility: AxBackendEnvVisibility,
    pub ty: String,
}

impl AxBackendEnv {
    pub fn new(
        name: impl Into<String>,
        visibility: AxBackendEnvVisibility,
        ty: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            visibility,
            ty: ty.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxBackendEnvVisibility {
    Public,
    Secret,
}

impl AxField {
    pub fn new(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            optional: false,
            default: None,
        }
    }

    pub fn optional(name: impl Into<String>, ty: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            optional: true,
            default: None,
        }
    }

    pub fn with_default(
        name: impl Into<String>,
        ty: impl Into<String>,
        default: impl Into<AxExpr>,
    ) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            optional: false,
            default: Some(default.into()),
        }
    }

    pub fn optional_with_default(
        name: impl Into<String>,
        ty: impl Into<String>,
        default: impl Into<AxExpr>,
    ) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            optional: true,
            default: Some(default.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxPatch {
    pub signal: AxExpr,
    pub value: AxExpr,
}

impl AxPatch {
    pub fn new(signal: impl Into<AxExpr>, value: impl Into<AxExpr>) -> Self {
        Self {
            signal: signal.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxResponseHeader {
    pub name: AxExpr,
    pub value: AxExpr,
}

impl AxResponseHeader {
    pub fn new(name: impl Into<AxExpr>, value: impl Into<AxExpr>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxResponseCookie {
    pub name: AxExpr,
    pub value: AxExpr,
}

impl AxResponseCookie {
    pub fn new(name: impl Into<AxExpr>, value: impl Into<AxExpr>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxRequirement {
    pub value: AxExpr,
    pub fallback: Option<AxReturn>,
}

impl AxRequirement {
    pub fn new(value: impl Into<AxExpr>) -> Self {
        Self {
            value: value.into(),
            fallback: None,
        }
    }

    pub fn fallback(mut self, fallback: impl Into<AxReturn>) -> Self {
        self.fallback = Some(fallback.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxReturn {
    Expr(AxExpr),
    Ok,
}

impl From<AxExpr> for AxReturn {
    fn from(value: AxExpr) -> Self {
        Self::Expr(value)
    }
}

impl From<&str> for AxReturn {
    fn from(value: &str) -> Self {
        if value == "ok" {
            Self::Ok
        } else {
            Self::Expr(AxExpr::from(value))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxSend {
    pub target: String,
    pub payload: AxExpr,
}

impl AxSend {
    pub fn new(target: impl Into<String>, payload: impl Into<AxExpr>) -> Self {
        Self {
            target: target.into(),
            payload: payload.into(),
        }
    }
}

pub mod prelude {
    pub use super::AxAction;
    pub use super::AxAssignment;
    pub use super::AxBackendBlock;
    pub use super::AxBackendData;
    pub use super::AxBackendDocument;
    pub use super::AxBackendEnv;
    pub use super::AxBackendEnvVisibility;
    pub use super::AxBackendFunction;
    pub use super::AxBackendImport;
    pub use super::AxBackendImportBinding;
    pub use super::AxBackendRoot;
    pub use super::AxBackendStmt;
    pub use super::AxBackendValue;
    pub use super::AxField;
    pub use super::AxHook;
    pub use super::AxHookPhase;
    pub use super::AxJob;
    pub use super::AxLoader;
    pub use super::AxMutation;
    pub use super::AxPatch;
    pub use super::AxRequirement;
    pub use super::AxResponseCookie;
    pub use super::AxResponseHeader;
    pub use super::AxReturn;
    pub use super::AxRoute;
    pub use super::AxScope;
    pub use super::AxScopeRender;
    pub use super::AxScopeState;
    pub use super::AxScopeStmt;
    pub use super::AxSend;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax_query_ast::prelude::*;

    #[test]
    fn builds_loader_action_and_route_blocks() {
        let document = AxBackendDocument::new([
            AxBackendBlock::Loader(AxLoader::new(
                "PostsList",
                [
                    AxBackendStmt::data(
                        "posts",
                        AxQuerySpec::new(AxQuerySource::Stream {
                            collection: "posts".to_string(),
                        })
                        .filter(AxQueryFilter::new(
                            "status",
                            AxQueryFilterOp::Eq,
                            AxExpr::string("published"),
                        ))
                        .order(AxQueryOrder::new("created_at", AxQueryOrderDirection::Desc))
                        .limit(20),
                    ),
                    AxBackendStmt::r#return(AxExpr::ident("posts")),
                ],
            )),
            AxBackendBlock::Action(
                AxAction::new("CreatePost")
                    .input([
                        AxField::new("title", "string"),
                        AxField::new("excerpt", "string"),
                    ])
                    .body([
                        AxBackendStmt::insert(
                            "posts",
                            [
                                AxAssignment::new("title", AxExpr::ident("input").member("title")),
                                AxAssignment::new(
                                    "excerpt",
                                    AxExpr::ident("input").member("excerpt"),
                                ),
                            ],
                        ),
                        AxBackendStmt::revalidate("/posts"),
                        AxBackendStmt::r#return("ok"),
                    ]),
            ),
            AxBackendBlock::Route(AxRoute::new(
                "GET",
                "/api/posts",
                [
                    AxBackendStmt::data(
                        "posts",
                        AxExpr::call(["Db", "Stream"], [AxExpr::string("posts")]),
                    ),
                    AxBackendStmt::r#return(AxExpr::ident("posts")),
                ],
            )),
            AxBackendBlock::Scope(AxScope::new(
                "Layout",
                ["RenderLayout", "setTheme"],
                [
                    AxScopeStmt::state("theme", "String", AxExpr::string("silver")),
                    AxScopeStmt::render(AxExpr::call(["RenderLayout"], [])),
                ],
            )),
        ]);

        assert_eq!(document.blocks.len(), 4);

        let AxBackendBlock::Action(action) = &document.blocks[1] else {
            panic!("expected action block");
        };

        assert_eq!(action.name, "CreatePost");
        assert_eq!(action.input.len(), 2);
        assert_eq!(action.body.len(), 3);

        let AxBackendBlock::Scope(scope) = &document.blocks[3] else {
            panic!("expected scope block");
        };
        assert_eq!(scope.name, "Layout");
        assert_eq!(scope.members, vec!["RenderLayout", "setTheme"]);
        assert_eq!(scope.body.len(), 2);
    }

    #[test]
    fn job_can_model_send_step() {
        let job = AxJob::new(
            "PublishDailyDigest",
            [
                AxBackendStmt::data("posts", AxExpr::call(["Query", "PublishedPosts"], [])),
                AxBackendStmt::send("DigestEmail", AxExpr::ident("posts")),
            ],
        );

        assert_eq!(job.name, "PublishDailyDigest");
        assert_eq!(job.body.len(), 2);
    }
}
