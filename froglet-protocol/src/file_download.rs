//! Application-level immutable file package. No Kernel artifact is changed.
use crate::{canonical_json, crypto};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CONTRACT: &str = "froglet.builtin.file_download.v1";
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_METADATA_BYTES: usize = 4096;
const MAGIC: &[u8] = b"FROGLET-FILE-V1\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMetadata {
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub expires_at: i64,
    pub max_downloads: u64,
    pub max_transfer_bytes: u64,
}

impl FileMetadata {
    pub fn validate(&self) -> Result<(), String> {
        // A narrow portable filename also makes Content-Disposition safe.
        if self.filename.is_empty()
            || self.filename.len() > 128
            || self.filename.starts_with('.')
            || self.filename.starts_with(' ')
            || !self
                .filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._- ".contains(&b))
            || self.filename.ends_with(' ')
            || self.filename.contains("..")
        {
            return Err("filename must be a safe 1–128 byte ASCII basename".into());
        }
        if self.media_type.len() > 128
            || self.media_type.matches('/').count() != 1
            || self.media_type.starts_with('/')
            || self.media_type.ends_with('/')
            || !self
                .media_type
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"/!#$&^_.+-".contains(&b))
        {
            return Err("media_type must be a lowercase MIME type without parameters".into());
        }
        if self.size_bytes > MAX_FILE_BYTES as u64
            || self.expires_at <= 0
            || self.max_downloads == 0
            || self.max_downloads > 1_000_000
            || self.max_transfer_bytes < self.size_bytes
            || self.max_transfer_bytes == 0
            || self.max_transfer_bytes > i64::MAX as u64
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("file requires a valid digest, expiry and finite download/byte limits; maximum size is 8 MiB".into());
        }
        Ok(())
    }

    pub fn output_schema(&self) -> Value {
        json!({"type":"object", "const":self})
    }
}

pub fn input_schema() -> Value {
    json!({"type":"object","properties":{"action":{"const":"describe"}},"required":["action"],"additionalProperties":false})
}

pub fn encode(metadata: &FileMetadata, bytes: &[u8]) -> Result<Vec<u8>, String> {
    metadata.validate()?;
    if bytes.len() as u64 != metadata.size_bytes || crypto::sha256_hex(bytes) != metadata.sha256 {
        return Err("file bytes do not match the approved metadata".into());
    }
    let header = canonical_json::to_vec(metadata).map_err(|e| e.to_string())?;
    if header.len() > MAX_METADATA_BYTES {
        return Err("file metadata is too large".into());
    }
    let mut package = Vec::with_capacity(MAGIC.len() + 4 + header.len() + bytes.len());
    package.extend_from_slice(MAGIC);
    package.extend_from_slice(&(header.len() as u32).to_be_bytes());
    package.extend(header);
    package.extend_from_slice(bytes);
    Ok(package)
}

pub fn decode(package: &[u8]) -> Result<(FileMetadata, &[u8]), String> {
    if package.len() > MAX_FILE_BYTES + MAX_METADATA_BYTES + MAGIC.len() + 4
        || !package.starts_with(MAGIC)
    {
        return Err("invalid file package".into());
    }
    let len_bytes: [u8; 4] = package
        .get(MAGIC.len()..MAGIC.len() + 4)
        .ok_or("truncated file package")?
        .try_into()
        .map_err(|_| "invalid file header")?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_METADATA_BYTES {
        return Err("file metadata is too large".into());
    }
    let start = MAGIC.len() + 4;
    let header = package
        .get(start..start + len)
        .ok_or("truncated file metadata")?;
    let metadata: FileMetadata =
        serde_json::from_slice(header).map_err(|_| "invalid file metadata")?;
    metadata.validate()?;
    if canonical_json::to_vec(&metadata).map_err(|e| e.to_string())? != header {
        return Err("file metadata is not canonical".into());
    }
    let bytes = &package[start + len..];
    if bytes.len() as u64 != metadata.size_bytes || crypto::sha256_hex(bytes) != metadata.sha256 {
        return Err("file content digest or size mismatch".into());
    }
    Ok((metadata, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(bytes: &[u8]) -> FileMetadata {
        FileMetadata {
            filename: "example.pdf".into(),
            media_type: "application/pdf".into(),
            size_bytes: bytes.len() as u64,
            sha256: crypto::sha256_hex(bytes),
            expires_at: 2_000_000_000,
            max_downloads: 3,
            max_transfer_bytes: 100,
        }
    }
    #[test]
    fn exact_bytes_empty_files_and_tampering() {
        for bytes in [b"hello".as_slice(), b"".as_slice()] {
            let m = metadata(bytes);
            let mut p = encode(&m, bytes).unwrap();
            assert_eq!(decode(&p).unwrap(), (m, bytes));
            p.push(0);
            assert!(decode(&p).is_err());
        }
    }
    #[test]
    fn rejects_paths_headers_and_unbounded_limits() {
        for name in [
            "../secret",
            "/secret",
            "x\\y",
            "x\r\nSet-Cookie: bad",
            ".hidden",
            "a\"b",
        ] {
            let mut m = metadata(b"x");
            m.filename = name.into();
            assert!(encode(&m, b"x").is_err());
        }
        let mut m = metadata(b"x");
        m.max_downloads = 0;
        assert!(m.validate().is_err());
        m.max_downloads = 1;
        m.media_type = "text/html; charset=utf-8".into();
        assert!(m.validate().is_err());
        assert!(decode(b"FROGLET-FILE-V1\n\xff\xff\xff\xff").is_err());
    }
}
