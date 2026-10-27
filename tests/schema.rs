mod common;
use common::*;
use compcache::Key;

#[test]
fn migrates_v1_directories_without_renaming_or_scanning_them() {
    let root = tempfile::tempdir().unwrap();
    let key = Key::from_parts("runtime", b"legacy");
    let directory = root.path().join("objects").join(key.to_hex());
    std::fs::create_dir_all(&directory).unwrap();
    let bytes = produce(&directory).unwrap();
    let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE entries (
        kind INTEGER NOT NULL, key BLOB NOT NULL, state INTEGER NOT NULL,
        bytes INTEGER NOT NULL, accessed INTEGER NOT NULL, payload BLOB,
        PRIMARY KEY(kind,key)) WITHOUT ROWID;
        CREATE INDEX prune_candidates ON entries(accessed) WHERE state=2;
        CREATE TABLE settings(name TEXT PRIMARY KEY,value BLOB NOT NULL) WITHOUT ROWID;
        PRAGMA user_version=1;",
    )
    .unwrap();
    db.execute(
        "INSERT INTO entries VALUES(1,?1,2,?2,?3,NULL)",
        rusqlite::params![key.as_bytes().as_slice(), bytes as i64, now()],
    )
    .unwrap();
    drop(db);
    let store = open(root.path());
    let lease = store.get_runtime(&key).unwrap().unwrap();
    assert_eq!(lease.path(), std::fs::canonicalize(&directory).unwrap());
    assert_eq!(
        std::fs::read(lease.path().join("lib/runtime.so")).unwrap(),
        b"complete runtime"
    );
    let db = rusqlite::Connection::open(root.path().join("cache.db")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT object_key FROM entries", [], |r| r
            .get::<_, Vec<u8>>(0))
            .unwrap(),
        key.as_bytes()
    );
}
