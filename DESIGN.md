# Mutsumi Mail — DESIGN.md（设计系统契约）

本文件是前端实现契约：所有颜色、字号、间距、组件形态、动效都必须能回溯到本文的 token。需要新 token 时先在此登记，再写代码。

## 0. Research Log

- **参考方向（用户指定）**：MikuBox for Android（HatsuneMikuUwU）"Miku UI"。实际查看其 GitHub 截图 3 张（浅色 2、深色 1），提炼语法：首页大圆角角色横幅卡 + 时间问候语（"Hi, miku" / "Good night…"）、超大圆角表面、底部圆角状态面板、贴纸式角色立绘、时间感知问候。
- **强制标准（用户指定）**：Material Design 3 规范 —— 颜色角色、表面容器色阶（tonal elevation）、形状比例、状态层（hover 8% / focus 12% / press 12%）、类型标尺、强调动效曲线。
- **角色主题素材（用户指定：禁 AI 图）**：若叶睦官方立绘取自动画《BanG Dream! It's MyGO!!!!!》官网角色页（透明背景 PNG，950×1439），本地处理为 `mutsumi-hero.webp`（1200px 高、保留 alpha）与 `mutsumi-avatar.webp`（96px 头部裁切）。来源与版权见 `public/themes/mutsumi/SOURCES.md`。
- 未运行 imagen 草稿（用户明确禁止 AI 生成图片）。

## 1. 设计方向（先立意，后取值）

**氛围**：安静、克制的 MD3 办公 surfaces；睦头模式下在首页展开一块"角色横幅 + 问候"的呼吸区，像 MikuBox 一样让角色成为回家的仪式感，其余界面保持严格 MD3 的秩序。

**签名材料**：睦头模式 hero = 官方透明立绘（贴纸感）+ 由 `primary-container → surface` 派生的柔和渐变场 + 底部融入 surface 的遮罩。立绘永远右锚定，文字永远在左，两者互不重叠。

**色彩故事**：种子色 `#76885F`（若叶睦发色的灰绿）→ SchemeTonalSpot 派生全部角色；辅助情绪色：tertiary（薰衣草，呼应发饰）、error/success/warning 语义色。

**记忆点**：打开睦头模式的首页 —— 横幅里安静站着的睦 + 当下时段的问候语。

## 2. 颜色 Token

运行时由 `src/lib/theme.ts` 用 `@material/material-color-utilities` 的 `SchemeTonalSpot(seed, dark, 0)` 生成并写入 `--md-sys-color-*`；CSS 中的 `:root` / `[data-theme='light']` 值是种子 `#879A6C`（抹茶）的离线 fallback，两者角色一一对应：

| 角色 | 用途 |
| --- | --- |
| `primary` / `on-primary` / `primary-container` / `on-primary-container` | 品牌动作：FAB、选中态、主按钮 |
| `secondary(-container)` | 激活导航丸、选中列表项 |
| `tertiary(-container)` | 次强调（发送人头像、徽章） |
| `surface` + `surface-container-lowest/low/(default)/high/highest` | 层级主载体（MD3 tonal elevation，优先于阴影） |
| `surface-dim/bright`、`inverse-*` | 特殊场景 |
| `outline` / `outline-variant` | 描边与分隔线（variant 用于非强调线） |
| `error(-container)`、`success`、`warning` | 语义状态（success/warning 为项目自定义扩展角色） |

**状态层（强制）**：hover = `on-surface 8%`、focus = 12%、pressed = 12%；容器类 hover 用 `surface-container` 逐级抬升。禁止与 `#fff`/`#000` 直混。

## 3. 字体与类型标尺

- **拉丁/数字**：本地打包 `public/fonts/manrope-latin.woff2`（可变字重 200–800，`font-display: swap`，离线可用）。
- **CJK**：系统栈 `'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Noto Sans SC'`；`Manrope` 缺字形自动回退，数字/邮箱获得 Manrope 质感。
- 数字场景（计数、时间）一律 `font-variant-numeric: tabular-nums`。
- **类型标尺**（MD3）：`display-small` 36/44、`headline` 24/32、`title-large` 22/28、`title-medium` 16/24、`body-large` 16/24、`body-medium` 14/20、`label-large` 14/20（500）、`label-medium` 12/16。顶栏标题 = `title-large`（weight 500）；阅读器正文 = `body-large` + 行高 1.75。

## 4. 形状与海拔

- 形状 token：`none/4/8/12/16/28/full`。卡片 16（large），对话框 28（extra-large），按钮/chips/FAB-vanue full 或 16，列表项 12（medium）。
- 海拔：MD3 五级阴影 token 仅用于真正悬浮物（FAB、菜单、对话框、抽屉）；表面层级优先用 surface-container 色阶表达，阴影作辅助。

## 5. 组件原语与状态（Section 5 — Primitives）

