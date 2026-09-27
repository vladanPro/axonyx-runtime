//! Registration predicates; these do not prove ownership or normalize input.

/// Static fallback for browser form submissions. Never echoes submitted values.
pub fn html_error_response(
    fields: &serde_json::Value,
    route: &str,
) -> crate::server::AxHttpResponse {
    fn escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }
    let route = if route.starts_with('/')
        && !route.starts_with("//")
        && !route.contains(['\\', '%'])
        && !route.chars().any(char::is_control)
    {
        route
    } else {
        "/"
    };
    let mut items = String::new();
    if let Some(fields) = fields.as_object() {
        for (field, message) in fields.iter().take(32) {
            if let Some(message) = message.as_str() {
                let field: String = field.chars().take(128).collect();
                let message: String = message.chars().take(512).collect();
                items.push_str(&format!(
                    "<li><strong>{}</strong>: {}</li>",
                    escape(&field),
                    escape(&message)
                ));
            }
        }
    }
    crate::server::AxHttpResponse::html(422, format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Check your input</title></head><body><main><h1>Check your input</h1><p>The form could not be submitted. Correct the fields and try again.</p><ul>{items}</ul><a href=\"{}\">Return to the form</a></main></body></html>", escape(route)))
        .with_no_store()
        .with_header("Content-Security-Policy", "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'")
        .with_header("X-Content-Type-Options", "nosniff")
        .with_header("Referrer-Policy", "no-referrer")
}

pub fn wants_html_error(request: &crate::server::AxHttpRequest) -> bool {
    let accept = request.header_value("Accept").unwrap_or_default();
    if accept.contains("application/ax-") || accept.contains("application/json") {
        return false;
    }
    accept.split(',').any(|entry| {
        let mut parts = entry.trim().split(';');
        parts
            .next()
            .is_some_and(|media| media.trim().eq_ignore_ascii_case("text/html"))
            && parts.all(|part| {
                let Some((name, value)) = part.trim().split_once('=') else {
                    return true;
                };
                !name.trim().eq_ignore_ascii_case("q")
                    || value
                        .trim()
                        .parse::<f32>()
                        .is_ok_and(|q| q > 0.0 && q <= 1.0)
            })
    })
}

pub fn email(value: &str) -> bool {
    if !value.is_ascii() || value.len() > 254 {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&b))
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// Baseline for password-only registration. Preserve Unicode and whitespace.
pub fn password(value: &str) -> bool {
    value.len() <= crate::password::MAX_PASSWORD_BYTES && value.chars().count() >= 15
}

#[cfg(test)]
mod tests {
    #[test]
    fn html_fallback_escapes_messages_and_rejects_external_links() {
        let response = super::html_error_response(
            &serde_json::json!({"email": "<script>alert(1)</script>"}),
            "//evil.example",
        );
        assert_eq!(response.status, 422);
        let body = String::from_utf8(response.body.clone().into_bytes()).unwrap();
        assert!(!body.contains("<script>"));
        assert!(body.contains("&lt;script&gt;"));
        assert!(body.contains("href=\"/\""));
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));
    }

    #[test]
    fn html_negotiation_preserves_json_and_bridge_responses() {
        for (accept, expected) in [
            ("text/html", true),
            ("text/html;q=0", false),
            ("application/json,text/html", false),
            ("application/ax-patch+json", false),
        ] {
            let request = crate::server::AxHttpRequest::new("POST", "/__axonyx/action")
                .with_header("Accept", accept);
            assert_eq!(super::wants_html_error(&request), expected);
        }
    }
    #[test]
    fn registration_policy_is_bounded_and_does_not_trim() {
        assert!(super::email("first+tag@example.com"));
        for value in [
            "",
            "a@",
            "a@b",
            "a..b@example.com",
            "a@example.com@evil.com",
            " a@example.com",
            "a@-example.com",
        ] {
            assert!(!super::email(value), "{value}");
        }
        assert!(!super::password("short"));
        assert!(super::password("long pass phrase here"));
        assert!(super::password(&"界".repeat(15)));
        assert!(!super::password(&"a".repeat(1025)));
    }
}
