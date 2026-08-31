# Changelog

All notable changes to the Axonyx runtime workspace will be documented in this file.

The format is intentionally simple while the project is still early-stage.

## Unreleased

### Added

- production Postgres TLS through Rustls
- verified TLS by default with optional `sslrootcert` provider CA support
- explicit `sslmode=require` compatibility for encrypted pooler connections

### Changed

- reject Postgres `sslmode=prefer` and `sslmode=allow` because they can fall back to plaintext

## 0.1.12 - 2026-05-30

### Added

- JSX-like `.ax` package use directives such as `use "@axonyx/ui"`
- parser/AST support for package-level asset activation before `page`
- auto-detection of `use` directives as AX v2 source

## 0.1.9 - 2026-05-21

### Added

- Server-Sent Events response contract through `AxHttpResponse::sse_events`
- `AxSseEvent` helper for typed event stream chunks
- JSX-like `page` params/defaults for importable `.ax` component files

## 0.1.8 - 2026-05-19

### Added

- scoped action/state patch support for app/layout/page state ownership
- `ActionForm` and `ActionStatus` lowering/runtime helpers
- typed, optional, and defaulted action input coercion for preview actions
- stronger state/action bridge contract for `cargo ax run dev`

## 0.1.7 - 2026-05-17

### Added

- release for `axonyx-core` and `axonyx-runtime`; `axonyx-macros` remains `0.1.0`
- server runtime contract with `AxServerConfig`, `AxServerMode`, `AxServer`, `AxHttpRequest`, and `AxHttpResponse`
- streaming-ready `AxBody` with fixed bytes and chunk collections
- response helpers for status lines, case-insensitive header lookup, no-store responses, and streaming chunk iteration

## 0.1.0 - Unreleased

### Added

- standalone `axonyx-runtime` workspace repository
- `axonyx-runtime` crate for runtime contracts and execution planning
- `axonyx-core` crate for parser, lowering, query modeling, and SQL draft compilation
- `axonyx-macros` crate for procedural macro ergonomics
- backend runtime contracts for:
  - environment loading
  - direct and api data transport
  - query, insert, update, delete, and send request types
- SQL execution planning draft for:
  - Postgres
  - MySQL
  - SQLite
- `.ax` backend authoring support for:
  - `route`
  - `loader`
  - `action`
  - `job`
  - `where`
  - `order`
  - `limit`
  - `offset`

### Notes

- `0.1.0` is meant to establish the first stable runtime dependency story for Axonyx apps.
- The public app-facing dependency should stay centered on `axonyx-runtime`, even while the workspace remains internally modular.
