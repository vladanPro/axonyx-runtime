//! Browser mutation boundary. This is not authentication or a CSRF token API.

use crate::server::AxHttpRequest;

/// Reject unsafe browser requests before invoking application code.
/// The transport must preserve the public Host; forwarded headers are not trusted.
pub fn rejects_mutation_request(request: &AxHttpRequest) -> bool {
    if matches!(
        request.method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    ) {
        return false;
    }
    let site = request.header_value("Sec-Fetch-Site");
    if site.is_some_and(|value| !value.eq_ignore_ascii_case("same-origin")) {
        return true;
    }
    if let Some(origin) = request
        .header_value("Origin")
        .or_else(|| request.header_value("Referer"))
    {
        return !matches_host(origin, request.header_value("Host"));
    }
    if site.is_some() {
        return false;
    }
    // Cookie credentials must not turn a metadata-free request into a trusted CLI.
    request.header_value("Cookie").is_some()
}

fn matches_host(origin: &str, host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let Some((scheme, remainder)) = origin.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
        return false;
    }
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '@' | '\\' | ','))
    {
        return false;
    }
    authority.eq_ignore_ascii_case(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cross_site_same_site_and_forwarded_host_spoofing() {
        let request = AxHttpRequest::new("POST", "/api/login").with_header("Host", "axonyx.dev");
        for origin in [
            "https://attacker.example",
            "https://axonyx.dev.attacker.example",
            "null",
            "https://user@axonyx.dev",
            "https://axonyx.dev\\attacker.example",
        ] {
            assert!(rejects_mutation_request(
                &request
                    .clone()
                    .with_header("Origin", origin)
                    .with_header("X-Forwarded-Host", "attacker.example")
            ));
        }
        for site in ["cross-site", "same-site", "none", "unknown"] {
            assert!(rejects_mutation_request(
                &request.clone().with_header("Sec-Fetch-Site", site)
            ));
        }
    }

    #[test]
    fn cookie_mutations_need_origin_or_same_origin_metadata() {
        let request = AxHttpRequest::new("POST", "/api/logout")
            .with_header("Host", "axonyx.dev")
            .with_header("Cookie", "session=secret");
        assert!(rejects_mutation_request(&request));
        assert!(!rejects_mutation_request(
            &request.clone().with_header("Origin", "https://axonyx.dev")
        ));
        assert!(!rejects_mutation_request(&request.clone().with_header(
            "Referer",
            "https://axonyx.dev/account?tab=profile"
        )));
        assert!(!rejects_mutation_request(
            &request.clone().with_header("Sec-Fetch-Site", "same-origin")
        ));
        assert!(rejects_mutation_request(
            &request
                .with_header("Sec-Fetch-Site", "same-origin")
                .with_header("Origin", "null")
        ));
    }

    #[test]
    fn preserves_reads_cli_and_explicit_ports() {
        for method in ["GET", "HEAD", "OPTIONS"] {
            assert!(!rejects_mutation_request(
                &AxHttpRequest::new(method, "/api/posts")
                    .with_header("Sec-Fetch-Site", "cross-site")
            ));
        }
        assert!(!rejects_mutation_request(&AxHttpRequest::new(
            "POST",
            "/api/posts"
        )));
        let request =
            AxHttpRequest::new("PATCH", "/api/posts").with_header("Host", "localhost:3000");
        assert!(!rejects_mutation_request(
            &request
                .clone()
                .with_header("Origin", "http://localhost:3000")
        ));
        assert!(rejects_mutation_request(
            &request.with_header("Origin", "http://localhost:3001")
        ));
    }
}
