use axonyx_core::ax_backend_codegen::{
    compile_backend_ax_to_module, AxBackendCodegenError, AxBackendCompileError,
};
use axonyx_core::ax_backend_lowering::{
    ax_step_uses_auth_subject, lower_backend_document, AxReturnPlan, AxStepPlan,
};
use axonyx_core::ax_backend_parser::parse_backend_ax;

#[test]
fn public_binary_uses_its_own_channel_without_implicit_session_discovery() {
    let source = "route GET \"/posts/:slug/image\" -> Binary {\n  return Native.publicResponse(\"post.image\", { slug: params.slug })\n}";
    let plan = lower_backend_document(&parse_backend_ax(source).unwrap()).unwrap();
    assert!(matches!(
        &plan.handlers[0].steps[0],
        AxStepPlan::Return(AxReturnPlan::PublicResponse { .. })
    ));
    assert!(!plan.handlers[0].steps.iter().any(ax_step_uses_auth_subject));
    let module = compile_backend_ax_to_module(source).unwrap();
    assert!(module.contains("runtime.runtime_native_public_response(request, \"post.image\""));
    assert!(!module.contains("runtime.runtime_native_private_response(request"));
    assert!(module.contains("finalize_private_binary_response(response)"));
    assert!(module.contains("validate_private_binary_response(&response)"));
    syn::parse_file(&module).unwrap();
}

#[test]
fn public_payload_auth_references_remain_explicit_and_strings_are_not_code() {
    for (payload, auth) in [
        ("{ subject: Auth.subject }", true),
        ("{ text: \"Auth.subject Native.publicResponse\" }", false),
    ] {
        let source = format!("route GET \"/image\" -> Binary {{\n  return Native.publicResponse(\"post.image\", {payload})\n}}");
        let plan = lower_backend_document(&parse_backend_ax(&source).unwrap()).unwrap();
        assert_eq!(
            plan.handlers[0].steps.iter().any(ax_step_uses_auth_subject),
            auth
        );
        syn::parse_file(&compile_backend_ax_to_module(&source).unwrap()).unwrap();
    }
}

#[test]
fn public_binary_rejects_illegal_locations_metadata_and_nested_calls() {
    for source in [
        "action A() {\n  return Native.publicResponse(\"post.image\", null)\n}",
        "query loadA() -> String {\n  return Native.publicResponse(\"post.image\", null)\n}",
        "fn helper() -> String {\n  return Native.publicResponse(\"post.image\", null)\n}",
        "route GET \"/a\" {\n  return Native.publicResponse(\"post.image\", null)\n}",
        "route POST \"/a\" -> Binary {\n  return Native.publicResponse(\"post.image\", null)\n}",
        "route GET \"/a\" -> Binary {\n  return Native.publicResponse(query.name, null)\n}",
        "route GET \"/a\" -> Binary {\n  return Native.publicResponse(\"post.image\")\n}",
        "route GET \"/a\" -> Binary {\n  header \"Cache-Control\" = \"public\"\n  return Native.publicResponse(\"post.image\", null)\n}",
        "route GET \"/a\" -> Binary {\n  require true else redirect(\"/login\")\n  return Native.publicResponse(\"post.image\", null)\n}",
        "route GET \"/a\" -> Binary {\n  require true else Native.publicResponse(\"post.image\", null)\n  return Native.publicResponse(\"post.image\", null)\n}",
        "route GET \"/a\" {\n  data value = Native.publicResponse(\"post.image\", null)\n  return json(value)\n}",
        "route GET \"/a\" {\n  return json({ image: Native.publicResponse(\"post.image\", null) })\n}",
        "route GET \"/a\" -> Binary {\n  return Native.publicResponse(\"post.image\", { nested: Native.privateResponse(\"private.image\", null) })\n}",
    ] {
        let result = compile_backend_ax_to_module(source);
        assert!(matches!(result, Err(AxBackendCompileError::Codegen(AxBackendCodegenError::InvalidPrivateResponse))), "{source}: {result:?}");
    }
}
