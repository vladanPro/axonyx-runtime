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

class FakeElement {
  constructor(tag, attrs = {}, children = []) {
    this.tag = tag;
    this.attrs = new Map(Object.entries(attrs));
    this.children = children;
    this.hidden = false;
    children.forEach((child) => { child.parent = this; });
  }

  getAttribute(name) { return this.attrs.get(name) ?? null; }
  setAttribute(name, value) { this.attrs.set(name, String(value)); }
  matches(selector) {
    if (selector === "ax-each-empty") return this.tag === "ax-each-empty";
    if (selector === "ax-each-item[data-ax-each-key-id]") {
      return this.tag === "ax-each-item" && this.attrs.has("data-ax-each-key-id");
    }
    return false;
  }
  remove() {
    if (!this.parent) return;
    this.parent.children = this.parent.children.filter((child) => child !== this);
    this.parent = undefined;
  }
  insertBefore(node, anchor) {
    if (node.parent) node.parent.children = node.parent.children.filter((child) => child !== node);
    const index = anchor ? this.children.indexOf(anchor) : this.children.length;
    this.children.splice(index < 0 ? this.children.length : index, 0, node);
    node.parent = this;
  }
}

const firstEachItem = new FakeElement("ax-each-item", {
  "data-ax-each-key-id": "string:first",
});
const secondEachItem = new FakeElement("ax-each-item", {
  "data-ax-each-key-id": "string:second",
});
const eachRoot = new FakeElement("ax-state-each", {
  "data-ax-each-protocol": "ax-each/1",
  "data-ax-each-signal": "page:probe:items:1",
  "data-ax-each-type": "List<EachItem>",
  "data-ax-each-initial": JSON.stringify([
    { id: "first", title: "First" },
    { id: "second", title: "Second" },
  ]),
  "data-ax-each-key-path": "id",
  "data-ax-each-key-kind": "string",
}, [firstEachItem, secondEachItem]);
const dispatchedEvents = [];

globalThis.CustomEvent = class CustomEvent {
  constructor(type, options = {}) {
    this.type = type;
    this.detail = options.detail;
  }
};
globalThis.document = {
  readyState: "complete",
  querySelectorAll: (selector) => (
    selector === "ax-state-each[data-ax-each-protocol]" ? [eachRoot] : []
  ),
  addEventListener: () => {},
};
globalThis.window = {
  WebAssembly,
  localStorage: storage(),
  sessionStorage: storage(),
  dispatchEvent: (event) => {
    dispatchedEvents.push(event);
    return true;
  },
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
  types: [
    {
      name: "Post",
      fields: [
        { name: "title", ty: "String", optional: false },
        { name: "summary", ty: "String", optional: true },
      ],
    },
    { name: "Theme", literals: ["silver", "bronze", "gold"] },
    {
      name: "EachItem",
      fields: [
        { name: "id", ty: "String", optional: false },
        { name: "title", ty: "String", optional: false },
      ],
    },
  ],
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
assert(state.validateValue("bronze", "Theme"), "literal union member was rejected");
assert(!state.validateValue("purple", "Theme"), "unknown literal union member was accepted");
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

state.set("page:probe:items:1", [
  { id: "second", title: "Second" },
  { id: "first", title: "First" },
]);
assert(
  eachRoot.children[0] === secondEachItem && eachRoot.children[1] === firstEachItem,
  "keyed Each did not reorder existing DOM ownership boundaries",
);
assert(
  eachRoot.getAttribute("data-ax-each-status") === "reconciled",
  "keyed Each did not report a reconciled state",
);

state.set("page:probe:items:1", [
  { id: "second", title: "Second" },
  { id: "first", title: "First" },
  { id: "third", title: "Third" },
]);
assert(eachRoot.children.length === 2, "unsupported keyed insert partially changed the DOM");
assert(
  eachRoot.getAttribute("data-ax-each-status") === "refresh-required",
  "unsupported keyed insert did not request the compiler render program",
);
assert(
  dispatchedEvents.some((event) => event.type === "axonyx:each-refresh-required"),
  "unsupported keyed insert did not emit its fallback event",
);

console.log("Axonyx typed state bridge + keyed Each reconciliation passed.");
