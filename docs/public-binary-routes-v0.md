# Public Binary Routes V0 - Unpublished Compiler Contract

This extends the Rust runtime foundation in public-native-responses-v0.md.
It is not published Cargo/CLI functionality and does not expose Legura images.
Framework issue 376 remains open for CLI adoption and product acceptance.

```ax
route GET "/posts/:slug/image/:id" -> Binary {
  return Native.publicResponse("post.image", { slug: params.slug, id: params.id })
}
```

The compiler lowers the final direct return to `AxReturnPlan::PublicResponse`
and invokes only the explicitly registered public runtime channel. Unlike a
private response, it does not implicitly request session configuration. Explicit
Auth.subject references in payloads/guards still participate in normal session
discovery. Product callbacks must check current publication and association
before reading bytes; knowing a media ID is not a read grant.

The existing reserved Binary contract applies to both channels: GET only, one
final direct native response, a source-owned literal callback name, one JSON
payload and only preceding data/forbidden-or-notFound guards. Other locations,
headers/cookies, writes, hooks, redirects and nested binary-as-value calls fail
before generated Rust is emitted. Token checks ignore string literal content.
The legacy validate_private_responses / InvalidPrivateResponse Rust API names
are retained, but their validation now covers both isolated binary channels.

Generated handlers reuse the conservative binary finalizer and validator:
private/no-store cache policy, nosniff, same-origin, no cookies and safe errors,
regardless of the API response-validation flag. Shared helper names do not route
public calls through private callbacks or manufacture a verified subject.

Core tests cover generated Rust syntax, channel choice, auth discovery, literal
data and invalid contracts/locations/nesting. The native consumer smoke compiles
and runs actual generated Rust for anonymous success, product not-found, safe
guard denial and attempted public-to-private dispatch, with validation on/off.
Existing private/logout/JSON/Domain proofs remain in that same fixture.

The runtime's direct preview interpreter still refuses Binary routes with the
compiled-server requirement. It does not attempt unsafe JSON interpretation.
CLI-host preview/compiled parity, reports/contracts and session discovery are
the next integration gate, not claimed complete here.

## Release Safety

PublicResponse is a new public enum variant. Exhaustive downstream matches,
including cargo-axonyx, must be updated before adoption. Do not publish this as
a compatible 0.7.x core/runtime patch: use the next coordinated minor line and
registry-only consumer checks so old CLI installations cannot resolve the new
enum through their existing compatible dependency range. Package versions
intentionally remain unchanged during this source-only feature PR.

No production deployment, global tooling update or operator CMS migration occurs.
