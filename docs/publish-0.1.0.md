# Publish Runbook: 0.1.0

This is the exact release-day flow for the first Axonix runtime publish.

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
cargo package -p axonix-macros --allow-dirty --no-verify
cargo package -p axonix-core --allow-dirty --no-verify
cargo package -p axonix-runtime --allow-dirty --no-verify
```

Interpretation:

- `axonix-macros` should package immediately
- `axonix-core` and `axonix-runtime` only package cleanly after their upstream crates are available on crates.io

## 3. Publish In Dependency Order

Run these one by one and wait for index propagation between publishes:

```bash
cargo publish -p axonix-macros
```

Wait until crates.io can resolve `axonix-macros`, then:

```bash
cargo publish -p axonix-core
```

Wait until crates.io can resolve `axonix-core`, then:

```bash
cargo publish -p axonix-runtime
```

## 4. Verify Registry Install Story

Create a smoke app from the framework repo:

```bash
cargo run -p create-axonix -- my-app --yes --runtime-source registry
```

Then inside the generated app:

```bash
cargo run
```

Expected result:

- the app resolves `axonix-runtime = "0.1.0"` from crates.io
- the generated app compiles without switching back to `git` or `path`

## 5. Tag The Release

Suggested commands:

```bash
git tag v0.1.0
git push origin v0.1.0
```

## 6. Announce Release Shape

The message should stay simple:

- Axonix apps depend on `axonix-runtime`
- the workspace remains internally modular
- `create-axonix --runtime-source registry` is now the normal package flow

## Rollback Mindset

If anything feels unstable during publish:

- stop after the last successful crate publish
- update docs and release notes honestly
- avoid rushing `axonix-runtime` if `axonix-core` still needs polish
