# Private Native Binary Responses V0 - Runtime Foundation

This is an unpublished Rust runtime API, not backend `.ax` syntax or a complete
compiled application feature. No compiler lowering, CLI, reports, API contracts
or Legura image URLs are changed here. Framework issue 366 tracks those gates.

## Separate From JSON Callbacks

`AxNativeRegistry::register_private_response` registers a trusted application
callback with typed JSON input and a separately bounded binary result. Existing
`register` / `Native.call` functions retain their JSON output and shape budgets.
Both registration kinds share duplicate-name protection, but cannot dispatch
through each other's channels.

The callback receives `&AxNativeResponseContext` and its deserialized input.
Context exposes only `subject()`, constructed inside the runtime after real
session verification. Its constructor/fields and registry dispatch are not public
application APIs. A payload claiming `subject` or a role is not authentication;
applications should deny unknown input fields and recheck current product roles.
The JSON callback registry is not enlarged to accommodate files.

`AxRuntimeEnvAccess::runtime_native_private_response(request, name, input)` is
unavailable by default for custom runtimes. AxDatabaseRuntime implements it for
GET requests only: it loads the actual session on every call, refuses missing
identity, then invokes the registered callback. Logout/revoked session-store
entries therefore deny subsequent requests before callback execution. Product
role revocation remains the callback's responsibility. HEAD/POST return 405 with
Allow: GET; there is no mutation or implicit session creation here.

## Output Policy

`AxNativeBinaryResponse` is not serializable. Its constructor accepts nonempty
bytes and a closed `AxNativeBinaryType` of PNG, JPEG or octet-stream. No arbitrary
HTML/SVG MIME or callback-provided headers/cookies are accepted by this channel.
The application validates image contents; this transport does not decode them.

Registration selects a positive per-callback cap, at most 8 MiB in V0. A result
over that cap becomes a safe failure. The JSON input limits still apply before
the callback, while the JSON output cap is unrelated to the binary result.
These are result/buffering bounds after trusted application allocation, not a
callback sandbox, streaming API or total/concurrent process-memory guarantee.

Successful responses use exact bytes, trusted MIME, private/no-store, nosniff,
same-origin resource policy and inline image/attachment binary disposition.
Callbacks can return only Forbidden, NotFound or Unavailable failures, mapped to
safe 403/404/500 text with the same privacy/security headers. Unsupported runtime,
session configuration, unknown registration and input contract failures remain
runtime errors: the future route boundary must map them to redacted private
errors, not echo diagnostics or inputs. JSON request/cookie behavior is unchanged.

## Evidence And Remaining Gates

Five local tests pass for this API: verified session versus forged input/cookie,
logout denial, typed/size input checks, independent output budgets, safe errors,
GET-only methods, registration collisions and channel separation. An actual
loopback Axum server verifies exact non-UTF8 bytes and private headers for an
authenticated request, and safe denial for an anonymous request. Session logout
is rechecked by the same runtime afterward. Existing eight JSON callback tests
also pass. This is not yet an `.ax` compiled-route or Legura browser proof.

The full local workspace all-features test suite and Rust 1.82 all-targets check
pass. Runtime-library strict Clippy passes on the current toolchain with
--no-deps; unrestricted current-toolchain Clippy encounters two existing core
diagnostics in ax_formatter/ax_types, not modified by this API. The pinned CI
toolchain's strict full-workspace/all-targets Clippy also passes locally on
Rust 1.82. Remote required gates remain authoritative before merge.

Next: compiler-owned return plan and diagnostics, binary API response contracts
and reports that work with response validation always mode, a compiled smoke
application, then coordinated package publication and Legura source/registry
adoption. Do not edit generated servers, place private files in public/dist or
serialize image bytes through Native.call as an interim endpoint.
