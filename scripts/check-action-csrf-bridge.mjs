import { readFileSync } from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";

const source = readFileSync(new URL("../crates/axonyx-runtime/src/lib.rs", import.meta.url), "utf8");
const marker = 'r##"<script data-ax-runtime="actions">';
const start = source.indexOf(marker);
assert.notEqual(start, -1);
const script = source.slice(start + marker.length, source.indexOf('</script>"##', start));

async function run(action, token, denied = false, invalid = false) {
  let submit;
  const calls = [];
  const attributes = new Map();
  const fieldAttributes = new Map();
  const field = { name: "email", hasAttribute: (key) => fieldAttributes.has(key), setAttribute: (key, value) => fieldAttributes.set(key, value), removeAttribute: (key) => fieldAttributes.delete(key) };
  const message = { textContent: "", getAttribute: () => "email", setAttribute() {} };
  class Form {
    constructor() { this.action = action; this.method = "POST"; this.elements = [field]; }
    getAttribute(name) { return attributes.get(name); }
    hasAttribute(name) { return attributes.has(name); }
    setAttribute(name, value) { attributes.set(name, value); }
    removeAttribute(name) { attributes.delete(name); }
    querySelectorAll(selector) { return selector === "[data-ax-field-error]" ? [message] : []; }
  }
  class Data {
    constructor() { this.fields = new Map(); }
    has(key) { return this.fields.has(key); }
    append(key, value) { this.fields.set(key, value); }
    values() { return this.fields.values(); }
    [Symbol.iterator]() { return this.fields[Symbol.iterator](); }
  }
  const form = new Form();
  const window = {
    location: { href: "https://axonyx.dev/posts", origin: "https://axonyx.dev", reload() {}, assign() {} },
    dispatchEvent() {},
  };
  vm.runInNewContext(script, {
    window,
    document: {
      readyState: "complete",
      querySelectorAll() { return []; },
      addEventListener(name, callback) { if (name === "submit") submit = callback; },
    },
    HTMLFormElement: Form, FormData: Data, File: class {}, URL, URLSearchParams,
    CustomEvent: class {}, console,
    fetch: async (url, options) => {
      calls.push({ url, options });
      return url === "/__axonyx/csrf"
        ? { ok: !denied, json: async () => ({ token }) }
        : invalid
          ? { ok: false, headers: { get: () => "application/ax-error+json" }, json: async () => ({ error: { status: 422, value: { fields: { email: "<script>not executable</script>" } } } }) }
          : { ok: true, headers: { get: () => "application/ax-patch+json" }, json: async () => ({ patches: [], invalidations: [], refreshes: [] }) };
    },
  });
  await submit({ target: form, preventDefault() {} });
  return { calls, attributes, fieldAttributes, message };
}

const proof = "axcsrf1." + "a".repeat(64);
const valid = await run("https://axonyx.dev/__axonyx/action?name=Save", proof);
assert.equal(valid.calls.length, 2);
assert.equal(valid.calls[1].options.headers["X-Axonyx-CSRF"], proof);
assert.equal(valid.calls[1].options.redirect, "error");
assert.equal(valid.attributes.get("data-ax-action-state"), "complete");
const anon = await run("https://axonyx.dev/__axonyx/action?name=Save", null);
assert.equal(anon.calls.length, 2);
assert.equal(anon.calls[1].options.headers["X-Axonyx-CSRF"], undefined);
const foreign = await run("https://attacker.example/__axonyx/action", proof);
assert.equal(foreign.calls.length, 0);
const denied = await run("https://axonyx.dev/__axonyx/action", proof, true);
assert.equal(denied.calls.length, 1);
assert.equal(denied.attributes.get("data-ax-action-state"), "error");
const malformed = await run("https://axonyx.dev/__axonyx/action", "bad");
assert.equal(malformed.calls.length, 1);
const invalid = await run("https://axonyx.dev/__axonyx/action?name=Register", proof, false, true);
assert.equal(invalid.attributes.get("data-ax-action-state"), "error");
assert.equal(invalid.fieldAttributes.get("aria-invalid"), "true");
assert.equal(invalid.message.textContent, "<script>not executable</script>");
console.log("Action CSRF bridge checks passed (6 scenarios).");
