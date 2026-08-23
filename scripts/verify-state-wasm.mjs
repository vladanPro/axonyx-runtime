import { readFile } from "node:fs/promises";

const path = new URL(
  "../crates/axonyx-runtime/assets/axonyx-state-v2.wasm",
  import.meta.url,
);
const bytes = await readFile(path);
const { instance } = await WebAssembly.instantiate(bytes, {});
const wasm = instance.exports;

const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

assert(wasm.ax_state_abi_version() === 3, "expected state ABI version 3");
assert(wasm.ax_state_apply_number(1, 2, 3) === 5, "number add failed");
assert(wasm.ax_state_apply_bool(3, 0, 0) === 1, "bool toggle failed");
assert(wasm.ax_state_supports_operation(0, 0) === 1, "string set should be supported");
assert(wasm.ax_state_supports_operation(0, 1) === 0, "string add should be rejected");
assert(wasm.ax_state_supports_operation(3, 0) === 1, "structured set should be supported");

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

const u32 = (value) => {
  const output = new Uint8Array(4);
  new DataView(output.buffer).setUint32(0, value, true);
  return output;
};
const frame = (tag, ...parts) => Uint8Array.from([
  65,
  88,
  1,
  tag,
  ...parts.flatMap((part) => Array.from(part)),
]);
const text = encoder.encode("published");
const textFrame = frame(1, u32(text.length), text);
const nullFrame = frame(0);
const listFrame = frame(6, u32(2), textFrame, nullFrame);
const key = encoder.encode("filters");
const objectFrame = frame(7, u32(1), u32(key.length), key, listFrame);
const valuePointer = wasm.ax_state_value_buffer_ptr();
const valueCapacity = wasm.ax_state_value_buffer_capacity();
assert(objectFrame.length <= valueCapacity, "test value exceeds WASM value buffer");
new Uint8Array(wasm.memory.buffer, valuePointer, objectFrame.length).set(objectFrame);
assert(
  wasm.ax_state_apply_value(0, objectFrame.length) === objectFrame.length,
  "nested object/list frame was rejected",
);
new Uint8Array(wasm.memory.buffer, valuePointer, 1)[0] = 0;
assert(
  (wasm.ax_state_apply_value(0, objectFrame.length) >>> 0) === 0xffffffff,
  "malformed value frame was accepted",
);

console.log(`Axonyx state WASM ABI v3 passed (${bytes.length} bytes).`);
