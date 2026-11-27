use std::{env, fs, path::PathBuf};

use axonyx_runtime::preview_ax_page_with_imports;

const TABLE_BODY: &str = r#"
component TableBody {
  return ASX {
    <tbody class="ax-table__body">
      <Slot />
    </tbody>
  }
}
"#;

const TABLE_ROW: &str = r#"
component TableRow {
  return ASX {
    <tr class="ax-table__row">
      <Slot />
    </tr>
  }
}
"#;

const TABLE_CELL: &str = r#"
component TableCell(align = "") {
  return ASX {
    <td class="ax-table__cell" data-align={align}>
      <Slot />
    </td>
  }
}
"#;

const SOURCE: &str = r#"
import { TableBody } from "@axonyx/ui/foundry/TableBody"
import { TableRow } from "@axonyx/ui/foundry/TableRow"
import { TableCell } from "@axonyx/ui/foundry/TableCell"

page ReactiveEachProbe() {
  state count: Int = 2
  state ratio: Float = 0.5
  state enabled: Bool = true
  state posts = [{ id: "first", title: "Alpha", disabled: false }, { id: "second", title: "Beta", disabled: true }]
  state fallbackPosts = [{ id: "fallback", title: "Stable", visible: true }]
  type Row {
    id: String
    title: String
  }
  state rowSets: List<List<Row>> = [[{ id: "a", title: "Alpha" }, { id: "b", title: "Beta" }], [{ id: "b", title: "Beta" }, { id: "a", title: "Alpha" }], []]
  state selectedRows: Int = 0
  type PostStatus = "draft" | "published" | "archived"
  type FilterPost {
    id: String
    title: String
    status: PostStatus
  }
  state filterPosts: List<FilterPost> = [{ id: "draft-a", title: "First draft", status: "draft" }, { id: "published-b", title: "Published post", status: "published" }, { id: "draft-c", title: "Second draft", status: "draft" }]
  state selectedStatus: String = "archived"

  return ASX {
    <>
      <button id="increase-count" on:click={count += 1}>Increase count</button>
      <span id="addition-label">{"Count: " + count}</span>
      <span id="addition-suffix">{count + " items"}</span>
      <span id="addition-total">{count + ratio}</span>
      <span id="addition-flag">{"enabled=" + enabled}</span>
      <span id="post-count">{List.length(posts)}</span>
      <button id="update" on:click={posts = [{ id: "first", title: "Alpha updated", disabled: false }, { id: "second", title: "Beta", disabled: false }]}>Update</button>
      <button id="insert" on:click={posts = [{ id: "first", title: "Alpha updated", disabled: false }, { id: "second", title: "Beta", disabled: false }, { id: "third", title: "<strong>Literal</strong>", disabled: false }]}>Insert</button>
      <button id="reorder" on:click={posts = [{ id: "third", title: "<strong>Literal</strong>", disabled: false }, { id: "first", title: "Alpha updated", disabled: false }, { id: "second", title: "Beta", disabled: false }]}>Reorder</button>
      <button id="remove" on:click={posts = [{ id: "third", title: "<strong>Literal</strong>", disabled: false }, { id: "first", title: "Alpha updated", disabled: false }]}>Remove</button>
      <button id="duplicate" on:click={posts = [{ id: "first", title: "Duplicate A", disabled: false }, { id: "first", title: "Duplicate B", disabled: false }]}>Duplicate</button>
      <button id="fallback" on:click={fallbackPosts = [{ id: "fallback", title: "Changed", visible: false }]}>Fallback</button>

      <Each items={posts} as="post" key={post.id}>
        <article data-post-id={post.id} title={post.title}>
          <input aria-label={post.id} value={post.title} disabled={post.disabled} />
          <span>{post.title}</span>
        </article>
      </Each>

      <Each items={fallbackPosts} as="post" key={post.id}>
        <If when={post.visible}><span>{post.title}</span></If>
      </Each>
      <button id="derived-reorder" on:click={selectedRows = 1}>Change order</button>
      <button id="derived-clear" on:click={selectedRows = 2}>Clear rows</button>
      <button id="derived-reset" on:click={selectedRows = 0}>Reset rows</button>
      <section id="derived-row-set">
        <Each items={rowSets[selectedRows]} as="row" key={row.id}>
          <article>{row.title}</article>
          <Empty><p>No rows</p></Empty>
        </Each>
      </section>
      <section id="status-filter">
        <button id="filter-drafts" on:click={selectedStatus = "draft"}>Draft</button>
        <button id="filter-published" on:click={selectedStatus = "published"}>Published</button>
        <button id="filter-archived" on:click={selectedStatus = "archived"}>Archived</button>
        <Each items={List.filterEq(filterPosts, "status", selectedStatus)} as="post" key={post.id}>
          <article>{post.title}</article>
          <Empty><p>No matching posts</p></Empty>
        </Each>
      </section>
      <table id="native-filter">
        <TableBody>
          <Each items={List.filterEq(filterPosts, "status", selectedStatus)} as="post" key={post.id}>
            <TableRow><TableCell>{post.title}</TableCell></TableRow>
            <Empty><tr><td>No matching table rows</td></tr></Empty>
          </Each>
        </TableBody>
      </table>
      <table id="native-order">
        <tbody>
          <Each items={rowSets[selectedRows]} as="row" key={row.id}>
            <tr><td>{row.title}</td></tr>
          </Each>
        </tbody>
      </table>
    </>
  }
}
"#;

fn main() {
    let output = env::var_os("AXONYX_E2E_OUTPUT")
        .map(PathBuf::from)
        .expect("AXONYX_E2E_OUTPUT must name the fixture output directory");
    let runtime_dir = output.join("_ax").join("runtime");
    fs::create_dir_all(&runtime_dir).expect("fixture output directory should be created");

    let resolver = |path: &str| {
        match path {
            "@axonyx/ui/foundry/TableBody" => Some(TABLE_BODY),
            "@axonyx/ui/foundry/TableRow" => Some(TABLE_ROW),
            "@axonyx/ui/foundry/TableCell" => Some(TABLE_CELL),
            _ => None,
        }
        .map(str::to_string)
    };
    let html = preview_ax_page_with_imports(SOURCE, &resolver)
        .expect("reactive Each fixture should compile");
    fs::write(output.join("index.html"), html).expect("fixture HTML should be written");
    fs::write(
        runtime_dir.join("axonyx-state-v2.wasm"),
        include_bytes!("../assets/axonyx-state-v2.wasm"),
    )
    .expect("fixture WASM should be written");
}
