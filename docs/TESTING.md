# 测试策略

## 当前自动化检查

- TypeScript strict、ESLint、Vitest：Provider 识别、搜索过滤、乐观 flag、添加失败不导航、防重复提交、零账户空状态、多账户 From 选择、HTML-only 阅读和回复账户归属。
- Rust unit/protocol tests（当前 94 项）：严格 tagged IMAP 结果、TLS 模式、LIST/FETCH、UIDVALIDITY、IDLE 握手/事件/keepalive、后台监听生命周期、批量增量同步、历史游标、正文懒加载、远端操作、SMTP 成功/失败/不确定结果、稳定 MIME 快照、钥匙串进程缓存和崩溃恢复。
- SQLite migration test：临时数据库执行全部 migration，验证账户唯一性、精确 message instance、账户隔离、同步游标、发件队列 CAS、待上传操作和覆盖发件人/收件人的 FTS5 搜索。

## 协议测试边界

自动化测试使用受控协议 transcript 和本地临时数据库，不依赖 QQ/163 真实账号，也不会把 mock 结果当作公网联通证据。真实服务商、系统钥匙串和网络故障仍由桌面 smoke test 覆盖。

## Android 成品 JNI 检查

CI 检查生成的 Debug APK；Release 在上传前检查经过 R8 混淆的 APK 和 AAB。`scripts/verify-android-jni.py` 使用 Android SDK 的 `dexdump` 读取所有 DEX 中的 native 方法，再用 NDK 的 `llvm-nm` 对照每个 ABI 实际打包的动态库导出符号。当前应用使用按名称解析的 JNI 方法；此检查不支持通过 `RegisterNatives` 动态注册的方法。

```bash
python3 scripts/verify-android-jni.py \
  --dexdump "$ANDROID_HOME/build-tools/36.0.0/dexdump" \
  --nm "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-nm" \
  path/to/app-universal-release.apk path/to/app-universal-release.aab
```

Linux 使用 `prebuilt/linux-x86_64`。缺少任意方法、DEX 或原生库都会返回非零退出码。v0.1.2 的原始 APK 可复现旧 `Keyring$Companion.initializeNdkContext` 在 arm64-v8a、armeabi-v7a 中均无对应导出的问题；切换到本地授权码存储后必须同时移除这段 Kotlin 启动调用。

此检查覆盖原生方法链接，不替代安装最终 Release APK 后的冷启动、界面显示和前后台切换验证。

2026-09-08 在 Android 16 / API 36 arm64 模拟器中安装 v0.1.2 原始 Release APK，复现 `MainActivity.onCreate` 的 `UnsatisfiedLinkError`。移除旧 Keyring 启动调用后，沿用原包两种 ABI 的原生库，使用临时测试签名重新执行 Release 编译、R8、APK/AAB 打包；两种成品均通过 29 个 JNI 方法的双架构检查，arm64 APK 显示零账户收件箱首页，连续 3 次冷启动及一次前后台切换后崩溃日志为空。本次验证不包含正式发行签名、armv7 运行或用户手机实测。

## 手工 smoke test

真实凭据只通过本机未纳入 Git 的环境或系统密码库提供。验收顺序：添加 QQ/163 → 分别测试收件与发件 → 同步 Inbox → 打开正文 → 已读/未读 → 回复 → SMTP 发送 → 检查 Sent → 重启读取 → 再次同步不重复插入。当前环境没有真实凭据，因此此项保持未验证。

## 浏览器预览回归（2026-09-03）

通过 Codex in-app browser 检查了零账户主界面、设置入口、Provider 选择、QQ 错误凭据路径和 Generic 高级服务器表单；错误凭据会停留在凭据页并显示错误，账户列表保持为空。手机窄屏下也验证了导航、表单和返回路径没有横向挤压。浏览器没有任何预填或演示邮件，这不替代 Tauri/真实服务器 smoke test。

## macOS 安装包回归（2026-09-03）

从非增量 release 构建安装到 `/Applications/Mutsumi Mail.app` 后，验证首次打开位于 `/mail` 零账户页，本地数据库 `quick_check` 为 `ok` 且账户数为 0。连续再次打开两次，系统中仍只有一个应用进程；零账户启动没有发起同步或访问旧钥匙串项。
