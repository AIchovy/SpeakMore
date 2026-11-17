CREATE TABLE activity_events (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('dictation','captions')),
  created_at INTEGER NOT NULL,
  duration_ms INTEGER CHECK(duration_ms IS NULL OR duration_ms>=0),
  characters INTEGER CHECK(characters IS NULL OR characters>=0),
  mode TEXT,
  translated INTEGER NOT NULL DEFAULT 0 CHECK(translated IN (0,1)),
  saved INTEGER NOT NULL DEFAULT 0 CHECK(saved IN (0,1))
);
CREATE INDEX activity_created_at ON activity_events(created_at);
PRAGMA user_version=9;
