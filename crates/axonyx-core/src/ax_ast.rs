use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxDocument {
    pub imports: Vec<AxImport>,
    pub functions: Vec<AxFunctionDef>,
    pub components: Vec<AxComponentDef>,
    pub head: AxHead,
    pub page: AxPage,
}

impl AxDocument {
    pub fn page(name: impl Into<String>, body: impl IntoIterator<Item = AxStatement>) -> Self {
        Self {
            imports: Vec::new(),
            functions: Vec::new(),
            components: Vec::new(),
            head: AxHead::default(),
            page: AxPage::new(name, body),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxImport {
    pub bindings: Vec<AxImportBinding>,
    pub source: String,
}

impl AxImport {
    pub fn new(
        bindings: impl IntoIterator<Item = AxImportBinding>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            bindings: bindings.into_iter().collect(),
            source: source.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxImportBinding {
    pub imported: String,
    pub local: String,
}

impl AxImportBinding {
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
pub struct AxFunctionDef {
    pub name: String,
    pub params: Vec<AxComponentParamDef>,
    pub body: AxExpr,
}

impl AxFunctionDef {
    pub fn new(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDef>,
        body: AxExpr,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
            body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxComponentDef {
    pub name: String,
    pub params: Vec<AxComponentParamDef>,
    pub states: Vec<AxComponentStateDef>,
    pub body: Vec<AxStatement>,
}

impl AxComponentDef {
    pub fn new(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDef>,
        body: impl IntoIterator<Item = AxStatement>,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
            states: Vec::new(),
            body: body.into_iter().collect(),
        }
    }

    pub fn with_states(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDef>,
        states: impl IntoIterator<Item = AxComponentStateDef>,
        body: impl IntoIterator<Item = AxStatement>,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
            states: states.into_iter().collect(),
            body: body.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxComponentStateDef {
    pub name: String,
    pub ty: String,
    pub initial: AxExpr,
    pub signal: String,
}

impl AxComponentStateDef {
    pub fn new(
        name: impl Into<String>,
        ty: impl Into<String>,
        initial: AxExpr,
        signal: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            ty: ty.into(),
            initial,
            signal: signal.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxComponentParamDef {
    pub name: String,
    pub default: Option<AxExpr>,
}

impl AxComponentParamDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: None,
        }
    }

    pub fn with_default(name: impl Into<String>, default: AxExpr) -> Self {
        Self {
            name: name.into(),
            default: Some(default),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxPage {
    pub name: String,
    pub params: Vec<AxComponentParamDef>,
    pub body: Vec<AxStatement>,
}

impl AxPage {
    pub fn new(name: impl Into<String>, body: impl IntoIterator<Item = AxStatement>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            body: body.into_iter().collect(),
        }
    }

    pub fn with_params(
        name: impl Into<String>,
        params: impl IntoIterator<Item = AxComponentParamDef>,
        body: impl IntoIterator<Item = AxStatement>,
    ) -> Self {
        Self {
            name: name.into(),
            params: params.into_iter().collect(),
            body: body.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AxHead {
    pub title: Option<AxExpr>,
    pub theme: Option<AxExpr>,
    pub theme_storage_key: Option<AxExpr>,
    pub theme_preflight: bool,
    pub metas: Vec<AxHeadTag>,
    pub links: Vec<AxHeadTag>,
    pub scripts: Vec<AxHeadTag>,
}

impl AxHead {
    pub fn with_title(mut self, value: impl Into<AxExpr>) -> Self {
        self.title = Some(value.into());
        self
    }

    pub fn with_theme(mut self, value: impl Into<AxExpr>) -> Self {
        self.theme = Some(value.into());
        self
    }

    pub fn with_theme_storage_key(mut self, value: impl Into<AxExpr>) -> Self {
        self.theme_storage_key = Some(value.into());
        self
    }

    pub fn with_theme_preflight(mut self) -> Self {
        self.theme_preflight = true;
        self
    }

    pub fn meta(mut self, tag: AxHeadTag) -> Self {
        self.metas.push(tag);
        self
    }

    pub fn link(mut self, tag: AxHeadTag) -> Self {
        self.links.push(tag);
        self
    }

    pub fn script(mut self, tag: AxHeadTag) -> Self {
        self.scripts.push(tag);
        self
    }

    pub fn merge(&mut self, other: AxHead) {
        if other.title.is_some() {
            self.title = other.title;
        }
        if other.theme.is_some() {
            self.theme = other.theme;
        }
        if other.theme_storage_key.is_some() {
            self.theme_storage_key = other.theme_storage_key;
        }
        if other.theme_preflight {
            self.theme_preflight = true;
        }
        self.metas.extend(other.metas);
        self.links.extend(other.links);
        self.scripts.extend(other.scripts);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AxHeadTag {
    pub attrs: Vec<AxProp>,
}

impl AxHeadTag {
    pub fn new(attrs: impl IntoIterator<Item = AxProp>) -> Self {
        Self {
            attrs: attrs.into_iter().collect(),
        }
    }

    pub fn attr(mut self, name: impl Into<String>, value: impl Into<AxExpr>) -> Self {
        self.attrs.push(AxProp::new(name, value));
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxStatement {
    Data(AxDataBinding),
    Each(AxEachBlock),
    If(AxIfBlock),
    Text(AxExpr),
    Component(AxComponent),
    Pipeline(AxPipeline),
}

impl AxStatement {
    pub fn data(name: impl Into<String>, value: AxExpr) -> Self {
        Self::Data(AxDataBinding::new(name, value))
    }

    pub fn each(
        binding: impl Into<String>,
        source: AxExpr,
        body: impl IntoIterator<Item = AxStatement>,
    ) -> Self {
        Self::Each(AxEachBlock::new(binding, source, body))
    }

    pub fn if_block(condition: AxExpr, body: impl IntoIterator<Item = AxStatement>) -> Self {
        Self::If(AxIfBlock::new(condition, body))
    }

    pub fn text(value: impl Into<AxExpr>) -> Self {
        Self::Text(value.into())
    }

    pub fn component(component: AxComponent) -> Self {
        Self::Component(component)
    }

    pub fn pipeline(pipeline: AxPipeline) -> Self {
        Self::Pipeline(pipeline)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxDataBinding {
    pub name: String,
    pub value: AxExpr,
}

impl AxDataBinding {
    pub fn new(name: impl Into<String>, value: AxExpr) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxEachBlock {
    pub binding: String,
    pub source: AxExpr,
    pub body: Vec<AxStatement>,
    pub empty_body: Vec<AxStatement>,
}

impl AxEachBlock {
    pub fn new(
        binding: impl Into<String>,
        source: AxExpr,
        body: impl IntoIterator<Item = AxStatement>,
    ) -> Self {
        Self {
            binding: binding.into(),
            source,
            body: body.into_iter().collect(),
            empty_body: Vec::new(),
        }
    }

    pub fn empty(mut self, body: impl IntoIterator<Item = AxStatement>) -> Self {
        self.empty_body = body.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AxStyle {
    pub recipe: Option<AxExpr>,
    pub class: Option<AxExpr>,
}

impl AxStyle {
    pub fn recipe(mut self, value: impl Into<AxExpr>) -> Self {
        self.recipe = Some(value.into());
        self
    }

    pub fn class(mut self, value: impl Into<AxExpr>) -> Self {
        self.class = Some(value.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxComponent {
    pub name: String,
    pub props: Vec<AxProp>,
    pub style: AxStyle,
    pub body: AxBody,
}

impl AxComponent {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            props: Vec::new(),
            style: AxStyle::default(),
            body: AxBody::Empty,
        }
    }

    pub fn prop(mut self, name: impl Into<String>, value: impl Into<AxExpr>) -> Self {
        self.props.push(AxProp::new(name, value));
        self
    }

    pub fn recipe(mut self, value: impl Into<AxExpr>) -> Self {
        self.style = self.style.recipe(value);
        self
    }

    pub fn class(mut self, value: impl Into<AxExpr>) -> Self {
        self.style = self.style.class(value);
        self
    }

    pub fn inline(mut self, value: impl Into<AxExpr>) -> Self {
        self.body = AxBody::Inline(value.into());
        self
    }

    pub fn fragment(body: impl IntoIterator<Item = AxStatement>) -> Self {
        Self::new("Fragment").block(body)
    }

    pub fn block(mut self, body: impl IntoIterator<Item = AxStatement>) -> Self {
        self.body = AxBody::Block(body.into_iter().collect());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxIfBlock {
    pub condition: AxExpr,
    pub body: Vec<AxStatement>,
    pub else_body: Vec<AxStatement>,
}

impl AxIfBlock {
    pub fn new(condition: AxExpr, body: impl IntoIterator<Item = AxStatement>) -> Self {
        Self {
            condition,
            body: body.into_iter().collect(),
            else_body: Vec::new(),
        }
    }

    pub fn else_body(mut self, body: impl IntoIterator<Item = AxStatement>) -> Self {
        self.else_body = body.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxProp {
    pub name: String,
    pub value: AxExpr,
}

impl AxProp {
    pub fn new(name: impl Into<String>, value: impl Into<AxExpr>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxBody {
    Empty,
    Inline(AxExpr),
    Block(Vec<AxStatement>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxPipeline {
    pub source: AxExpr,
    pub stages: Vec<AxPipelineStage>,
}

impl AxPipeline {
    pub fn new(source: impl Into<AxExpr>) -> Self {
        Self {
            source: source.into(),
            stages: Vec::new(),
        }
    }

    pub fn stage(mut self, stage: AxPipelineStage) -> Self {
        self.stages.push(stage);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxPipelineStage {
    Component(AxComponent),
    Each(AxEachStage),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxEachStage {
    pub binding: String,
}

impl AxEachStage {
    pub fn new(binding: impl Into<String>) -> Self {
        Self {
            binding: binding.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxExpr {
    String(String),
    Number(i64),
    Float(AxFloat),
    Bool(bool),
    List(Vec<AxExpr>),
    Object(BTreeMap<String, AxExpr>),
    Identifier(String),
    Unary {
        op: AxUnaryOp,
        expr: Box<AxExpr>,
    },
    Binary {
        op: AxBinaryOp,
        left: Box<AxExpr>,
        right: Box<AxExpr>,
    },
    Index {
        object: Box<AxExpr>,
        index: Box<AxExpr>,
    },
    Member {
        object: Box<AxExpr>,
        property: String,
    },
    OptionalMember {
        object: Box<AxExpr>,
        property: String,
    },
    Call {
        path: Vec<String>,
        args: Vec<AxExpr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxFloat(f64);

impl AxFloat {
    pub fn new(value: f64) -> Option<Self> {
        value
            .is_finite()
            .then_some(Self(if value == 0.0 { 0.0 } else { value }))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

impl Eq for AxFloat {}

impl Serialize for AxFloat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for AxFloat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| serde::de::Error::custom("float must be finite"))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxUnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    In,
    And,
    Or,
    Fallback,
}

impl AxExpr {
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    pub fn number(value: i64) -> Self {
        Self::Number(value)
    }

    pub fn float(value: f64) -> Self {
        Self::Float(AxFloat::new(value).expect("AxExpr::float requires a finite value"))
    }

    pub fn bool(value: bool) -> Self {
        Self::Bool(value)
    }

    pub fn list(items: impl IntoIterator<Item = AxExpr>) -> Self {
        Self::List(items.into_iter().collect())
    }

    pub fn object(fields: impl IntoIterator<Item = (impl Into<String>, AxExpr)>) -> Self {
        Self::Object(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        )
    }

    pub fn ident(value: impl Into<String>) -> Self {
        Self::Identifier(value.into())
    }

    pub fn unary(op: AxUnaryOp, expr: AxExpr) -> Self {
        Self::Unary {
            op,
            expr: Box::new(expr),
        }
    }

    pub fn binary(op: AxBinaryOp, left: AxExpr, right: AxExpr) -> Self {
        Self::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    pub fn index(self, index: AxExpr) -> Self {
        Self::Index {
            object: Box::new(self),
            index: Box::new(index),
        }
    }

    pub fn member(self, property: impl Into<String>) -> Self {
        Self::Member {
            object: Box::new(self),
            property: property.into(),
        }
    }

    pub fn optional_member(self, property: impl Into<String>) -> Self {
        Self::OptionalMember {
            object: Box::new(self),
            property: property.into(),
        }
    }

    pub fn call(
        path: impl IntoIterator<Item = impl Into<String>>,
        args: impl IntoIterator<Item = AxExpr>,
    ) -> Self {
        Self::Call {
            path: path.into_iter().map(Into::into).collect(),
            args: args.into_iter().collect(),
        }
    }
}

impl From<&str> for AxExpr {
    fn from(value: &str) -> Self {
        AxExpr::String(value.to_string())
    }
}

impl From<String> for AxExpr {
    fn from(value: String) -> Self {
        AxExpr::String(value)
    }
}

impl From<i64> for AxExpr {
    fn from(value: i64) -> Self {
        AxExpr::Number(value)
    }
}

impl From<f64> for AxExpr {
    fn from(value: f64) -> Self {
        AxExpr::float(value)
    }
}

impl From<bool> for AxExpr {
    fn from(value: bool) -> Self {
        AxExpr::Bool(value)
    }
}

pub mod prelude {
    pub use super::AxBinaryOp;
    pub use super::AxBody;
    pub use super::AxComponent;
    pub use super::AxComponentDef;
    pub use super::AxComponentParamDef;
    pub use super::AxComponentStateDef;
    pub use super::AxDataBinding;
    pub use super::AxDocument;
    pub use super::AxEachBlock;
    pub use super::AxEachStage;
    pub use super::AxExpr;
    pub use super::AxFloat;
    pub use super::AxFunctionDef;
    pub use super::AxHead;
    pub use super::AxHeadTag;
    pub use super::AxIfBlock;
    pub use super::AxImport;
    pub use super::AxImportBinding;
    pub use super::AxPage;
    pub use super::AxPipeline;
    pub use super::AxPipelineStage;
    pub use super::AxProp;
    pub use super::AxStatement;
    pub use super::AxStyle;
    pub use super::AxUnaryOp;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_ast_for_indentation_first_page() {
        let posts = AxExpr::ident("posts");
        let post = AxExpr::ident("post");

        let document = AxDocument::page(
            "Home",
            [
                AxStatement::data(
                    "posts",
                    AxExpr::call(["Db", "Stream"], [AxExpr::string("posts")]),
                ),
                AxStatement::component(
                    AxComponent::new("Container").prop("max", "xl").block([
                        AxStatement::component(
                            AxComponent::new("Grid")
                                .prop("cols", 3_i64)
                                .prop("gap", "md")
                                .block([AxStatement::each(
                                    "post",
                                    posts.clone(),
                                    [AxStatement::component(
                                        AxComponent::new("Card")
                                            .prop("title", post.clone().member("title"))
                                            .block([AxStatement::component(
                                                AxComponent::new("Copy")
                                                    .inline(post.clone().member("excerpt")),
                                            )]),
                                    )],
                                )]),
                        ),
                    ]),
                ),
            ],
        );

        assert_eq!(document.page.name, "Home");
        assert_eq!(document.page.body.len(), 2);
        assert_eq!(
            document.page.body[0],
            AxStatement::Data(AxDataBinding::new(
                "posts",
                AxExpr::Call {
                    path: vec!["Db".to_string(), "Stream".to_string()],
                    args: vec![AxExpr::String("posts".to_string())],
                }
            ))
        );
    }

    #[test]
    fn component_keeps_style_layers_separate_from_semantic_props() {
        let node = AxComponent::new("Button")
            .prop("tone", "primary")
            .prop("size", "lg")
            .recipe("hero-cta")
            .class("w-full")
            .inline("Launch");

        assert_eq!(
            node,
            AxComponent {
                name: "Button".to_string(),
                props: vec![AxProp::new("tone", "primary"), AxProp::new("size", "lg"),],
                style: AxStyle {
                    recipe: Some(AxExpr::String("hero-cta".to_string())),
                    class: Some(AxExpr::String("w-full".to_string())),
                },
                body: AxBody::Inline(AxExpr::String("Launch".to_string())),
            }
        );
    }

    #[test]
    fn pipeline_ast_can_represent_each_and_component_stages() {
        let pipeline = AxPipeline::new(AxExpr::call(["Db", "Stream"], [AxExpr::string("posts")]))
            .stage(AxPipelineStage::Component(
                AxComponent::new("Grid")
                    .prop("cols", 3_i64)
                    .prop("gap", "md"),
            ))
            .stage(AxPipelineStage::Each(AxEachStage::new("post")))
            .stage(AxPipelineStage::Component(
                AxComponent::new("Card").prop("title", AxExpr::ident("post").member("title")),
            ));

        assert_eq!(pipeline.stages.len(), 3);
        assert_eq!(
            pipeline.stages[1],
            AxPipelineStage::Each(AxEachStage {
                binding: "post".to_string(),
            })
        );
    }
}
