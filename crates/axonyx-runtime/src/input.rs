//! Contract validation for required named-record request inputs.

use axonyx_core::ax_types::prelude::{AxDataContext, AxType};
use serde_json::Value;

use crate::backend::{AxRuntimeError, AxRuntimeResult};
use crate::server::AxHttpRequest;

pub fn decode_record(
    request: Option<&AxHttpRequest>,
    explicit: Option<&str>,
    field: &str,
    contract: &str,
    context: &AxDataContext,
) -> AxRuntimeResult<Value> {
    let invalid = || AxRuntimeError::invalid_input(field);
    let value = if let Some(raw) = explicit {
        serde_json::from_str(raw).map_err(|_| invalid())?
    } else if let Some(request) = request {
        if request.header_value("Content-Type").is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        }) {
            request.json_field_value(field).ok_or_else(invalid)?
        } else {
            let raw = request.form_value(field).ok_or_else(invalid)?;
            serde_json::from_str(&raw).map_err(|_| invalid())?
        }
    } else {
        return Err(invalid());
    };
    let expected = AxType::record(contract);
    context
        .validate_json(&expected, &value)
        .map_err(|_| invalid())?;
    project(value, &expected, context).ok_or_else(invalid)
}

// Match generated Serde records: omit undeclared properties and materialize
// missing optional properties as null, including nested records and lists.
fn project(value: Value, ty: &AxType, context: &AxDataContext) -> Option<Value> {
    match (value, ty) {
        (Value::Object(mut fields), AxType::Record(name)) if context.record(name).is_some() => {
            Some(Value::Object(
                context
                    .record(name)
                    .unwrap()
                    .fields
                    .iter()
                    .map(|(name, ty)| {
                        let value = fields.remove(name).unwrap_or(Value::Null);
                        Some((name.clone(), project(value, ty, context)?))
                    })
                    .collect::<Option<_>>()?,
            ))
        }
        (value, AxType::Optional(inner)) if !value.is_null() => project(value, inner, context),
        (Value::Array(items), AxType::List(inner)) => Some(Value::Array(
            items
                .into_iter()
                .map(|value| project(value, inner, context))
                .collect::<Option<_>>()?,
        )),
        (value, AxType::Int) => value.as_i64().map(|value| Value::Number(value.into())),
        (value, AxType::Float | AxType::Number) => value
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        (Value::Object(fields), AxType::Map(_, inner)) => Some(Value::Object(
            fields
                .into_iter()
                .map(|(name, value)| Some((name, project(value, inner, context)?)))
                .collect::<Option<_>>()?,
        )),
        (
            value,
            AxType::String
            | AxType::Bool
            | AxType::DateTime
            | AxType::Date
            | AxType::Time
            | AxType::Uuid,
        ) => Some(value),
        (Value::Null, AxType::Optional(_)) => Some(Value::Null),
        (value, AxType::Record(name)) if context.literal_union(name).is_some() => Some(value),
        // V0 deliberately rejects carriers whose preview/Serde representation
        // has not been aligned yet (Decimal, Set, Result, Json, wrappers).
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonyx_core::ax_types::prelude::AxRecordType;

    #[test]
    fn explicit_and_form_json_records_share_the_contract_boundary() {
        let context = AxDataContext::new().with_record(
            AxRecordType::new("Post")
                .field("title", AxType::String)
                .field("summary", AxType::optional(AxType::String)),
        );
        let raw = r#"{"title":"Hello","extra":true}"#;
        let expected = serde_json::json!({"title":"Hello", "summary":null});
        assert_eq!(
            decode_record(None, Some(raw), "post", "Post", &context).unwrap(),
            expected
        );
        let request = AxHttpRequest::new("POST", "/save")
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_body(b"post=%7B%22title%22%3A%22Hello%22%2C%22extra%22%3Atrue%7D".to_vec());
        assert_eq!(
            decode_record(Some(&request), None, "post", "Post", &context).unwrap(),
            expected
        );
        for raw in ["broken", "null", "[]", r#"{"title":123}"#] {
            assert!(
                matches!(decode_record(None, Some(raw), "post", "Post", &context), Err(AxRuntimeError::InvalidInput { ref field }) if field == "post")
            );
        }
    }
}
