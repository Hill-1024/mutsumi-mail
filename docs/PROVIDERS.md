# Provider 能力矩阵

| Provider | 收件 | 发件 | 认证 | TLS 默认 | 说明 |
| --- | --- | --- | --- | --- | --- |
| QQ Mail | IMAP | SMTP | 客户端授权码 | IMAPS 993 / SMTPS 465 | 必须先在网页端开启 IMAP/SMTP；使用完整邮箱地址 |
| 网易 163 | IMAP | SMTP | 客户端授权码 | IMAPS 993 / SMTPS 465 | 授权码不是网页登录密码 |
| Generic | 手动 IMAP | 手动 SMTP | 密码或客户端授权码 | 隐式 TLS 或 STARTTLS | 可分别填写收件与发件服务器、用户名和凭据 |
| Cloudflare Email Sending | — | SMTP | API token | SMTPS 465 | registry 中保留的 outbound-only preset，不在标准收件账户入口展示 |
| Gmail | Gmail API（规划） | Gmail API（规划） | OAuth2 PKCE | HTTPS | 不要求 Google 密码 |
| Microsoft 365 | Graph（规划） | Graph（规划） | OAuth2 PKCE | HTTPS | delta query 规划中 |

QQ preset 使用 `imap.qq.com:993` 与 `smtp.qq.com:465`；163 preset 使用 `imap.163.com:993` 与 `smtp.163.com:465`。这些参数集中在 Rust provider registry，UI 不复制 host/port。当前 Generic 流程不宣称支持 POP3、API 发信、OAuth 或 XOAUTH2。
