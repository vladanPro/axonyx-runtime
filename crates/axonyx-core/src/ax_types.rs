use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ax_ast::prelude::{
    AxBinaryOp, AxBody, AxComponent, AxDocument, AxExpr, AxPipelineStage, AxStatement, AxUnaryOp,
};
use crate::ax_ast_v2::prelude::AxFileV2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxType {
    String,
    Number,
    Int,
    Float,
    Bool,
    DateTime,
    Date,
    Time,
    Uuid,
    Bytes,
    Json,
    Never,
    Void,
    List(Box<AxType>),
    Map(Box<AxType>, Box<AxType>),
    Set(Box<AxType>),
    Optional(Box<AxType>),
    Result(Box<AxType>, Box<AxType>),
    Secret(Box<AxType>),
    Public(Box<AxType>),
    Signal(Box<AxType>),
    Resource(Box<AxType>, Box<AxType>),
    Record(String),
    Unknown,
}

impl AxType {
    pub fn list(item: AxType) -> Self {
        Self::List(Box::new(item))
    }

    pub fn optional(item: AxType) -> Self {
        match item {
            Self::Optional(_) => item,
            other => Self::Optional(Box::new(other)),
        }
    }

    pub fn record(name: impl Into<String>) -> Self {
        Self::Record(name.into())
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::String => "String".to_string(),
            Self::Number => "Number".to_string(),
            Self::Int => "Int".to_string(),
            Self::Float => "Float".to_string(),
            Self::Bool => "Bool".to_string(),
            Self::DateTime => "DateTime".to_string(),
            Self::Date => "Date".to_string(),
            Self::Time => "Time".to_string(),
            Self::Uuid => "Uuid".to_string(),
            Self::Bytes => "Bytes".to_string(),
            Self::Json => "Json".to_string(),
            Self::Never => "Never".to_string(),
            Self::Void => "Void".to_string(),
            Self::List(item) => format!("List<{}>", item.display_name()),
            Self::Map(key, value) => {
                format!("Map<{}, {}>", key.display_name(), value.display_name())
            }
            Self::Set(item) => format!("Set<{}>", item.display_name()),
            Self::Optional(item) => format!("Optional<{}>", item.display_name()),
            Self::Result(ok, error) => {
                format!("Result<{}, {}>", ok.display_name(), error.display_name())
            }
            Self::Secret(item) => format!("Secret<{}>", item.display_name()),
            Self::Public(item) => format!("Public<{}>", item.display_name()),
            Self::Signal(item) => format!("Signal<{}>", item.display_name()),
            Self::Resource(value, error) => {
                format!(
                    "Resource<{}, {}>",
                    value.display_name(),
                    error.display_name()
                )
            }
            Self::Record(name) => name.clone(),
            Self::Unknown => "Unknown".to_string(),
        }
    }

    pub fn list_item(&self) -> Option<&AxType> {
        match self {
            Self::List(item) | Self::Set(item) => Some(item),
            _ => None,
        }
    }

    pub fn parse_annotation(input: &str) -> Result<Self, AxTypeParseError> {
        parse_type_annotation(input)
    }

    pub fn supports_client_state(&self) -> bool {
        match self {
            Self::Never | Self::Void | Self::Secret(_) | Self::Signal(_) | Self::Resource(_, _) => {
                false
            }
            Self::List(item) | Self::Set(item) | Self::Optional(item) | Self::Public(item) => {
                item.supports_client_state()
            }
            Self::Map(key, value) | Self::Result(key, value) => {
                key.supports_client_state() && value.supports_client_state()
            }
            _ => true,
        }
    }

    pub fn accepts_state_initializer(&self, value: &AxExpr) -> bool {
        match self {
            Self::String | Self::DateTime | Self::Date | Self::Time | Self::Uuid => {
                matches!(value, AxExpr::String(_))
            }
            Self::Number | Self::Int | Self::Float => matches!(value, AxExpr::Number(_)),
            Self::Bool => matches!(value, AxExpr::Bool(_)),
            Self::Bytes => matches!(value, AxExpr::List(items) if items.iter().all(|item| {
                matches!(item, AxExpr::Number(number) if (0..=255).contains(number))
            })),
            Self::Json | Self::Unknown => matches!(
                value,
                AxExpr::String(_)
                    | AxExpr::Number(_)
                    | AxExpr::Bool(_)
                    | AxExpr::List(_)
                    | AxExpr::Object(_)
                    | AxExpr::Identifier(_)
            ),
            Self::List(item) | Self::Set(item) => matches!(
                value,
                AxExpr::List(items) if items.iter().all(|value| item.accepts_state_initializer(value))
            ),
            Self::Optional(item) => {
                matches!(value, AxExpr::Identifier(name) if name == "null")
                    || item.accepts_state_initializer(value)
            }
            Self::Map(key, item) => matches!(value, AxExpr::Object(fields) if fields.iter().all(
                |(name, value)| key.accepts_map_key(name) && item.accepts_state_initializer(value)
            )),
            Self::Result(ok, error) => matches!(value, AxExpr::Object(fields) if {
                fields.len() == 1
                    && (fields.get("Ok").is_some_and(|value| ok.accepts_state_initializer(value))
                        || fields.get("Err").is_some_and(|value| error.accepts_state_initializer(value)))
            }),
            Self::Record(_) => matches!(value, AxExpr::Object(_)),
            Self::Public(item) => item.accepts_state_initializer(value),
            Self::Never | Self::Void | Self::Secret(_) | Self::Signal(_) | Self::Resource(_, _) => {
                false
            }
        }
    }

    fn accepts_map_key(&self, value: &str) -> bool {
        match self {
            Self::String | Self::DateTime | Self::Date | Self::Time | Self::Uuid => true,
            Self::Int => value.parse::<i64>().is_ok(),
            Self::Bool => matches!(value, "true" | "false"),
            Self::Public(inner) => inner.accepts_map_key(value),
            _ => false,
        }
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

    pub fn accepts_state_initializer(&self, ty: &AxType, value: &AxExpr) -> bool {
        self.accepts_state_initializer_at_depth(ty, value, 0)
    }

    fn accepts_state_initializer_at_depth(
        &self,
        ty: &AxType,
        value: &AxExpr,
        depth: usize,
    ) -> bool {
        if depth > 32 {
            return false;
        }
        match ty {
            AxType::Record(name) => {
                let AxExpr::Object(values) = value else {
                    return false;
                };
                let Some(record) = self.record(name) else {
                    return true;
                };
                if values.keys().any(|name| !record.fields.contains_key(name)) {
                    return false;
                }
                record.fields.iter().all(|(name, field_ty)| {
                    values.get(name).map_or_else(
                        || matches!(field_ty, AxType::Optional(_)),
                        |value| self.accepts_state_initializer_at_depth(field_ty, value, depth + 1),
                    )
                })
            }
            AxType::List(item) | AxType::Set(item) => matches!(
                value,
                AxExpr::List(items) if items.iter().all(|value| {
                    self.accepts_state_initializer_at_depth(item, value, depth + 1)
                })
            ),
            AxType::Map(key, item) => matches!(value, AxExpr::Object(fields) if fields.iter().all(
                |(name, value)| key.accepts_map_key(name)
                    && self.accepts_state_initializer_at_depth(item, value, depth + 1)
            )),
            AxType::Optional(item) => {
                matches!(value, AxExpr::Identifier(name) if name == "null")
                    || self.accepts_state_initializer_at_depth(item, value, depth + 1)
            }
            AxType::Result(ok, error) => matches!(value, AxExpr::Object(fields) if {
                fields.len() == 1
                    && (fields.get("Ok").is_some_and(|value| {
                        self.accepts_state_initializer_at_depth(ok, value, depth + 1)
                    }) || fields.get("Err").is_some_and(|value| {
                        self.accepts_state_initializer_at_depth(error, value, depth + 1)
                    }))
            }),
            AxType::Public(item) => self.accepts_state_initializer_at_depth(item, value, depth + 1),
            _ => ty.accepts_state_initializer(value),
        }
    }

    pub fn from_v2_let_types(file: &AxFileV2) -> Result<Self, AxTypeParseError> {
        let mut context = Self::new();
        for record in &file.types {
            if context.records.contains_key(&record.name) {
                return Err(AxTypeParseError::DuplicateRecord {
                    name: record.name.clone(),
                });
            }
            let mut record_type = AxRecordType::new(record.name.clone());
            for field in &record.fields {
                if record_type.fields.contains_key(&field.name) {
                    return Err(AxTypeParseError::DuplicateField {
                        record: record.name.clone(),
                        field: field.name.clone(),
                    });
                }
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
            AxExpr::List(items) => self.resolve_list_type(items),
            AxExpr::Object(_) => Ok(AxType::Json),
            AxExpr::Identifier(name) => self
                .bindings
                .get(name)
                .cloned()
                .ok_or_else(|| AxTypeError::UnknownBinding { name: name.clone() }),
            AxExpr::Unary { op, expr } => {
                let ty = self.resolve_expr_type(expr)?;
                match op {
                    AxUnaryOp::Not => Ok(AxType::Bool),
                    AxUnaryOp::Neg => match ty {
                        AxType::Number | AxType::Int | AxType::Float | AxType::Unknown => Ok(ty),
                        other => Err(AxTypeError::ExpectedNumber {
                            found: other.display_name(),
                        }),
                    },
                }
            }
            AxExpr::Binary { op, left, right } => {
                let left_type = self.resolve_expr_type(left)?;
                let right_type = self.resolve_expr_type(right)?;
                match op {
                    AxBinaryOp::Add => match (&left_type, &right_type) {
                        (left, right) if is_numeric_type(left) && is_numeric_type(right) => {
                            Ok(promote_numeric_type(left, right))
                        }
                        (AxType::Unknown, _) | (_, AxType::Unknown) => Ok(AxType::Unknown),
                        _ => Ok(AxType::String),
                    },
                    AxBinaryOp::Sub | AxBinaryOp::Mul | AxBinaryOp::Div | AxBinaryOp::Rem => {
                        if matches!(
                            left_type,
                            AxType::Number | AxType::Int | AxType::Float | AxType::Unknown
                        ) && matches!(
                            right_type,
                            AxType::Number | AxType::Int | AxType::Float | AxType::Unknown
                        ) {
                            Ok(promote_numeric_type(&left_type, &right_type))
                        } else {
                            Err(AxTypeError::ExpectedNumber {
                                found: format!(
                                    "{} and {}",
                                    left_type.display_name(),
                                    right_type.display_name()
                                ),
                            })
                        }
                    }
                    AxBinaryOp::Eq
                    | AxBinaryOp::Ne
                    | AxBinaryOp::Gt
                    | AxBinaryOp::Ge
                    | AxBinaryOp::Lt
                    | AxBinaryOp::Le
                    | AxBinaryOp::In
                    | AxBinaryOp::And
                    | AxBinaryOp::Or => Ok(AxType::Bool),
                    AxBinaryOp::Fallback => match left_type {
                        AxType::Optional(item) => Ok(*item),
                        AxType::Unknown => Ok(right_type),
                        other => Ok(other),
                    },
                }
            }
            AxExpr::Index { object, index } => {
                let object_type = self.resolve_expr_type(object)?;
                match &object_type {
                    AxType::List(item) => Ok((**item).clone()),
                    AxType::Map(_, value) => Ok((**value).clone()),
                    AxType::Record(_) => match &**index {
                        AxExpr::String(property) => {
                            self.resolve_member_type(&object_type, property)
                        }
                        _ => Ok(AxType::Unknown),
                    },
                    AxType::Unknown => Ok(AxType::Unknown),
                    other => Err(AxTypeError::CannotIndex {
                        ty: other.display_name(),
                    }),
                }
            }
            AxExpr::Member { object, property } => {
                let object_type = self.resolve_expr_type(object)?;
                self.resolve_member_type(&object_type, property)
            }
            AxExpr::OptionalMember { object, property } => {
                let object_type = self.resolve_expr_type(object)?;
                match self.resolve_member_type(&object_type, property) {
                    Ok(ty) => Ok(AxType::optional(ty)),
                    Err(
                        AxTypeError::UnknownField { .. } | AxTypeError::CannotAccessField { .. },
                    ) => Ok(AxType::Unknown),
                    Err(error) => Err(error),
                }
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

    fn resolve_list_type(&self, items: &[AxExpr]) -> Result<AxType, AxTypeError> {
        let Some(first) = items.first() else {
            return Ok(AxType::list(AxType::Unknown));
        };

        let first_type = self.resolve_expr_type(first)?;
        for item in &items[1..] {
            let item_type = self.resolve_expr_type(item)?;
            if item_type != first_type {
                return Ok(AxType::list(AxType::Unknown));
            }
        }

        Ok(AxType::list(first_type))
    }
}

fn is_numeric_type(ty: &AxType) -> bool {
    matches!(ty, AxType::Number | AxType::Int | AxType::Float)
}

fn promote_numeric_type(left: &AxType, right: &AxType) -> AxType {
    if matches!(left, AxType::Float) || matches!(right, AxType::Float) {
        AxType::Float
    } else if matches!(left, AxType::Number) || matches!(right, AxType::Number) {
        AxType::Number
    } else if matches!(left, AxType::Int) && matches!(right, AxType::Int) {
        AxType::Int
    } else {
        AxType::Number
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AxTypeParseError {
    #[error("empty type annotation")]
    Empty,
    #[error("invalid type annotation `{raw}`")]
    Invalid { raw: String },
    #[error("duplicate type declaration `{name}`")]
    DuplicateRecord { name: String },
    #[error("duplicate field `{field}` in type `{record}`")]
    DuplicateField { record: String, field: String },
}

fn parse_type_annotation(input: &str) -> Result<AxType, AxTypeParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AxTypeParseError::Empty);
    }

    if let Some(item) = input.strip_suffix("[]") {
        return Ok(AxType::list(parse_type_annotation(item)?));
    }

    if let Some(inner) = parse_wrapped_type(input, "List")? {
        return Ok(AxType::list(parse_type_annotation(inner)?));
    }

    if let Some(inner) = parse_wrapped_type(input, "Optional")? {
        return Ok(AxType::optional(parse_type_annotation(inner)?));
    }

    if let Some(inner) = parse_wrapped_type(input, "Set")? {
        return Ok(AxType::Set(Box::new(parse_type_annotation(inner)?)));
    }

    if let Some(inner) = parse_wrapped_type(input, "Secret")? {
        return Ok(AxType::Secret(Box::new(parse_type_annotation(inner)?)));
    }

    if let Some(inner) = parse_wrapped_type(input, "Public")? {
        return Ok(AxType::Public(Box::new(parse_type_annotation(inner)?)));
    }

    if let Some(inner) = parse_wrapped_type(input, "Signal")? {
        return Ok(AxType::Signal(Box::new(parse_type_annotation(inner)?)));
    }

    if let Some(inner) = parse_wrapped_type(input, "Map")? {
        let (key, value) = split_type_pair(inner, input)?;
        return Ok(AxType::Map(
            Box::new(parse_type_annotation(key)?),
            Box::new(parse_type_annotation(value)?),
        ));
    }

    if let Some(inner) = parse_wrapped_type(input, "Result")? {
        let (ok, error) = split_type_pair(inner, input)?;
        return Ok(AxType::Result(
            Box::new(parse_type_annotation(ok)?),
            Box::new(parse_type_annotation(error)?),
        ));
    }

    if let Some(inner) = parse_wrapped_type(input, "Resource")? {
        let (value, error) = split_type_pair(inner, input)?;
        return Ok(AxType::Resource(
            Box::new(parse_type_annotation(value)?),
            Box::new(parse_type_annotation(error)?),
        ));
    }

    Ok(match input {
        "String" => AxType::String,
        "Number" => AxType::Number,
        "Int" => AxType::Int,
        "Float" => AxType::Float,
        "Bool" => AxType::Bool,
        "DateTime" => AxType::DateTime,
        "Date" => AxType::Date,
        "Time" => AxType::Time,
        "Uuid" => AxType::Uuid,
        "Bytes" => AxType::Bytes,
        "Json" => AxType::Json,
        "Never" => AxType::Never,
        "Void" => AxType::Void,
        "Unknown" => AxType::Unknown,
        name if is_type_identifier(name) => AxType::record(name),
        source => {
            return Err(AxTypeParseError::Invalid {
                raw: source.to_string(),
            })
        }
    })
}

fn split_type_pair<'a>(
    inner: &'a str,
    source: &str,
) -> Result<(&'a str, &'a str), AxTypeParseError> {
    let mut angle_depth = 0usize;
    let mut separator = None;

    for (index, ch) in inner.char_indices() {
        match ch {
            '<' => angle_depth += 1,
            '>' => {
                angle_depth =
                    angle_depth
                        .checked_sub(1)
                        .ok_or_else(|| AxTypeParseError::Invalid {
                            raw: source.to_string(),
                        })?;
            }
            ',' if angle_depth == 0 => {
                if separator.replace(index).is_some() {
                    return Err(AxTypeParseError::Invalid {
                        raw: source.to_string(),
                    });
                }
            }
            _ => {}
        }
    }

    let Some(separator) = separator else {
        return Err(AxTypeParseError::Invalid {
            raw: source.to_string(),
        });
    };
    let left = inner[..separator].trim();
    let right = inner[separator + 1..].trim();
    if left.is_empty() || right.is_empty() {
        return Err(AxTypeParseError::Invalid {
            raw: source.to_string(),
        });
    }
    Ok((left, right))
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
    #[error("expected Number, found {found}")]
    ExpectedNumber { found: String },
    #[error("cannot index value of type {ty}")]
    CannotIndex { ty: String },
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
    pub expression: Option<String>,
    pub message: String,
}

impl AxTypeCheckError {
    fn new(location: impl Into<String>, expression: Option<String>, error: AxTypeError) -> Self {
        Self {
            location: location.into(),
            expression,
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
                Err(error) => self.push_expr_error(
                    format!("{location}.data.{}", binding.name),
                    &binding.value,
                    error,
                ),
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
                    Err(error) => self.push_expr_error(
                        format!("{location}.each.source"),
                        &block.source,
                        error,
                    ),
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
            self.push_expr_error(location, expr, error);
        }
    }

    fn push_error(&mut self, location: impl Into<String>, error: AxTypeError) {
        self.errors
            .push(AxTypeCheckError::new(location, None, error));
    }

    fn push_expr_error(&mut self, location: impl Into<String>, expr: &AxExpr, error: AxTypeError) {
        self.errors.push(AxTypeCheckError::new(
            location,
            Some(format_expr(expr)),
            error,
        ));
    }
}

fn format_expr(expr: &AxExpr) -> String {
    match expr {
        AxExpr::String(value) => format!("{value:?}"),
        AxExpr::Number(value) => value.to_string(),
        AxExpr::Bool(value) => value.to_string(),
        AxExpr::List(items) => {
            let items = items.iter().map(format_expr).collect::<Vec<_>>().join(", ");
            format!("[{items}]")
        }
        AxExpr::Object(fields) => {
            let fields = fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", format_expr(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{fields}}}")
        }
        AxExpr::Identifier(name) => name.clone(),
        AxExpr::Unary { op, expr } => format!("{}{}", format_unary_op(*op), format_expr(expr)),
        AxExpr::Binary { op, left, right } => format!(
            "{} {} {}",
            format_expr(left),
            format_binary_op(*op),
            format_expr(right)
        ),
        AxExpr::Index { object, index } => format!(
            "{}[{}]",
            format_index_object_expr(object),
            format_expr(index)
        ),
        AxExpr::Member { object, property } => format!("{}.{}", format_expr(object), property),
        AxExpr::OptionalMember { object, property } => {
            format!("{}?.{}", format_expr(object), property)
        }
        AxExpr::Call { path, args } => {
            let args = args.iter().map(format_expr).collect::<Vec<_>>().join(", ");
            format!("{}({args})", path.join("."))
        }
    }
}

fn format_index_object_expr(expr: &AxExpr) -> String {
    let value = format_expr(expr);
    if index_object_needs_grouping(expr) {
        format!("({value})")
    } else {
        value
    }
}

fn index_object_needs_grouping(expr: &AxExpr) -> bool {
    matches!(expr, AxExpr::Binary { .. } | AxExpr::Unary { .. })
}

fn format_unary_op(op: AxUnaryOp) -> &'static str {
    match op {
        AxUnaryOp::Not => "!",
        AxUnaryOp::Neg => "-",
    }
}

