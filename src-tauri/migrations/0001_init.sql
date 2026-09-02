PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY NOT NULL,
  provider_id TEXT NOT NULL,
  email TEXT NOT NULL,
  display_name TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  sync_policy TEXT NOT NULL DEFAULT 'automatic',
  incoming_endpoint_id TEXT,
  default_outgoing_endpoint_id TEXT,
  incoming_secret_ref TEXT,
  outgoing_secret_ref TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS identities (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  display_name TEXT NOT NULL,
  email TEXT NOT NULL,
  reply_to TEXT,
  signature TEXT,
  is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1))
);

CREATE TABLE IF NOT EXISTS incoming_endpoints (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  protocol TEXT NOT NULL,
  host TEXT NOT NULL,
  port INTEGER NOT NULL,
  tls_mode TEXT NOT NULL,
  auth_method TEXT NOT NULL,
  username TEXT NOT NULL,
  folder_prefix TEXT,
  UNIQUE(account_id)
);

CREATE TABLE IF NOT EXISTS outgoing_endpoints (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  protocol TEXT NOT NULL,
  host TEXT NOT NULL,
  port INTEGER NOT NULL,
  tls_mode TEXT NOT NULL,
  auth_method TEXT NOT NULL,
  username TEXT NOT NULL,
  UNIQUE(account_id)
);

CREATE TABLE IF NOT EXISTS mailboxes (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  remote_id TEXT NOT NULL,
  name TEXT NOT NULL,
  display_name TEXT NOT NULL,
  parent_id TEXT REFERENCES mailboxes(id) ON DELETE SET NULL,
  delimiter TEXT,
  special_role TEXT,
  unread_count INTEGER NOT NULL DEFAULT 0,
  total_count INTEGER NOT NULL DEFAULT 0,
  selectable INTEGER NOT NULL DEFAULT 1 CHECK (selectable IN (0, 1)),
  sync_enabled INTEGER NOT NULL DEFAULT 1 CHECK (sync_enabled IN (0, 1)),
  UNIQUE(account_id, remote_id)
);

CREATE TABLE IF NOT EXISTS threads (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  normalized_subject TEXT NOT NULL,
  last_message_at TEXT,
  message_count INTEGER NOT NULL DEFAULT 0,
  unread_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
  rfc_message_id TEXT,
  subject TEXT NOT NULL DEFAULT '',
  normalized_subject TEXT NOT NULL DEFAULT '',
  sent_at TEXT,
  received_at TEXT,
  preview TEXT NOT NULL DEFAULT '',
  size_bytes INTEGER,
  has_attachment INTEGER NOT NULL DEFAULT 0 CHECK (has_attachment IN (0, 1)),
  body_cache_state TEXT NOT NULL DEFAULT 'metadata',
  headers_json TEXT NOT NULL DEFAULT '{}',
  parse_warning TEXT,
  body_text TEXT,
  body_html_text TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS message_instances (
  id TEXT PRIMARY KEY NOT NULL,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  mailbox_id TEXT NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  remote_locator TEXT NOT NULL,
  uid_validity INTEGER,
  uid INTEGER,
  flags_json TEXT NOT NULL DEFAULT '[]',
  keywords_json TEXT NOT NULL DEFAULT '[]',
  modseq INTEGER,
  is_deleted INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0, 1)),
  last_synced_at TEXT NOT NULL,
  UNIQUE(mailbox_id, remote_locator)
);

CREATE TABLE IF NOT EXISTS message_addresses (
  id TEXT PRIMARY KEY NOT NULL,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('from', 'to', 'cc', 'bcc', 'reply_to')),
  display_name TEXT,
  email TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS message_parts (
  id TEXT PRIMARY KEY NOT NULL,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  parent_id TEXT REFERENCES message_parts(id) ON DELETE CASCADE,
  mime_type TEXT NOT NULL,
  charset TEXT,
  disposition TEXT,
  content_id TEXT,
  remote_part_locator TEXT,
  size_bytes INTEGER,
  transfer_encoding TEXT,
  body_cache_state TEXT NOT NULL DEFAULT 'none'
);

