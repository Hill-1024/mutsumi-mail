# 安全模型

- 默认只允许 IMAPS/SMTPS/STARTTLS；不接受所有证书，不自动降级明文认证。
- React 不接触密码、授权码、access token 或 refresh token；Rust 通过 `SecretStore` 抽象访问系统安全存储。
- 邮件正文默认视为不可信。当前 UI 只展示 Rust/前端提取的安全文本，不把原始 HTML 插入主应用 DOM。后续 HTML renderer 必须使用 sanitizer + sandbox + 严格 CSP，默认阻止远程图片/CSS。
- 日志使用结构化字段，禁止 Authorization header、secret、完整正文和完整收件人列表。
- 本地邮件缓存第一版不宣称静态加密；凭据保护与邮件缓存保护是两个独立问题。
- 删除账号时停止同步任务、删除对应 secret，并按用户选择清理本地邮件/附件；不删除服务器远端邮件。
- 项目源码没有使用 Rust `unsafe`；若未来引入确有必要的 FFI/系统 API，将单独记录边界、理由和审查结果。
