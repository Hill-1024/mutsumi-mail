# Mutsumi Mail 实施主计划

> 当前阶段：Phase 2（账号、Provider、缓存与离线交互基线）
>
> 真实 QQ/163 账号未提供，因此本仓库只声明协议实现与手工 smoke-test 步骤，不把 mock 结果表述为真实联通证据。

## 验收状态

- [x] Phase 0：仓库检查、架构基线、领域模型、Provider 能力矩阵、安全模型、数据库设计
- [x] Phase 1：React/Vite/Tauri v2 骨架、MD3 token、三栏布局、Rust commands、SQLite migration、统一错误 DTO、CI 基础
- [x] Phase 2：账号与 Provider 系统（preset、向导、凭据存储、独立连接测试代码）
- [ ] Phase 3：QQ vertical slice（IMAP 元数据、按需正文、SMTP、Sent、离线重启）
- [ ] Phase 4：163 与 Generic IMAP/SMTP
- [ ] Phase 5：可靠增量同步（UIDVALIDITY、MODSEQ、IDLE、操作队列）
- [ ] Phase 6：完整 MIME、附件、撰写、HTML 安全渲染
- [x] Phase 7：本地 FTS5/搜索 command、虚拟列表、主题与基础日用体验（高级搜索/通知待补）
- [ ] Phase 8：OAuth PKCE、Gmail、Microsoft Graph
- [ ] Phase 9：POP3、JMAP、移动端准备

## 当前可验证切片

1. `pnpm install --frozen-lockfile` 安装前端依赖。
2. `pnpm lint`、`pnpm typecheck`、`pnpm test`、`pnpm build` 验证 React UI。
3. `cargo fmt --check`、`cargo test` 验证 Rust 领域逻辑与 migration SQL。
4. `pnpm tauri dev` 启动桌面壳（需要本机 Tauri 系统依赖）。
5. 浏览器模式下没有 Tauri IPC 时，UI 使用明确标注的本地演示数据；桌面运行时优先调用 Rust commands。

## 下一步动作

- 将 `ImapIncomingBackend` 的 TLS、CAPABILITY、LIST、增量 FETCH 接到 `SyncEngine`（当前仅完成连接探测）。
- 为 `SmtpOutgoingBackend` 增加附件 MIME、Sent APPEND 与服务端结果确认；当前纯文本 MIME 已由 outbox worker 尝试发送，网络错误会回到 queued，结果不确定会固定为 outcome_unknown。
- 使用 Docker 测试服务器完成 IMAP/SMTP/POP3 集成测试，再进行 QQ/163 手工 smoke test。

当前同步 command 会发出可取消的进度事件用于 UI 验证，但尚未宣称完成真实远端同步；未提供凭据时不会伪造 QQ/163 结果。
