# Axonyx Runtime 0.1.0 Checklist

This document tracks the minimum bar for the first publishable Axonyx runtime release.

## Goal

Ship a small, honest `0.1.0` that gives Axonyx apps a stable runtime dependency and lets `create-axonix --runtime-source registry` become real.

## Package Scope

- Publish `axonix-macros`
- Publish `axonix-core`
- Publish `axonix-runtime`
- Keep the app-facing story centered on `axonix-runtime`

## Publish Order

1. `axonix-macros`
2. `axonix-core`
3. `axonix-runtime`

That order matches the dependency graph and avoids temporary broken releases.

## Preflight

- Confirm `cargo test` passes in the standalone `axonix-runtime` workspace
- Confirm `cargo package --allow-dirty --no-verify` succeeds for each crate
- Confirm crate metadata is present:
  - `description`
  - `license`
  - `repository`
  - `readme`
  - `keywords`
  - `categories`
- Confirm internal path dependencies also declare matching crate versions for packaging
- Confirm README examples do not depend on files outside this repo
- Confirm no unpublished local-only path assumptions remain in app-facing docs

## API Freeze For 0.1.0

- `AxEnv`
- `AxDatabaseDriver`
- `AxDataTransport`
- `AxDatabaseConfig`
- `AxQueryRequest`
- `AxInsertRequest`
- `AxUpdateRequest`
- `AxDeleteRequest`
- `AxSendRequest`
- `AxBackendRuntime`
- `runtime_from_env`
- `backend_prelude`

If any of these still feel unstable, delay publish instead of shipping a misleading `0.1.0`.

## Known Follow-Up Work

- tighten doctest/doc examples around `axonix-core`
- decide whether `axonix-core` should remain user-visible or mostly internal
- tag and announce the first release
- add CI workflow for test + package verification
- decide whether to expose optional cargo features before `0.2.0`

## Definition Of Done

The release is ready when:

1. the workspace passes tests
2. each crate packages cleanly
3. crates publish in dependency order
4. `create-axonix --runtime-source registry` generates an app that compiles against the published version
