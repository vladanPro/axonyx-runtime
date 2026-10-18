use axonyx_core::ax_backend_codegen::{
    compile_backend_ax_to_module, AxBackendCodegenError, AxBackendCompileError,
};

#[test]
fn binary_route_is_explicit_private_and_keeps_declared_validation() {
    let source = r#"route GET "/private/:id" -> Binary {
  require Auth.subject else forbidden()
  return Native.privateResponse("media.read", {id: params.id})
}"#;
    let module = compile_backend_ax_to_module(source).unwrap();
    assert!(module.contains("runtime.runtime_native_private_response(request, \"media.read\""));
    assert!(module.contains("finalize_private_binary_response(response)"));
    assert!(module.contains("if validate_responses"));
    assert!(module.contains("validate_private_binary_response(&response)"));
    assert!(!module.contains("chunks_iter().flat_map"));
    syn::parse_file(&module).unwrap();
}

#[test]
fn illegal_binary_locations_and_metadata_fail_before_rust_generation() {
    for source in [
        "action A() {\n  return Native.privateResponse(\"media.read\", null)\n}",
        "query loadA() -> String {\n  return Native.privateResponse(\"media.read\", null)\n}",
        "fn helper() -> String {\n  return Native.privateResponse(\"media.read\", null)\n}",
        "route GET \"/a\" {\n  return Native.privateResponse(\"media.read\", null)\n}",
        "route POST \"/a\" -> Binary {\n  return Native.privateResponse(\"media.read\", null)\n}",
        "route GET \"/a\" -> Binary {\n  return json({ok: true})\n}",
        "route GET \"/a\" -> Binary {\n  return Native.privateResponse(query.name, null)\n}",
        "route GET \"/a\" -> Binary {\n  return Native.privateResponse(\"media.read\")\n}",
        "route GET \"/a\" -> Binary {\n  header \"Cache-Control\" = \"public\"\n  return Native.privateResponse(\"media.read\", null)\n}",
        "route GET \"/a\" -> Binary {\n  require true else redirect(\"/login\")\n  return Native.privateResponse(\"media.read\", null)\n}",
        "route GET \"/a\" -> Binary {\n  require true else Native.privateResponse(\"media.read\", null)\n  return Native.privateResponse(\"media.read\", null)\n}",
        "route GET \"/a\" {\n  data result = Native.privateResponse(\"media.read\", null)\n  return json(result)\n}",
    ] {
        let result = compile_backend_ax_to_module(source);
        assert!(
            matches!(
                result,
                Err(AxBackendCompileError::Codegen(
                    AxBackendCodegenError::InvalidPrivateResponse
                ))
            ),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn native_payloads_keep_empty_objects_and_arrays_including_nested_values() {
    for payload in ["{}", "{nested: {}, items: []}", "[]", "[{}, []]"] {
        let source = format!(
            "route GET \"/a\" -> Binary {{\n  return Native.privateResponse(\"media.read\", {payload})\n}}"
        );
        let module = compile_backend_ax_to_module(&source).unwrap();
        syn::parse_file(&module).unwrap();
        let source = format!(
            "route GET \"/a\" {{\n  data result = Native.call(\"media.read\", {payload})\n  return json(result)\n}}"
        );
        syn::parse_file(&compile_backend_ax_to_module(&source).unwrap()).unwrap();
    }
}