fn format_binary_op(op: AxBinaryOp) -> &'static str {
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
    fn parses_canonical_scalar_and_collection_types() {
        let cases = [
            ("Int", AxType::Int),
            ("Float", AxType::Float),
            ("Date", AxType::Date),
            ("Time", AxType::Time),
            ("Uuid", AxType::Uuid),
            ("Bytes", AxType::Bytes),
            ("Json", AxType::Json),
            ("Never", AxType::Never),
            ("Void", AxType::Void),
            ("Post[]", AxType::list(AxType::record("Post"))),
            (
                "Map<String, List<Post>>",
                AxType::Map(
                    Box::new(AxType::String),
                    Box::new(AxType::list(AxType::record("Post"))),
                ),
            ),
            ("Set<Uuid>", AxType::Set(Box::new(AxType::Uuid))),
            (
                "Result<Post, String>",
                AxType::Result(Box::new(AxType::record("Post")), Box::new(AxType::String)),
            ),
            (
                "Resource<List<Post>, String>",
                AxType::Resource(
                    Box::new(AxType::list(AxType::record("Post"))),
                    Box::new(AxType::String),
                ),
            ),
        ];

        for (source, expected) in cases {
            assert_eq!(AxType::parse_annotation(source), Ok(expected), "{source}");
        }
    }

    #[test]
    fn rejects_malformed_multi_parameter_types() {
        for source in [
            "Map<String>",
            "Map<String, Post, Bool>",
            "Result<Post>",
            "Resource<Post,>",
        ] {
            assert!(AxType::parse_annotation(source).is_err(), "{source}");
        }
    }

    #[test]
    fn promotes_numeric_types_without_falling_back_to_string() {
        let context = AxDataContext::new()
            .with_binding("count", AxType::Int)
            .with_binding("delta", AxType::Int)
            .with_binding("ratio", AxType::Float);

        let int_sum = context
            .resolve_expr_type(&AxExpr::binary(
                AxBinaryOp::Add,
                AxExpr::ident("count"),
                AxExpr::ident("delta"),
            ))
            .expect("integer addition should resolve");
        let mixed_sum = context
            .resolve_expr_type(&AxExpr::binary(
                AxBinaryOp::Add,
                AxExpr::ident("count"),
                AxExpr::ident("ratio"),
            ))
            .expect("mixed numeric addition should resolve");

        assert_eq!(int_sum, AxType::Int);
        assert_eq!(mixed_sum, AxType::Float);
    }

    #[test]
    fn set_items_can_bind_each_and_optional_wrappers_are_idempotent() {
        let context =
            AxDataContext::new().with_binding("tags", AxType::Set(Box::new(AxType::record("Tag"))));
        let each = context
            .bind_each_item("tag", &AxExpr::ident("tags"))
            .expect("set should be iterable");

        assert_eq!(each.binding("tag"), Some(&AxType::record("Tag")));
        assert_eq!(
            AxType::optional(AxType::optional(AxType::String)),
            AxType::optional(AxType::String)
        );
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
    fn resolves_list_literal_item_type() {
        let context = AxDataContext::new();

        let strings = context
            .resolve_expr_type(&AxExpr::list([
                AxExpr::string("silver"),
                AxExpr::string("gold"),
            ]))
            .expect("list should resolve");
        let mixed = context
            .resolve_expr_type(&AxExpr::list([AxExpr::string("silver"), AxExpr::number(1)]))
            .expect("mixed list should resolve");

        assert_eq!(strings, AxType::list(AxType::String));
        assert_eq!(mixed, AxType::list(AxType::Unknown));
    }

    #[test]
    fn resolves_index_expression_type() {
        let context = post_context().with_binding("post", AxType::record("Post"));

        let list_item = context
            .resolve_expr_type(&AxExpr::list([AxExpr::string("hello")]).index(AxExpr::number(0)))
            .expect("list index should resolve");
        let record_field = context
            .resolve_expr_type(&AxExpr::ident("post").index(AxExpr::string("title")))
            .expect("record string index should resolve");

        assert_eq!(list_item, AxType::String);
        assert_eq!(record_field, AxType::String);
    }

    #[test]
    fn formats_index_expression_without_dropping_composite_grouping() {
        let expr = AxExpr::binary(AxBinaryOp::Fallback, AxExpr::ident("a"), AxExpr::ident("b"))
            .index(AxExpr::number(0));

        assert_eq!(format_expr(&expr), "(a ?? b)[0]");
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
    fn optional_member_allows_missing_record_field() {
        let context = post_context();
        let each_context = context
            .bind_each_item("post", &AxExpr::ident("posts"))
            .expect("posts should be iterable");

        let ty = each_context
            .resolve_expr_type(&AxExpr::ident("post").optional_member("summary"))
            .expect("optional member should not fail on missing field");

        assert_eq!(ty, AxType::Unknown);
    }

    #[test]
    fn optional_member_wraps_existing_field_type() {
        let context = post_context();
        let each_context = context
            .bind_each_item("post", &AxExpr::ident("posts"))
            .expect("posts should be iterable");

        let ty = each_context
            .resolve_expr_type(&AxExpr::ident("post").optional_member("title"))
            .expect("optional member should resolve existing field");

        assert_eq!(ty, AxType::optional(AxType::String));
    }

    #[test]
    fn optional_type_field_allows_regular_member_access() {
        let file = parse_ax_v2(
            r#"
page Blog

type Post {
  title: String
  summary?: String
}

let posts: List<Post> = load PostsList

<Each items={posts} as="post">
  <Card title={post.summary} />
</Each>
"#,
        )
        .expect("source should parse");
        let context = AxDataContext::from_v2_let_types(&file).expect("context should build");
        let each_context = context
            .bind_each_item("post", &AxExpr::ident("posts"))
            .expect("posts should be iterable");

        let ty = each_context
            .resolve_expr_type(&AxExpr::ident("post").member("summary"))
            .expect("optional type field should resolve");

        assert_eq!(ty, AxType::optional(AxType::String));
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
    fn classifies_client_state_types_and_initializers() {
        let list = AxType::parse_annotation("List<Optional<String>>")
            .expect("list state type should parse");
        assert!(list.supports_client_state());
        assert!(list.accepts_state_initializer(&AxExpr::List(vec![
            AxExpr::string("published"),
            AxExpr::ident("null"),
        ])));
        assert!(!AxType::Secret(Box::new(AxType::String)).supports_client_state());
        assert!(!AxType::record("Post").accepts_state_initializer(&AxExpr::string("draft")));
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
    fn rejects_duplicate_v2_type_fields() {
        let file = parse_ax_v2(
            r#"
page Blog

type Post {
  title: String
  title: String
}

<Copy>Blog</Copy>
"#,
        )
        .expect("source should parse");

        let error =
            AxDataContext::from_v2_let_types(&file).expect_err("duplicate field should fail");

        assert_eq!(
            error,
            AxTypeParseError::DuplicateField {
                record: "Post".to_string(),
                field: "title".to_string(),
            }
        );
    }

    #[test]
    fn rejects_duplicate_v2_type_declarations() {
        let file = parse_ax_v2(
            r#"
page Blog

type Post {
  title: String
}

type Post {
  slug: String
}

<Copy>Blog</Copy>
"#,
        )
        .expect("source should parse");

        let error =
            AxDataContext::from_v2_let_types(&file).expect_err("duplicate type should fail");

        assert_eq!(
            error,
            AxTypeParseError::DuplicateRecord {
                name: "Post".to_string(),
            }
        );
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
                expression: Some("post.summary".to_string()),
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
                expression: Some("post.title".to_string()),
                message: "unknown binding `post`".to_string(),
            }]
        );
    }
}
