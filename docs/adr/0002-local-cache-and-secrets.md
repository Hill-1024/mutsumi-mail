# ADR-0002：本地缓存与秘密引用分离

## 状态

已采纳。

## 决策

SQLite 保存可搜索的邮件元数据与 `secret_ref`，凭据通过 `SecretStore` 写入操作系统安全存储（Stronghold/keychain adapter）。数据库不保存 secret 明文，邮件正文第一版不宣称静态加密。

## 原因

离线阅读需要本地数据库；凭据泄露风险与缓存加密需求不同，必须能独立轮换/删除 secret。
