# SQLite 数据库

数据库由 Tauri app data 目录管理，Rust repository 独占连接。启动时执行正式 migration，并设置：

- `PRAGMA foreign_keys = ON`
- `PRAGMA journal_mode = WAL`
- `PRAGMA busy_timeout = 5000`

核心表：accounts、identities、incoming_endpoints、outgoing_endpoints、mailboxes、threads、messages、message_instances、message_addresses、message_parts、attachments、drafts、outbox、pending_operations、sync_cursors、provider_metadata、app_settings、message_fts。

秘密只以 `secret_ref` 形式进入数据库，例如 `account/{id}/incoming`；授权码、密码、OAuth token 不进入 SQLite。

`message_fts` 使用 FTS5 表并由 SQLite 触发器维护 subject、正文与 HTML 提取文本；sender、recipients、附件文件名字段为同步阶段填充预留。查询通过 Rust `search_messages` command 暴露，输入会先转换为安全的词组前缀查询。
