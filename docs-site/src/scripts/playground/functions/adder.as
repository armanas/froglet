// Adds and multiplies two integers: {"a": 6, "b": 7} answers {"sum":13,"product":42}.
// Change anything, then publish it. This page compiles it to a WebAssembly module and runs that module.

// The largest integer that every JSON reader, a browser included, holds exactly (RFC 7493).
const MAX_SAFE: i64 = 9007199254740991;
// What intField returns when a field is missing, is not an integer, or is beyond MAX_SAFE.
const NONE: i64 = i64.MIN_VALUE;

function respond(request: string): string {
  const a = intField(request, "a");
  const b = intField(request, "b");
  if (a == NONE || b == NONE) {
    return '{"error":"send integers like {\\"a\\": 6, \\"b\\": 7}, each within +-9007199254740991"}';
  }
  const sum = a + b;
  // a * b stays within MAX_SAFE only if |b| <= MAX_SAFE / |a|, which finds out without overflowing 64 bits on the way.
  if (abs(sum) > MAX_SAFE || (a != 0 && abs(b) > MAX_SAFE / abs(a))) {
    return '{"error":"the result is outside +-9007199254740991"}';
  }
  return `{"sum":${sum},"product":${a * b}}`;
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
