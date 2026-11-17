CREATE TABLE app_rules_v4 (
  platform TEXT NOT NULL CHECK(platform IN ('windows','macos')),
  app_identity TEXT NOT NULL,
  manual_only INTEGER NOT NULL DEFAULT 1 CHECK(manual_only IN (0,1)),
  history_excluded INTEGER NOT NULL DEFAULT 0 CHECK(history_excluded IN (0,1)),
  display_name TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(platform,app_identity)
);
-- macOS bundle identifiers are case insensitive. Preserve the stricter rule
-- if a pre-feature database contains different spellings of the same app.
INSERT INTO app_rules_v4(platform,app_identity,manual_only,history_excluded)
SELECT platform, CASE WHEN platform='macos' THEN lower(app_identity) ELSE app_identity END,
       max(manual_only), max(history_excluded)
FROM app_rules
GROUP BY platform, CASE WHEN platform='macos' THEN lower(app_identity) ELSE app_identity END;
DROP TABLE app_rules;
ALTER TABLE app_rules_v4 RENAME TO app_rules;
PRAGMA user_version = 4;
