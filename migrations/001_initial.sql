-- Generated from speakmore-design-v1.1.md, section 12.2.
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 3000;

CREATE TABLE models (
  model_id TEXT PRIMARY KEY,
  filename TEXT NOT NULL,
  revision TEXT NOT NULL,
  sha256 TEXT NOT NULL CHECK(length(sha256)=64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
  size_bytes INTEGER NOT NULL CHECK(size_bytes>0),
  install_state TEXT NOT NULL CHECK(install_state IN ('absent','downloading','verifying','installed','invalid')),
  local_path TEXT,
  verified_at INTEGER,
  CHECK(install_state!='installed' OR (local_path IS NOT NULL AND verified_at IS NOT NULL))
);

CREATE TABLE providers (
  provider_id TEXT PRIMARY KEY,
  revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
  kind TEXT NOT NULL CHECK(kind IN ('openai','anthropic','ollama','openai_compatible')),
  endpoint TEXT NOT NULL,
  model_name TEXT NOT NULL,
  credential_ref TEXT,
  request_profile_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(request_profile_json)),
  consent_origin TEXT,
  tested_at INTEGER,
  updated_at INTEGER NOT NULL
);

CREATE TABLE settings (
  id INTEGER PRIMARY KEY CHECK(id=1),
  revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
  active_model_id TEXT REFERENCES models(model_id) ON DELETE SET NULL,
  active_provider_id TEXT REFERENCES providers(provider_id) ON DELETE SET NULL,
  language TEXT NOT NULL DEFAULT 'auto' CHECK(language IN ('auto','zh','en')),
  ui_locale TEXT NOT NULL DEFAULT 'zh-CN' CHECK(ui_locale IN ('zh-CN','en')),
  theme TEXT NOT NULL DEFAULT 'system' CHECK(theme IN ('system','light','dark')),
  motion_policy TEXT NOT NULL DEFAULT 'system' CHECK(motion_policy IN ('system','reduced')),
  sidebar_collapsed INTEGER NOT NULL DEFAULT 0 CHECK(sidebar_collapsed IN (0,1)),
  delivery_mode TEXT NOT NULL DEFAULT 'paste' CHECK(delivery_mode IN ('paste','manual')),
  polish_enabled INTEGER NOT NULL DEFAULT 0 CHECK(polish_enabled IN (0,1)),
  polish_mode TEXT NOT NULL DEFAULT 'standard' CHECK(polish_mode IN ('standard','email','code_comment','custom')),
  custom_prompt TEXT NOT NULL DEFAULT '' CHECK(length(CAST(custom_prompt AS BLOB))<=4096),
  processing_policy TEXT NOT NULL DEFAULT 'local_only' CHECK(processing_policy IN ('local_only','allow_cloud')),
  history_enabled INTEGER NOT NULL DEFAULT 0 CHECK(history_enabled IN (0,1)),
  history_limit INTEGER NOT NULL DEFAULT 500 CHECK(history_limit IN (100,500,1000,5000)),
  history_days INTEGER NOT NULL DEFAULT 30 CHECK(history_days IN (1,7,30)),
  input_device_id TEXT,
  updated_at INTEGER NOT NULL
);
INSERT INTO settings(id,updated_at) VALUES(1,0);

CREATE TABLE shortcuts (
  action TEXT PRIMARY KEY CHECK(action IN ('push_to_talk','cancel')),
  accelerator TEXT NOT NULL UNIQUE,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1))
);

CREATE TABLE app_rules (
  platform TEXT NOT NULL CHECK(platform IN ('windows','macos')),
  app_identity TEXT NOT NULL,
  manual_only INTEGER NOT NULL DEFAULT 1 CHECK(manual_only IN (0,1)),
  history_excluded INTEGER NOT NULL DEFAULT 0 CHECK(history_excluded IN (0,1)),
  PRIMARY KEY(platform,app_identity)
);

CREATE TABLE dictionary_entries (
  entry_id TEXT PRIMARY KEY,
  term TEXT NOT NULL CHECK(length(trim(term))>0 AND length(CAST(term AS BLOB))<=160),
  normalized_key TEXT NOT NULL,
  locale TEXT NOT NULL DEFAULT 'all' CHECK(locale IN ('all','zh','en')),
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
  pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1)),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(normalized_key,locale)
);

CREATE TABLE transcriptions (
  session_id TEXT PRIMARY KEY,
  raw_text TEXT NOT NULL CHECK(length(CAST(raw_text AS BLOB)) BETWEEN 1 AND 16384),
  polished_text TEXT CHECK(polished_text IS NULL OR length(CAST(polished_text AS BLOB)) BETWEEN 1 AND 16384),
  model_id TEXT NOT NULL,
  model_sha256 TEXT NOT NULL,
  dictionary_revision INTEGER NOT NULL CHECK(dictionary_revision>0),
  language TEXT NOT NULL,
  duration_ms INTEGER NOT NULL CHECK(duration_ms BETWEEN 0 AND 300000),
  asr_ms INTEGER NOT NULL CHECK(asr_ms>=0),
  polish_ms INTEGER CHECK(polish_ms IS NULL OR polish_ms>=0),
  polish_outcome TEXT NOT NULL CHECK(polish_outcome IN ('off','applied','fallback','skipped')),
  delivery_status TEXT NOT NULL CHECK(delivery_status IN ('ready_for_copy','copied_only','submitted','verified','uncertain','cancelled','failed')),
  app_identity TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX transcriptions_created ON transcriptions(created_at DESC,session_id);

CREATE TABLE usage_events (
  session_id TEXT PRIMARY KEY,
  quota_day TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX usage_day ON usage_events(quota_day);

CREATE TABLE license (
  id INTEGER PRIMARY KEY CHECK(id=1),
  signed_token TEXT NOT NULL,
  imported_at INTEGER NOT NULL
);

PRAGMA user_version = 1;
