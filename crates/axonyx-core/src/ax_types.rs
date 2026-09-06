use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::ax_ast::prelude::{
    AxBinaryOp, AxBody, AxComponent, AxDocument, AxExpr, AxPipelineStage, AxStatement, AxUnaryOp,
};
use crate::ax_ast_v2::prelude::AxFileV2;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AxDecimal(String);

impl AxDecimal {
    pub fn new(value: impl Into<String>) -> Result<Self, AxDecimalError> {
        let value = value.into();
        if is_valid_decimal(&value) {
            Ok(Self(value))
        } else {
            Err(AxDecimalError::Invalid { value })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AxDecimal {
    fn default() -> Self {
        Self("0".to_string())
    }
}

impl fmt::Display for AxDecimal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for AxDecimal {
    type Err = AxDecimalError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for AxDecimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AxDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let value = match value {
            serde_json::Value::String(value) => value,
            serde_json::Value::Number(value) => value.to_string(),
            _ => {
                return Err(D::Error::custom(
                    "decimal value must be a JSON string or number",
                ))
            }
        };
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AxDecimalError {
    #[error("invalid decimal value `{value}`")]
    Invalid { value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AxType {
    String,
    Number,
    Int,
    Float,
    Decimal,
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
            Self::Decimal => "Decimal".to_string(),
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
            Self::Decimal
            | Self::Never
            | Self::Void
            | Self::Secret(_)
            | Self::Signal(_)
            | Self::Resource(_, _) => false,
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
            Self::String => matches!(value, AxExpr::String(_)),
            Self::DateTime | Self::Date | Self::Time | Self::Uuid => {
                matches!(value, AxExpr::String(value) if self.accepts_string_literal(value))
            }
            Self::Number => matches!(value, AxExpr::Number(_) | AxExpr::Float(_)),
            Self::Int => matches!(value, AxExpr::Number(_)),
            Self::Float => matches!(value, AxExpr::Number(_) | AxExpr::Float(_)),
            Self::Decimal => match value {
                AxExpr::String(value) => is_valid_decimal(value),
                AxExpr::Number(_) | AxExpr::Float(_) => true,
                _ => false,
            },
            Self::Bool => matches!(value, AxExpr::Bool(_)),
            Self::Bytes => matches!(value, AxExpr::List(items) if items.iter().all(|item| {
                matches!(item, AxExpr::Number(number) if (0..=255).contains(number))
            })),
            Self::Json | Self::Unknown => matches!(
                value,
                AxExpr::String(_)
                    | AxExpr::Number(_)
                    | AxExpr::Float(_)
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
            Self::String => true,
            Self::DateTime | Self::Date | Self::Time | Self::Uuid => {
                self.accepts_string_literal(value)
            }
            Self::Int => value.parse::<i64>().is_ok(),
            Self::Decimal => is_valid_decimal(value),
            Self::Bool => matches!(value, "true" | "false"),
            Self::Public(inner) => inner.accepts_map_key(value),
            _ => false,
        }
    }

    fn accepts_string_literal(&self, value: &str) -> bool {
        match self {
            Self::String => true,
            Self::Date => is_valid_date(value),
            Self::Time => is_valid_time(value),
            Self::DateTime => is_valid_datetime(value),
            Self::Uuid => is_valid_uuid(value),
            Self::Decimal => is_valid_decimal(value),
            Self::Public(inner) => inner.accepts_string_literal(value),
            _ => false,
        }
    }
}

fn is_valid_date(value: &str) -> bool {
    if !value.is_ascii()
        || value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
    {
        return false;
    }
    let Some(year) = parse_fixed_u32(&value[0..4]) else {
        return false;
    };
    let Some(month) = parse_fixed_u32(&value[5..7]) else {
        return false;
    };
    let Some(day) = parse_fixed_u32(&value[8..10]) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=max_day).contains(&day)
}

fn is_valid_decimal(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    if value.is_empty() {
        return false;
    }
    let mut parts = value.split('.');
    let Some(integer) = parts.next() else {
        return false;
    };
    let fraction = parts.next();
    if parts.next().is_some()
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    fraction.is_none_or(|fraction| {
        !fraction.is_empty() && fraction.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn is_valid_time(value: &str) -> bool {
    if !value.is_ascii() {
        return false;
    }
    let (clock, fraction) = value
        .split_once('.')
        .map_or((value, None), |(clock, fraction)| (clock, Some(fraction)));
    if clock.len() != 8 || clock.as_bytes()[2] != b':' || clock.as_bytes()[5] != b':' {
        return false;
    }
    let Some(hour) = parse_fixed_u32(&clock[0..2]) else {
        return false;
    };
    let Some(minute) = parse_fixed_u32(&clock[3..5]) else {
        return false;
    };
    let Some(second) = parse_fixed_u32(&clock[6..8]) else {
        return false;
    };
    hour <= 23
        && minute <= 59
        && second <= 59
        && fraction.is_none_or(|fraction| {
            (1..=9).contains(&fraction.len()) && fraction.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_valid_datetime(value: &str) -> bool {
    if !value.is_ascii() {
        return false;
    }
    let Some((date, time_and_zone)) = value.split_once('T') else {
        return false;
    };
    if !is_valid_date(date) {
        return false;
    }
    let (time, zone) = if let Some(time) = time_and_zone.strip_suffix('Z') {
        (time, "Z")
    } else {
        let Some(index) = time_and_zone.rfind(['+', '-']) else {
            return false;
        };
        (&time_and_zone[..index], &time_and_zone[index..])
    };
    if !is_valid_time(time) {
        return false;
    }
    if zone == "Z" {
        return true;
    }
    zone.len() == 6
        && matches!(zone.as_bytes()[0], b'+' | b'-')
        && zone.as_bytes()[3] == b':'
        && parse_fixed_u32(&zone[1..3]).is_some_and(|hour| hour <= 23)
        && parse_fixed_u32(&zone[4..6]).is_some_and(|minute| minute <= 59)
}

fn is_valid_uuid(value: &str) -> bool {
    value.is_ascii()
        && value.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| value.as_bytes()[index] == b'-')
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn parse_fixed_u32(value: &str) -> Option<u32> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
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
    pub literal_unions: BTreeMap<String, Vec<String>>,
}

impl AxDataContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_record(mut self, record: AxRecordType) -> Self {
        self.records.insert(record.name.clone(), record);
        self
    }

    pub fn with_literal_union(
        mut self,
        name: impl Into<String>,
        literals: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.literal_unions
            .insert(name.into(), literals.into_iter().map(Into::into).collect());
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

    pub fn literal_union(&self, name: &str) -> Option<&[String]> {
        self.literal_unions.get(name).map(Vec::as_slice)
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
                if let Some(literals) = self.literal_unions.get(name) {
                    return matches!(value, AxExpr::String(value) if literals.contains(value));
                }
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
        for declaration in &file.types {
            if context.records.contains_key(&declaration.name)
                || context.literal_unions.contains_key(&declaration.name)
            {
                return Err(AxTypeParseError::DuplicateRecord {
                    name: declaration.name.clone(),
                });
            }
            if declaration.is_literal_union() {
                context = context
                    .with_literal_union(declaration.name.clone(), declaration.literals.clone());
                continue;
            }
            let mut record_type = AxRecordType::new(declaration.name.clone());
            for field in &declaration.fields {
                if record_type.fields.contains_key(&field.name) {
                    return Err(AxTypeParseError::DuplicateField {
                        record: declaration.name.clone(),
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
        for binding in &file.states {
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
            AxExpr::Float(_) => Ok(AxType::Float),
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
                if let (AxExpr::Object(fields), AxExpr::String(property)) =
                    (object.as_ref(), index.as_ref())
                {
                    return self.resolve_object_literal_field(fields, property);
                }
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
                if let AxExpr::Object(fields) = object.as_ref() {
                    return self.resolve_object_literal_field(fields, property);
                }
                let object_type = self.resolve_expr_type(object)?;
                self.resolve_member_type(&object_type, property)
            }
            AxExpr::OptionalMember { object, property } => {
                if let AxExpr::Object(fields) = object.as_ref() {
                    return Ok(match fields.get(property) {
                        Some(value) => AxType::optional(self.resolve_expr_type(value)?),
                        None => AxType::Unknown,
                    });
                }
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

    fn resolve_object_literal_field(
        &self,
        fields: &BTreeMap<String, AxExpr>,
        property: &str,
    ) -> Result<AxType, AxTypeError> {
        let value = fields
            .get(property)
            .ok_or_else(|| AxTypeError::UnknownField {
                record: "object literal".to_string(),
                field: property.to_string(),
            })?;
        self.resolve_expr_type(value)
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
        "Decimal" => AxType::Decimal,
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
    #[error("duplicate match case `{value}`")]
    DuplicateMatchCase { value: String },
    #[error("match case `{value}` is not part of literal union `{union}`")]
    UnknownMatchCase { union: String, value: String },
    #[error("match on literal union `{union}` is not exhaustive; missing {missing}")]
    NonExhaustiveMatch { union: String, missing: String },
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
                Ok(_) if self.context.binding(&binding.name).is_some() => {}
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
                        if let Some(key) = &block.key {
                            body_checker.check_expr(key, format!("{location}.each.key"));
                        }
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
            AxStatement::Match(block) => {
                self.check_expr(&block.value, format!("{location}.match.value"));
                for (case_index, case) in block.cases.iter().enumerate() {
                    self.check_statements(
                        &case.body,
                        &format!("{location}.match.case[{case_index}]"),
                    );
                }
                self.check_statements(
                    block.default_body.as_deref().unwrap_or_default(),
                    &format!("{location}.match.default"),
                );

                let union = match self.context.resolve_expr_type(&block.value) {
                    Ok(AxType::Record(union)) => Some(union),
                    _ => None,
                };
                self.check_match_cases(
                    union,
                    block.cases.iter().map(|case| case.value.clone()).collect(),
                    block.default_body.is_some(),
                    &format!("{location}.match"),
                );
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
        if component.name == "__AxStateMatch" {
            self.check_reactive_match(component, location);
        }
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

    fn check_reactive_match(&mut self, component: &AxComponent, location: &str) {
        let state_name = component.props.iter().find_map(|prop| {
            (prop.name == "data-ax-state-match-name")
                .then_some(&prop.value)
                .and_then(|value| match value {
                    AxExpr::String(value) => Some(value.as_str()),
                    _ => None,
                })
        });
        let union = state_name.and_then(|name| match self.context.binding(name) {
            Some(AxType::Record(union)) => Some(union.clone()),
            _ => None,
        });
        let AxBody::Block(branches) = &component.body else {
            return;
        };
        let cases = branches
            .iter()
            .filter_map(|statement| match statement {
                AxStatement::Component(branch) if branch.name == "__AxStateMatchCase" => branch
                    .props
                    .iter()
                    .find_map(|prop| match (prop.name.as_str(), &prop.value) {
                        ("case", AxExpr::String(value)) => Some(value.clone()),
                        _ => None,
                    }),
                _ => None,
            })
            .collect();
        let has_default = branches.iter().any(|statement| {
            matches!(statement, AxStatement::Component(branch) if branch.name == "__AxStateMatchDefault")
        });
        self.check_match_cases(union, cases, has_default, &format!("{location}.match"));
    }

    fn check_match_cases(
        &mut self,
        union: Option<String>,
        cases: Vec<String>,
        has_default: bool,
        location: &str,
    ) {
        let mut seen = BTreeSet::new();
        for (index, value) in cases.iter().enumerate() {
            if !seen.insert(value.as_str()) {
                self.push_error(
                    format!("{location}.case[{index}]"),
                    AxTypeError::DuplicateMatchCase {
                        value: value.clone(),
                    },
                );
            }
        }
        let Some(union) = union else {
            return;
        };
        let Some(literals) = self.context.literal_union(&union).map(<[_]>::to_vec) else {
            return;
        };
        for value in &seen {
            if !literals.iter().any(|literal| literal == *value) {
                self.push_error(
                    location,
                    AxTypeError::UnknownMatchCase {
                        union: union.clone(),
                        value: (*value).to_string(),
                    },
                );
            }
        }
        if has_default {
            return;
        }
        let missing = literals
            .iter()
            .filter(|literal| !seen.contains(literal.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            self.push_error(
                location,
                AxTypeError::NonExhaustiveMatch {
                    union,
                    missing: missing.join(", "),
                },
            );
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

pub(crate) fn format_expr(expr: &AxExpr) -> String {
    match expr {
        AxExpr::String(value) => format!("{value:?}"),
        AxExpr::Number(value) => value.to_string(),
        AxExpr::Float(value) => value.get().to_string(),
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
    pub use super::AxDecimal;
    pub use super::AxDecimalError;
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
            ("Decimal", AxType::Decimal),
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
    fn decimal_contract_preserves_exact_json_digits() {
        let source = "12345678901234567890.12345678901234567890";
        let decimal: AxDecimal = serde_json::from_str(source)
            .expect("an arbitrary-precision JSON number should deserialize");

        assert_eq!(decimal.as_str(), source);
        assert_eq!(
            serde_json::to_string(&decimal).expect("decimal should serialize"),
            format!("\"{source}\"")
        );
        assert!("12.30".parse::<AxDecimal>().is_ok());
        assert!("-0.0012".parse::<AxDecimal>().is_ok());
        assert!("1e3".parse::<AxDecimal>().is_err());
        assert!(".5".parse::<AxDecimal>().is_err());
    }

    #[test]
    fn validates_decimal_state_literals_without_float_coercion() {
        assert!(AxType::Decimal.accepts_state_initializer(&AxExpr::string("12.30")));
        assert!(AxType::Decimal.accepts_state_initializer(&AxExpr::number(12)));
        assert!(!AxType::Decimal.accepts_state_initializer(&AxExpr::string("1e3")));
        assert!(!AxType::Decimal.accepts_state_initializer(&AxExpr::string("12.")));
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
        assert_eq!(
            context.resolve_expr_type(&AxExpr::float(0.5)),
            Ok(AxType::Float)
        );
    }

    #[test]
    fn validates_canonical_date_time_and_uuid_literals() {
        for (ty, valid, invalid) in [
            (AxType::Date, "2024-02-29", "2023-02-29"),
            (AxType::Time, "23:59:59.125", "24:00:00"),
            (
                AxType::DateTime,
                "2026-08-23T10:15:30Z",
                "2026-08-23T10:15:30",
            ),
            (
                AxType::Uuid,
                "550e8400-e29b-41d4-a716-446655440000",
                "550e8400e29b41d4a716446655440000",
            ),
        ] {
            assert!(
                ty.accepts_state_initializer(&AxExpr::string(valid)),
                "{valid}"
            );
            assert!(
                !ty.accepts_state_initializer(&AxExpr::string(invalid)),
                "{invalid}"
            );
        }

        assert!(AxType::DateTime
            .accepts_state_initializer(&AxExpr::string("2026-08-23T10:15:30.250+02:00")));
        assert!(!AxType::Date.accepts_state_initializer(&AxExpr::string("é026-08-23")));
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
    fn resolves_object_literal_field_types() {
        let context = AxDataContext::new();
        let object = AxExpr::object([("active", AxExpr::bool(true)), ("count", AxExpr::number(2))]);

        assert_eq!(
            context.resolve_expr_type(&object.clone().member("active")),
            Ok(AxType::Bool)
        );
        assert_eq!(
            context.resolve_expr_type(&object.clone().index(AxExpr::string("count"))),
            Ok(AxType::Number)
        );
        assert_eq!(
            context.resolve_expr_type(&object.clone().optional_member("active")),
            Ok(AxType::optional(AxType::Bool))
        );
        assert_eq!(
            context.resolve_expr_type(&object.member("missing")),
            Err(AxTypeError::UnknownField {
                record: "object literal".to_string(),
                field: "missing".to_string(),
            })
        );
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
    fn validates_literal_union_state_initializers() {
        let file = parse_ax_v2(
            r#"page ThemePreview() {
type Theme = "silver" | "bronze" | "gold"
state theme: Theme = "silver"
return ASX { <Copy>{theme}</Copy> }
}
"#,
        )
        .expect("literal union state should parse");
        let context = AxDataContext::from_v2_let_types(&file).expect("context should build");

        assert!(
            context.accepts_state_initializer(&AxType::record("Theme"), &AxExpr::string("gold"))
        );
        assert!(
            !context.accepts_state_initializer(&AxType::record("Theme"), &AxExpr::string("purple"))
        );
    }

    fn check_v2_source(source: &str) -> AxTypeCheckReport {
        let file = parse_ax_v2(source).expect("source should parse as v2");
        let context = AxDataContext::from_v2_let_types(&file).expect("context should build");
        let document = crate::ax_parser_auto::parse_ax_auto(source)
            .expect("source should convert into runtime AST");
        check_document_types(&document, &context)
    }

    #[test]
    fn accepts_exhaustive_literal_union_match() {
        let report = check_v2_source(
            r#"page ThemePreview() {
type Theme = "silver" | "bronze" | "gold"
state theme: Theme = "silver"
return ASX {
  <Match value={theme}>
    <Case is="silver"><Copy>Silver</Copy></Case>
    <Case is="bronze"><Copy>Bronze</Copy></Case>
    <Case is="gold"><Copy>Gold</Copy></Case>
  </Match>
}
}
"#,
        );

        assert!(report.is_ok(), "{report:#?}");
    }

    #[test]
    fn reports_missing_literal_union_match_cases() {
        let report = check_v2_source(
            r#"page ThemePreview() {
type Theme = "silver" | "bronze" | "gold"
state theme: Theme = "silver"
return ASX {
  <Match value={theme}>
    <Case is="silver"><Copy>Silver</Copy></Case>
  </Match>
}
}
"#,
        );

        assert!(report.errors.iter().any(|error| {
            error.message
                == "match on literal union `Theme` is not exhaustive; missing bronze, gold"
        }));
    }

    #[test]
    fn reports_unknown_and_duplicate_literal_union_match_cases() {
        let report = check_v2_source(
            r#"page ThemePreview() {
type Theme = "silver" | "gold"
state theme: Theme = "silver"
return ASX {
  <Match value={theme}>
    <Case is="silver"><Copy>Silver</Copy></Case>
    <Case is="purple"><Copy>Purple</Copy></Case>
    <Case is="purple"><Copy>Purple again</Copy></Case>
    <Default><Copy>Other</Copy></Default>
  </Match>
}
}
"#,
        );

        assert!(report
            .errors
            .iter()
            .any(|error| error.message == "duplicate match case `purple`"));
        assert!(report.errors.iter().any(|error| {
            error.message == "match case `purple` is not part of literal union `Theme`"
        }));
    }

    #[test]
    fn empty_default_makes_literal_union_match_exhaustive() {
        let report = check_v2_source(
            r#"page ThemePreview() {
type Theme = "silver" | "gold"
state theme: Theme = "silver"
return ASX {
  <Match value={theme}>
    <Case is="silver"><Copy>Silver</Copy></Case>
    <Default />
  </Match>
}
}
"#,
        );

        assert!(report.is_ok(), "{report:#?}");
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
