# Changelog

All notable changes to the Axonyx runtime workspace will be documented in this file.

The format is intentionally simple while the project is still early-stage.

## Unreleased

## 0.3.0 - 2026-09-06

### Added

- direct PostgreSQL execution with Rustls TLS, connection pooling, bounded
  checkout timeouts, statement timeouts, and safe transient read retries
- atomic generated transactions and serialized migration batches for SQLite and
  PostgreSQL
- typed database resources, mutations, scalar contracts, foreign-key relation
  contracts, and explicit typed inner joins
- database health/readiness observability with redacted public failures and
  internal pool/operation reports
- mandatory PostgreSQL CI coverage for health, CRUD, transactions, and
  migration apply/rollback

### Changed

- compiled database handlers run on Tokio's blocking pool instead of occupying
  asynchronous network workers
- PostgreSQL scalar transport preserves exact numeric, enum, domain, array,
  UUID, temporal, JSON, and byte contracts
- PostgreSQL `sslmode=prefer` and `sslmode=allow` are rejected because they can
  fall back to plaintext

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
