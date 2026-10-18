# Compound Request Expressions V0

This compiler fix follows framework issue 368. A request member must resolve
through the same server context whether used alone or inside an expression:

```ax
route GET "/posts/:id" {
  require params.id != "" else notFound()
  require query.status in ["draft", "published"] else forbidden()
  return json({ id: params.id, status: query.status })
}
```

Previously a standalone binding worked, but an inline comparison could emit an
undefined Rust `query`/`params` identifier. The code generator now rewrites real
request-member tokens recursively inside grouped expressions and object/list
literals. String literals and same-named record/module members are not rewritten.
Existing request headers/cookies/form/JSON aliases keep their supported string
lookup contract. This does not add arbitrary Rust member syntax or new coercions.

Compound `Auth.subject` expressions also trigger the one server-owned session
lookup, including nested object values. Requestless jobs and Domain functions
cannot read this implicit session. Explicitly pass the required value to a pure
helper instead. A query parameter named `subject` is never authentication.

Logical short-circuit evaluation stays intact: an unused RHS is not resolved.
Membership checks allow fallible request lookups without putting `?` inside a
boolean-returning closure. Missing query/route values keep the existing error
contract; this change does not silently turn missing values into empty strings.

Acceptance includes code-generation regressions and the generated Rust consumer
in `scripts/smoke-native-calls.ps1`. The latter compiles and runs the real module,
then compares it with preview for accepted/rejected query/parameter/header
guards, membership, short-circuiting and authenticated/anonymous session guards.
Remote CI and framework submodule adoption remain separate gates. No package
version, operator installation or deployed site is updated by this fix.
