ALTER TABLE transcriptions ADD COLUMN polish_reason TEXT
  CHECK (polish_reason IS NULL OR polish_reason IN (
    'pro_required', 'provider_unavailable', 'policy_revoked', 'cancelled',
    'deadline', 'text_too_long', 'needs_review', 'invalid_prompt',
    'network_unavailable', 'response_rejected'
  ));
PRAGMA user_version = 7;
