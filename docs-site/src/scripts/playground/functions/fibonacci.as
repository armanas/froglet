// The nth Fibonacci number: {"n": 10} answers {"n":10,"fibonacci":55}.
// Change anything, then publish it. This page compiles it to a WebAssembly module and runs that module.

// F(78) is the largest Fibonacci number a browser holds exactly (RFC 7493), so n stops there.
const LARGEST_N: i64 = 78;
// The largest integer that every JSON reader, a browser included, holds exactly.
const MAX_SAFE: i64 = 9007199254740991;
// What intField returns when a field is missing, is not an integer, or is beyond MAX_SAFE.
const NONE: i64 = i64.MIN_VALUE;

function respond(request: string): string {
  const n = intField(request, "n");
  if (n == NONE) return '{"error":"send an object like {\\"n\\": 10}"}';
  if (n < 0 || n > LARGEST_N) return `{"error":"n must be between 0 and ${LARGEST_N}"}`;
  let previous: i64 = 0;
  let current: i64 = 1;
  for (let step: i64 = 0; step < n; step++) {
    const next = previous + current;
    previous = current;
    current = next;
  }
  return `{"n":${n},"fibonacci":${previous}}`;
}

// The integer after `"key":` in a flat JSON object. It does not skip over string values, which is enough here.
function intField(json: string, key: string): i64 {
  const needle = '"' + key + '"';
  const at = json.indexOf(needle);
  if (at < 0) return NONE;
  let i = skipSpaces(json, at + needle.length);
  if (i >= json.length || json.charCodeAt(i) != 58) return NONE; // the colon
  i = skipSpaces(json, i + 1);
  let end = i;
  while (end < json.length && (json.charCodeAt(end) == 45 || isDigit(json.charCodeAt(end)))) end++;
  return parseInteger(json.substring(i, end));
}

function skipSpaces(text: string, from: i32): i32 {
  let i = from;
  while (i < text.length && (text.charCodeAt(i) == 32 || (text.charCodeAt(i) >= 9 && text.charCodeAt(i) <= 13))) i++;
  return i;
}

function isDigit(code: i32): bool {
  return code >= 48 && code <= 57;
}

// "-123" as a number. Anything else, or a number beyond MAX_SAFE, is NONE.
function parseInteger(text: string): i64 {
  let i = 0;
  const negative = text.length > 0 && text.charCodeAt(0) == 45;
  if (negative) i = 1;
  if (i == text.length) return NONE;
  let value: i64 = 0;
  for (; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (!isDigit(code)) return NONE; // another minus sign
    value = value * 10 + <i64>(code - 48);
    if (value > MAX_SAFE) return NONE;
  }
  return negative ? -value : value;
}
