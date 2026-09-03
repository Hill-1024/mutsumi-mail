# 架构基线

## 分层

```text
React UI ── typed Tauri IPC ── application services ── domain
                                      │
                         storage / sync / MIME / backends
                                      │
                         SQLite + OS SecretStore + network
```

Rust 是唯一业务核心。前端只持有 DTO、UI 状态和查询缓存，不能访问 SQL、邮箱凭据或网络协议。

## 关键边界

- `IncomingMailBackend` 与 `OutgoingMailBackend` 独立，账号可只收不发、只发不收，或分别使用不同端点。
- `ProviderPreset` 只保存配置默认值、说明与能力补丁；协议实现位于 `backends/`，不在组件里散落供应商分支。
- `SyncCursor` 是 tagged enum，IMAP/POP3/Gmail/Graph/JMAP 各自保存服务端游标。
- `Message` 是逻辑邮件；`MessageInstance` 是 mailbox/label 中的远端实例，不能用 Message-ID 作为主键。
- 长同步任务由 supervisor/task 管理，command handler 只创建任务并返回状态。
- `realtime_sync_service` 只为启用的 `automatic` IMAP 账户维持一个 `IDLE` socket；服务器的 EXISTS/FETCH/EXPUNGE 等事件才唤醒 UID 增量同步。IDLE 每 25 分钟在同一 socket 上续约，不进行定时全量拉取；不支持 IDLE 才降级为低频兜底。
- 桌面进程在窗口失焦或最小化时继续保有 listener；Android/iOS 生命周期 suspend 时取消 listener，resume 后重建，遵循移动系统的后台限制。
- `send_draft` 只负责事务性写入草稿和 outbox；独立 Tokio worker 从 SQLite 读取草稿、从 SecretStore 取发件凭据，经 `SmtpOutgoingBackend` 发送完整原始 MIME，并把 queued/sending/sent/failed/outcome_unknown 状态写回数据库。
- IPC 命令集合在 `src-tauri/src/commands/mod.rs` 集中注册；尚未连到协议 worker 的扩展命令统一返回 `capability` 错误，不返回假数据。

## ADR

- [ADR-0001：模块化单体与 IPC 边界](adr/0001-modular-monolith.md)
- [ADR-0002：本地缓存与秘密引用分离](adr/0002-local-cache-and-secrets.md)
