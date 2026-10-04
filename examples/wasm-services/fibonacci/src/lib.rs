//! A `froglet.wasm.run_json.v1` service: reads `{"n": 10}` and returns `{"n": 10, "fibonacci": 55}`.
//!
//! See the `adder` sample for how the three-export contract works. This one has no dependencies either.

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

/// F(78) is the largest Fibonacci number below 2^53, the largest integer every JSON reader, including JavaScript, holds
/// exactly (RFC 7493). Past it a browser would round the answer.
const LARGEST_N: i64 = 78;

fn respond(request: &str) -> String {
    match int_field(request, "n") {
        Some(n) if (0..=LARGEST_N).contains(&n) => {
            format!("{{\"n\":{n},\"fibonacci\":{}}}", fibonacci(n as u32))
        }
        Some(_) => format!("{{\"error\":\"n must be between 0 and {LARGEST_N}\"}}"),
        None => "{\"error\":\"send an object like {\\\"n\\\": 10}\"}".to_string(),
    }
}

fn fibonacci(n: u32) -> i64 {
    let (mut previous, mut current) = (0i64, 1i64);
    for _ in 0..n {
        (previous, current) = (current, previous + current);
    }
    previous
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
