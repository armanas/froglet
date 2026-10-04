//! A `froglet.wasm.run_json.v1` service: reads `{"a": 6, "b": 7}` and returns `{"sum": 13, "product": 42}`.
//!
//! The contract is three exports and no imports. The host writes the request JSON into memory it gets from `alloc`,
//! calls `run(pointer, length)`, and reads the result JSON from the packed `(pointer << 32) | length` that `run`
//! returns. This sample has no dependencies, so it reads just enough JSON for its own input; a real service would use
//! `serde_json`.

/// Reserve `len` bytes for the host to write the request into.
#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    let mut buffer = Vec::<u8>::with_capacity(len as usize);
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer as i32
}

/// Read the request at `pointer`, and return the response's location as `(pointer << 32) | length`.
#[no_mangle]
pub extern "C" fn run(pointer: i32, len: i32) -> i64 {
    let request = unsafe { std::slice::from_raw_parts(pointer as *const u8, len as usize) };
    let response = respond(&String::from_utf8_lossy(request));
    let bytes = response.into_bytes();
    let packed = ((bytes.as_ptr() as u32 as i64) << 32) | bytes.len() as i64;
    std::mem::forget(bytes);
    packed
}

/// 2^53 - 1: the largest integer that every JSON reader, including JavaScript, holds exactly (RFC 7493). A service whose
/// numbers stay inside it returns the same value to a browser as to a Rust node.
const MAX_SAFE: u64 = 9_007_199_254_740_991;

fn respond(request: &str) -> String {
    let (a, b) = match (int_field(request, "a"), int_field(request, "b")) {
        (Some(a), Some(b)) if a.unsigned_abs() <= MAX_SAFE && b.unsigned_abs() <= MAX_SAFE => (a, b),
        _ => return format!("{{\"error\":\"send integers like {{\\\"a\\\": 6, \\\"b\\\": 7}}, each within +-{MAX_SAFE}\"}}"),
    };
    match (a.checked_add(b), a.checked_mul(b)) {
        (Some(sum), Some(product))
            if sum.unsigned_abs() <= MAX_SAFE && product.unsigned_abs() <= MAX_SAFE =>
        {
            format!("{{\"sum\":{sum},\"product\":{product}}}")
        }
        _ => format!("{{\"error\":\"the result is outside +-{MAX_SAFE}\"}}"),
    }
}

/// The integer after `"key":` in a flat JSON object. It does not skip over string values, which is enough here.
fn int_field(json: &str, key: &str) -> Option<i64> {
    let needle = format!("\"{key}\"");
    let after_key = json.find(&needle)? + needle.len();
    let rest = json[after_key..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start();
    let end = rest
        .find(|c: char| !(c == '-' || c.is_ascii_digit()))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}
