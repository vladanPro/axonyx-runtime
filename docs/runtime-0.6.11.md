# Runtime 0.6.11 - Scoped Request Body Limits

Release candidate; this document alone does not confirm crates.io publication.

## Scope

Adds opt-in compiled Axum host APIs with a trusted per-request body-limit
resolver. Legacy constructors retain their existing behavior. The default
budget remains the caller's choice; returning None from the resolver retains it.

The opt-in path bounds the complete body before conversion/dispatch, including
multipart epilogues. Size failures return safe no-store 413 responses and
malformed requests return safe 400 responses, without private parser diagnostics.
This is bounded buffering, not upload streaming or a total process-memory limit.
Application authorization, CSRF, file validation and decode concurrency remain
separate boundaries. See scoped-request-body-limits-v0.md for APIs and tests.

No AST/Domain changes, DB migrations, state ABI changes or automatic CMS Media
activation. Core stays 0.6.9, macros 0.1.0 and state WASM is unchanged.

## Gates And Consumers

Runtime/all-feature tests, strict Rust 1.82 Clippy, browser/native proofs and
Postgres CI must pass before promotion to main. Package verification must use
published core 0.6.9. Publish and create the runtime tag/release only after the
main gate. This candidate introduces no new global CLI installation.

Framework scoped-budget dispatch and its strict selector are separate CLI
changes planned for cargo-axonyx 0.6.16, with create-axonyx 0.6.11 selecting this
runtime for new projects. Existing applications need a checked dependency update
and compiled rebuild; installing a CLI does not replace an application's runtime.
The first CLI scope is compiled-host only; unsupported preview/fallback hosts
refuse the configuration instead of ignoring it.

Legura already passed isolated source-consumer acceptance, including upload
budgets, canonical images, auth, backup and recovery. Repeat that proof with
registry dependencies and no source overrides after publication. Do not treat
this release as product Media activation or authenticated image delivery.
