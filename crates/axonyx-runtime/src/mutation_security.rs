//! Browser mutation boundary. This is not authentication or a CSRF token API.

use crate::server::AxHttpRequest;

/// Reject unsafe browser requests before invoking application code.
/// The transport must preserve the public Host; forwarded headers are not trusted.
pub fn rejects_mutation_request(request: &AxHttpRequest) -> bool {
    rejects_mutation_request_with_origin(request, None)
}

/// When configured, source proof must match the canonical scheme, host and port.
pub fn rejects_mutation_request_with_origin(
    request: &AxHttpRequest,
    public_origin: Option<&str>,
) -> bool {
    if matches!(
        request.method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    ) {
        return false;
    }
    if public_origin.is_some_and(|origin| !valid_public_origin(origin)) {
        return true;
    }
    let site = request.header_value("Sec-Fetch-Site");
    if site.is_some_and(|value| !value.eq_ignore_ascii_case("same-origin")) {
        return true;
    }
    if let Some(origin) = request
        .header_value("Origin")
        .or_else(|| request.header_value("Referer"))
    {
        return match public_origin {
            Some(expected) => match (parse_origin(origin), parse_origin(expected)) {
                (Some(actual), Some(expected)) => actual != expected,
                _ => true,
            },
            None => !matches_host(origin, request.header_value("Host")),
        };
    }
    if site.is_some() {
        return public_origin.is_some();
    }
    // Cookie credentials must not turn a metadata-free request into a trusted CLI.
    request.header_value("Cookie").is_some()
}

/// Configuration accepts an origin only, not a URL with path, credentials or query.
pub fn valid_public_origin(value: &str) -> bool {
    parse_origin(value).is_some() && !value.split_once("://").unwrap().1.contains(['/', '?', '#'])
}

fn parse_origin(value: &str) -> Option<(String, String, u16)> {
    let (scheme, rest) = value.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '@' | '\\' | ','))
    {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let (host, suffix) = ipv6.split_once(']')?;
        let host = host.parse::<std::net::Ipv6Addr>().ok()?.to_string();
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix.strip_prefix(':')?.parse::<u16>().ok()?
        };
        (host, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok()?),
            None => (authority, default_port),
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-'))
        {
            return None;
        }
        (host.to_ascii_lowercase(), port)
    };
    if port == 0 {
        return None;
    }
    Some((scheme, host, port))
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
    fn canonical_origin_checks_scheme_and_port_without_trusting_host() {
        let request = AxHttpRequest::new("POST", "/api/login").with_header("Host", "internal:3000");
        for origin in ["https://axonyx.dev", "https://AXONYX.dev:443"] {
            assert!(!rejects_mutation_request_with_origin(
                &request.clone().with_header("Origin", origin),
                Some("https://axonyx.dev")
            ));
        }
        for origin in [
            "http://axonyx.dev",
            "https://axonyx.dev:444",
            "https://attacker.example",
            "null",
        ] {
            assert!(rejects_mutation_request_with_origin(
                &request.clone().with_header("Origin", origin),
                Some("https://axonyx.dev")
            ));
        }
        assert!(rejects_mutation_request_with_origin(
            &request.with_header("Sec-Fetch-Site", "same-origin"),
            Some("https://axonyx.dev")
        ));
        for invalid in [
            "https://axonyx.dev/",
            "https://user@axonyx.dev",
            "https://axonyx.dev?x=1",
            "https://axonyx.dev:0",
            "ftp://axonyx.dev",
            "https://axonyx.dev:abc",
        ] {
            assert!(!valid_public_origin(invalid));
        }
        assert!(valid_public_origin("http://[::1]:3000"));
        assert!(rejects_mutation_request_with_origin(
            &AxHttpRequest::new("POST", "/api/login"),
            Some("invalid")
        ));
    }

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
