# Scoped Request Body Limits V0 - Runtime Foundation

This is the first runtime slice of framework issue
[360](https://github.com/vladanPro/axonyx-framework/issues/360), not a completed
compiler/configuration feature or a published release. No Legura upload limit
or operator installation is changed.

## Trusted Request-Head Policy

With the `axum` feature, compiled hosts can use:

- `AxBodyLimitResolver`: shared `Fn(method, target) -> Option<usize>`.
- `axum_router_from_compiled_handler_with_body_limits`.
- `serve_compiled_axum_with_body_limits`.

The host provides the default limit and a trusted resolver. `None` keeps the
default; `Some(limit)` selects that request's budget. The target is the URI
path-and-query, not a filesystem path or an already validated action identity.
The resolver runs before reading the body, on the async worker, so it must be
cheap, nonblocking and deterministic. Do not read a DB, do file I/O or perform
authentication in this callback. It is not a plugin sandbox.

The forthcoming compiler integration must resolve known route/action identities
canonically before granting overrides. Substring matching, MIME declarations or
client-supplied budget headers are not an allowlist. The runtime tests use an
exact-match fixture only; they do not claim production action-selector validation.
Authentication, current-user authorization, origin and CSRF checks still belong
in the normal request/action pipeline before mutation or image processing.

## Bounded Collection And Errors

An oversized declared Content-Length is rejected before polling the body. The
actual entire body is then collected with the selected cap, independently of
missing or misleading length metadata, before normal multipart conversion.
This also bounds epilogue bytes arriving after the final multipart boundary.
The new API returns generic `413 Payload Too Large` for typed size-limit errors
and generic `400 Bad Request` for malformed bodies, without executing the
application handler or reflecting internal parser/transport diagnostics.

This is bounded-buffered upload handling, not streaming files to storage.
Multipart conversion can additionally allocate field/file buffers; the cap is
not a process RSS limit. Connection limits, timeouts, codec allocation budgets
and nonblocking media-processing admission remain separate protections.
Synchronous compiled handlers continue running through `spawn_blocking`.

Existing `serve_compiled_axum` and router constructors preserve their previous
single-limit behavior and error mapping. In particular, this slice does not
silently change their historic 400 size-error responses to 413. Other transports
and the route-table adapter are not switched to the new policy API.

## Verification And Remaining Gates

Tests cover default/selected boundaries, method and exact-target selection,
unknown/ambiguous/malformed fixture selectors retaining the default, ignored
client budget headers, streamed overflow without a length, dishonest lengths,
pre-poll length refusal, multipart overflows/epilogues, private error responses,
handler nonexecution and the legacy response behavior. An actual loopback HTTP
server additionally checks chunked multipart requests through the public router.

Remaining: compiler-owned route/action resolution, validated configuration and
Check/Doctor reports, generated entry wiring, preview/std support or explicit
refusal, integrated 64 KiB/default versus 9 MiB/upload acceptance, release and
registry consumer adoption. Do not enable CMS media by raising the global limit.
