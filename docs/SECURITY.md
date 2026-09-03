# 安全模型

- 默认只允许 IMAPS/SMTPS/STARTTLS；不接受所有证书，不自动降级明文认证。
- React 不接触密码、授权码、access token 或 refresh token；Rust 通过 `SecretStore` 抽象访问系统安全存储。
- 邮件正文默认视为不可信。当前 UI 只展示 Rust/前端提取的安全文本，不把原始 HTML 插入主应用 DOM。后续 HTML renderer 必须使用 sanitizer + sandbox + 严格 CSP，默认阻止远程图片/CSS。
- 日志使用结构化字段，禁止 Authorization header、secret、完整正文和完整收件人列表。
- 本地邮件缓存第一版不宣称静态加密；凭据保护与邮件缓存保护是两个独立问题。
- 删除账号时停止同步任务、删除对应 secret，并按用户选择清理本地邮件/附件；不删除服务器远端邮件。
- 项目源码没有使用 Rust `unsafe`；若未来引入确有必要的 FFI/系统 API，将单独记录边界、理由和审查结果。

## macOS 本机签名与钥匙串

- 钥匙串授权绑定代码签名的 designated requirement。普通 ad-hoc 签名以构建产物的 CDHash 标识，二进制改变后“始终允许”不会延续。
- `scripts/build-install-macos-local.sh` 生成一次本机自签名开发证书并在后续构建中复用，使 designated requirement 同时约束 bundle identifier 和证书。证书与私钥仅保存在当前用户的应用支持目录，私钥权限为 `0600`，不会进入仓库或系统钥匙串。
- 脚本固定并校验 `rcodesign` 下载的 SHA-256，签名后使用系统 `codesign --verify --deep --strict` 验证，再替换 `/Applications` 中的安装包。
- 该身份只解决本机反复构建时的身份稳定性，不建立系统信任，也不能替代正式发行需要的 Apple Developer ID 与公证。
- 从旧 ad-hoc 构建首次切换到稳定身份时，macOS 仍可能要求对已有钥匙串项目最后授权一次；后续使用同一证书构建不会再次变更应用身份。
