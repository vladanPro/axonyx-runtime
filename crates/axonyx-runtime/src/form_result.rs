use std::collections::BTreeMap;

// Bound the complete encoded request, not individual editor controls. Decoding
// cannot expand a URL-encoded value beyond this request-wide byte budget.
const MAX_REPLAY_BODY_BYTES: usize = 65536;

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

/// Public errors plus private, bounded candidates for explicitly opted-in forms.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AxFormResult {
    version: u8,
    action: String,
    route: String,
    status: u16,
    fields: BTreeMap<String, String>,
    #[serde(skip)]
    submitted: Option<BTreeMap<String, String>>,
}

impl std::fmt::Debug for AxFormResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AxFormResult")
            .field("action", &self.action)
            .field("route", &self.route)
            .field("status", &self.status)
            .field("fields", &self.fields)
            .finish_non_exhaustive()
    }
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
            submitted: None,
        })
    }

    /// Retain candidates only for this action request, never arbitrary GET/JSON data.
    /// Actual replay also requires the server-rendered form's explicit allowlist.
    pub fn with_request_values(mut self, request: &crate::server::AxHttpRequest) -> Self {
        let query = crate::parse_preview_query_fields(&request.target);
        if request.method != "POST"
            || request.target.split('?').next() != Some("/__axonyx/action")
            || query.get("name") != Some(&self.action)
            || query.get("path").map(String::as_str).unwrap_or("/") != self.route
        {
            return self;
        }
        let content_type = request
            .header_value("content-type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if !content_type.eq_ignore_ascii_case("application/x-www-form-urlencoded")
            || request.body.len() > MAX_REPLAY_BODY_BYTES
            || request.multipart.is_some()
        {
            return self;
        }
        let Ok(body) = std::str::from_utf8(&request.body) else {
            return self;
        };
        let mut values = BTreeMap::new();
        for (index, pair) in body.split('&').filter(|pair| !pair.is_empty()).enumerate() {
            if index >= 32 {
                return self;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = crate::url_decode(name);
            if !safe_replay_name(&name) {
                continue;
            }
            let value = crate::url_decode(value);
            // Ambiguous repeated controls disable replay for the whole form.
            if values.insert(name, value).is_some() {
                return self;
            }
        }
        self.submitted = Some(values);
        self
    }

    pub fn for_form(&self, action: &str, route: &str) -> Option<&BTreeMap<String, String>> {
        (self.action == action && self.route == route).then_some(&self.fields)
    }

    pub fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }

    /// Apply public errors to a render tree, never to serialized HTML.
    pub fn apply_to_node(&self, node: &mut axonyx_core::reactive::AxNode) {
        self.apply_node(node, false, &[]);
    }

    fn apply_node(
        &self,
        node: &mut axonyx_core::reactive::AxNode,
        mut matching: bool,
        allowed: &[String],
    ) {
        use axonyx_core::reactive::{attr, AxNode};
        let AxNode::Element {
            tag,
            attrs,
            children,
        } = node
        else {
            return;
        };
        let mut form_allowed = allowed.to_vec();
        if *tag == "form" {
            form_allowed = attrs
                .iter()
                .find(|attr| attr.name == "data-ax-retain-fields")
                .map(|attr| {
                    attr.value
                        .split(',')
                        .map(str::trim)
                        .filter(|name| safe_replay_name(name))
                        .take(32)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
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
            if let Some(submitted) = &self.submitted {
                replay_control(tag, attrs, children, &form_allowed, submitted);
            }
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
            self.apply_node(child, matching, &form_allowed);
        }
    }
}

fn safe_replay_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && !lower.starts_with("__ax")
        && ![
            "password",
            "passwd",
            "secret",
            "token",
            "credential",
            "csrf",
            "api_key",
            "apikey",
        ]
        .iter()
        .any(|word| lower.contains(word))
}

fn replay_control(
    tag: &str,
    attrs: &mut Vec<axonyx_core::reactive::Attribute>,
    children: &mut Vec<axonyx_core::reactive::AxNode>,
    allowed: &[String],
    submitted: &BTreeMap<String, String>,
) {
    use axonyx_core::reactive::{attr, AxNode};
    let Some(name) = attrs
        .iter()
        .find(|attr| attr.name == "name")
        .map(|attr| attr.value.clone())
    else {
        return;
    };
    if !allowed.contains(&name) || attrs.iter().any(|attr| attr.name == "disabled") {
        return;
    }
    let value = submitted.get(&name);
    match tag {
        "input" => {
            let kind = attrs
                .iter()
                .find(|attr| attr.name == "type")
                .map(|attr| attr.value.to_ascii_lowercase())
                .unwrap_or_else(|| "text".into());
            if attrs.iter().any(|attr| {
                attr.name == "autocomplete"
                    && attr.value.split_ascii_whitespace().any(|token| {
                        ["current-password", "new-password", "one-time-code"]
                            .iter()
                            .any(|secret| token.eq_ignore_ascii_case(secret))
                    })
            }) {
                return;
            }
            if matches!(kind.as_str(), "checkbox" | "radio") {
                let expected = attrs
                    .iter()
                    .find(|attr| attr.name == "value")
                    .map(|attr| attr.value.as_str())
                    .unwrap_or("on");
                let checked = value.is_some_and(|value| value == expected);
                attrs.retain(|attr| attr.name != "checked");
                if checked {
                    attrs.push(attr("checked", "true"));
                }
            } else if matches!(
                kind.as_str(),
                "text"
                    | "email"
                    | "search"
                    | "tel"
                    | "url"
                    | "number"
                    | "range"
                    | "color"
                    | "date"
                    | "time"
                    | "datetime-local"
                    | "month"
                    | "week"
            ) {
                if let Some(value) = value {
                    attrs.retain(|attr| attr.name != "value");
                    attrs.push(attr("value", value));
                }
            }
        }
        "textarea" => {
            if let Some(value) = value {
                *children = vec![AxNode::Text(value.clone())];
            }
        }
        "select" if !attrs.iter().any(|attr| attr.name == "multiple") => {
            if let Some(value) = value {
                if has_select_option(children, value) {
                    select_options(children, value);
                }
            }
        }
        _ => {}
    }
}

fn select_options(nodes: &mut [axonyx_core::reactive::AxNode], value: &str) {
    use axonyx_core::reactive::{attr, AxNode};
    for node in nodes {
        if let AxNode::Element {
            tag,
            attrs,
            children,
        } = node
        {
            if *tag == "option" {
                let matches = attrs
                    .iter()
                    .find(|attr| attr.name == "value")
                    .is_some_and(|attr| attr.value == value);
                attrs.retain(|attr| attr.name != "selected");
                if matches {
                    attrs.push(attr("selected", "true"));
                }
            } else if *tag == "optgroup" {
                select_options(children, value);
            }
        }
    }
}

fn has_select_option(nodes: &[axonyx_core::reactive::AxNode], value: &str) -> bool {
    use axonyx_core::reactive::AxNode;
    nodes.iter().any(|node| match node {
        AxNode::Element {
            tag: "option",
            attrs,
            ..
        } => attrs
            .iter()
            .any(|attr| attr.name == "value" && attr.value == value),
        AxNode::Element {
            tag: "optgroup",
            children,
            ..
        } => has_select_option(children, value),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_respects_radio_disabled_and_secret_autocomplete_controls() {
        use axonyx_core::reactive::attr;
        let values = BTreeMap::from([("choice".into(), "gold".into())]);
        let allowed = vec!["choice".into()];
        let mut children = vec![];
        let mut radio = vec![
            attr("name", "choice"),
            attr("type", "radio"),
            attr("value", "gold"),
        ];
        replay_control("input", &mut radio, &mut children, &allowed, &values);
        assert!(radio.iter().any(|attr| attr.name == "checked"));
        let other = BTreeMap::from([("choice".into(), "silver".into())]);
        replay_control("input", &mut radio, &mut children, &allowed, &other);
        assert!(!radio.iter().any(|attr| attr.name == "checked"));

        for restriction in [
            attr("disabled", "true"),
            attr("autocomplete", "section-login ONE-TIME-CODE"),
        ] {
            let mut attrs = vec![
                attr("name", "choice"),
                attr("value", "initial"),
                restriction,
            ];
            replay_control("input", &mut attrs, &mut children, &allowed, &values);
            assert!(attrs
                .iter()
                .any(|attr| attr.name == "value" && attr.value == "initial"));
        }
    }

    fn replay_request(body: &str) -> crate::server::AxHttpRequest {
        crate::server::AxHttpRequest::new("POST", "/__axonyx/action?name=Save&path=%2Fposts")
            .with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_body(body.as_bytes().to_vec())
    }

    fn replay_document(result: &AxFormResult) -> String {
        let document = crate::compose_compiled_page_document(&[], r#"
page Form() {
  return ASX {
    <>
      <form action="/__axonyx/action?name=Save&amp;path=%2Fposts" data-ax-retain-fields="title,summary,palette,enabled,secret,password,opaque,hidden,file">
        <input name="title" value="Initial" />
        <textarea name="summary">Initial summary</textarea>
        <select name="palette"><option value="silver" selected={true}>Silver</option><optgroup label="Metals"><option value="gold">Gold</option></optgroup></select>
        <input type="checkbox" name="enabled" checked={true} />
        <input name="unlisted" value="Initial unlisted" />
        <input name="secret" />
        <input type="password" name="password" />
        <input type="password" name="opaque" />
        <input type="hidden" name="hidden" value="Server owned" />
        <input type="file" name="file" />
        <span data-ax-field-error="title"></span>
      </form>
      <form action="/__axonyx/action?name=Other&amp;path=%2Fposts" data-ax-retain-fields="title"><input name="title" value="Other action" /></form>
      <form action="/__axonyx/action?name=Save&amp;path=%2Fother" data-ax-retain-fields="title"><input name="title" value="Other route" /></form>
    </>
  }
}
"#).unwrap();
        crate::render_compiled_page_document(
            &serde_json::to_string(&document)
                .unwrap()
                .replace("&amp;", "&"),
            &[],
            "/posts",
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some(result),
        )
        .unwrap()
    }

    #[test]
    fn native_replay_is_opted_in_scoped_escaped_and_private() {
        let result = AxFormResult::validation("Save", "/posts", &serde_json::json!({"title":"Required"})).unwrap()
            .with_request_values(&replay_request("title=%22%3E%3Cscript%3E&summary=%3C%2Ftextarea%3E%3Cscript%3E&palette=gold&unlisted=UNLISTED_VALUE&secret=SECRET_VALUE&password=PASSWORD_VALUE&opaque=OPAQUE_VALUE&hidden=HIDDEN_VALUE&file=FILE_VALUE"));
        let serialized = serde_json::to_string(&result).unwrap();
        let debug = format!("{result:?}");
        let html = replay_document(&result);
        assert!(html.contains("value=\"&quot;&gt;&lt;script&gt;\""));
        assert!(html.contains("&lt;/textarea&gt;&lt;script&gt;"));
        assert!(html.contains("value=\"gold\" selected=\"true\""));
        assert!(!html.contains("value=\"silver\" selected="));
        assert!(html.contains("name=\"enabled\" type=\"checkbox\"></input>"));
        assert!(html.contains("value=\"Other action\""));
        assert!(html.contains("value=\"Other route\""));
        assert!(html.contains("value=\"Initial unlisted\""));
        assert!(html.contains("value=\"Server owned\""));
        assert!(html.contains("aria-invalid=\"true\""));
        for value in [
            "UNLISTED_VALUE",
            "SECRET_VALUE",
            "PASSWORD_VALUE",
            "OPAQUE_VALUE",
            "HIDDEN_VALUE",
            "FILE_VALUE",
        ] {
            assert!(!html.contains(value), "must not replay {value}");
            assert!(!serialized.contains(value));
            assert!(!debug.contains(value));
        }
        assert!(!html.contains("<script>"));
        assert!(serialized.contains("Required"));
        assert!(!serialized.contains("summary"));
    }

    #[test]
    fn replay_capture_rejects_wrong_requests_ambiguity_and_oversize() {
        for request in [
            replay_request("title=one&title=two"),
            replay_request("title=one&t%69tle=two"),
            replay_request(&format!("title={}", "x".repeat(MAX_REPLAY_BODY_BYTES))),
            replay_request(&vec!["x=y"; 33].join("&")),
            crate::server::AxHttpRequest::new("POST", "/__axonyx/action?name=Other&path=%2Fposts")
                .with_header("content-type", "application/x-www-form-urlencoded")
                .with_body(b"title=wrong".to_vec()),
            crate::server::AxHttpRequest::new("POST", "/__axonyx/action?name=Save&path=%2Fother")
                .with_header("content-type", "application/x-www-form-urlencoded")
                .with_body(b"title=wrong".to_vec()),
            crate::server::AxHttpRequest::new("GET", "/__axonyx/action?name=Save&path=%2Fposts")
                .with_header("content-type", "application/x-www-form-urlencoded")
                .with_body(b"title=wrong".to_vec()),
            crate::server::AxHttpRequest::new("POST", "/__axonyx/action?name=Save&path=%2Fposts")
                .with_header("content-type", "application/json")
                .with_body(b"title=wrong".to_vec()),
        ] {
            let result = AxFormResult::validation("Save", "/posts", &serde_json::json!({}))
                .unwrap()
                .with_request_values(&request);
            assert!(result.submitted.is_none());
        }
    }

    #[test]
    fn replay_preserves_long_editor_text_within_request_budget() {
        let text = format!(
            "{}\n</textarea><script>unsafe</script>",
            "editor text ".repeat(1000)
        );
        let result = AxFormResult::validation("Save", "/posts", &serde_json::json!({}))
            .unwrap()
            .with_request_values(&replay_request(&format!("summary={text}&title=Retry")));
        assert_eq!(
            result.submitted.as_ref().unwrap().get("summary"),
            Some(&text)
        );
        let html = replay_document(&result);
        assert!(html.contains("&lt;/textarea&gt;&lt;script&gt;unsafe&lt;/script&gt;"));
        assert!(!html.contains("<script>unsafe</script>"));
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("editor text"));

        let boundary = format!("title={}", "x".repeat(MAX_REPLAY_BODY_BYTES - 6));
        let result = AxFormResult::validation("Save", "/posts", &serde_json::json!({}))
            .unwrap()
            .with_request_values(&replay_request(&boundary));
        assert!(result.submitted.is_some());
    }

    #[test]
    fn replay_checks_checkbox_values_and_excludes_transport_fields() {
        let result = AxFormResult::validation("Save", "/posts", &serde_json::json!({}))
            .unwrap()
            .with_request_values(&replay_request(
                "enabled=on&__ax_patch=true&csrf=private&token=private",
            ));
        assert_eq!(result.submitted.as_ref().unwrap().len(), 1);
        let html = replay_document(&result);
        assert!(html.contains("name=\"enabled\" type=\"checkbox\" checked=\"true\""));
        let wrong = AxFormResult::validation("Save", "/posts", &serde_json::json!({}))
            .unwrap()
            .with_request_values(&replay_request("enabled=unexpected"));
        assert!(replay_document(&wrong).contains("name=\"enabled\" type=\"checkbox\"></input>"));
    }

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
