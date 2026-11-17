-- Store only an enumerated fallback reason, never target context or identity.
ALTER TABLE transcriptions ADD COLUMN delivery_reason TEXT
  CHECK(delivery_reason IS NULL OR delivery_reason IN (
    'main_window','user_requested','accessibility_required','unsupported_target',
    'sensitive_target','read_only_target','target_changed','target_unavailable',
    'probe_timeout','needs_review','text_too_long','compatibility_unverified'
  ));
PRAGMA user_version = 3;
