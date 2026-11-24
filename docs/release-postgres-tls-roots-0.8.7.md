# PostgreSQL TLS Roots - Runtime 0.8.7 Candidate

Runtime 0.8.7 updates the bundled PostgreSQL TLS trust store to
`webpki-roots 1.0.12` for [RUSTSEC-2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381.html).
Core stays at 0.8.6. No connection-mode changes or database migrations are
part of this patch.

Promote through `dev` and `main`, then wait for CI and rehearse packaging
against published Core 0.8.6 before publishing Runtime. After publication,
verify the registry package resolves the patched root store. Applications
need to update their lockfile and rebuild and redeploy to use it.
