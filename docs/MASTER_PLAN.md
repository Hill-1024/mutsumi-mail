# Mutsumi Mail 实施主计划

> 当前阶段：收发邮件纵向切片已接通，真实账户 smoke test 与高级协议能力待完成
>
> 真实 QQ/163 账号未提供，因此本仓库只声明协议实现与手工 smoke-test 步骤，不把 mock 结果表述为真实联通证据。

## 验收状态

- [x] Phase 0：仓库检查、架构基线、领域模型、Provider 能力矩阵、安全模型、数据库设计
- [x] Phase 1：React/Vite/Tauri v2 骨架、MD3 token、三栏布局、Rust commands、SQLite migration、统一错误 DTO、CI 基础
- [x] Phase 2：账号与 Provider 系统（preset、向导、凭据存储、独立连接测试代码）
- [x] Phase 3：QQ vertical slice（IMAP 元数据、按需正文、SMTP、Sent 对账、离线重启）；真实账号 smoke test 待执行
- [x] Phase 4：163 与 Generic IMAP/SMTP 密码/授权码流程；真实账号 smoke test 待执行
- [ ] Phase 5：UID 增量同步、UIDVALIDITY、分批回填和远端操作队列已完成；CONDSTORE/QRESYNC、IDLE 与周期调度待补
- [ ] Phase 6：纯文本/HTML 文本解析、撰写、回复与线程头已完成；附件和富文本编写待补
- [x] Phase 7：本地 FTS5/搜索 command、虚拟列表、主题与基础日用体验（高级搜索/通知待补）
- [ ] Phase 8：OAuth PKCE、Gmail、Microsoft Graph
- [ ] Phase 9：POP3、JMAP、移动端准备

## 当前可验证切片

1. `pnpm install --frozen-lockfile` 安装前端依赖。
2. `pnpm lint`、`pnpm typecheck`、`pnpm test`、`pnpm build` 验证 React UI。
3. `cargo fmt --check`、`cargo test` 验证 Rust 领域逻辑与 migration SQL。
4. `pnpm tauri dev` 启动桌面壳（需要本机 Tauri 系统依赖）。
5. 浏览器模式只用于空状态和交互回归，不伪造账户或邮件；账户、同步和发送必须在 Tauri 桌面运行时验证。

## 下一步动作

- 使用有效 QQ/163 授权码完成添加、同步、回复、发送、Sent 对账和离线重启的手工 smoke test。
- 增加附件下载/编写、富文本编写，以及明确服务商策略后的 Sent APPEND。
- 增加 CONDSTORE/QRESYNC、IDLE、周期调度与更长历史的后台分段回填。
- 后续再扩展 OAuth PKCE、Gmail API、Microsoft Graph、POP3 和 JMAP。

未提供凭据时不会伪造 QQ/163 成功结果；自动化协议测试和浏览器回归不能替代真实服务商 smoke test。
