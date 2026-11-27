# Changelog

All notable changes to the Axonyx runtime workspace will be documented in this file.

The format is intentionally simple while the project is still early-stage.

## Unreleased

- Support `List.length(items)` in Pages. It returns an `Int` for a list in
  the first server render and for supported reactive expressions in WASM.
  Counting does not mutate the list or make a server request.
- Support keyed `<Each>` over a derived reactive list, such as
  `items={rowSets[selected]}`. WASM evaluates the source and the browser
  reconciles supported rows by key. When no row template is available,
  insertion retains the existing refresh-required fallback.
- Add `List.filterEq(posts, "status", selectedStatus)` for exact,
  case-sensitive text matching in Pages and client WASM. It returns a
  new list in the original order and skips missing or non-text fields. See
  [reactive list filtering](docs/reactive-list-filter.md).
- Prepare a keyed row template for an initially unmatched filter over a
  nonempty state list, without running server data callbacks for the sample.

## axonyx-core / axonyx-runtime 0.8.5

- Resolve `Title`, `Meta`, and `Link` from top-level page bindings in
  server-rendered documents. Preserve layout assets and escape rendered values.
- Omit `Meta` with null content and `Link` with null href. Fragment rendering
  and script/theme preflight behavior are unchanged.
- Runtime 0.8.5 requires Core 0.8.5. Publish Core first and verify it is
  resolvable from crates.io before publishing Runtime. See
  [server-rendered metadata](docs/server-rendered-head.md) for scope and syntax.

## axonyx-core 0.8.4

- Preserve native JSON string redirect targets in compiled actions and API
  responses instead of serializing additional URL quote characters. Non-string
  targets fail with a fixed error. Generated consumer coverage checks native
  aliases, API members/status, exact action payloads and API preview parity.
- Accept and preserve line comments in backend `.ax` parsing/formatting and
  language-service diagnostics, already merged in development before this patch.
- Only core is versioned for this patch. Runtime remains 0.8.4. Preview action
  `return redirect(...)` parity is tracked separately in issue #298.

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
