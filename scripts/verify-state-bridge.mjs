import { readFile } from "node:fs/promises";
import vm from "node:vm";

const runtimeSource = await readFile(
  new URL("../crates/axonyx-runtime/src/lib.rs", import.meta.url),
  "utf8",
);
const match = runtimeSource.match(
  /r##"<script data-ax-runtime="state-bridge">([\s\S]*?)<\/script>"##/,
);
if (!match) throw new Error("state bridge script was not found");

const wasmBytes = await readFile(
  new URL("../crates/axonyx-runtime/assets/axonyx-state-v2.wasm", import.meta.url),
);
const storage = () => ({
  getItem: () => null,
  setItem: () => {},
});

globalThis.CustomEvent = class CustomEvent {
  constructor(type, options = {}) {
    this.type = type;
    this.detail = options.detail;
  }
};
globalThis.document = {
  readyState: "complete",
  querySelectorAll: () => [],
  addEventListener: () => {},
};
globalThis.window = {
  WebAssembly,
  localStorage: storage(),
  sessionStorage: storage(),
  dispatchEvent: () => true,
};
globalThis.fetch = async () => ({
  ok: true,
  arrayBuffer: async () => wasmBytes.buffer.slice(
    wasmBytes.byteOffset,
    wasmBytes.byteOffset + wasmBytes.byteLength,
  ),
  json: async () => ({ files: [], signals: [] }),
});
window.fetch = globalThis.fetch;

vm.runInThisContext(match[1], { filename: "axonyx-state-bridge.js" });
const state = window.__axonyx.state;
const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

assert(await state.loadWasm("memory://axonyx-state-v2.wasm"), "WASM executor did not load");
state.hydrateManifest({
  types: [{
    name: "Post",
    fields: [
      { name: "title", ty: "String", optional: false },
      { name: "summary", ty: "String", optional: true },
    ],
  }],
  files: [{
    file: "app/posts/page.asx",
    signals: [
      {
        key: "page:posts:items:1",
        name: "items",
        scope: "page:posts",
        owner: "page:/posts",
        ty: "List<Optional<Post>>",
      },
      {
        key: "page:posts:ratio:2",
        name: "ratio",
        scope: "page:posts",
        owner: "page:/posts",
        ty: "Float",
      },
    ],
  }],
});

const valid = [{ title: "First", summary: null }, null];
assert(state.validateValue(valid, "List<Optional<Post>>"), "valid record list was rejected");
assert(!state.validateValue([{ title: 7 }], "List<Post>"), "invalid record field was accepted");
assert(state.validateValue(0.625, "Float"), "finite Float was rejected");
assert(!state.validateValue(Number.POSITIVE_INFINITY, "Float"), "infinite Float was accepted");
assert(state.validateValue("2024-02-29", "Date"), "valid leap date was rejected");
assert(!state.validateValue("2023-02-29", "Date"), "invalid leap date was accepted");
assert(
  state.validateValue("2026-08-23T10:15:30Z", "DateTime"),
  "valid DateTime was rejected",
);
assert(
  !state.validateValue("2026-08-23T10:15:30", "DateTime"),
  "timezone-free DateTime was accepted",
);
assert(
  state.validateValue("550e8400-e29b-41d4-a716-446655440000", "Uuid"),
  "canonical UUID was rejected",
);
assert(!state.validateValue("550e8400e29b41d4a716446655440000", "Uuid"), "invalid UUID was accepted");
assert(state.dispatch({
  protocol: "ax-state-event/1",
  event: "click",
  signal: "page:posts:items:1",
  op: "set",
  type: "List<Optional<Post>>",
  initial: "[]",
  valueSource: "literal",
  value: valid,
}), "typed state event was rejected");
assert(state.dispatch({
  protocol: "ax-state-event/1",
  event: "click",
  signal: "page:posts:ratio:2",
  op: "add",
  type: "Float",
  initial: "0.5",
  valueSource: "literal",
  value: 0.25,
}), "Float state event was rejected");
assert(state.runtime() === "wasm", "state runtime did not stay in WASM mode");
assert(state.diagnostics().wasmOperations === 2, "typed state did not execute through WASM");
assert(state.get("page:posts:items:1")[0].title === "First", "record list changed in transport");
assert(state.get("page:posts:ratio:2") === 0.75, "Float arithmetic changed in WASM transport");

console.log("Axonyx typed state bridge passed (records and Float via WASM ABI 3).");