CREATE TABLE IF NOT EXISTS attachments (
  id TEXT PRIMARY KEY NOT NULL,
  message_part_id TEXT NOT NULL REFERENCES message_parts(id) ON DELETE CASCADE,
  filename TEXT,
  sanitized_filename TEXT,
  content_type TEXT NOT NULL,
  content_id TEXT,
  disposition TEXT,
  size_bytes INTEGER,
  transfer_encoding TEXT,
  remote_part_locator TEXT,
  local_cache_path TEXT,
  sha256 TEXT,
  download_state TEXT NOT NULL DEFAULT 'not_downloaded',
  is_pinned INTEGER NOT NULL DEFAULT 0 CHECK (is_pinned IN (0, 1)),
  last_accessed_at TEXT
);

CREATE TABLE IF NOT EXISTS drafts (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  identity_id TEXT REFERENCES identities(id) ON DELETE SET NULL,
  to_json TEXT NOT NULL DEFAULT '[]',
  cc_json TEXT NOT NULL DEFAULT '[]',
  bcc_json TEXT NOT NULL DEFAULT '[]',
  subject TEXT NOT NULL DEFAULT '',
  body_text TEXT NOT NULL DEFAULT '',
  body_html TEXT,
  in_reply_to TEXT,
  references_json TEXT NOT NULL DEFAULT '[]',
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS outbox (
  id TEXT PRIMARY KEY NOT NULL,
  draft_id TEXT REFERENCES drafts(id) ON DELETE SET NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mime_cache_path TEXT,
  state TEXT NOT NULL DEFAULT 'queued',
  retry_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_error_code TEXT,
  last_error_message TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pending_operations (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mailbox_id TEXT REFERENCES mailboxes(id) ON DELETE SET NULL,
  message_instance_id TEXT REFERENCES message_instances(id) ON DELETE CASCADE,
  operation_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'pending',
  retry_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  last_error_code TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_cursors (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  mailbox_id TEXT REFERENCES mailboxes(id) ON DELETE CASCADE,
  backend_kind TEXT NOT NULL,
  cursor_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(account_id, mailbox_id, backend_kind)
);

CREATE TABLE IF NOT EXISTS provider_metadata (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(account_id, key)
);

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
  message_id UNINDEXED,
  subject,
  sender,
  recipients,
  plain_body,
  html_text,
  attachment_filename,
  tokenize = 'unicode61'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_ai AFTER INSERT ON messages BEGIN
  INSERT INTO message_fts(message_id, subject, sender, recipients, plain_body, html_text, attachment_filename)
  VALUES (NEW.id, NEW.subject, '', '', COALESCE(NEW.body_text, ''), COALESCE(NEW.body_html_text, ''), '');
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_au AFTER UPDATE OF subject, body_text, body_html_text ON messages BEGIN
  DELETE FROM message_fts WHERE message_id = OLD.id;
  INSERT INTO message_fts(message_id, subject, sender, recipients, plain_body, html_text, attachment_filename)
  VALUES (NEW.id, NEW.subject, '', '', COALESCE(NEW.body_text, ''), COALESCE(NEW.body_html_text, ''), '');
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_ad AFTER DELETE ON messages BEGIN
  DELETE FROM message_fts WHERE message_id = OLD.id;
END;

CREATE INDEX IF NOT EXISTS idx_mailboxes_account_role ON mailboxes(account_id, special_role);
CREATE INDEX IF NOT EXISTS idx_messages_account_date ON messages(account_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_instances_mailbox_uid ON message_instances(mailbox_id, uid);
CREATE INDEX IF NOT EXISTS idx_instances_message ON message_instances(message_id);
CREATE INDEX IF NOT EXISTS idx_pending_state ON pending_operations(state, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_outbox_state ON outbox(state, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_addresses_message_kind ON message_addresses(message_id, kind, position);
