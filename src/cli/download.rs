//! Recipient downloads verify the current signed metadata before writing bytes.
use super::{
    CliError, pop_flag, pop_kv,
    service_link::{self, ServiceLink},
};
use froglet_protocol::file_download::{CONTRACT, FileMetadata};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    pop_flag(&mut args, "--json");
    let link = pop_kv(&mut args, "--service-url").ok_or_else(|| {
        CliError::BadArgs("download requires --service-url and --destination".into())
    })?;
    let destination = pop_kv(&mut args, "--destination")
        .ok_or_else(|| CliError::BadArgs("download requires --destination".into()))?;
    let token = pop_kv(&mut args, "--access-token-file").map(PathBuf::from);
    if !args.is_empty() {
        return Err(CliError::BadArgs("unexpected download arguments".into()));
    }
    let result = download(
        &ServiceLink::parse(&link)?,
        Path::new(&destination),
        token.as_deref(),
    )
    .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| CliError::Other(e.to_string()))?
    );
    Ok(())
}
fn destination_check(path: &Path) -> Result<(), CliError> {
    if !path.is_absolute() || path.file_name().is_none() || fs::symlink_metadata(path).is_ok() {
        return Err(CliError::BadArgs(
            "destination must be a new absolute file path; existing files are never overwritten"
                .into(),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| CliError::BadArgs("invalid destination".into()))?;
    if parent.canonicalize()? != parent {
        return Err(CliError::BadArgs(
            "destination directory must be canonical and contain no symlinks".into(),
        ));
    }
    Ok(())
}
pub async fn download(
    link: &ServiceLink,
    destination: &Path,
    token_path: Option<&Path>,
) -> Result<Value, CliError> {
    destination_check(destination)?;
    let inspected = service_link::inspect(link).await?;
    if inspected["contract_version"] != CONTRACT
        || inspected["price"]["settlement_method"] != "none"
        || inspected["price"]["base_amount_minor"] != 0
        || inspected["price"]["success_amount_minor"] != 0
    {
        return Err(CliError::Other(
            "service is not a free file download".into(),
        ));
    }
    let metadata: FileMetadata =
        serde_json::from_value(inspected["output_schema"]["const"].clone())
            .map_err(|_| CliError::Other("invalid signed file metadata".into()))?;
    metadata.validate().map_err(CliError::Other)?;
    if metadata.expires_at <= crate::settlement::current_unix_timestamp() {
        return Err(CliError::Other("file share expired".into()));
    }
    match inspected["availability"]["execution_access"].as_str() {
        Some("open" | "trial") => {}
        Some("invite") if token_path.is_some() => {}
        _ => {
            return Err(CliError::Other(
                "download requires current access; invitations require access_token_file".into(),
            ));
        }
    }
    let revision = inspected["verification"]["revision_hash"]
        .as_str()
        .ok_or_else(|| CliError::Other("missing signed revision".into()))?;
    let url = format!(
        "{}/v1/provider/services/{}/files/{revision}/download",
        link.provider_url, link.service_id
    );
    let token = token_path
        .map(super::invoke::read_access_token_file)
        .transpose()?;
    let result = crate::safe_fetch::safe_file_fetch(
        &url,
        token.as_ref().map(|s| s.as_str()),
        crate::safe_fetch::FetchPolicy {
            max_bytes: metadata.size_bytes.max(1024),
            timeout_ms: 30000,
            ..Default::default()
        },
    )
    .await
    .map_err(CliError::Other)?;
    if result.status_code != 200 {
        return Err(CliError::Other(format!(
            "download refused (HTTP {}); retrying may consume another allowance",
            result.status_code
        )));
    }
    verify_and_save(destination, &metadata, &result.body)?;
    Ok(
        json!({"status":"downloaded","destination":destination,"size_bytes":metadata.size_bytes,"sha256":metadata.sha256,
        "revision_hash":revision,"checksum_verified":true,"execution_receipt":null}),
    )
}
fn verify_and_save(path: &Path, metadata: &FileMetadata, bytes: &[u8]) -> Result<(), CliError> {
    if bytes.len() as u64 != metadata.size_bytes
        || crate::crypto::sha256_hex(bytes) != metadata.sha256
    {
        return Err(CliError::Other(
            "download size or checksum mismatch; nothing saved".into(),
        ));
    }
    destination_check(path)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_mismatches_and_overwrites() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let p = root.join("file");
        let m = FileMetadata {
            filename: "file".into(),
            media_type: "text/plain".into(),
            size_bytes: 2,
            sha256: crate::crypto::sha256_hex(b"ab"),
            expires_at: 123,
            max_downloads: 1,
            max_transfer_bytes: 2,
        };
        assert!(verify_and_save(&p, &m, b"cd").is_err());
        assert!(!p.exists());
        verify_and_save(&p, &m, b"ab").unwrap();
        assert!(verify_and_save(&p, &m, b"ab").is_err());
        assert_eq!(fs::read(p).unwrap(), b"ab");
    }
}
