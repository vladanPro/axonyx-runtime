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

When the first render has no matches, a filter over a named state list can
use its first loaded item to prepare an inert row template. The sample cannot
read surrounding page data or run server data callbacks. If the input itself
is empty or the row body cannot be safely cloned, reconciliation still reports
that a refresh is required.

## Native table rows

Put one keyed `Each` inside a `tbody` or Foundry `TableBody`. Render a single
native `tr` per item, directly or through `TableRow`. An `Empty` block is
optional; when provided it also renders a single row.

```ax
<TableBody>
  <Each items={List.filterEq(posts, "status", selectedStatus)} as="post" key={post.id}>
    <TableRow><TableCell>{post.title}</TableCell></TableRow>
    <Empty><tr><td>No matching posts</td></tr></Empty>
  </Each>
</TableBody>
```

## Sort loaded rows

`List.sortBy(posts, "title", direction)` returns a new list and accepts
`"asc"` or `"desc"` for direction. Text is compared case-sensitively,
without locale rules. Equal values keep their input order; missing or
non-text values go last in both directions.

```ax
<Each
  items={List.sortBy(List.filterEq(posts, "status", selectedStatus), "title", direction)}
  as="post"
  key={post.id}
>
  <TableRow><TableCell>{post.title}</TableCell></TableRow>
</Each>
```

The browser reconciles native rows directly. Keep static or independently
updated rows in separate table bodies. This does not change the Foundry
`DataTable` search or paging behavior.
