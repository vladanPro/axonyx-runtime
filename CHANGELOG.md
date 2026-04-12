# Changelog

All notable changes to the Axonix runtime workspace will be documented in this file.

The format is intentionally simple while the project is still early-stage.

## 0.1.0 - Unreleased

### Added

- standalone `axonix-runtime` workspace repository
- `axonix-runtime` crate for runtime contracts and execution planning
- `axonix-core` crate for parser, lowering, query modeling, and SQL draft compilation
- `axonix-macros` crate for procedural macro ergonomics
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

- `0.1.0` is meant to establish the first stable runtime dependency story for Axonix apps.
- The public app-facing dependency should stay centered on `axonix-runtime`, even while the workspace remains internally modular.
