# 平台权限

Mutsumi Mail 只在用户启动对应功能时请求系统权限。邮件数据库、索引和缓存位于应用私有目录，不依赖共享存储权限。

| 平台 | 通知 | 文件附件 |
| --- | --- | --- |
| Android | 设置中点击“开启通知”后请求 `POST_NOTIFICATIONS`。 | 清单声明 `MANAGE_EXTERNAL_STORAGE`；设置中的“管理权限”打开系统“所有文件访问权限”页面。 |
| macOS | 设置中点击后由系统处理通知授权。 | 原生文件选择器授权选中的文件；当前发布目标不是 App Sandbox，不添加伪造的 sandbox entitlement。若未来发布到 Mac App Store，需要在专用签名配置中加入 `com.apple.security.files.user-selected.read-write`。 |
| Windows | 设置中点击后由系统通知中心处理。 | 原生文件选择器，无独立存储授权弹窗。 |
| Linux | 设置中点击后由桌面通知服务处理。 | 原生选择器；沙盒发行格式由桌面 portal 授权。 |
| iOS | 设置中点击后由系统处理通知授权。 | 原生文档选择器授予选择文件的访问权；iOS 不提供应用级“全盘访问”权限。 |

即使 Android 已被授予“所有文件访问权限”，前端仍只拥有文件选择器动态加入的读取范围（`fs:read-files`），不会把全盘读取 IPC 暴露给 WebView。选中的附件在发送前构造成不可变 MIME 载荷并写入发件队列，因此重试不需要再次读取原文件。
