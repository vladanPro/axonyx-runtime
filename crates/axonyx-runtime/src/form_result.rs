use std::collections::BTreeMap;

/// Request-local public validation metadata. Never accepts submitted values.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AxFormResult {
    version: u8,
    action: String,
    route: String,
    status: u16,
    fields: BTreeMap<String, String>,
}

impl AxFormResult {
    pub fn validation(action: &str, route: &str, fields: &serde_json::Value) -> Option<Self> {
        if action.is_empty()
            || action.len() > 128
            || !action
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !route.starts_with('/')
            || route.starts_with("//")
            || route.len() > 2048
            || route.contains(['\\', '%', '?', '#'])
            || route.chars().any(char::is_control)
        {
            return None;
        }
        let fields = fields
            .as_object()
            .into_iter()
            .flat_map(|fields| fields.iter())
            .filter_map(|(name, message)| {
                if name.is_empty()
                    || name.len() > 128
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return None;
                }
                message
                    .as_str()
                    .map(|message| (name.clone(), message.chars().take(512).collect()))
            })
            .take(32)
            .collect();
        Some(Self {
            version: 1,
            action: action.into(),
            route: route.into(),
            status: 422,
            fields,
        })
    }

    pub fn for_form(&self, action: &str, route: &str) -> Option<&BTreeMap<String, String>> {
        (self.action == action && self.route == route).then_some(&self.fields)
    }

    pub fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_is_bound_to_action_and_route_without_input_values() {
        let result = AxFormResult::validation(
            "Register",
            "/register",
            &serde_json::json!({"email":"Invalid email.", "object": {"secret":"not-a-message"}}),
        )
        .unwrap();
        assert!(result.for_form("Login", "/register").is_none());
        assert!(result.for_form("Register", "/other").is_none());
        assert_eq!(result.for_form("Register", "/register").unwrap().len(), 1);
        let payload = serde_json::to_value(result).unwrap();
        assert_eq!(payload["version"], 1);
        assert_eq!(payload["status"], 422);
        assert!(payload.get("input").is_none());
        assert!(!payload.to_string().contains("not-a-message"));
    }

    #[test]
    fn identifiers_and_messages_are_bounded() {
        for route in [
            "//evil.example",
            "/\\evil",
            "/%2f%2fevil",
            "/form?token=secret",
        ] {
            assert!(AxFormResult::validation("Register", route, &serde_json::json!({})).is_none());
        }
        assert!(AxFormResult::validation("bad action", "/", &serde_json::json!({})).is_none());
        let result =
            AxFormResult::validation("Save", "/", &serde_json::json!({"text": "x".repeat(600)}))
                .unwrap();
        assert_eq!(result.fields()["text"].len(), 512);
    }
}
