# Public Native Binary Responses V0 - Runtime Foundation

Unpublished Rust runtime API only. Backend `.ax` syntax, lowering, generated
handlers, preview routing, CLI reports and Legura image URLs are unchanged.
Framework issue 376 tracks the complete public-response integration.

## Explicit Registration

`AxNativeRegistry::register_public_response` takes typed JSON input and returns
an independently bounded `AxNativeBinaryResponse`. Names are unique across JSON,
private binary and public binary registrations. Each invocation channel can
dispatch only to its own callback kind; `Native.call` cannot serialize image
bytes and a public invocation cannot expose a private callback.

`AxRuntimeEnvAccess::runtime_native_public_response` admits only exact GET
requests without consulting session configuration or creating an identity.
No runtime with a default implementation is silently opted in. The callback
receives no subject or browser-supplied role: public admission is NOT product
authorization. Trusted application code must establish its current publication/
association grant before reading any private storage. This is not a sandbox.

For example, Legura's grant checks both the current published post slug and its
exact selected media ID in one database snapshot. Registering arbitrary object
lookup by ID would be an application vulnerability, not an intended feature.

## Bounds And Failures

Existing input byte, depth, node and typed-deserialization limits apply before
the callback. Binary results have an independent per-registration budget bounded
by the existing 8 MiB hard cap, not JSON's output byte budget. Empty/oversized
binary construction fails. Supported MIME and disposition remain PNG/JPEG inline
and octet-stream attachment; no arbitrary header or cookie output is accepted.

Successful responses and safe 403/404/500 failures use `private, no-store`,
`nosniff` and same-origin resource policy. Public accessibility deliberately does
not imply cacheability. Unknown names, input errors and missing registries return
a closed 500 without callback names, values or configuration details. Other HTTP
methods return 405/Allow: GET before invoking callbacks. Conditional request
headers do not bypass a fresh product callback and cannot synthesize a 304.

The shared bounded binary encoder and conservative headers are reused by both
channels. Private responses still require a verified runtime session; that
contract and its existing logout/anonymous-denial tests are not weakened.

## Acceptance Boundary

The runtime tests cover anonymous reads without session secrets, repeated product
grant checks, forged cookies, conditional headers, registration/channel isolation,
malformed and oversized input, output budgets, method denial and safe errors.
The optional Axum test uses an actual isolated loopback server to verify exact
non-UTF8 bytes and headers, then a 404 after revoking the fixture publication
grant. It is a Rust handler fixture, not `.ax` generated-route/preview parity or
Legura browser proof. These latter gates remain required before publication and
product integration.
