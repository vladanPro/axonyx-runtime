import { readFile } from "node:fs/promises";

const path = new URL(
  "../crates/axonyx-runtime/assets/axonyx-state-v1.wasm",
  import.meta.url,
);
const bytes = await readFile(path);
const { instance } = await WebAssembly.instantiate(bytes, {});
const wasm = instance.exports;

const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

assert(wasm.ax_state_abi_version() === 2, "expected state ABI version 2");
assert(wasm.ax_state_apply_number(1, 2, 3) === 5, "number add failed");
assert(wasm.ax_state_apply_bool(3, 0, 0) === 1, "bool toggle failed");
assert(wasm.ax_state_supports_operation(0, 0) === 1, "string set should be supported");
assert(wasm.ax_state_supports_operation(0, 1) === 0, "string add should be rejected");

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
const input = encoder.encode("Axonyx state");
const pointer = wasm.ax_state_string_buffer_ptr();
const capacity = wasm.ax_state_string_buffer_capacity();
assert(input.length <= capacity, "test string exceeds WASM buffer");

new Uint8Array(wasm.memory.buffer, pointer, input.length).set(input);
const outputLength = wasm.ax_state_apply_string(0, input.length);
assert(outputLength === input.length, "string set returned an invalid length");
const output = decoder.decode(new Uint8Array(wasm.memory.buffer, pointer, outputLength));
assert(output === "Axonyx state", "string set changed the payload");

new Uint8Array(wasm.memory.buffer, pointer, 1)[0] = 0xff;
assert(
  (wasm.ax_state_apply_string(0, 1) >>> 0) === 0xffffffff,
  "invalid UTF-8 was accepted",
);

console.log(`Axonyx state WASM ABI v2 passed (${bytes.length} bytes).`);
