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
    this.textContent = "";
    children.forEach((child) => { child.parent = this; });
  }

  getAttribute(name) { return this.attrs.get(name) ?? null; }
  setAttribute(name, value) { this.attrs.set(name, String(value)); }
  removeAttribute(name) { this.attrs.delete(name); }
  matches(selector) {
    if (selector === "ax-each-empty") return this.tag === "ax-each-empty";
    if (selector === "ax-each-item[data-ax-each-key-id]") {
      return this.tag === "ax-each-item" && this.attrs.has("data-ax-each-key-id");
    }
    if (selector === "template[data-ax-each-render-protocol='ax-each-render/1']") {
      return this.tag === "template"
        && this.attrs.get("data-ax-each-render-protocol") === "ax-each-render/1";
    }
    if (selector === "[data-ax-each-render-target], [data-ax-each-render-attrs]") {
      return this.attrs.has("data-ax-each-render-target")
        || this.attrs.has("data-ax-each-render-attrs");
    }
    return false;
  }
  querySelectorAll(selector) {
    return this.children.flatMap((child) => [
      ...(child.matches?.(selector) ? [child] : []),
      ...(child.querySelectorAll?.(selector) || []),
    ]);
  }
  cloneNode(deep = false) {
    const clone = new FakeElement(
      this.tag,
      Object.fromEntries(this.attrs),
      deep ? this.children.map((child) => child.cloneNode(true)) : [],
    );
    clone.textContent = this.textContent;
    return clone;
  }
  append(child) {
    const appended = child.tag === "#fragment" ? [...child.children] : [child];
    appended.forEach((node) => {
      this.children.push(node);
      node.parent = this;
    });
    if (child.tag === "#fragment") child.children = [];
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

class FakeTemplate extends FakeElement {
  constructor(children) {
    super("template", { "data-ax-each-render-protocol": "ax-each-render/1" });
    this.content = new FakeElement("#fragment", {}, children);
  }
}

const renderTitle = (value) => {
  const node = new FakeElement("ax-each-value", {
    "data-ax-each-render-target": "text",
    "data-ax-each-render-path": "title",
  });
  node.textContent = value;
  return node;
};

const renderAttributes = (title, disabled) => {
  const node = new FakeElement("button", {
    title,
    "data-ax-each-render-attrs": JSON.stringify([
      { target: "title", path: "title", mode: "attribute" },
      { target: "disabled", path: "disabled", mode: "boolean" },
    ]),
  });
  if (disabled) node.setAttribute("disabled", "");
  return node;
};

const firstEachItem = new FakeElement("ax-each-item", {
  "data-ax-each-key-id": "string:first",
}, [renderTitle("First"), renderAttributes("First", false)]);
const secondEachItem = new FakeElement("ax-each-item", {
  "data-ax-each-key-id": "string:second",
}, [renderTitle("Second"), renderAttributes("Second", false)]);
secondEachItem.children[0].focused = true;
const eachTemplate = new FakeTemplate([
  renderTitle("First"),
  renderAttributes("First", false),
]);
const eachRoot = new FakeElement("ax-state-each", {
  "data-ax-each-protocol": "ax-each/1",
  "data-ax-each-signal": "page:probe:items:1",
  "data-ax-each-type": "List<EachItem>",
  "data-ax-each-initial": JSON.stringify([
    { id: "first", title: "First", disabled: false },
    { id: "second", title: "Second", disabled: false },
  ]),
  "data-ax-each-key-path": "id",
  "data-ax-each-key-kind": "string",
  "data-ax-each-render-status": "ready",
}, [firstEachItem, secondEachItem, eachTemplate]);
const fallbackText = new FakeElement("span");
fallbackText.textContent = "Published";
const fallbackItem = new FakeElement("ax-each-item", {
  "data-ax-each-key-id": "string:fallback",
}, [fallbackText]);
const fallbackEachRoot = new FakeElement("ax-state-each", {
  "data-ax-each-protocol": "ax-each/1",
  "data-ax-each-signal": "page:probe:fallback:1",
  "data-ax-each-type": "List<EachItem>",
  "data-ax-each-initial": JSON.stringify([
    { id: "fallback", title: "Published", disabled: false },
  ]),
  "data-ax-each-key-path": "id",
  "data-ax-each-key-kind": "string",
  "data-ax-each-render-status": "fallback",
}, [fallbackItem]);
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
    selector === "ax-state-each[data-ax-each-protocol]"
      ? [eachRoot, fallbackEachRoot]
      : []
  ),
  addEventListener: () => {},
  createElement: (tag) => new FakeElement(tag),
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
        { name: "disabled", ty: "Bool", optional: false },
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
  { id: "second", title: "Second", disabled: false },
  { id: "first", title: "First", disabled: false },
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
  { id: "second", title: "Second updated", disabled: true },
  { id: "first", title: "<strong>Still text</strong>", disabled: false },
]);
assert(eachRoot.children[0] === secondEachItem, "same-key update replaced its DOM owner");
assert(secondEachItem.children[0].focused, "same-key update lost component-local focus state");
assert(
  secondEachItem.children[0].textContent === "Second updated",
  "same-key update did not write the changed item text",
);
assert(
  firstEachItem.children[0].textContent === "<strong>Still text</strong>",
  "item text was interpreted as markup instead of literal text",
);
assert(
  secondEachItem.children[1].getAttribute("title") === "Second updated"
    && secondEachItem.children[1].getAttribute("disabled") === "",
  "same-key update did not apply safe attribute and boolean writes",
);

state.set("page:probe:items:1", [
  { id: "second", title: "Second updated", disabled: true },
  { id: "first", title: "<strong>Still text</strong>", disabled: false },
  { id: "third", title: "Third", disabled: false },
]);
assert(eachRoot.children.length === 4, "keyed insert did not preserve items plus template");
assert(
  eachRoot.children[2].getAttribute("data-ax-each-key-id") === "string:third",
  "keyed insert did not create the new item in collection order",
);
assert(
  eachRoot.children[2].children[0].textContent === "Third",
  "keyed insert did not apply the compiler render program",
);

eachTemplate.content.children[0].setAttribute("data-ax-each-render-path", "missing");
state.set("page:probe:items:1", [
  { id: "second", title: "Second updated", disabled: true },
  { id: "first", title: "<strong>Still text</strong>", disabled: false },
  { id: "third", title: "Third", disabled: false },
  { id: "fourth", title: "Fourth", disabled: false },
]);
assert(eachRoot.children.length === 4, "invalid render program partially inserted an item");
assert(
  eachRoot.getAttribute("data-ax-each-status") === "refresh-required",
  "invalid render program did not request deterministic refresh fallback",
);
assert(
  dispatchedEvents.some((event) => (
    event.type === "axonyx:each-refresh-required"
      && event.detail.reason === "item-render-program-required"
  )),
  "invalid render program did not emit its fallback reason",
);

state.set("page:probe:fallback:1", [
  { id: "fallback", title: "Draft", disabled: false },
]);
assert(
  fallbackEachRoot.children[0] === fallbackItem
    && fallbackText.textContent === "Published",
  "unsupported same-key item update partially changed its DOM",
);
assert(
  fallbackEachRoot.getAttribute("data-ax-each-status") === "refresh-required",
  "unsupported same-key item update did not require refresh",
);

console.log("Axonyx typed state bridge + keyed Each reconciliation passed.");
