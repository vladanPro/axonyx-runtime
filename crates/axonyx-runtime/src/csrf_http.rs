//! Session proof delivery and mutation validation; callers also enforce origin policy.
use crate::backend::{AxBackendRuntime, AxRuntimeResult};
use crate::server::{AxHttpRequest, AxHttpResponse};

pub const CSRF_PATH: &str = "/__axonyx/csrf";
pub const CSRF_HEADER: &str = "X-Axonyx-CSRF";
pub const CSRF_FIELD: &str = "__ax_csrf";
pub const FORM_MARKER: &str = "<!--axonyx:csrf-field-->";
const ANONYMOUS_TTL: u64 = 1800;

fn now() -> AxRuntimeResult<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .map_err(|_| crate::backend::AxRuntimeError::message("CSRF clock unavailable"))
}

fn local_request(request: &AxHttpRequest) -> bool {
    request.header_value("Host").is_some_and(|host| {
        host == "localhost"
            || host.starts_with("localhost:")
            || host == "127.0.0.1"
            || host.starts_with("127.0.0.1:")
            || host == "[::1]"
            || host.starts_with("[::1]:")
    })
}

fn anonymous_name(request: &AxHttpRequest) -> &'static str {
    if local_request(request) {
        "axonyx_csrf"
    } else {
        "__Host-axonyx-csrf"
    }
}

fn anonymous_identity(
    request: &AxHttpRequest,
    secret: &str,
    time: u64,
) -> AxRuntimeResult<Option<String>> {
    let Some(cookie) = request.cookie_value(anonymous_name(request)) else {
        return Ok(None);
    };
    let Some((identity, signature)) = cookie.split_once(".axcsrf1.") else {
        return Ok(None);
    };
    let Some(rest) = identity.strip_prefix("anonymous:") else {
        return Ok(None);
    };
    let Some((timestamp, nonce)) = rest.split_once(':') else {
        return Ok(None);
    };
    let Ok(issued) = timestamp.parse::<u64>() else {
        return Ok(None);
    };
    if issued > time || time - issued >= ANONYMOUS_TTL || uuid::Uuid::parse_str(nonce).is_err() {
        return Ok(None);
    }
    let token = format!("axcsrf1.{signature}");
    Ok(crate::session::csrf::verify(identity, &token, secret)?.then(|| identity.to_string()))
}

fn delivery(
    runtime: &impl AxBackendRuntime,
    request: &AxHttpRequest,
) -> AxRuntimeResult<(String, Option<crate::server::AxCookie>)> {
    let secret = runtime.env().secret("session_key")?;
    if let Some(session) = runtime.load_session(request)? {
        return Ok((crate::session::csrf::issue(&session.id, &secret)?, None));
    }
    let time = now()?;
    if let Some(identity) = anonymous_identity(request, &secret, time)? {
        return Ok((crate::session::csrf::issue(&identity, &secret)?, None));
    }
    let identity = format!("anonymous:{time}:{}", uuid::Uuid::new_v4());
    let token = crate::session::csrf::issue(&identity, &secret)?;
    let mut cookie =
        crate::server::AxCookie::new(anonymous_name(request), format!("{identity}.{token}"))
            .with_path("/")
            .http_only()
            .same_site("Strict")
            .with_max_age(ANONYMOUS_TTL as i64);
    if !local_request(request) {
        cookie = cookie.secure();
    }
    Ok((token, Some(cookie)))
}

