// The contract, froglet.wasm.run_json.v1. It is the same for every function, and it comes after your code.
//
// The host calls alloc(len) for room to put the request, writes the request JSON there, and calls run(pointer, len).
// run answers with one i64: the response JSON's pointer in the high 32 bits and its length in the low 32.
// Your part is a function named respond(request: string): string. It takes the request JSON text and returns the response
// JSON text. Only alloc, run, and the memory are exported, and the module imports nothing.

export function alloc(len: i32): usize {
  return heap.alloc(len);
}

export function run(pointer: usize, len: i32): i64 {
  const response = respond(String.UTF8.decodeUnsafe(pointer, len));
  const bytes = String.UTF8.encode(response);
  return (<i64>changetype<usize>(bytes) << 32) | <i64>bytes.byteLength;
}
