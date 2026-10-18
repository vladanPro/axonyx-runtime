use axonyx_core::ax_types::AxDataContext;
use axonyx_runtime::server::AxHttpResponse;
use axonyx_runtime::{
    finalize_private_binary_response, validate_api_response_bytes,
    validate_private_binary_response, AxApiResponseValidationError,
};

#[test]
fn binary_validation_is_separate_from_json_and_requires_a_nonempty_bounded_200() {
    let context = AxDataContext::default();
    for mime in ["image/png", "image/jpeg", "application/octet-stream"] {
        assert!(validate_api_response_bytes(200, mime, &[0, 255, 128], "Binary", &context).is_ok());
    }
    for (status, mime, body) in [
        (204, "image/png", &[][..]),
        (201, "image/png", &[1][..]),
        (200, "application/json", &b"{}"[..]),
        (200, "image/svg+xml", &b"svg"[..]),
        (200, "image/png", &[][..]),
    ] {
        assert_eq!(
            validate_api_response_bytes(status, mime, body, "Binary", &context),
            Err(AxApiResponseValidationError::InvalidBinaryResponse)
        );
    }
    assert!(validate_api_response_bytes(200, "image/png", &[1], "String", &context).is_err());
    assert!(
        validate_api_response_bytes(200, "application/json", b"\"value\"", "String", &context)
            .is_ok()
    );
    assert_eq!(
        validate_api_response_bytes(
            200,
            "image/png",
            &vec![1; 8 * 1024 * 1024 + 1],
            "Binary",
            &context
        ),
        Err(AxApiResponseValidationError::InvalidBinaryResponse)
    );
}

#[test]
fn binary_route_boundary_redacts_failures_and_preserves_private_policy() {
    for (status, expected, body) in [
        (403, 403, "forbidden"),
        (404, 404, "not found"),
        (401, 500, "binary response unavailable"),
        (500, 500, "binary response unavailable"),
        (302, 500, "binary response unavailable"),
    ] {
        let response = finalize_private_binary_response(
            AxHttpResponse::text(status, "private database/root/session detail")
                .with_header("Cache-Control", "public")
                .with_header("Location", "https://private.invalid"),
        );
        assert_eq!(response.status, expected);
        assert_eq!(
            response.header_value("Cache-Control"),
            Some("private, no-store")
        );
        assert_eq!(
            response.header_value("X-Content-Type-Options"),
            Some("nosniff")
        );
        assert_eq!(response.header_value("Location"), None);
        assert_eq!(response.body.into_bytes(), body.as_bytes());
    }
    let response = finalize_private_binary_response(
        AxHttpResponse::json(200, &serde_json::json!({"secret":"not binary"})).unwrap(),
    );
    assert_eq!(response.status, 500);
    assert_eq!(response.body.into_bytes(), b"binary response unavailable");
}

#[test]
fn borrowed_binary_validation_requires_the_private_response_policy() {
    let response = AxHttpResponse::bytes(200, "image/png", vec![0, 255, 128]);
    assert_eq!(
        validate_private_binary_response(&response),
        Err(AxApiResponseValidationError::InvalidBinaryResponse)
    );
    let response = finalize_private_binary_response(response);
    assert_eq!(response.status, 200);
    assert!(validate_private_binary_response(&response).is_ok());
    assert_eq!(response.body.into_bytes(), [0, 255, 128]);

    let response = finalize_private_binary_response(AxHttpResponse::bytes(
        200,
        "image/png",
        vec![1; 8 * 1024 * 1024 + 1],
    ));
    assert_eq!(response.status, 500);
    assert_eq!(response.body.into_bytes(), b"binary response unavailable");
}
