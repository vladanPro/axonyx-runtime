use std::collections::BTreeMap;

/// Keep request identity/context, but never forward mutation payloads to reads.
pub fn page_read_request(
    request: &crate::server::AxHttpRequest,
    target: &str,
) -> crate::server::AxHttpRequest {
    let mut read = crate::server::AxHttpRequest::new("GET", target);
    read.headers = request
        .headers
        .iter()
        .filter(|(name, _)| {
            ![
                "content-type",
                "content-length",
                "transfer-encoding",
                "expect",
                "x-axonyx-csrf",
            ]
            .iter()
            .any(|excluded| name.eq_ignore_ascii_case(excluded))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    read
}

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

    /// Apply public errors to a render tree, never to serialized HTML.
    pub fn apply_to_node(&self, node: &mut axonyx_core::reactive::AxNode) {
        self.apply_node(node, false);
    }

    fn apply_node(&self, node: &mut axonyx_core::reactive::AxNode, mut matching: bool) {
        use axonyx_core::reactive::{attr, AxNode};
        let AxNode::Element {
            tag,
            attrs,
            children,
        } = node
        else {
            return;
        };
        if *tag == "form" {
            matching = attrs
                .iter()
                .find(|attr| attr.name == "action")
                .is_some_and(|action| {
                    if action.value.split('?').next() != Some("/__axonyx/action") {
                        return false;
                    }
                    let query = crate::parse_preview_query_fields(&action.value);
                    query.get("name").is_some_and(|name| name == &self.action)
                        && query.get("path").map(String::as_str).unwrap_or("/") == self.route
                });
        }
        if matching {
            if let Some(message) = attrs
                .iter()
                .find(|attr| attr.name == "data-ax-field-error")
                .and_then(|attr| self.fields.get(&attr.value))
            {
                *children = vec![AxNode::Text(message.clone())];
                attrs.retain(|attr| attr.name != "aria-live");
                attrs.push(attr("aria-live", "polite"));
            }
            if matches!(*tag, "input" | "select" | "textarea")
                && attrs
                    .iter()
                    .any(|attr| attr.name == "name" && self.fields.contains_key(&attr.value))
            {
                attrs.retain(|attr| attr.name != "aria-invalid");
                attrs.push(attr("aria-invalid", "true"));
            }
        }
        for child in children {
            self.apply_node(child, matching);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_read_context_drops_mutation_body_and_transport_headers() {
        let request = crate::server::AxHttpRequest::new("POST", "/__axonyx/action")
            .with_header("Cookie", "session=identity")
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_header("Content-Length", "18")
            .with_body(b"password=nevercopy".to_vec());
        let read = page_read_request(&request, "/forms");
        assert_eq!(read.method, "GET");
        assert_eq!(read.target, "/forms");
        assert!(read.body.is_empty());
        assert_eq!(read.header_value("Cookie"), Some("session=identity"));
        assert!(read.header_value("Content-Type").is_none());
        assert!(read.header_value("Content-Length").is_none());
    }

    #[test]
    fn compiled_action_form_url_uses_original_page_route() {
        let document = crate::compose_compiled_page_document(
            &[],
            "page Form\n<ActionForm name=\"Save\"><input name=\"email\" /></ActionForm>",
        )
        .unwrap();
        let html = crate::render_compiled_page_document(
            &serde_json::to_string(&document).unwrap(),
            &[],
            "/forms?ignored=1",
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        assert!(html.contains("action=\"/__axonyx/action?path=%2Fforms&amp;name=Save\""));
    }

    #[test]
    fn compiled_document_keeps_head_and_nested_layouts_with_form_errors() {
        let document = crate::compose_compiled_page_document(&[
            "page Shell\n<header>Application header</header>\n<Slot />",
            "page Section\n<section id=\"section\"><Slot /></section>",
        ], "page Register\n  title \"Registration\"\n<form method=\"post\" action=\"/__axonyx/action?name=Register&amp;path=%2Fregister\"><input name=\"email\" /><span data-ax-field-error=\"email\"></span></form>").unwrap();
        let mut document = document;
        document.head.title = Some("Registration".into());
        let result = AxFormResult::validation(
            "Register",
            "/register",
            &serde_json::json!({"email":"Invalid email."}),
        )
        .unwrap();
        let encoded = serde_json::to_string(&document)
            .unwrap()
            .replace("&amp;", "&");
        let html = crate::render_compiled_page_document(
            &encoded,
            &[],
            "/register",
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some(&result),
        )
        .unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<title>Registration</title>"));
        assert!(html.contains("Application header"));
        assert!(html.contains("id=\"section\""));
        assert!(html.contains("Invalid email."));
        assert!(html.contains("aria-invalid=\"true\""));
    }

    #[test]
    fn node_rendering_targets_only_the_matching_form_and_escapes_text() {
        use axonyx_core::reactive::{attr, AxNode};
        let result = AxFormResult::validation(
            "Save",
            "/posts",
            &serde_json::json!({"email":"<script>bad</script>"}),
        )
        .unwrap();
        let form = |name| AxNode::Element {
            tag: "form",
            attrs: vec![attr(
                "action",
                format!("/__axonyx/action?name={name}&path=%2Fposts"),
            )],
            children: vec![
                AxNode::Element {
                    tag: "input",
                    attrs: vec![attr("name", "email")],
                    children: vec![],
                },
                AxNode::Element {
                    tag: "span",
                    attrs: vec![attr("data-ax-field-error", "email")],
                    children: vec![],
                },
            ],
        };
        let mut node = AxNode::Element {
            tag: "div",
            attrs: vec![],
            children: vec![form("Save"), form("Other")],
        };
        result.apply_to_node(&mut node);
        let mut html = String::new();
        crate::render_node(&node, &mut html);
        assert_eq!(html.matches("aria-invalid=\"true\"").count(), 1);
        assert_eq!(html.matches("&lt;script&gt;").count(), 1);
        assert!(!html.contains("<script>"));
    }

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
