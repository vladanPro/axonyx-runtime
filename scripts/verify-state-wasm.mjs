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
assert(
  typeof wasm.ax_state_evaluate_expression === "function",
  "expression evaluator export is missing",
);
assert(
  typeof wasm.ax_state_reconcile_keys === "function",
  "keyed Each reconciliation export is missing",
);

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

const intFrame = (value) => {
  const payload = new Uint8Array(8);
  new DataView(payload.buffer).setBigInt64(0, BigInt(value), true);
  return frame(4, payload);
};
const expressionProgram = Uint8Array.from([
  65, 88, 69, 1,
  0, 0, 0,
  0, 1, 0,
  20,
]);
const expressionRequest = Uint8Array.from([
  ...u32(expressionProgram.length),
  ...expressionProgram,
  ...u32(2),
  ...intFrame(3),
  ...intFrame(4),
]);
assert(expressionRequest.length <= valueCapacity, "expression request exceeds WASM value buffer");
new Uint8Array(wasm.memory.buffer, valuePointer, expressionRequest.length).set(expressionRequest);
const expressionLength = wasm.ax_state_evaluate_expression(expressionRequest.length) >>> 0;
assert(expressionLength === 12, "expression evaluator returned an invalid frame");
const expressionResult = new Uint8Array(wasm.memory.buffer, valuePointer, expressionLength);
assert(
  expressionResult[0] === 65
    && expressionResult[1] === 88
    && expressionResult[2] === 1
    && expressionResult[3] === 4,
  "expression evaluator returned an invalid value protocol frame",
);
assert(
  new DataView(expressionResult.buffer, expressionResult.byteOffset).getBigInt64(4, true) === 7n,
  "expression evaluator returned the wrong result",
);

const stringFrame = (value) => {
  const encoded = encoder.encode(value);
  return frame(1, u32(encoded.length), encoded);
};
const stringListFrame = (values) => frame(6, u32(values.length), ...values.map(stringFrame));
const reconcileObjectFrame = (oldKeys, nextKeys) => {
  const entries = [
    ["next", stringListFrame(nextKeys)],
    ["old", stringListFrame(oldKeys)],
  ];
  return frame(
    7,
    u32(entries.length),
    ...entries.flatMap(([name, value]) => {
      const encoded = encoder.encode(name);
      return [u32(encoded.length), encoded, value];
    }),
  );
};
const reconcileRequest = reconcileObjectFrame(
  ["string:a", "string:b", "string:c"],
  ["string:c", "string:a", "string:d"],
);
new Uint8Array(wasm.memory.buffer, valuePointer, reconcileRequest.length).set(reconcileRequest);
const reconcileLength = wasm.ax_state_reconcile_keys(reconcileRequest.length) >>> 0;
assert(reconcileLength !== 0xffffffff, "keyed Each reconciliation rejected a valid request");
const reconcileResult = new Uint8Array(wasm.memory.buffer, valuePointer, reconcileLength);
assert(
  reconcileResult[0] === 65 && reconcileResult[1] === 88 && reconcileResult[3] === 7,
  "keyed Each reconciliation returned an invalid object frame",
);

console.log(`Axonyx state WASM ABI v3 + expression/1 + ax-each/1 passed (${bytes.length} bytes).`);
