# Axonyx Runtime

Standalone runtime workspace for Axonyx applications.

The public framework name and crate identifiers now use `Axonyx` / `axonyx-*`.
Repository URLs and local workspace folders may still use older `axonix-*` names until the repo migration is finished.

Included crates:

- `axonyx-runtime`: runtime contract and execution layer
- `axonyx-core`: parser, lowering, query model, and SQL draft compiler
- `axonyx-macros`: component ergonomics macros

## Local Development

```bash
cargo test
```

## Git Dependency

Generated apps can depend on the runtime crate directly from Git:

```toml
[dependencies]
axonyx-runtime = { git = "https://github.com/vladanPro/axonix-runtime" }
```

## Release Shape

The public package model is intentionally simple:

- `axonyx-runtime`: main package that applications depend on
- `axonyx-core`: internal-facing but publishable support crate used by the runtime workspace
- `axonyx-macros`: ergonomic procedural macros used by the core layer

The expected long-term install story for applications is:

```toml
[dependencies]
axonyx-runtime = "0.1.0"
```

## 0.1.0 Focus

The first `0.1.0` release aims to stabilize:

- runtime env loading with `AX_PUBLIC_*` and `AX_SECRET_*`
- direct and api data transport contracts
- backend query and mutation request types
- `.ax` backend lowering and generated runtime handler contracts

Anything beyond that can continue to evolve behind new minor releases.
