use crate::{DiskConfig, Error, Result, metadata::Metadata};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use std::{path::Path, time::Duration};

impl Metadata {
    pub(crate) fn open(path: &Path, config: &DiskConfig) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(Duration::from_millis(config.busy_timeout_ms))?;
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if mode != "wal" {
            return Err(Error::WalUnavailable);
        }
        db.pragma_update(None, "synchronous", "FULL")?;
        db.pragma_update(None, "cache_size", -(config.sqlite_cache_kib as i64))?;
        db.pragma_update(None, "wal_autocheckpoint", config.wal_autocheckpoint_pages)?;
        // Serialize schema inspection with other first-open processes.
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version = tx.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?;
        match version {
            0 => tx.execute_batch(include_str!("schema.sql"))?,
            1 => tx.execute_batch(
                "ALTER TABLE entries ADD COLUMN object_key BLOB CHECK(object_key IS NULL OR length(object_key)=32);
                 UPDATE entries SET object_key=key WHERE kind=1;
                 PRAGMA user_version=2;"
            )?,
            2 => {}
            v => return Err(Error::Schema(v)),
        }
        tx.commit()?;
        let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(Error::Integrity(check));
        }
        db.set_prepared_statement_cache_capacity(16);
        Ok(Self {
            connection: Mutex::new(db),
        })
    }
}
impl Metadata {
    pub(crate) fn bind_directory_root(&self, root: Option<&Path>) -> Result<()> {
        let mut db = self.connection.lock();
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let recorded: Option<Vec<u8>> = tx
            .query_row(
                "SELECT value FROM settings WHERE name='directory_root'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(recorded) = recorded {
            if root.is_some_and(|p| encode_path(p) != recorded) {
                return Err(Error::DirectoryRoot {
                    recorded: decode_path(recorded),
                    requested: root.map(Path::to_path_buf),
                });
            }
        } else if let Some(root) = root {
            tx.execute(
                "INSERT INTO settings VALUES('directory_root',?1)",
                [encode_path(root)],
            )?;
        }
        if root.is_none() {
            let rows: i64 = tx.query_row("SELECT count(*) FROM entries WHERE kind=1", [], |r| {
                r.get(0)
            })?;
            if rows > 0 {
                return Err(Error::DirectoriesDisabled);
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(unix)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(unix)]
fn decode_path(bytes: Vec<u8>) -> std::path::PathBuf {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes).into()
}
#[cfg(windows)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}
#[cfg(windows)]
fn decode_path(bytes: Vec<u8>) -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<_> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    std::ffi::OsString::from_wide(&wide).into()
}
