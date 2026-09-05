
-- Earlier versions cached stripped text as HTML and marked it complete.
-- Keep the cached content available offline; refresh it when the reader opens.
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY);
UPDATE messages SET body_cache_state='stale_html'
WHERE body_html_text IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM schema_migrations WHERE version=2);
INSERT OR IGNORE INTO schema_migrations (version) VALUES (2);
COMMIT;
