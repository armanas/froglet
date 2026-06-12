use serde::de::DeserializeOwned;

pub(crate) async fn read_response_bytes_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("{label}: failed to read response body: {error}"))?
    {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| format!("{label}: response body is too large"))?;
        if next_len > max_bytes {
            return Err(format!(
                "{label}: response body exceeded {max_bytes} byte limit"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) async fn read_json_response_limited<T: DeserializeOwned>(
    response: reqwest::Response,
    max_bytes: usize,
    label: &str,
) -> Result<T, String> {
    let body = read_response_bytes_limited(response, max_bytes, label).await?;
    serde_json::from_slice(&body).map_err(|error| format!("{label}: invalid JSON: {error}"))
}