| 原语 | 规格 | 状态 |
| --- | --- | --- |
| 导航项（丸） | 高 48（rail 中 64）、`full` 丸、激活 `secondary-container` | hover 8% 层 / active 丸 / disabled 38% |
| 图标按钮 | 48×48 触控目标（视觉图标 24）、圆 | hover 8% / active 12% |
| FAB | 桌面 extended 56 高 `primary-container`；移动 56×56 圆角 16 | hover 4 级 / press 2 级 |
| 顶栏 | 64 高 `surface`，`title-large` | 滚动前无阴影；睦头 hero 顶层时透明 |
| 列表行（邮件） | 高 ≥84、圆 12、hover `surface-container`、选中 `secondary-container` | 未读加粗 + 未读点、hover 显操作 |
| Chip / 过滤丸 | 高 32、边 `outline-variant`、选中 `secondary-container` | hover 8% / selected |
| 开关 | 52×32 MD3 标准，thumb 24/16 | on=`primary` |
| 分段控制 | 丸形容器 + `secondary-container` 激活块 | — |
| 对话框 | 28 圆角、`surface-container-high`、头部 24px 边距、动作右对齐 | scrim `scrim` 色角色 32% |
| 卡片 | `surface-container-low` + 16 圆角 + `outline-variant` 描边（低强调） | hover 抬升一级 |
| 输入框 | outlined：`outline-variant` 边、focus `primary` 2px | error=`error` |
| 状态徽标/同步 chip | `full` 丸 + 语义点 | offline=`warning` |

## 6. 动效

- 仅 `transform` / `opacity` / `filter`；时长 `short 150ms / medium 250ms / long 400ms`；缓动 `emphasized cubic-bezier(.2,0,0,1)`、`decelerate`。
- 动效必须有信息目的（状态切换、进入、悬浮确认）；禁止装饰性微动画。
- `prefers-reduced-motion` 全局降级。

## 7. 睦头模式主题规范（`data-special-theme="mutsumi"`）

- **种子**：`#76885F`；开关：`themePalette === 'mutsumi'`（详见 `theme.ts`）。
- **Hero 横幅**（仅首页、未选中邮件时渲染于 topbar 之下）：
  - 结构：`.mutsumi-hero`（渐变场）> `.mutsumi-hero-art`（右锚定官方立绘，`aria-hidden`）+ `.mutsumi-hero-copy`（左侧：时段问候 `mutsumi-hero-greeting` + 日期 `mutsumi-hero-date`）。
  - 渐变场：暗色 `primary-container→surface-container-low`；亮色 `primary-container 60%→surface-bright`；均用 color-mix 从 token 派生，明暗两套都有对应文字色。
  - 立绘高 115%，右 -24px 锚定，底部 `mask-image` 线性渐隐入 surface；移动端高度 200px 并收紧右锚。
  - 问候语：`title-large` 500；日期：`label-medium` `on-surface-variant`。文案随时间变化（5–11 早上好 / 11–13 中午好 / 13–18 下午好 / 18–23 晚上好 / 其他 夜深了）。
- **调色板选择器**：`data-palette-id='mutsumi'` 的 swatch 使用 `mutsumi-avatar.webp` 头像裁切。
- 侧栏顶部叠加 `primary-container` 8% 的同族渐变；列表选中丸不变（严格 MD3）。
- **禁止**：硬编码浅色文字（旧版问题）；粉紫渐变；非 token 圆角。

## 8. 无障碍约束

- 焦点：`:focus-visible` 2px `primary` 外环 + 2px offset；全部触控目标 ≥48×48；32 高 Chip 使用上下各 8px 的触控扩展区域。
- 对比：正文 `on-surface`，辅助 `on-surface-variant`（4.5:1），meta `outline` 仅用于非必要信息。
- hero 为纯装饰 + 短问候，`aria-hidden` 只加在立绘上，问候文字可读。

## 9. 已接受的债务

- Manrope 仅 latin 子集（CJK 走系统字体）。
- Android dynamic color 流程不变；睦头模式在 dynamic color 开启时被禁用（现状保持）。
- Hero 仅出现在 mail 首页；阅读态/其他页保持安静。
- 官方立绘随官方站点改版可能失效 —— 已本地化打包，不依赖远程。

## 10. 交互补充

- 撰写与账户对话框约束 Tab 焦点，关闭后还原焦点。保存/发送期间禁用编辑，发送成功不再接受重复提交。
- 撰写关闭保护使用 `error-container/on-error-container`、16 圆角和 16 内边距；动作采用文本按钮。
- Chip 保留 32px 可视高度，48px 触控范围；相邻行间距至少 16px。

- 账户授权码恢复表单：容器使用 surface-container，24 内边距、16 圆角；输入框高 56、outline 描边、4 圆角；保存后清空输入并展示状态。

- HTML 阅读区域保留发件方排版，独立白色文档画布，高度至少 480px 或视口的 65%，内部可滚动。
