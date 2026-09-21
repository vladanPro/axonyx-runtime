# Axonyx Runtime 0.4.1

Axonyx Runtime 0.4.1 completes the first typed session and application-policy
path on top of the 0.4 line.

## Highlights

- persistent SQLite and Postgres session stores with signed secure cookies
- request-owned `Auth.subject` resolution
- typed query composition from authenticated subjects
- `User?` return contracts with direct `require` narrowing
- safe `forbidden()` policy responses
- borrow-aware record arguments for pure policy helpers without hidden clones
- warning-free generated Rust visibility for private and exported helpers

## Compatibility

This is a backward-compatible 0.4 patch release. Existing 0.4 applications can
upgrade with:

```bash
cargo ax upgrade
cargo update
cargo ax check
cargo ax build --clean
```
