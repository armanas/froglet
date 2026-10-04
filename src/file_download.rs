//! Bounded file snapshots and durable transfer reservations, outside the Kernel.
use froglet_protocol::file_download::{FileMetadata, MAX_FILE_BYTES, MAX_METADATA_BYTES};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::Path,
    sync::{Arc, atomic::AtomicU64},
};

#[derive(Debug, Clone, Serialize)]
pub struct FileLimits {
    pub max_downloads: u64,
    pub max_transfer_bytes: u64,
    pub max_storage_bytes: u64,
    #[serde(skip)]
    pub gate: Arc<tokio::sync::Semaphore>,
    #[serde(skip)]
    pub abort_generation: Arc<AtomicU64>,
}

impl FileLimits {
    pub fn new(downloads: u64, bytes: u64, storage: u64) -> Result<Self, String> {
        if [downloads, bytes, storage]
            .iter()
            .any(|v| *v == 0 || *v > i64::MAX as u64)
            || downloads > 1_000_000
        {
            return Err(
                "file allowances must be positive finite integers; at most 1000000 downloads"
                    .into(),
            );
        }
        Ok(Self {
            max_downloads: downloads,
            max_transfer_bytes: bytes,
            max_storage_bytes: storage,
            gate: Arc::new(tokio::sync::Semaphore::new(1)),
            abort_generation: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn from_env() -> Result<Option<Self>, String> {
        let names = [
            "FROGLET_FILE_MAX_TOTAL_DOWNLOADS",
            "FROGLET_FILE_MAX_TOTAL_BYTES",
            "FROGLET_FILE_MAX_STORAGE_BYTES",
        ];
        let values = names.map(|name| std::env::var(name).ok());
        if values.iter().all(Option::is_none) {
            return Ok(None);
        }
        let mut limits = [0; 3];
        for i in 0..3 {
            limits[i] = values[i]
                .as_ref()
                .ok_or_else(|| format!("{} is required when file sharing is enabled", names[i]))?
                .parse::<u64>()
                .map_err(|_| format!("{} must be a finite positive integer", names[i]))?;
        }
        Self::new(limits[0], limits[1], limits[2]).map(Some)
    }
}

pub fn validate_publication(metadata: &FileMetadata, limits: &FileLimits) -> Result<(), String> {
    metadata.validate()?;
    let now = crate::settlement::current_unix_timestamp();
    if metadata.expires_at <= now || metadata.expires_at > now.saturating_add(30 * 86400) {
        return Err("file expiry must be in the next 30 days".into());
    }
    if metadata.max_downloads > limits.max_downloads
        || metadata.max_transfer_bytes > limits.max_transfer_bytes
    {
        return Err("file allowance exceeds the provider's cumulative limits".into());
    }
    Ok(())
}

pub fn read_snapshot(root: &Path, digest: &str) -> Result<Vec<u8>, String> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid snapshot digest".into());
    }
    let bytes = read_regular(
        &root.join(format!("{digest}.file")),
        MAX_FILE_BYTES + MAX_METADATA_BYTES + 64,
    )?;
    if crate::crypto::sha256_hex(&bytes) != digest {
        return Err("file snapshot changed".into());
    }
    froglet_protocol::file_download::decode(&bytes)?;
    Ok(bytes)
}

pub fn read_regular(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| "file is unavailable or is not a regular file")?;
    let meta = file.metadata().map_err(|_| "cannot inspect file")?;
    if !meta.is_file() || meta.len() > max as u64 {
        return Err("file must be regular and within its size limit".into());
    }
    let mut bytes = Vec::new();
    file.take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read file")?;
    if bytes.len() > max {
        return Err("file grew beyond its size limit".into());
    }
    Ok(bytes)
}