/// Personalize only framework-rendered local action forms, never the build artifact.
pub fn protect_form_response(
    runtime: &impl AxBackendRuntime,
    request: &AxHttpRequest,
    mut response: AxHttpResponse,
) -> AxRuntimeResult<AxHttpResponse> {
    if request.method != "GET" || !response.content_type.starts_with("text/html") {
        return Ok(response);
    }
    let bytes = response.body.clone().into_bytes();
    let Ok(html) = std::str::from_utf8(&bytes) else {
        return Ok(response);
    };
    if !html.contains(FORM_MARKER) {
        return Ok(response);
    }
    let (token, cookie) = delivery(runtime, request)?;
    let field = format!("<input type=\"hidden\" name=\"{CSRF_FIELD}\" value=\"{token}\">");
    response.body = crate::server::AxBody::fixed(html.replace(FORM_MARKER, &field).into_bytes());
    response.headers.retain(|name, _| {
        !matches!(
            name.to_ascii_lowercase().as_str(),
            "etag" | "content-length" | "last-modified"
        )
    });
    response = response.with_no_store().with_header("Vary", "Cookie");
    if let Some(cookie) = cookie {
        response = response.with_cookie(cookie);
    }
    Ok(response)
}

/// Token delivery is same-origin only; metadata-free cookie CLI clients send Origin.
pub fn rejects_token_source(request: &AxHttpRequest, public_origin: Option<&str>) -> bool {
    if request.header_value("Origin").is_none()
        && request.header_value("Referer").is_none()
        && request
            .header_value("Sec-Fetch-Site")
            .is_some_and(|site| site.eq_ignore_ascii_case("same-origin"))
    {
        return false;
    }
    let mut probe = request.clone();
    probe.method = "POST".into();
    crate::mutation_security::rejects_mutation_request_with_origin(&probe, public_origin)
}

pub fn token_response(
    runtime: &impl AxBackendRuntime,
    request: &AxHttpRequest,
) -> AxRuntimeResult<AxHttpResponse> {
    let (token, cookie) = delivery(runtime, request)?;
    AxHttpResponse::json(200, &serde_json::json!({ "token": token }))
        .map(|response| {
            let response = response.with_no_store().with_header("Vary", "Cookie");
            if let Some(cookie) = cookie {
                response.with_cookie(cookie)
            } else {
                response
            }
        })
        .map_err(|_| {
            crate::backend::AxRuntimeError::message("CSRF response could not be generated")
        })
}

/// Return a generic rejection before invoking a mutation authenticated by a session.
/// Anonymous requests still require origin checking and application authorization.
pub fn reject_mutation(
    runtime: &impl AxBackendRuntime,
    request: &AxHttpRequest,
) -> AxRuntimeResult<Option<AxHttpResponse>> {
    if matches!(
        request.method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    ) {
        return Ok(None);
    }
    let session = runtime.load_session(request)?;
    // Metadata-free, cookie-free non-browser API clients keep their existing auth path.
    if session.is_none()
        && request.header_value("Cookie").is_none()
        && request.header_value("Origin").is_none()
        && request.header_value("Referer").is_none()
        && request.header_value("Sec-Fetch-Site").is_none()
    {
        return Ok(None);
    }
    let secret = runtime.env().secret("session_key")?;
    let identity = match session {
        Some(session) => Some(session.id),
        None => anonymous_identity(request, &secret, now()?)?,
    };
    let valid = match proof(request) {
        Some(token) if identity.is_some() => crate::session::csrf::verify(
            identity.as_deref().unwrap(),
            &token,
            &runtime.env().secret("session_key")?,
        )?,
        _ => false,
    };
    Ok(
        (!valid)
            .then(|| AxHttpResponse::text(403, "Forbidden: invalid CSRF proof").with_no_store()),
    )
}

