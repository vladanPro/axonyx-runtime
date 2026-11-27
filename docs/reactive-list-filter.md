# Reactive list filtering

`List.filterEq(posts, "status", selectedStatus)` returns a list with the same
item type and order as `posts`, keeping only items whose `status` exactly
matches the selected text. The field name must be a string literal. Named record
contracts are checked; missing or non-text fields in untyped items do not match.

```ax
<Each items={List.filterEq(posts, "status", selectedStatus)} as="post" key={post.id}>
  <article>{post.title}</article>
  <Empty><p>No matching posts</p></Empty>
</Each>
```

For a reactive filter, `posts` and `selectedStatus` in this example are page
state. The first render applies the same filter on the server. Later state
changes filter the list locally through the WASM expression executor. The
source list is not mutated, and the filter only sees rows already in state.
Use a database query for filtering across unloaded pages.

Keyed `Each` still needs an initial row template to insert elements locally.
If the first render has no matches and provides no template, reconciliation
reports that a refresh is required. This feature does not change native
table-row rendering.
