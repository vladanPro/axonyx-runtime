# Axonix Runtime

Standalone runtime workspace for Axonix applications.

Included crates:

- `axonix-runtime`: runtime contract and execution layer
- `axonix-core`: parser, lowering, query model, and SQL draft compiler
- `axonix-macros`: component ergonomics macros

## Local Development

```bash
cargo test
```

## Git Dependency

Generated apps can depend on the runtime crate directly from Git:

```toml
[dependencies]
axonix-runtime = { git = "https://github.com/vladanPro/axonix-runtime" }
```

## Release Shape

The public package model is intentionally simple:

- `axonix-runtime`: main package that applications depend on
- `axonix-core`: internal-facing but publishable support crate used by the runtime workspace
- `axonix-macros`: ergonomic procedural macros used by the core layer

The expected long-term install story for applications is:

```toml
[dependencies]
axonix-runtime = "0.1.0"
```

## 0.1.0 Focus

The first `0.1.0` release aims to stabilize:

- runtime env loading with `AX_PUBLIC_*` and `AX_SECRET_*`
- direct and api data transport contracts
- backend query and mutation request types
- `.ax` backend lowering and generated runtime handler contracts

Anything beyond that can continue to evolve behind new minor releases.
