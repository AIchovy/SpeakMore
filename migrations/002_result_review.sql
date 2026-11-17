-- Existing results have no retained quality evidence and require review.
ALTER TABLE transcriptions ADD COLUMN needs_review INTEGER NOT NULL DEFAULT 1 CHECK(needs_review IN (0,1));
PRAGMA user_version = 2;
