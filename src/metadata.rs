use crate::{Error, Key, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) const BLOB: i64 = 0;
pub(crate) const DIRECTORY: i64 = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum EntryStatus {
    Building = 1,
    Ready = 2,
    Pruning = 3,
}
impl EntryStatus {
    fn decode(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::Building),
            2 => Ok(Self::Ready),
            3 => Ok(Self::Pruning),
            _ => Err(Error::UnknownStatus(value)),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum EntryKind {
    Blob = 0,
    Directory = 1,
}
#[derive(Debug)]
pub(crate) struct Record {
    pub kind: i64,
    pub key: Key,
    pub status: EntryStatus,
}
pub(crate) struct DirectoryRecord {
    pub status: EntryStatus,
    pub object: Key,
}
pub(crate) struct Metadata {
    pub(crate) connection: Mutex<Connection>,
}
impl Metadata {
    pub(crate) fn status(&self, kind: i64, key: &Key) -> Result<Option<EntryStatus>> {
        let db = self.connection.lock();
        let value: Option<i64> = db
            .prepare_cached("SELECT state FROM entries WHERE kind=?1 AND key=?2")?
            .query_row(params![kind, key.0.as_slice()], |r| r.get(0))
            .optional()?;
        value.map(EntryStatus::decode).transpose()
    }
    pub(crate) fn directory(&self, key: &Key) -> Result<Option<DirectoryRecord>> {
        let row: Option<(i64, Vec<u8>)> = self
            .connection
            .lock()
            .prepare_cached("SELECT state,object_key FROM entries WHERE kind=1 AND key=?1")?
            .query_row([key.0.as_slice()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        row.map(|(status, bytes)| {
            Ok(DirectoryRecord {
                status: EntryStatus::decode(status)?,
                object: Key(bytes.try_into().map_err(|_| Error::InvalidKey)?),
            })
        })
        .transpose()
    }
    pub(crate) fn blob(&self, key: &Key, now: i64) -> Result<Option<Vec<u8>>> {
        let db = self.connection.lock();
        let row: Option<(Vec<u8>, i64)> = db
            .prepare_cached(
                "SELECT payload,accessed FROM entries WHERE kind=0 AND key=?1 AND state=2",
            )?
            .query_row([key.0.as_slice()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((bytes, accessed)) = row else {
            return Ok(None);
        };
        if accessed < now {
            Self::touch_locked(&db, BLOB, key, now)?;
        }
        Ok(Some(bytes))
    }
    pub(crate) fn touch(&self, kind: i64, key: &Key, now: i64) -> Result<bool> {
        let db = self.connection.lock();
        let accessed: Option<i64> = db
            .prepare_cached("SELECT accessed FROM entries WHERE kind=?1 AND key=?2 AND state=2")?
            .query_row(params![kind, key.0.as_slice()], |r| r.get(0))
            .optional()?;
        let Some(accessed) = accessed else {
            return Ok(false);
        };
        if accessed < now {
            return Self::touch_locked(&db, kind, key, now);
        }
        Ok(true)
    }
    fn touch_locked(db: &Connection, kind: i64, key: &Key, now: i64) -> Result<bool> {
        // The predicate also avoids needless index writes when another process
        // persisted the same second between our SELECT and UPDATE.
        db.prepare_cached(
            "UPDATE entries SET accessed=?1 WHERE kind=?2 AND key=?3 AND state=2 AND accessed<?1",
        )?
        .execute(params![now, kind, key.0.as_slice()])?;
        Ok(db
            .prepare_cached("SELECT 1 FROM entries WHERE kind=?1 AND key=?2 AND state=2")?
            .query_row(params![kind, key.0.as_slice()], |_| Ok(()))
            .optional()?
            .is_some())
    }
    pub(crate) fn insert_blob(&self, key: &Key, bytes: &[u8], now: i64) -> Result<()> {
        self.connection.lock().prepare_cached(
            "INSERT INTO entries(kind,key,state,bytes,accessed,payload) VALUES(0,?1,2,?2,?3,?4)")?
            .execute(params![key.0.as_slice(), bytes.len() as i64, now, bytes])?;
        Ok(())
    }
    pub(crate) fn begin_build(&self, key: &Key, object: &Key, now: i64) -> Result<()> {
        self.connection
            .lock()
            .prepare_cached(
                "INSERT INTO entries(kind,key,state,bytes,accessed,object_key) VALUES(1,?1,1,0,?2,?3)",
            )?
            .execute(params![key.0.as_slice(), now, object.0.as_slice()])?;
        Ok(())
    }
    pub(crate) fn publish_directory(&self, key: &Key, bytes: u64, now: i64) -> Result<()> {
        let bytes = i64::try_from(bytes).map_err(|_| Error::SizeOverflow)?;
        let n = self.connection.lock().prepare_cached(
            "UPDATE entries SET state=2, bytes=?1, accessed=?2 WHERE kind=1 AND key=?3 AND state=1")?
            .execute(params![bytes, now, key.0.as_slice()])?;
        if n != 1 {
            return Err(Error::LostBuild);
        }
        Ok(())
    }
    pub(crate) fn mark_pruning(&self, key: &Key, deadline: Option<i64>) -> Result<bool> {
        let n = if let Some(deadline) = deadline {
            self.connection.lock().prepare_cached(
                "UPDATE entries SET state=3 WHERE kind=1 AND key=?1 AND state=2 AND accessed<?2")?
                .execute(params![key.0.as_slice(), deadline])?
        } else {
            self.connection
                .lock()
                .prepare_cached("UPDATE entries SET state=3 WHERE kind=1 AND key=?1")?
                .execute([key.0.as_slice()])?
        };
        Ok(n == 1)
    }
    pub(crate) fn forget_directory(&self, key: &Key) -> Result<()> {
        self.connection
            .lock()
            .prepare_cached("DELETE FROM entries WHERE kind=1 AND key=?1 AND state=3")?
            .execute([key.0.as_slice()])?;
        Ok(())
    }
    pub(crate) fn prune_blob(&self, key: &Key, deadline: i64) -> Result<bool> {
        Ok(self
            .connection
            .lock()
            .prepare_cached("DELETE FROM entries WHERE kind=0 AND key=?1 AND accessed<?2")?
            .execute(params![key.0.as_slice(), deadline])?
            == 1)
    }
    pub(crate) fn candidates(&self, deadline: i64) -> Result<Vec<Record>> {
        self.records(
            "SELECT kind,key,state FROM entries WHERE state=2 AND accessed<?1 ORDER BY accessed",
            Some(deadline),
        )
    }
    pub(crate) fn directories(&self) -> Result<Vec<Record>> {
        self.records("SELECT kind,key,state FROM entries WHERE kind=1", None)
    }
    fn records(&self, sql: &str, deadline: Option<i64>) -> Result<Vec<Record>> {
        let db = self.connection.lock();
        let mut stmt = db.prepare_cached(sql)?;
        let mut rows = match deadline {
            Some(t) => stmt.query([t])?,
            None => stmt.query([])?,
        };
        let mut records = Vec::new();
        while let Some(r) = rows.next()? {
            let bytes: Vec<u8> = r.get(1)?;
            let key = bytes.try_into().map_err(|_| Error::InvalidKey)?;
            records.push(Record {
                kind: r.get(0)?,
                key: Key(key),
                status: EntryStatus::decode(r.get(2)?)?,
            });
        }
        Ok(records)
    }
    pub(crate) fn directory_bytes(&self) -> Result<u64> {
        let bytes: i64 = self.connection.lock().query_row(
            "SELECT coalesce(sum(bytes),0) FROM entries WHERE kind=1",
            [],
            |r| r.get(0),
        )?;
        u64::try_from(bytes).map_err(|_| Error::NegativeSize(bytes))
    }
}
impl Metadata {
    pub(crate) fn checkpoint(&self) -> Result<bool> {
        let busy: i64 =
            self.connection
                .lock()
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        Ok(busy == 0)
    }
}
