use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ax_ast::prelude::{
    AxBody, AxComponent, AxDocument, AxExpr, AxPipelineStage, AxStatement,
};
use crate::ax_ast_v2::prelude::AxFileV2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxType {
    String,
    Number,
    Bool,
    DateTime,
    List(Box<AxType>),
    Optional(Box<AxType>),
    Record(String),
    Unknown,
}

impl AxType {
    pub fn list(item: AxType) -> Self {
        Self::List(Box::new(item))
    }

    pub fn optional(item: AxType) -> Self {
        Self::Optional(Box::new(item))
    }

    pub fn record(name: impl Into<String>) -> Self {
        Self::Record(name.into())
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::String => "String".to_string(),
            Self::Number => "Number".to_string(),
            Self::Bool => "Bool".to_string(),
            Self::DateTime => "DateTime".to_string(),
            Self::List(item) => format!("List<{}>", item.display_name()),
            Self::Optional(item) => format!("Optional<{}>", item.display_name()),
            Self::Record(name) => name.clone(),
            Self::Unknown => "Unknown".to_string(),
        }
    }

    pub fn list_item(&self) -> Option<&AxType> {
        match self {
            Self::List(item) => Some(item),
            _ => None,
        }
    }

    pub fn parse_annotation(input: &str) -> Result<Self, AxTypeParseError> {
        parse_type_annotation(input)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxRecordType {
    pub name: String,
    pub fields: BTreeMap<String, AxType>,
}

impl AxRecordType {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn field(mut self, name: impl Into<String>, ty: AxType) -> Self {
        self.fields.insert(name.into(), ty);
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxDataContext {
    pub bindings: BTreeMap<String, AxType>,
    pub records: BTreeMap<String, AxRecordType>,
}

impl AxDataContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_record(mut self, record: AxRecordType) -> Self {
        self.records.insert(record.name.clone(), record);
        self
    }

    pub fn with_binding(mut self, name: impl Into<String>, ty: AxType) -> Self {
        self.bindings.insert(name.into(), ty);
        self
    }

    pub fn bind(&mut self, name: impl Into<String>, ty: AxType) {
        self.bindings.insert(name.into(), ty);
    }

    pub fn record(&self, name: &str) -> Option<&AxRecordType> {
        self.records.get(name)
    }

    pub fn binding(&self, name: &str) -> Option<&AxType> {
        self.bindings.get(name)
    }

    pub fn from_v2_let_types(file: &AxFileV2) -> Result<Self, AxTypeParseError> {
        let mut context = Self::new();
        for record in &file.types {
            let mut record_type = AxRecordType::new(record.name.clone());
            for field in &record.fields {
                record_type =
                    record_type.field(field.name.clone(), AxType::parse_annotation(&field.ty)?);
            }
            context = context.with_record(record_type);
        }

        for binding in &file.lets {
            let Some(ty) = &binding.ty else {
                continue;
            };
            context.bind(binding.name.clone(), AxType::parse_annotation(ty)?);
        }
        Ok(context)
    }

    pub fn resolve_expr_type(&self, expr: &AxExpr) -> Result<AxType, AxTypeError> {
        match expr {
            AxExpr::String(_) => Ok(AxType::String),
            AxExpr::Number(_) => Ok(AxType::Number),
            AxExpr::Bool(_) => Ok(AxType::Bool),
            AxExpr::Identifier(name) => self
                .bindings
                .get(name)
                .cloned()
                .ok_or_else(|| AxTypeError::UnknownBinding { name: name.clone() }),
            AxExpr::Member { object, property } => {
                let object_type = self.resolve_expr_type(object)?;
                self.resolve_member_type(&object_type, property)
            }
            AxExpr::Call { .. } => Ok(AxType::Unknown),
        }
    }

    pub fn bind_each_item(
        &self,
        binding: impl Into<String>,
        source: &AxExpr,
    ) -> Result<Self, AxTypeError> {
        let source_type = self.resolve_expr_type(source)?;
        let Some(item_type) = source_type.list_item() else {
            return Err(AxTypeError::ExpectedList {
                found: source_type.display_name(),
            });
        };

        let mut next = self.clone();
        next.bind(binding, item_type.clone());
        Ok(next)
    }

    fn resolve_member_type(
        &self,
        object_type: &AxType,
        property: &str,
    ) -> Result<AxType, AxTypeError> {
        match object_type {
            AxType::Record(record_name) => {
                let record =
                    self.records
                        .get(record_name)
                        .ok_or_else(|| AxTypeError::UnknownRecord {
                            name: record_name.clone(),
                        })?;
                record
                    .fields
                    .get(property)
                    .cloned()
                    .ok_or_else(|| AxTypeError::UnknownField {
                        record: record_name.clone(),
                        field: property.to_string(),
                    })
            }
            AxType::Optional(inner) => self.resolve_member_type(inner, property),
            AxType::Unknown => Ok(AxType::Unknown),
            other => Err(AxTypeError::CannotAccessField {
                ty: other.display_name(),
                field: property.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AxTypeParseError {
    #[error("empty type annotation")]
    Empty,
    #[error("invalid type annotation `{raw}`")]
    Invalid { raw: String },
}

fn parse_type_annotation(input: &str) -> Result<AxType, AxTypeParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AxTypeParseError::Empty);
    }

    if let Some(inner) = parse_wrapped_type(input, "List")? {
        return Ok(AxType::list(parse_type_annotation(inner)?));
    }

    if let Some(inner) = parse_wrapped_type(input, "Optional")? {
        return Ok(AxType::optional(parse_type_annotation(inner)?));
    }

    Ok(match input {
        "String" => AxType::String,
        "Number" => AxType::Number,
        "Bool" => AxType::Bool,
        "DateTime" => AxType::DateTime,
        "Unknown" => AxType::Unknown,
        name if is_type_identifier(name) => AxType::record(name),
        source => {
            return Err(AxTypeParseError::Invalid {
                raw: source.to_string(),
            })
        }
    })
}

fn parse_wrapped_type<'a>(
    input: &'a str,
    wrapper: &str,
) -> Result<Option<&'a str>, AxTypeParseError> {
    let Some(rest) = input.strip_prefix(wrapper) else {
        return Ok(None);
    };

    let rest = rest.trim_start();
    if !rest.starts_with('<') {
        return Ok(None);
    }
    if !rest.ends_with('>') {
        return Err(AxTypeParseError::Invalid {
            raw: input.to_string(),
        });
    }

    let inner = &rest[1..rest.len() - 1];
    if !has_balanced_angle_brackets(inner) {
        return Err(AxTypeParseError::Invalid {
            raw: input.to_string(),
        });
    }

    Ok(Some(inner.trim()))
}

fn has_balanced_angle_brackets(input: &str) -> bool {
    let mut depth = 0usize;
    for ch in input.chars() {
        match ch {
            '<' => depth += 1,
            '>' => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
            }
            _ => {}
        }
    }
    depth == 0
}

fn is_type_identifier(input: &str) -> bool {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AxTypeError {
    #[error("unknown binding `{name}`")]
    UnknownBinding { name: String },
    #[error("unknown record type `{name}`")]
    UnknownRecord { name: String },
    #[error("unknown field `{field}` on {record}")]
    UnknownField { record: String, field: String },
    #[error("cannot access field `{field}` on {ty}")]
    CannotAccessField { ty: String, field: String },
    #[error("cannot iterate value; expected List<T>, found {found}")]
    ExpectedList { found: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AxTypeCheckReport {
    pub errors: Vec<AxTypeCheckError>,
}

impl AxTypeCheckReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn into_result(self) -> Result<(), Vec<AxTypeCheckError>> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self.errors)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AxTypeCheckError {
    pub location: String,
    pub message: String,
}

impl AxTypeCheckError {
    fn new(location: impl Into<String>, error: AxTypeError) -> Self {
        Self {
            location: location.into(),
            message: error.to_string(),
        }
    }
}

pub fn check_document_types(document: &AxDocument, context: &AxDataContext) -> AxTypeCheckReport {
    let mut checker = AxTypeChecker::new(context.clone());
    checker.check_statements(&document.page.body, "page");
    AxTypeCheckReport {
        errors: checker.errors,
    }
}

struct AxTypeChecker {
    context: AxDataContext,
    errors: Vec<AxTypeCheckError>,
}

impl AxTypeChecker {
    fn new(context: AxDataContext) -> Self {
        Self {
            context,
            errors: Vec::new(),
        }
    }

    fn fork(&self, context: AxDataContext) -> Self {
        Self {
            context,
            errors: Vec::new(),
        }
    }

    fn check_statements(&mut self, statements: &[AxStatement], location: &str) {
        for (index, statement) in statements.iter().enumerate() {
            self.check_statement(statement, &format!("{location}[{index}]"));
        }
    }

    fn check_statement(&mut self, statement: &AxStatement, location: &str) {
        match statement {
            AxStatement::Data(binding) => match self.context.resolve_expr_type(&binding.value) {
                Ok(AxType::Unknown) if self.context.binding(&binding.name).is_some() => {}
                Ok(ty) => self.context.bind(binding.name.clone(), ty),
                Err(error) => self.push_error(format!("{location}.data.{}", binding.name), error),
            },
            AxStatement::Each(block) => {
                match self.context.bind_each_item(&block.binding, &block.source) {
                    Ok(each_context) => {
                        let mut body_checker = self.fork(each_context);
                        body_checker.check_statements(&block.body, &format!("{location}.each"));
                        self.errors.extend(body_checker.errors);

                        let mut empty_checker = self.fork(self.context.clone());
                        empty_checker
                            .check_statements(&block.empty_body, &format!("{location}.empty"));
                        self.errors.extend(empty_checker.errors);
                    }
                    Err(error) => self.push_error(format!("{location}.each.source"), error),
                }
            }
            AxStatement::If(block) => {
                self.check_expr(&block.condition, format!("{location}.if.condition"));
                self.check_statements(&block.body, &format!("{location}.if"));
                self.check_statements(&block.else_body, &format!("{location}.else"));
            }
            AxStatement::Text(expr) => self.check_expr(expr, format!("{location}.text")),
            AxStatement::Component(component) => self.check_component(component, location),
            AxStatement::Pipeline(pipeline) => {
                self.check_expr(&pipeline.source, format!("{location}.pipeline.source"));
                let mut current_source = self.context.resolve_expr_type(&pipeline.source).ok();
                for (stage_index, stage) in pipeline.stages.iter().enumerate() {
                    match stage {
                        AxPipelineStage::Component(component) => {
                            self.check_component(
                                component,
                                &format!("{location}.pipeline.stage[{stage_index}]"),
                            );
                        }
                        AxPipelineStage::Each(each) => match current_source.as_ref() {
                            Some(AxType::List(item)) => {
                                self.context
                                    .bind(each.binding.clone(), item.as_ref().clone());
                                current_source = Some(item.as_ref().clone());
                            }
                            Some(other) => self.push_error(
                                format!("{location}.pipeline.stage[{stage_index}].each"),
                                AxTypeError::ExpectedList {
                                    found: other.display_name(),
                                },
                            ),
                            None => {}
                        },
                    }
                }
            }
        }
    }

    fn check_component(&mut self, component: &AxComponent, location: &str) {
        for prop in &component.props {
            self.check_expr(
                &prop.value,
                format!("{location}.{}.prop.{}", component.name, prop.name),
            );
        }
        if let Some(recipe) = &component.style.recipe {
            self.check_expr(recipe, format!("{location}.{}.recipe", component.name));
        }
        if let Some(class) = &component.style.class {
            self.check_expr(class, format!("{location}.{}.class", component.name));
        }

        match &component.body {
            AxBody::Empty => {}
            AxBody::Inline(expr) => {
                self.check_expr(expr, format!("{location}.{}.body", component.name));
            }
            AxBody::Block(body) => {
                self.check_statements(body, &format!("{location}.{}", component.name));
            }
        }
    }

    fn check_expr(&mut self, expr: &AxExpr, location: String) {
        if let Err(error) = self.context.resolve_expr_type(expr) {
            self.push_error(location, error);
        }
    }

    fn push_error(&mut self, location: impl Into<String>, error: AxTypeError) {
        self.errors.push(AxTypeCheckError::new(location, error));
    }
}

pub mod prelude {
    pub use super::check_document_types;
    pub use super::AxDataContext;
    pub use super::AxRecordType;
    pub use super::AxType;
    pub use super::AxTypeCheckError;
    pub use super::AxTypeCheckReport;
    pub use super::AxTypeError;
    pub use super::AxTypeParseError;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax_ast::prelude::AxEachBlock;
    use crate::ax_parser_v2::parse_ax_v2;

    fn post_context() -> AxDataContext {
        AxDataContext::new()
            .with_record(
                AxRecordType::new("Post")
                    .field("title", AxType::String)
                    .field("slug", AxType::String)
                    .field("excerpt", AxType::String),
            )
            .with_binding("posts", AxType::list(AxType::record("Post")))
    }

    #[test]
    fn resolves_each_item_member_type() {
        let context = post_context();
        let each_context = context
            .bind_each_item("post", &AxExpr::ident("posts"))
            .expect("posts should be iterable");

        let ty = each_context
            .resolve_expr_type(&AxExpr::ident("post").member("title"))
            .expect("post.title should resolve");

        assert_eq!(ty, AxType::String);
    }

    #[test]
    fn reports_unknown_record_field() {
        let context = post_context();
        let each_context = context
            .bind_each_item("post", &AxExpr::ident("posts"))
            .expect("posts should be iterable");

        let error = each_context
            .resolve_expr_type(&AxExpr::ident("post").member("summary"))
            .expect_err("summary should not exist");

        assert_eq!(
            error,
            AxTypeError::UnknownField {
                record: "Post".to_string(),
                field: "summary".to_string(),
            }
        );
    }

    #[test]
    fn reports_non_list_each_source() {
        let context = AxDataContext::new().with_binding("post", AxType::record("Post"));
        let error = context
            .bind_each_item("item", &AxExpr::ident("post"))
            .expect_err("record should not be iterable");

        assert_eq!(
            error,
            AxTypeError::ExpectedList {
                found: "Post".to_string(),
            }
        );
    }

    #[test]
    fn parses_type_annotations() {
        assert_eq!(AxType::parse_annotation("String"), Ok(AxType::String));
        assert_eq!(
            AxType::parse_annotation("List<Optional<Post>>"),
            Ok(AxType::list(AxType::optional(AxType::record("Post"))))
        );
    }

    #[test]
    fn builds_context_from_v2_typed_lets() {
        let file = parse_ax_v2(
            r#"
page Blog

type Post {
  title: String
  slug: String
}

let posts: List<Post> = load PostsList
let title = "Blog"

<Copy>{title}</Copy>
"#,
        )
        .expect("source should parse");

        let context = AxDataContext::from_v2_let_types(&file).expect("typed lets should resolve");

        assert_eq!(
            context.binding("posts"),
            Some(&AxType::list(AxType::record("Post")))
        );
        assert_eq!(
            context.record("Post"),
            Some(
                &AxRecordType::new("Post")
                    .field("title", AxType::String)
                    .field("slug", AxType::String)
            )
        );
        assert_eq!(context.binding("title"), None);
    }

    #[test]
    fn checks_document_each_item_members() {
        let document = AxDocument::page(
            "Blog",
            [AxStatement::each(
                "post",
                AxExpr::ident("posts"),
                [AxStatement::component(
                    AxComponent::new("Card")
                        .prop("title", AxExpr::ident("post").member("title"))
                        .block([AxStatement::component(
                            AxComponent::new("Copy")
                                .inline(AxExpr::ident("post").member("excerpt")),
                        )]),
                )],
            )],
        );

        let report = check_document_types(&document, &post_context());

        assert!(report.is_ok(), "{report:#?}");
    }

    #[test]
    fn checks_document_reports_unknown_member_in_component_props() {
        let document = AxDocument::page(
            "Blog",
            [AxStatement::each(
                "post",
                AxExpr::ident("posts"),
                [AxStatement::component(
                    AxComponent::new("Card").prop("title", AxExpr::ident("post").member("summary")),
                )],
            )],
        );

        let report = check_document_types(&document, &post_context());

        assert_eq!(
            report.errors,
            vec![AxTypeCheckError {
                location: "page[0].each[0].Card.prop.title".to_string(),
                message: "unknown field `summary` on Post".to_string(),
            }]
        );
    }

    #[test]
    fn checks_document_keeps_each_binding_out_of_empty_branch() {
        let document = AxDocument::page(
            "Blog",
            [AxStatement::Each(
                AxEachBlock::new("post", AxExpr::ident("posts"), []).empty([
                    AxStatement::component(
                        AxComponent::new("Copy").inline(AxExpr::ident("post").member("title")),
                    ),
                ]),
            )],
        );

        let report = check_document_types(&document, &post_context());

        assert_eq!(
            report.errors,
            vec![AxTypeCheckError {
                location: "page[0].empty[0].Copy.body".to_string(),
                message: "unknown binding `post`".to_string(),
            }]
        );
    }
}
