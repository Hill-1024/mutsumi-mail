# Mutsumi Mail

Mutsumi Mail 是一个 Rust + Tauri v2 + React/Vite 的离线优先桌面邮件客户端。邮件元数据、正文缓存、草稿、发件队列和同步游标由 Rust/SQLite 管理；前端只通过类型化 IPC 读取或提交意图。

## 当前实现

- Material Design 3 组件与 token：深色/浅色主题，以及手机、平板、桌面分别适配的单栏、双栏和三栏布局。
- QQ、网易 163 和 Generic IMAP/SMTP 添加流程；只有 IMAP 与 SMTP 都验证成功后才保存账户，失败不会进入主界面或启动同步。
- 多账户独立同步、统一收件箱、单账户范围切换，以及按收件账户回复和按所选账户发信。
- 严格的 IMAP TLS/STARTTLS 会话、文件夹发现、UID 增量同步、历史回填、UIDVALIDITY 处理、正文按需获取和本地操作上传。
- SMTP 发送前校验、持久化发件队列、稳定 Message-ID 与不可变 MIME 快照；结果不确定时不会盲目重发。
- SQLite migration：accounts、mailboxes、messages、instances、drafts、outbox、pending operations、sync cursors、FTS5。
- 系统 keyring-backed `SecretStore` 抽象；SQLite 只保留 `secret_ref`，进程内串行读取并缓存凭据，收发使用同一授权码时只创建一个钥匙串项目。
- 桌面单实例保护；重复打开只聚焦已有窗口，不会启动第二套同步任务或重复请求钥匙串授权。
- 本地虚拟化邮件列表、安全文本阅读、草稿自动保存、可恢复发件队列和覆盖发件人/收件人的 FTS5 搜索。

## 运行

```bash
pnpm install --frozen-lockfile
pnpm dev                 # 浏览器界面预览（账户、同步与发送必须在 Tauri 桌面运行时使用）
pnpm tauri:dev           # 桌面运行，需要本机 Tauri 系统依赖
pnpm tauri:install:macos # 非增量构建、稳定本机签名、安装并清理构建产物
```

macOS 本机安装脚本会在 `~/Library/Application Support/moe.mutsumi.mail/local-signing/` 生成并复用一个仅供本机开发安装使用的自签名身份。这样每次重新构建后的 designated requirement 保持一致，钥匙串的“始终允许”不会因 ad-hoc 签名 CDHash 改变而失效。首次从旧 ad-hoc 构建切换时，已有凭据仍可能各需最后确认一次。正式分发仍应改用 Apple Developer ID 并完成公证。

检查命令：

```bash
pnpm lint
pnpm typecheck
pnpm test
pnpm build
(cd src-tauri && CARGO_INCREMENTAL=0 cargo fmt --check && CARGO_INCREMENTAL=0 cargo clippy --all-targets --all-features -- -D warnings && CARGO_INCREMENTAL=0 cargo test --all-targets --all-features)
```

## QQ / 163 设置

在 QQ 邮箱或 163 网页设置里先开启 IMAP/SMTP 并生成客户端授权码。添加向导里填写完整邮箱地址和客户端授权码，不要填写网页登录密码。收件与发件连接会分别测试：

- QQ：`imap.qq.com:993`（IMAPS）、`smtp.qq.com:465`（SMTPS）
- 163：`imap.163.com:993`（IMAPS）、`smtp.163.com:465`（SMTPS）

仓库和 CI 不保存真实授权码；协议 smoke test 必须在本机使用用户自己的凭据，并检查重启后离线读取、Sent 副本和重复同步。当前版本不支持附件编写、OAuth、POP3/JMAP 或后台 IDLE 推送。

发送成功后会重新同步对应账户的 Sent 文件夹。客户端不会在未确认服务商策略时盲目 APPEND，以免和服务器自动保存产生重复邮件；界面会如实显示 Sent 副本仍待确认或当前不可用。

## 文档

- [实施主计划](docs/MASTER_PLAN.md)
- [架构基线](docs/ARCHITECTURE.md)
- [Provider 能力矩阵](docs/PROVIDERS.md)
- [SQLite 设计](docs/DATABASE.md)
- [安全模型](docs/SECURITY.md)
- [测试策略](docs/TESTING.md)
- [发行与固定签名](docs/RELEASING.md)

## `unsafe` 说明

本实现未在 `src-tauri/src` 中添加 Rust `unsafe`。当前协议、SQLite、keyring 和 Tauri 路径都不需要绕过 Rust 安全检查，因此没有使用 `unsafe`；依赖库自身的内部实现不属于本项目源码。
