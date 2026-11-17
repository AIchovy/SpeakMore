CREATE TABLE writing_rules (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('app','website')),
  target TEXT NOT NULL,
  display_name TEXT NOT NULL,
  prompt TEXT NOT NULL,
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  UNIQUE(kind,target)
);
PRAGMA user_version=8;
