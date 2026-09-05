# 安全模型

- 默认只允许 IMAPS/SMTPS/STARTTLS；不接受所有证书，不自动降级明文认证。
- React 仅在账户表单提交期间持有输入的凭据，不写入浏览器持久存储；Rust 通过 `SecretStore` 抽象访问系统安全存储。
- 邮件正文默认视为不可信。HTML 正文先经过严格标签、属性、URL 与内联样式白名单清洗再渲染；脚本、表单、嵌入对象和事件属性会被移除，默认阻止远程图片，并由 CSP 限制可加载内容。
- 日志使用结构化字段，禁止 Authorization header、secret、完整正文和完整收件人列表。
- 本地邮件缓存第一版不宣称静态加密；凭据保护与邮件缓存保护是两个独立问题。
- 删除账号时停止同步任务、删除对应 secret，并按用户选择清理本地邮件/附件；不删除服务器远端邮件。
- 项目源码没有使用 Rust `unsafe`；若未来引入确有必要的 FFI/系统 API，将单独记录边界、理由和审查结果。

## 统一凭据存储

业务代码只依赖一个 `SecretStore` 接口，不按平台散落读写逻辑，也不提供明文 SQLite 或配置文件兜底。各平台实现如下：

| 平台 | 原生存储 |
| --- | --- |
| macOS | Apple Keychain |
| iOS | Apple Protected Data credential store |
| Android | Android Keystore 保护的加密 SharedPreferences |
| Windows | Windows Credential Manager |
| Linux | Secret Service（例如 GNOME Keyring、KWallet 的兼容服务） |

应用只在添加或更新账号、连接测试、自动同步首次建立会话、发送邮件，以及未缓存正文/附件需要按需下载时读取相应凭据。成功读取和读取失败均在当前进程缓存；同一进程内不会为每个同步阶段重复访问系统存储。安全存储或认证失败会终止该账户的后台实时任务，失败的存储读取在当前进程不重试；成功写入新凭据或重启后才清除失败缓存，避免形成授权弹窗循环。

## macOS 本机签名与钥匙串

- 钥匙串授权绑定代码签名的 designated requirement。普通 ad-hoc 签名以构建产物的 CDHash 标识，二进制改变后“始终允许”不会延续。
- `scripts/build-install-macos-local.sh` 生成一次本机自签名开发证书并在后续构建中复用，使 designated requirement 同时约束 bundle identifier 和证书。证书与私钥仅保存在当前用户的应用支持目录，私钥权限为 `0600`，不会进入仓库或系统钥匙串。
- 脚本固定并校验 `rcodesign` 下载的 SHA-256，签名后使用系统 `codesign --verify --deep --strict` 验证，再替换 `/Applications` 中的安装包。
- 该身份只解决本机反复构建时的身份稳定性，不建立系统信任，也不能替代正式发行需要的 Apple Developer ID 与公证。
- macOS 通过进程生命周期内的 `SecKeychain::disable_user_interaction()` 禁止钥匙串授权弹窗。旧签名、锁定或无权限的项目会返回可用性错误，不触发后台授权申请，也不降级为明文存储。