/// Caller holds the publication directory lock until commit/rollback.
pub fn check_storage(
    root: &Path,
    filename: &str,
    additional: u64,
    limits: Option<&FileLimits>,
) -> Result<(), String> {
    let limits = limits.ok_or("file sharing is disabled")?;
    let mut total = 0u64;
    let mut exists = false;
    for entry in fs::read_dir(root).map_err(|_| "file storage cannot be inspected")? {
        let entry = entry.map_err(|_| "file storage cannot be inspected")?;
        if entry.path().extension().is_some_and(|e| e == "file") {
            let meta = fs::symlink_metadata(entry.path())
                .map_err(|_| "file storage cannot be inspected")?;
            if !meta.is_file() {
                return Err("invalid entry in file snapshot storage".into());
            }
            total = total
                .checked_add(meta.len())
                .ok_or("file storage size overflow")?;
            exists |= entry.file_name() == filename;
        }
    }
    if !exists {
        total = total
            .checked_add(additional)
            .ok_or("file storage size overflow")?;
    }
    if total > limits.max_storage_bytes {
        return Err("file snapshot storage allowance exhausted".into());
    }
    Ok(())
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct TransferUsage {
    pub downloads: u64,
    pub bytes: u64,
}

pub fn usage(conn: &Connection, key: &str) -> Result<TransferUsage, String> {
    let row = conn
        .query_row(
            "SELECT downloads,bytes FROM file_transfer_usage WHERE scope=?1",
            [key],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or((0, 0));
    Ok(TransferUsage {
        downloads: row
            .0
            .try_into()
            .map_err(|_| "invalid download accounting")?,
        bytes: row
            .1
            .try_into()
            .map_err(|_| "invalid transfer accounting")?,
    })
}

/// Called in the same immediate transaction as lifecycle and access checks.
/// Every physical download counts, even when a caller retries an old request.
pub fn reserve(
    conn: &Connection,
    key: &str,
    metadata: &FileMetadata,
    limits: &FileLimits,
    now: i64,
) -> Result<(), String> {
    crate::provider_policy::require_not_paused(conn)?;
    metadata.validate()?;
    if now >= metadata.expires_at {
        return Err("file publication has expired".into());
    }
    for (scope, max_downloads, max_bytes) in [
        ("provider", limits.max_downloads, limits.max_transfer_bytes),
        (key, metadata.max_downloads, metadata.max_transfer_bytes),
    ] {
        let used = usage(conn, scope)?;
        if used.downloads >= max_downloads
            || used
                .bytes
                .checked_add(metadata.size_bytes)
                .is_none_or(|b| b > max_bytes)
        {
            return Err("file transfer allowance exhausted".into());
        }
    }
    for scope in ["provider", key] {
        conn.execute("INSERT INTO file_transfer_usage(scope,downloads,bytes) VALUES(?1,1,?2) ON CONFLICT(scope) DO UPDATE SET downloads=downloads+1,bytes=bytes+excluded.bytes",params![scope,metadata.size_bytes as i64]).map_err(|e|e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn init(c: &Connection) {
        c.execute_batch("CREATE TABLE file_transfer_usage(scope TEXT PRIMARY KEY,downloads INTEGER NOT NULL,bytes INTEGER NOT NULL);CREATE TABLE provider_control(id INTEGER PRIMARY KEY,pause_reason TEXT);INSERT INTO provider_control VALUES(1,NULL);").unwrap();
    }
    fn metadata() -> FileMetadata {
        FileMetadata {
            filename: "one.bin".into(),
            media_type: "application/octet-stream".into(),
            size_bytes: 2,
            sha256: crate::crypto::sha256_hex(b"ab"),
            expires_at: 20,
            max_downloads: 2,
            max_transfer_bytes: 4,
        }
    }
    #[test]
    fn failed_reservations_do_not_partially_charge_and_restart_preserves_usage() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("usage.db");
        let c = Connection::open(&p).unwrap();
        init(&c);
        let limits = FileLimits::new(3, 5, 1024).unwrap();
        crate::db::with_immediate_transaction(&c, |c| {
            reserve(c, "service:a", &metadata(), &limits, 1)
        })
        .unwrap();
        crate::db::with_immediate_transaction(&c, |c| {
            reserve(c, "service:a", &metadata(), &limits, 1)
        })
        .unwrap();
        assert!(
            crate::db::with_immediate_transaction(&c, |c| reserve(
                c,
                "service:b",
                &metadata(),
                &limits,
                1
            ))
            .is_err()
        );
        drop(c);
        let c = Connection::open(p).unwrap();
        assert_eq!(
            usage(&c, "provider").unwrap(),
            TransferUsage {
                downloads: 2,
                bytes: 4
            }
        );
        assert_eq!(usage(&c, "service:b").unwrap().downloads, 0);
    }
    #[test]
    fn expiry_pause_and_symlinks_fail_closed() {
        let c = Connection::open_in_memory().unwrap();
        init(&c);
        let limits = FileLimits::new(2, 4, 1024).unwrap();
        assert!(
            crate::db::with_immediate_transaction(&c, |c| reserve(
                c,
                "x",
                &metadata(),
                &limits,
                20
            ))
            .is_err()
        );
        crate::provider_policy::set_pause(&c, Some("stop")).unwrap();
        assert!(
            crate::db::with_immediate_transaction(&c, |c| reserve(c, "x", &metadata(), &limits, 1))
                .is_err()
        );
        assert_eq!(usage(&c, "provider").unwrap().downloads, 0);
        #[cfg(unix)]
        {
            let d = tempfile::tempdir().unwrap();
            fs::write(d.path().join("real"), b"x").unwrap();
            std::os::unix::fs::symlink(d.path().join("real"), d.path().join("link")).unwrap();
            assert!(read_regular(&d.path().join("link"), 10).is_err());
        }
    }
}
