//! Session proof delivery and mutation validation; callers also enforce origin policy.
use crate::backend::{AxBackendRuntime, AxRuntimeResult};
use crate::server::{AxHttpRequest, AxHttpResponse};

pub const CSRF_PATH: &str = "/__axonyx/csrf";
pub const CSRF_HEADER: &str = "X-Axonyx-CSRF";
pub const CSRF_FIELD: &str = "__ax_csrf";

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
    let token = match runtime.load_session(request)? {
        Some(session) => Some(crate::session::csrf::issue(
            &session.id,
            &runtime.env().secret("session_key")?,
        )?),
        None => None,
    };
    AxHttpResponse::json(200, &serde_json::json!({ "token": token }))
        .map(|response| response.with_no_store().with_header("Vary", "Cookie"))
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
    ) || request.header_value("Cookie").is_none()
    {
        return Ok(None);
    }
    let Some(session) = runtime.load_session(request)? else {
        return Ok(None);
    };
    let valid = match proof(request) {
        Some(token) => crate::session::csrf::verify(
            &session.id,
            &token,
            &runtime.env().secret("session_key")?,
        )?,
        None => false,
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
