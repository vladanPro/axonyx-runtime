# Private Binary Routes V0 - Unpublished Compiler Contract

This extends the runtime foundation in `private-native-responses-v0.md`.
It is source-only work, not a published Cargo/CLI capability or a live Legura
image endpoint. Framework issue 366 tracks the remaining integration.

## Authoring

```ax
route GET "/private/:id" -> Binary {
  require Auth.subject else forbidden()
  return Native.privateResponse("media.read", { id: params.id })
}
```

`Binary` is a reserved HTTP response marker, not a general data type or a
user-defined record/union. The final direct return must name a statically
registered private callback and supply exactly one JSON payload. The runtime
rechecks the real session even without an explicit subject guard; lowering also
marks this return as requiring authenticated-session configuration.

V0 allows only GET routes, preceding data bindings and guards whose failure is
`forbidden()` or `notFound()`. It rejects action/query/Domain returns, response
headers/cookies, writes, hooks, redirects, earlier returns, data-binding use of
the private response and non-data global statements. This is deliberately not
a general streaming or arbitrary-response escape hatch. Trusted callbacks still
own product authorization and image-content validation.

## Response Boundary

Generated Rust uses the private runtime channel, not `Native.call` JSON output.
Successful replies retain exact non-UTF8 bytes and the closed PNG/JPEG/binary MIME
policy. Every response is finalized regardless of API validation mode: private
no-store/nosniff policy, no session cookies, safe 403/404 or redacted 500 failures.
Enabled validation inspects the response by reference without flattening/copying
its body. JSON routes retain their existing JSON response validation.

Preview explicitly refuses private Binary routes before interpreting their body;
they require the compiled server. Empty object/array native payload generation
is also fixed here, including nested JSON callback input (framework issue 365).

## Evidence And Release Gate

Core tests cover valid generated Rust syntax and rejected contracts/locations.
Runtime tests cover closed MIME, status/size/empty-body checks, private policy and
failure redaction. `scripts/smoke-native-calls.ps1` compiles and executes generated
Rust against the source runtime, including exact bytes with API validation both
enabled and disabled, empty payloads, unknown callbacks, preview refusal and
logout denial. The preceding foundation separately exercises real loopback Axum
HTTP. Neither proof replaces the future framework CLI host and Legura browser
acceptance.

New public `AxReturnPlan` and API-validation-error enum variants are breaking for
exhaustive downstream matches. Do NOT publish these as compatible 0.6.x patches.
Coordinate core/runtime/framework adoption and a 0.7 release, including all CLI
matches, reports/OpenAPI/contracts, session discovery, generated hosts and registry
consumer checks. Package versions intentionally remain unchanged in this PR.
Existing published CLI installations must not resolve these source changes via
their old compatible dependency ranges.

Legura continues to display placeholders until that integration, publication and
isolated end-to-end acceptance are complete. Private storage stays outside public
and dist; no base64 JSON workaround is introduced.
