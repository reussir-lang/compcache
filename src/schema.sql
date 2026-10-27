CREATE TABLE entries (
    kind INTEGER NOT NULL CHECK(kind IN (0, 1)),
    key BLOB NOT NULL CHECK(length(key) = 32),
    state INTEGER NOT NULL CHECK(state IN (1, 2, 3)),
    bytes INTEGER NOT NULL CHECK(bytes >= 0),
    accessed INTEGER NOT NULL,
    payload BLOB,
    object_key BLOB CHECK(object_key IS NULL OR length(object_key) = 32),
    PRIMARY KEY(kind, key),
    CHECK((kind = 0 AND state = 2 AND payload IS NOT NULL AND bytes = length(payload) AND object_key IS NULL)
       OR (kind = 1 AND payload IS NULL AND object_key IS NOT NULL))
) WITHOUT ROWID;
CREATE INDEX prune_candidates ON entries(accessed) WHERE state = 2;
PRAGMA user_version = 2;
CREATE TABLE settings (name TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
