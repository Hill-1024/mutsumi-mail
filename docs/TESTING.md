# 测试策略

## 当前自动化检查

- TypeScript strict、ESLint、Vitest：Provider 识别、搜索过滤、乐观 flag、添加失败不导航、防重复提交、零账户空状态、多账户 From 选择、HTML-only 阅读和回复账户归属。
- Rust unit/protocol tests（当前 94 项）：严格 tagged IMAP 结果、TLS 模式、LIST/FETCH、UIDVALIDITY、IDLE 握手/事件/keepalive、后台监听生命周期、批量增量同步、历史游标、正文懒加载、远端操作、SMTP 成功/失败/不确定结果、稳定 MIME 快照、钥匙串进程缓存和崩溃恢复。
- SQLite migration test：临时数据库执行全部 migration，验证账户唯一性、精确 message instance、账户隔离、同步游标、发件队列 CAS、待上传操作和覆盖发件人/收件人的 FTS5 搜索。

## 协议测试边界

自动化测试使用受控协议 transcript 和本地临时数据库，不依赖 QQ/163 真实账号，也不会把 mock 结果当作公网联通证据。真实服务商、系统钥匙串和网络故障仍由桌面 smoke test 覆盖。

## 手工 smoke test

真实凭据只通过本机未纳入 Git 的环境或系统密码库提供。验收顺序：添加 QQ/163 → 分别测试收件与发件 → 同步 Inbox → 打开正文 → 已读/未读 → 回复 → SMTP 发送 → 检查 Sent → 重启读取 → 再次同步不重复插入。当前环境没有真实凭据，因此此项保持未验证。

## 浏览器预览回归（2026-09-03）

通过 Codex in-app browser 检查了零账户主界面、设置入口、Provider 选择、QQ 错误凭据路径和 Generic 高级服务器表单；错误凭据会停留在凭据页并显示错误，账户列表保持为空。手机窄屏下也验证了导航、表单和返回路径没有横向挤压。浏览器没有任何预填或演示邮件，这不替代 Tauri/真实服务器 smoke test。

## macOS 安装包回归（2026-09-03）

从非增量 release 构建安装到 `/Applications/Mutsumi Mail.app` 后，验证首次打开位于 `/mail` 零账户页，本地数据库 `quick_check` 为 `ok` 且账户数为 0。连续再次打开两次，系统中仍只有一个应用进程；零账户启动没有发起同步或访问旧钥匙串项。