fn proof(request: &AxHttpRequest) -> Option<String> {
    if let Some(value) = request.header_value(CSRF_HEADER) {
        return Some(value.to_string());
    }
    if request
        .header_value("Content-Type")
        .is_some_and(|value| value.starts_with("application/json"))
    {
        return request
            .json_field_value(CSRF_FIELD)
            .and_then(|value| value.as_str().map(str::to_string));
    }
    request.form_value(CSRF_FIELD)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn runtime() -> impl AxBackendRuntime {
        crate::backend::runtime_from_env(
            crate::backend::AxEnv::new()
                .with_secret("db_driver", "memory")
                .with_secret("session_key", "csrf-test-secret-at-least-32-bytes"),
        )
        .unwrap()
    }

    #[test]
    fn anonymous_form_proof_is_private_bound_and_expires() {
        let runtime = runtime();
        let request = AxHttpRequest::new("GET", "/login").with_header("Host", "example.test");
        let response = protect_form_response(
            &runtime,
            &request,
            AxHttpResponse::html(200, format!("<form>{FORM_MARKER}</form>")),
        )
        .unwrap();
        assert_eq!(response.header_value("Cache-Control"), Some("no-store"));
        let cookie = response.set_cookies[0].split(';').next().unwrap();
        assert!(cookie.starts_with("__Host-axonyx-csrf="));
        assert!(response.set_cookies[0].contains("Secure"));
        assert!(response.set_cookies[0].contains("HttpOnly"));
        let html = String::from_utf8(response.body.into_bytes()).unwrap();
        assert!(!html.contains(FORM_MARKER));
        let token = html
            .split("value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let post = AxHttpRequest::new("POST", "/api/login")
            .with_header("Host", "example.test")
            .with_header("Origin", "https://example.test")
            .with_header("Cookie", cookie);
        assert!(reject_mutation(&runtime, &post).unwrap().is_some());
        assert!(reject_mutation(
            &runtime,
            &post
                .clone()
                .with_body(format!("__ax_csrf={token}").into_bytes())
        )
        .unwrap()
        .is_none());
        assert!(reject_mutation(
            &runtime,
            &post.clone().with_header(CSRF_HEADER, "axcsrf1.bad")
        )
        .unwrap()
        .is_some());
        let identity =
            anonymous_identity(&post, "csrf-test-secret-at-least-32-bytes", now().unwrap())
                .unwrap()
                .unwrap();
        let issued = identity.split(':').nth(1).unwrap().parse::<u64>().unwrap();
        assert!(anonymous_identity(
            &post,
            "csrf-test-secret-at-least-32-bytes",
            issued + ANONYMOUS_TTL
        )
        .unwrap()
        .is_none());
        let other = token_response(&runtime, &request).unwrap();
        let other_cookie = other.set_cookies[0].split(';').next().unwrap();
        assert!(reject_mutation(
            &runtime,
            &AxHttpRequest::new("POST", "/api/login")
                .with_header("Host", "example.test")
                .with_header("Cookie", other_cookie)
                .with_header(CSRF_HEADER, token)
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn browser_login_requires_proof_but_cookie_free_cli_is_unchanged() {
        let runtime = runtime();
        let post = AxHttpRequest::new("POST", "/api/login");
        assert!(reject_mutation(&runtime, &post).unwrap().is_none());
        assert!(reject_mutation(
            &runtime,
            &post.with_header("Origin", "https://example.test")
        )
        .unwrap()
        .is_some());
        let response = protect_form_response(
            &runtime,
            &AxHttpRequest::new("GET", "/"),
            AxHttpResponse::html(200, "<p>Public</p>"),
        )
        .unwrap();
        assert!(response.set_cookies.is_empty());
    }
    #[test]
    fn proof_is_explicit_not_taken_from_url_or_cookie() {
        let request = AxHttpRequest::new("POST", "/save?__ax_csrf=unsafe")
            .with_header("Cookie", "__ax_csrf=unsafe");
        assert_eq!(proof(&request), None);
        assert_eq!(
            proof(&request.clone().with_body(b"__ax_csrf=form-proof".to_vec())),
            Some("form-proof".into())
        );
        assert_eq!(
            proof(
                &request
                    .with_header(CSRF_HEADER, "header-proof")
                    .with_body(b"__ax_csrf=form-proof".to_vec())
            ),
            Some("header-proof".into())
        );
        let request =
            AxHttpRequest::new("POST", "/save").with_header("Content-Type", "application/json");
        assert_eq!(
            proof(
                &request
                    .clone()
                    .with_body(br#"{"__ax_csrf":"json-proof"}"#.to_vec())
            ),
            Some("json-proof".into())
        );
        assert_eq!(
            proof(&request.with_body(br#"{"__ax_csrf":42}"#.to_vec())),
            None
        );
    }
}
