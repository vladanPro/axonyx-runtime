# Publish Runbook: 0.1.0

This is the exact release-day flow for the first Axonyx runtime publish.

## Assumptions

- you are in `H:\CODE\axonix\axonix-runtime`
- the working tree is clean
- you are logged into crates.io with `cargo login`
- `CHANGELOG.md` and release notes are up to date

## 1. Verify Workspace

Run:

```bash
cargo test
```

Expected result:

- the workspace passes

## 2. Package Dry Run

Run these in order:

```bash
cargo package -p axonyx-macros --allow-dirty --no-verify
cargo package -p axonyx-core --allow-dirty --no-verify
cargo package -p axonyx-runtime --allow-dirty --no-verify
```

Interpretation:

- `axonyx-macros` should package immediately
- `axonyx-core` and `axonyx-runtime` only package cleanly after their upstream crates are available on crates.io

## 3. Publish In Dependency Order

Run these one by one and wait for index propagation between publishes:

```bash
cargo publish -p axonyx-macros
```

Wait until crates.io can resolve `axonyx-macros`, then:

```bash
cargo publish -p axonyx-core
```

Wait until crates.io can resolve `axonyx-core`, then:

```bash
cargo publish -p axonyx-runtime
```

## 4. Verify Registry Install Story

Create a smoke app from the framework repo:

```bash
cargo run -p create-axonyx -- my-app --yes --runtime-source registry
```

Then inside the generated app:

```bash
cargo run
```

Expected result:

- the app resolves `axonyx-runtime = "0.1.0"` from crates.io
- the generated app compiles without switching back to `git` or `path`

## 5. Tag The Release

Suggested commands:

```bash
git tag v0.1.0
git push origin v0.1.0
```

## 6. Announce Release Shape

The message should stay simple:

- Axonyx apps depend on `axonyx-runtime`
- the workspace remains internally modular
- `create-axonyx --runtime-source registry` is now the normal package flow

## Rollback Mindset

If anything feels unstable during publish:

- stop after the last successful crate publish
- update docs and release notes honestly
- avoid rushing `axonyx-runtime` if `axonyx-core` still needs polish
