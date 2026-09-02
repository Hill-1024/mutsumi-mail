# ADR-0001：模块化单体与 IPC 边界

## 状态

已采纳。

## 决策

使用一个 Tauri v2 进程承载 Rust domain/application/storage/backends/sync，React 通过类型化 IPC 调用服务。暂不拆微服务或 localhost HTTP。

## 原因

邮件同步、SQLite 事务、凭据与任务取消需要共享生命周期；模块边界比进程边界更适合 P0，未来仍可把稳定模块拆成 crate。
