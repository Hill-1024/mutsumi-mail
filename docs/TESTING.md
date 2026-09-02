# 测试策略

## 当前自动化检查

- TypeScript strict、ESLint、Vitest：Provider 识别、搜索过滤、乐观 flag 变化。
- Rust unit tests：Provider domain detection、能力序列化、subject normalization、UIDVALIDITY reset、outbox/pending 状态、纯文本 MIME/密送 envelope 与文件名清洗。
- SQLite migration test：临时数据库执行全部 migration，确认 FTS5 表/触发器存在，并验证缓存行搜索与 flags 投影。

## 协议集成测试规划

Docker Compose 测试服务器覆盖 IMAP/SMTP/POP3、TLS、flags、move、delete、APPEND、IDLE/reconnect 与错误认证。CI 不依赖 QQ/163 真实账号。

## 手工 smoke test

真实凭据只通过本机未纳入 Git 的环境或系统密码库提供。验收顺序：添加 QQ/163 → 分别测试收件与发件 → 同步 Inbox → 打开正文 → 已读/未读 → 回复 → SMTP 发送 → 检查 Sent → 重启读取 → 再次同步不重复插入。当前环境没有真实凭据，因此此项保持未验证。

## 浏览器预览回归（2026-09-02）

通过 Codex in-app browser 访问 `http://localhost:1420/mail`，检查了桌面首屏、账户向导、搜索、主题切换、撰写入队、阅读器返回，以及归档/删除后的列表与未读计数。此前也在 390×844 窄屏检查了导航 rail、添加账户入口和移动阅读器返回；这属于浏览器演示数据验证，不替代 Tauri/真实服务器 smoke test。
