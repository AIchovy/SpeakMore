ALTER TABLE providers ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
UPDATE providers SET display_name = CASE kind
  WHEN 'openai' THEN 'OpenAI' WHEN 'anthropic' THEN 'Anthropic'
  WHEN 'ollama' THEN 'Ollama' ELSE 'AI service' END;
-- Profiles saved before native credential/configuration integration need a new test.
UPDATE providers SET request_profile_json=CASE WHEN kind='openai_compatible'
  THEN json_object('kind',kind,'budget_field','max_tokens','instruction_role','system')
  ELSE json_object('kind',kind) END WHERE request_profile_json='{}';
UPDATE providers SET tested_at=NULL;
UPDATE settings SET polish_enabled=0;
-- Metadata only. Key bytes are never stored in this database.
-- No foreign key: these cleanup records must survive deletion of the provider.
CREATE TABLE provider_credentials_pending (
  credential_ref TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  purpose TEXT NOT NULL CHECK(purpose IN ('candidate','cleanup')),
  created_at INTEGER NOT NULL
);
PRAGMA user_version = 5;
