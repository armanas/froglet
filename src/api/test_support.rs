#![cfg(test)]

use super::*;

#[allow(dead_code)]
pub(crate) fn auth_header(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).expect("valid auth header"),
    );
    headers
}
