# Axonyx Runtime

Standalone Rust runtime workspace for Axonyx applications.

Axonyx Runtime provides the execution contract for Axonyx apps: environment loading, backend request types, `.ax` lowering support, generated handler contracts, and the runtime foundation for future fine-grained UI updates.

## Crates

This workspace includes:

- `axonyx-runtime` - public runtime contract and execution layer
- `axonyx-core` - parser, lowering, query model, SQL draft compiler, and shared core types
- `axonyx-macros` - ergonomic procedural macros used by the core/runtime layer

## Install

For generated Axonyx apps, prefer the crates.io package:

```toml
[dependencies]
axonyx-runtime = "0.1.12"
```

Use the Git dependency only when testing unreleased runtime work:

```toml
[dependencies]
axonyx-runtime = { git = "https://github.com/vladanPro/axonyx-runtime" }
```

## Local Development

```bash
cargo test
```

## Runtime Role

The runtime is intentionally separate from the site/design packages:

```txt
axonyx-runtime  = Rust execution contracts and runtime support
axonyx-core     = parser, lowering, query, SQL, and shared compiler-facing types
axonyx-ui       = Foundry CSS/assets/.ax UI components
axonyx-framework = CLIs and app scaffolding workflow
```

Generated apps should depend on `axonyx-runtime`; framework tooling can additionally use `axonyx-core` and `axonyx-macros` as needed.

## 0.1.0 Focus

The first `0.1.0` release aims to stabilize:

- runtime env loading with `AX_PUBLIC_*` and `AX_SECRET_*`
- direct and api data transport contracts
- backend query and mutation request types
- `.ax` backend lowering and generated runtime handler contracts
- stable seams for generated Axonyx apps to call into runtime code

Anything beyond that can continue to evolve behind new minor releases.

## Environment Convention

Public values use `AX_PUBLIC_*` and may be exposed to rendered output when appropriate.

Secret values use `AX_SECRET_*` and should remain server/runtime-only.

Examples:

```env
AX_PUBLIC_APP_NAME=Axonyx Site
AX_SECRET_DB_URL=postgres://...
AX_SECRET_DB_DIALECT=postgres
AX_SECRET_DB_TRANSPORT=direct
```

Postgres connections are encrypted and certificate-verified by default. For a provider with a
private CA, use `sslmode=verify-full&sslrootcert=/path/to/provider-ca.crt`. The explicit
`sslmode=require` compatibility mode still encrypts traffic, but does not verify the server
certificate or hostname. `prefer` and `allow` are rejected because they can fall back to plaintext.

## Data Transport Direction

The runtime keeps data access contracts transport-aware:

- `direct` is the default runtime mode for normal database connections
- `api` is an explicit mode for API-key-backed data providers
- query and mutation requests stay framework-shaped while adapters translate into concrete driver/provider behavior

## Reactivity Direction

Axonyx should not become a virtual-DOM rerender framework.

The preferred UI runtime direction is fine-grained and compiler-assisted:

```txt
compile .ax
  -> static HTML with stable node ids
  -> dependency graph
  -> small runtime patcher
```

A signal should map to exact patch targets, not whole component rerenders.

Example dependency shape:

```txt
count -> [
  { node: copy_text_1, target: Text },
  { node: reset_button, target: Attribute("disabled") }
]
```

When `count` changes, the runtime should patch only those targets.

Preferred phrasing:

```txt
Axonyx does not rerender components by default.
Axonyx patches exact targets produced by the compiler.
```

## Binding Model Direction

Axonyx should separate storage from binding:

```txt
global/state = storage model
hard/soft = binding model
```

- `global` means app-level state
- `state` means component/route/local scoped state
- hard/signal binding means live DOM binding through a stable signal identity
- soft/value binding means snapshot or event/app-flow logic

Use this mental model:

```txt
Soft = snapshot
Hard = live handle
```

Avoid promising "zero runtime". More accurate terms are:

- minimal runtime
- compiler-generated runtime
- no virtual DOM
- no component rerender by default

## Links

- crates.io: https://crates.io/crates/axonyx-runtime
- GitHub: https://github.com/vladanPro/axonyx-runtime
