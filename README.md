# Mutsumi Mail

Mutsumi Mail 是一个 Rust + Tauri v2 + React/Vite 的离线优先桌面邮件客户端骨架。邮件元数据、草稿、发件队列和同步游标由 Rust/SQLite 管理；前端只通过类型化 IPC 读取或提交意图。

## 当前实现

- Material Design 3 token 层：深色/浅色主题、响应式桌面三栏与紧凑单栏布局。
- Provider registry：QQ、网易 163、Generic IMAP/SMTP、Cloudflare Email Sending outbound-only preset。
- Rust `IncomingMailBackend` / `OutgoingMailBackend` trait；QQ/163 的 IMAPS/SMTPS 配置集中管理。
- SQLite migration：accounts、mailboxes、messages、instances、drafts、outbox、pending operations、sync cursors、FTS5。
- 系统 keyring-backed `SecretStore` 抽象；SQLite 只保留 `secret_ref`。
- 本地虚拟化邮件列表、安全文本阅读模式、草稿自动保存、持久化 outbox 队列 UI；本地 FTS5 搜索 command 与 SQLite 触发器已就位。
- IMAP TLS/CAPABILITY/LOGIN 探测（IMAPS 993 与 STARTTLS 143）与 SMTP TLS 连接测试代码；纯文本 MIME 发件会进入可持久化 outbox worker，真实账号仍需本机手工 smoke test。

## 运行

```bash
pnpm install --frozen-lockfile
pnpm dev                 # 浏览器设计/交互预览（无 Tauri 时使用明确的本地演示数据）
pnpm tauri:dev           # 桌面运行，需要本机 Tauri 系统依赖
```

检查命令：

```bash
pnpm lint
pnpm typecheck
pnpm test
pnpm build
(cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
```

## QQ / 163 设置

在 QQ 邮箱或 163 网页设置里先开启 IMAP/SMTP 并生成客户端授权码。添加向导里填写完整邮箱地址和客户端授权码，不要填写网页登录密码。收件与发件连接会分别测试：

- QQ：`imap.qq.com:993`（IMAPS）、`smtp.qq.com:465`（SMTPS）
- 163：`imap.163.com:993`（IMAPS）、`smtp.163.com:465`（SMTPS）

当前环境未提供真实授权码，因此不能声称 QQ/163 已完成真实收发验证。协议 smoke test 应使用本机凭据或测试服务器，并检查重启后离线读取、Sent 副本和重复同步。附件 MIME、Sent APPEND 和完整增量 FETCH 仍在后续切片。

Cloudflare Email Sending 为 outbound-only：向导提供“只配置 Cloudflare 发件”入口，可填写自定义发件 identity。当前官方文档的 SMTP endpoint 是 `smtp.mx.cloudflare.net:465`、隐式 TLS，用户名固定为 `api_token`，密码为具备 Email Sending: Edit 权限的 API Token。它不提供收件文件夹。

## 文档

- [实施主计划](docs/MASTER_PLAN.md)
- [架构基线](docs/ARCHITECTURE.md)
- [Provider 能力矩阵](docs/PROVIDERS.md)
- [SQLite 设计](docs/DATABASE.md)
- [安全模型](docs/SECURITY.md)
- [测试策略](docs/TESTING.md)

## `unsafe` 说明

本实现未在 `src-tauri/src` 中添加 Rust `unsafe`。当前协议、SQLite、keyring 和 Tauri 路径都不需要绕过 Rust 安全检查，因此没有使用 `unsafe`；依赖库自身的内部实现不属于本项目源码。
