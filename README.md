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
