# Expression and Action Diagnostics - 0.8.1 Candidate

Preparation only. `axonyx-core` and `axonyx-runtime` advance to 0.8.1;
`axonyx-macros` and internal state WASM remain unchanged. Tooling versions and
consumer defaults must be updated in the framework repository before release.

This patch contains three fixes already merged to runtime `dev`:

- Grouped `??` comparisons in `require` guards parse correctly, including
  quoted defaults containing delimiter words and escaped quotes (#377).
- Compiled actions referencing `params.*` or `query.*` receive a diagnostic
  before Rust generation, rather than an undefined context variable (#374).
  This does not add action route-parameter binding.
- Direct optional member access on raw DB query records uses null-safe JSON
  lookup, preserving fallback behavior for missing records or fields (#375).
  Typed Rust bindings retain their existing lowering.

The core unit suite and framework compiled HTTP smoke cover these boundaries.
Promotion still requires green exact-head CI, separate dev-to-main PRs,
package verification against published dependencies, core-before-runtime
publication, fresh registry-only consumer tests, and package-scoped tags.
No CMS/product deployment or global tool upgrade is part of this candidate.
