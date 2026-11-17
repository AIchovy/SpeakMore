-- Binds an explicit protocol test to the native configuration and credential revision.
-- No API key bytes or key hashes are stored here.
ALTER TABLE providers ADD COLUMN test_checkpoint TEXT
  CHECK(test_checkpoint IS NULL OR (length(test_checkpoint)=67 AND substr(test_checkpoint,1,3)='v1.' AND substr(test_checkpoint,4) NOT GLOB '*[^0-9a-f]*'));
-- Old timestamps have no durable binding. Keep their display history but require
-- a new explicit test before enabling polishing with these configurations.
UPDATE settings SET polish_enabled=0;
PRAGMA user_version = 6;
