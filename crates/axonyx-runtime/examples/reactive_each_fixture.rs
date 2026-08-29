use std::{env, fs, path::PathBuf};

use axonyx_runtime::preview_ax_page;

const SOURCE: &str = r#"
page ReactiveEachProbe() {
  state posts = [{ id: "first", title: "Alpha", disabled: false }, { id: "second", title: "Beta", disabled: true }]
  state fallbackPosts = [{ id: "fallback", title: "Stable", visible: true }]

  return ASX {
    <>
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

    let html = preview_ax_page(SOURCE).expect("reactive Each fixture should compile");
    fs::write(output.join("index.html"), html).expect("fixture HTML should be written");
    fs::write(
        runtime_dir.join("axonyx-state-v2.wasm"),
        include_bytes!("../assets/axonyx-state-v2.wasm"),
    )
    .expect("fixture WASM should be written");
}
