//! Operator policy, outside signed Kernel artifacts. Allowances are cumulative
//! reservations, not billing estimates: failures and unpaid reservations are
//! deliberately not refunded. SQLite transactions serialize all admissions.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

pub const EXHAUSTED: &str = "provider execution allowance exhausted";

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProviderPolicy {
    pub require_payment: bool,
    pub max_total_deals: Option<u64>,
    pub max_total_runtime_ms: Option<u64>,
    pub max_total_quotes: Option<u64>,
    pub access_mode: AccessMode,
    #[serde(skip_serializing)]
    pub invite_token_hashes: Vec<String>,
    pub min_free_bytes: u64,
    pub max_database_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    #[default]
    Open,
    Private,
    Invite,
    Trial,
    Paid,
}

impl AccessMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "open" => Ok(Self::Open),
            "private" => Ok(Self::Private),
            "invite" => Ok(Self::Invite),
            "trial" => Ok(Self::Trial),
            "paid" => Ok(Self::Paid),
            _ => Err("provider access mode must be open, private, invite, trial or paid".into()),
        }
    }
}

impl ProviderPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if (self.require_payment || self.access_mode != AccessMode::Open)
            && (self.max_total_deals.is_none()
                || self.max_total_runtime_ms.is_none()
                || self.max_total_quotes.is_none())
        {
            return Err("protected providers require FROGLET_PROVIDER_MAX_TOTAL_DEALS, FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS and FROGLET_PROVIDER_MAX_TOTAL_QUOTES".into());
        }
        if self.access_mode == AccessMode::Paid && !self.require_payment {
            return Err("paid access mode requires FROGLET_PROVIDER_REQUIRE_PAYMENT=true".into());
        }
        if self.access_mode == AccessMode::Trial && self.require_payment {
            return Err("trial access mode cannot require payment".into());
        }
        // An empty invite store denies all guests; the owner can issue the
        // first durable invitation without restarting the provider.
        for limit in [
            self.max_total_deals,
            self.max_total_runtime_ms,
            self.max_total_quotes,
        ]
        .into_iter()
        .flatten()
        {
            if limit > i64::MAX as u64 {
                return Err("provider allowance exceeds SQLite integer range".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ProviderUsage {
    pub reserved_deals: u64,
    pub reserved_runtime_ms: u64,
    pub issued_quotes: u64,
}

fn read_nonnegative(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

pub fn usage(conn: &Connection) -> Result<ProviderUsage, String> {
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM provider_admissions),
                (SELECT COALESCE(SUM(max_runtime_ms), 0) FROM provider_admissions),
                (SELECT issued_quotes FROM provider_allowance WHERE id = 1)",
        [],
        |row| {
            Ok(ProviderUsage {
                reserved_deals: read_nonnegative(row, 0)?,
                reserved_runtime_ms: read_nonnegative(row, 1)?,
                issued_quotes: read_nonnegative(row, 2)?,
            })
        },
    )
    .map_err(|error| error.to_string())
}

/// Call within the same immediate transaction that persists a new deal.
/// A replay retains its reservation; archiving a deal never resets this ledger.
pub fn reserve_deal(
    conn: &Connection,
    policy: &ProviderPolicy,
    key: &str,
    max_runtime_ms: u64,
) -> Result<(), String> {
    let existing: Option<u64> = conn
        .query_row(
            "SELECT max_runtime_ms FROM provider_admissions WHERE admission_key = ?1",
            [key],
            |row| read_nonnegative(row, 0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some(existing) = existing {
        return if existing == max_runtime_ms {
            Ok(())
        } else {
            Err("provider admission replay changed its resource reservation".into())
        };
    }
    require_not_paused(conn)?;
    let totals = usage(conn)?;
    let runtime = totals
        .reserved_runtime_ms
        .checked_add(max_runtime_ms)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| EXHAUSTED.to_string())?;
    if policy
        .max_total_deals
        .is_some_and(|limit| totals.reserved_deals >= limit)
        || policy
            .max_total_runtime_ms
            .is_some_and(|limit| runtime > limit)
    {
        return Err(EXHAUSTED.into());
    }
    conn.execute(
        "INSERT INTO provider_admissions (admission_key, max_runtime_ms) VALUES (?1, ?2)",
        params![
            key,
            i64::try_from(max_runtime_ms).map_err(|_| EXHAUSTED.to_string())?
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

/// Quote creation is itself metered, before external wallet calls and writes.
/// Repeated quote requests count even if their signed payload would be equal.
pub fn reserve_quote(conn: &Connection, policy: &ProviderPolicy) -> Result<(), String> {
    require_not_paused(conn)?;
    let limit = policy.max_total_quotes.unwrap_or(i64::MAX as u64);
    let totals = usage(conn)?;
    if totals.issued_quotes >= limit
        || policy
            .max_total_deals
            .is_some_and(|limit| totals.reserved_deals >= limit)
        || policy
            .max_total_runtime_ms
            .is_some_and(|limit| totals.reserved_runtime_ms >= limit)
    {
        return Err(EXHAUSTED.into());
    }
    conn.execute(
        "UPDATE provider_allowance SET issued_quotes = issued_quotes + 1 WHERE id = 1",
        [],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn pause_reason(conn: &Connection) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT pause_reason FROM provider_control WHERE id=1",
        [],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn require_not_paused(conn: &Connection) -> Result<(), String> {
    if pause_reason(conn)?.is_some() {
        Err("provider is paused by its operator".into())
    } else {
        Ok(())
    }
}

pub fn set_pause(conn: &Connection, reason: Option<&str>) -> Result<(), String> {
    if reason.is_some_and(|r| r.trim().is_empty() || r.len() > 256) {
        return Err("pause reason must contain 1-256 bytes".into());
    }
    conn.execute(
        "UPDATE provider_control SET pause_reason=?1 WHERE id=1",
        [reason],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize)]
pub struct StorageStatus {
    pub database_bytes: u64,
    pub free_bytes: Option<u64>,
    pub blocked_reason: Option<String>,
}

pub fn storage_status(
    policy: &ProviderPolicy,
    root: &std::path::Path,
    db: &std::path::Path,
) -> Result<StorageStatus, String> {
    let mut database_bytes = 0u64;
    for path in [
        db.to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", db.display())),
        std::path::PathBuf::from(format!("{}-shm", db.display())),
    ] {
        match std::fs::metadata(path) {
            Ok(meta) => database_bytes = database_bytes.saturating_add(meta.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let free_bytes = if policy.min_free_bytes > 0 {
        Some(free_disk_bytes(root)?)
    } else {
        None
    };
    let blocked_reason = if policy
        .max_database_bytes
        .is_some_and(|max| database_bytes >= max)
    {
        Some("database high-water limit reached".into())
    } else if free_bytes.is_some_and(|bytes| bytes < policy.min_free_bytes) {
        Some("disk free-space reserve reached".into())
    } else {
        None
    };
    Ok(StorageStatus {
        database_bytes,
        free_bytes,
        blocked_reason,
    })
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // statvfs field widths differ across Unix targets.
fn free_disk_bytes(root: &std::path::Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(root.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: statvfs receives a valid terminated path and writable output.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let stats = unsafe { stats.assume_init() };
    Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}
#[cfg(not(unix))]
fn free_disk_bytes(_: &std::path::Path) -> Result<u64, String> {
    Err("free-space admission checks require a supported Unix host".into())
}

/// Only derived CSV indexes are disposable. Never delete published data,
/// receipts, deal results, accounting, or signed artifacts for retention.
pub fn prune_cache(root: &std::path::Path, older_than: std::time::Duration) -> Result<u64, String> {
    let dir = root.join("publication-data");
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.to_string()),
    };
    let mut removed = 0;
    for entry in entries.take(10_000) {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(digest) = name.strip_suffix(".csv.sqlite") else {
            continue;
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let meta = entry.path().symlink_metadata().map_err(|e| e.to_string())?;
        if meta.is_file()
            && meta
                .modified()
                .map_err(|e| e.to_string())?
                .elapsed()
                .unwrap_or_default()
                >= older_than
        {
            std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Invitations authorize new quote/deal requests on this provider, never
/// operator controls. Only token hashes are durable; revocation is immediate.
#[derive(Debug, Serialize)]
pub struct ProviderInvite {
    pub id: String,
    pub name: String,
    pub expires_at: i64,
    pub max_requests: u64,
    pub used_requests: u64,
    pub revoked: bool,
}

pub fn create_invite(
    conn: &Connection,
    name: &str,
    expires_at: i64,
    max_requests: u64,
    now: i64,
) -> Result<(String, String), String> {
    if name.trim().is_empty()
        || name.len() > 80
        || name.chars().any(char::is_control)
        || expires_at <= now
        || expires_at > now.saturating_add(30 * 86400)
        || !(2..=10_000).contains(&max_requests)
    {
        return Err("invite requires a name (1–80 characters), expiry within 30 days and 2–10000 new-work requests".into());
    }
    crate::db::with_immediate_transaction(conn, |conn| {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM provider_invites", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if count >= 1000 {
            return Err("provider invitation capacity reached (1000 retained records)".into());
        }
        let token = hex::encode(rand::random::<[u8; 32]>());
        let hash = crate::crypto::sha256_hex(token.as_bytes());
        conn.execute("INSERT INTO provider_invites (token_hash,name,expires_at,max_requests) VALUES (?1,?2,?3,?4)", params![hash,name,expires_at,max_requests as i64]).map_err(|e| e.to_string())?;
        Ok((hash, token))
    })
}

pub fn list_invites(conn: &Connection) -> Result<Vec<ProviderInvite>, String> {
    let mut stmt = conn.prepare("SELECT token_hash,name,expires_at,max_requests,used_requests,revoked FROM provider_invites ORDER BY token_hash LIMIT 1000").map_err(|e| e.to_string())?;
    stmt.query_map([], |row| {
        Ok(ProviderInvite {
            id: row.get(0)?,
            name: row.get(1)?,
            expires_at: row.get(2)?,
            max_requests: read_nonnegative(row, 3)?,
            used_requests: read_nonnegative(row, 4)?,
            revoked: row.get(5)?,
        })
    })
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())
}

pub fn revoke_invite(conn: &Connection, id: &str) -> Result<(), String> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invite id must be a lowercase SHA-256 hash".into());
    }
    // Tombstones also revoke legacy configured hashes without a restart.
    conn.execute("INSERT INTO provider_invites (token_hash,name,expires_at,max_requests,revoked) VALUES (?1,'revoked legacy invite',0,1,1) ON CONFLICT(token_hash) DO UPDATE SET revoked=1", [id]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Atomic conditional update bounds parallel requests. A refused/failed request
/// is not refunded. A normal invocation uses a quote and a deal request.
pub fn authorize_invite(
    conn: &Connection,
    policy: &ProviderPolicy,
    hash: &str,
    now: i64,
) -> Result<bool, String> {
    crate::db::with_immediate_transaction(conn, |conn| {
        let updated = conn.execute("UPDATE provider_invites SET used_requests=used_requests+1 WHERE token_hash=?1 AND revoked=0 AND expires_at>?2 AND used_requests<max_requests", params![hash,now]).map_err(|e| e.to_string())?;
        if updated == 1 {
            return Ok(true);
        }
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_invites WHERE token_hash=?1)",
                [hash],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists {
            return Ok(false);
        }
        use subtle::ConstantTimeEq;
        Ok(policy
            .invite_token_hashes
            .iter()
            .fold(0u8, |matched, expected| {
                matched | hash.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8()
            })
            != 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(conn: &Connection) {
        conn.execute_batch("CREATE TABLE provider_admissions (admission_key TEXT PRIMARY KEY, max_runtime_ms INTEGER NOT NULL);
            CREATE TABLE provider_allowance (id INTEGER PRIMARY KEY, issued_quotes INTEGER NOT NULL);
            INSERT INTO provider_allowance VALUES (1, 0);").unwrap();
        conn.execute_batch("CREATE TABLE provider_control (id INTEGER PRIMARY KEY, pause_reason TEXT); INSERT INTO provider_control VALUES (1, NULL);").unwrap();
    }

    #[test]
    fn reservations_are_cumulative_idempotent_and_fail_closed() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn);
        let policy = ProviderPolicy {
            require_payment: true,
            max_total_deals: Some(2),
            max_total_runtime_ms: Some(15),
            max_total_quotes: Some(2),
            ..Default::default()
        };
        let reserve = |key, ms| {
            crate::db::with_immediate_transaction(&conn, |conn| {
                reserve_deal(conn, &policy, key, ms)
            })
        };
        reserve("one", 10).unwrap();
        reserve("one", 10).unwrap();
        assert!(reserve("one", 1).is_err());
        assert_eq!(reserve("two", 6).unwrap_err(), EXHAUSTED);
        reserve("two", 5).unwrap();
        assert_eq!(reserve("three", 0).unwrap_err(), EXHAUSTED);
        assert_eq!(usage(&conn).unwrap().reserved_runtime_ms, 15);
        assert_eq!(reserve_quote(&conn, &policy).unwrap_err(), EXHAUSTED);
    }

    #[test]
    fn quote_allowance_does_not_reset_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.db");
        let conn = Connection::open(&path).unwrap();
        init(&conn);
        let policy = ProviderPolicy {
            max_total_quotes: Some(1),
            ..Default::default()
        };
        crate::db::with_immediate_transaction(&conn, |conn| reserve_quote(conn, &policy)).unwrap();
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(reserve_quote(&conn, &policy).unwrap_err(), EXHAUSTED);
    }

    #[test]
    fn paid_only_requires_all_allowances_and_zero_is_a_kill_switch() {
        assert!(
            ProviderPolicy {
                require_payment: true,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        let policy = ProviderPolicy {
            require_payment: true,
            max_total_deals: Some(0),
            max_total_runtime_ms: Some(0),
            max_total_quotes: Some(0),
            ..Default::default()
        };
        policy.validate().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        init(&conn);
        assert!(reserve_deal(&conn, &policy, "one", 1).is_err());
        assert!(reserve_quote(&conn, &policy).is_err());
    }

    #[test]
    fn concurrent_admissions_cannot_oversubscribe_the_allowance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.db");
        init(&Connection::open(&path).unwrap());
        let handles = (0..16)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    let policy = ProviderPolicy {
                        max_total_deals: Some(3),
                        max_total_runtime_ms: Some(30),
                        ..Default::default()
                    };
                    crate::db::with_immediate_transaction(&conn, |conn| {
                        reserve_deal(conn, &policy, &index.to_string(), 10)
                    })
                    .is_ok()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            handles
                .into_iter()
                .map(|handle| usize::from(handle.join().unwrap()))
                .sum::<usize>(),
            3
        );
        let totals = usage(&Connection::open(path).unwrap()).unwrap();
        assert_eq!(totals.reserved_deals, 3);
        assert_eq!(totals.reserved_runtime_ms, 30);
    }
    #[test]
    fn pause_restart_rollback_and_resume_preserve_reservations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.db");
        let conn = Connection::open(&path).unwrap();
        init(&conn);
        let policy = ProviderPolicy {
            max_total_deals: Some(1),
            max_total_runtime_ms: Some(10),
            max_total_quotes: Some(2),
            ..Default::default()
        };
        crate::db::with_immediate_transaction(&conn, |conn| reserve_deal(conn, &policy, "one", 10))
            .unwrap();
        set_pause(&conn, Some("maintenance")).unwrap();
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(pause_reason(&conn).unwrap().as_deref(), Some("maintenance"));
        assert!(reserve_quote(&conn, &policy).is_err());
        reserve_deal(&conn, &policy, "one", 10).unwrap(); // replay is not charged twice
        set_pause(&conn, None).unwrap();
        assert_eq!(
            reserve_deal(&conn, &policy, "two", 1).unwrap_err(),
            EXHAUSTED
        );
        // Changing configuration and rolling it back never recreates the ledger.
        assert!(ProviderPolicy::default().validate().is_ok());
        assert_eq!(usage(&conn).unwrap().reserved_runtime_ms, 10);
    }
    #[test]
    fn full_database_refuses_admission_atomically() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn);
        let pages: i64 = conn
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap();
        conn.pragma_update(None, "max_page_count", pages).unwrap();
        let result = crate::db::with_immediate_transaction(&conn, |conn| {
            reserve_deal(conn, &ProviderPolicy::default(), &"x".repeat(100_000), 10)
        });
        assert!(result.unwrap_err().contains("full"));
        assert_eq!(usage(&conn).unwrap().reserved_deals, 0);
        conn.pragma_update(None, "max_page_count", pages + 100)
            .unwrap();
        reserve_deal(&conn, &ProviderPolicy::default(), "fits", 10).unwrap();
        assert_eq!(usage(&conn).unwrap().reserved_deals, 1);
    }
    #[test]
    fn storage_watermarks_include_wal_and_preserve_financial_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("node.db");
        std::fs::write(&db, [0; 5]).unwrap();
        std::fs::write(dir.path().join("node.db-wal"), [0; 5]).unwrap();
        let policy = ProviderPolicy {
            max_database_bytes: Some(10),
            ..Default::default()
        };
        assert!(
            storage_status(&policy, dir.path(), &db)
                .unwrap()
                .blocked_reason
                .is_some()
        );
        let policy = ProviderPolicy {
            min_free_bytes: u64::MAX,
            ..Default::default()
        };
        assert!(
            storage_status(&policy, dir.path(), &db)
                .unwrap()
                .blocked_reason
                .is_some()
        );
        let data = dir.path().join("publication-data");
        std::fs::create_dir(&data).unwrap();
        let cache = data.join(format!("{}.csv.sqlite", "a".repeat(64)));
        std::fs::write(&cache, b"cache").unwrap();
        for file in ["receipt.json", "data.csv", "data.sqlite"] {
            std::fs::write(data.join(file), b"keep").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&db, data.join(format!("{}.csv.sqlite", "b".repeat(64))))
            .unwrap();
        assert_eq!(
            prune_cache(dir.path(), std::time::Duration::ZERO).unwrap(),
            1
        );
        assert!(db.exists());
        assert!(data.join("receipt.json").exists());
        assert!(data.join("data.sqlite").exists());
    }
    #[test]
    fn protected_access_modes_require_explicit_finite_allowances() {
        for access_mode in [
            AccessMode::Private,
            AccessMode::Invite,
            AccessMode::Trial,
            AccessMode::Paid,
        ] {
            assert!(
                ProviderPolicy {
                    access_mode,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        let bounded = ProviderPolicy {
            access_mode: AccessMode::Trial,
            max_total_deals: Some(2),
            max_total_runtime_ms: Some(10),
            max_total_quotes: Some(2),
            ..Default::default()
        };
        bounded.validate().unwrap();
        assert!(
            ProviderPolicy {
                require_payment: true,
                ..bounded
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn invites_expire_exhaust_revoke_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.db");
        let conn = crate::db::initialize_db(&path).unwrap();
        let policy = ProviderPolicy::default();
        let (id, token) = create_invite(&conn, "Alice", 200, 2, 100).unwrap();
        assert_eq!(id, crate::crypto::sha256_hex(token.as_bytes()));
        assert!(
            !serde_json::to_string(&list_invites(&conn).unwrap())
                .unwrap()
                .contains(&token)
        );
        assert!(authorize_invite(&conn, &policy, &id, 101).unwrap());
        drop(conn);
        let conn = crate::db::initialize_db(&path).unwrap();
        assert!(authorize_invite(&conn, &policy, &id, 101).unwrap());
        assert!(!authorize_invite(&conn, &policy, &id, 101).unwrap());
        let (exp, _) = create_invite(&conn, "Expires", 105, 10, 100).unwrap();
        assert!(!authorize_invite(&conn, &policy, &exp, 105).unwrap());
        let (rev, _) = create_invite(&conn, "Revoked", 200, 10, 100).unwrap();
        revoke_invite(&conn, &rev).unwrap();
        assert!(!authorize_invite(&conn, &policy, &rev, 101).unwrap());
        let legacy = ProviderPolicy {
            invite_token_hashes: vec!["a".repeat(64)],
            ..Default::default()
        };
        assert!(authorize_invite(&conn, &legacy, &"a".repeat(64), 101).unwrap());
        revoke_invite(&conn, &"a".repeat(64)).unwrap();
        assert!(!authorize_invite(&conn, &legacy, &"a".repeat(64), 101).unwrap());
        assert!(create_invite(&conn, "Bad", 100, 10, 100).is_err());
        assert!(create_invite(&conn, "Bad", 200, u64::MAX, 100).is_err());
    }

    #[test]
    fn concurrent_invites_cannot_overbook_the_request_allowance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.db");
        let conn = crate::db::initialize_db(&path).unwrap();
        let (id, _) = create_invite(&conn, "shared", 200, 6, 100).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let path = path.clone();
                let id = id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    barrier.wait();
                    authorize_invite(&conn, &ProviderPolicy::default(), &id, 101).unwrap()
                })
            })
            .collect();
        let accepted = threads
            .into_iter()
            .filter_map(|thread| thread.join().unwrap().then_some(()))
            .count();
        assert_eq!(accepted, 6);
        assert_eq!(list_invites(&conn).unwrap()[0].used_requests, 6);
    }
}
