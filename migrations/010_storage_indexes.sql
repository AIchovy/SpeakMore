CREATE INDEX usage_created_at ON usage_events(created_at);
CREATE INDEX activity_kind_created_at ON activity_events(kind,created_at);
PRAGMA user_version=10;
