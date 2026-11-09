# Joplin RS

用 **Rust + Tauri 2** 重写的 Joplin 桌面客户端，目标是解决 Electron 版内存占用过大的问题。技术路线参照 [Textus](https://github.com/CatVinci-Studio/Textus)：Rust 原生外壳 + 系统 WebView + 轻量 React/TypeScript 前端，把重活全部留在 Rust 侧。

## 为什么

本机实测（2026-11-08，Joplin 与 Joplin RS 同时运行，均为 release 版）：

| | Joplin (Electron) | Textus (Tauri 2) | Joplin RS |
|---|---|---|---|
| 渲染引擎 | 内置完整 Chromium | 系统 WebView2 / WKWebView | 系统 WebView2 |
| 进程数 | 6 | 2~3 | 6（1 主 + 5 WebView2） |
| 常驻内存 | **821.7 MB** | ~110 MB（官方数据） | **308.1 MB** |
| 数据/搜索/渲染 | JS (Node + React) | Rust + JS | **全部 Rust** |
| 可执行文件大小 | ~200 MB+ 安装目录 | ~16.5 MB Linux 二进制 | **8.7 MB** |

**降幅 62.5%**。明细：Rust 主进程 28.4 MB + 5 个 WebView2 子进程 279.7 MB。
debug 构建实测 345 MB（主进程 31.5 MB + 6 个 WebView2 进程）。

Textus 的 README 原文：*"A window takes about 110 MB, and each further window about 55 MB more."*

> 为什么还没到 Textus 的 110 MB？Textus 主要在 macOS 的 WKWebView 上测量（渲染进程由系统共享、不计入应用），且做了更激进的懒加载、后台标签页释放与撤销历史落盘。Windows WebView2 的多进程开销本身就是大头。本项目已做的对应优化：CodeMirror 与 Milkdown 两个编辑器都按需加载，首屏 JS 约 250 KB（gzip 79 KB），字数 / 大纲由 Rust 渲染结果提供。继续压缩的空间见「后续路线」。

## 架构

```
┌──────────────────────────── WebView2 (系统提供，不内嵌 Chromium) ────────────────────────────┐
│  React 19 + TypeScript + Vite                                                                 │
│  ├─ 三栏 UI：笔记本树 / 笔记列表 / 编辑器                                                      │
│  ├─ CodeMirror 6：源码模式（彩色语法高亮，懒加载）                                          │
│  ├─ Milkdown Crepe：打字机视图（WYSIWYG，仅改显示名称，懒加载，与源码互斥挂载）              │
│  └─ Rust 渲染结果：只用于字数 / 大纲统计，不加载 markdown-it / KaTeX / Mermaid JS            │
└────────────────────────────────── typed IPC (Tauri commands) ───────────────────────────────┘
┌────────────────────────────────────── Rust 进程 ───────────────────────────────────────────┐
│  db.rs        rusqlite 读写 Joplin 兼容的 SQLite schema                                       │
│  markdown.rs  pulldown-cmark 渲染 + Joplin `:/id` 链接改写 + 大纲 / 字数统计                  │
│  search.rs    内存全文索引（子串匹配，中文友好，无 JS 搜索引擎）                                │
│  commands.rs  IPC 接口、附件读取（base64）、profile 管理                                       │
│  state.rs     工作副本同步、连接生命周期                                                        │
│  sync/        WebDAV 客户端、Joplin `<id>.md` 协议、双向同步引擎、配置存储                      │
│  lib.rs       原生菜单（文件 / 视图 / 工具 / 帮助）、菜单事件、启动日志                         │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

与 Textus 技术栈的对应关系：

| Textus | Joplin RS |
|---|---|
| Tauri 2 (`src-tauri`) | Tauri 2 (`src-tauri`) |
| React + TypeScript + Vite | React + TypeScript + Vite |
| Milkdown (ProseMirror WYSIWYG) | Milkdown Crepe 7 打字机 **+** CodeMirror 6 源码，双模式切换；Rust 渲染只提供字数 / 大纲 |
| `aicompat` 等 Rust crate 承担业务逻辑 | 数据层 / 渲染 / 搜索 / 附件全部 Rust |
| `lazyModule` + startup budget 控内存 | 两个编辑器各自独立 chunk、按需加载，首屏零额外 JS 解析开销 |

**关键取舍**：Textus 也不是"纯 Rust UI"，它用 WebView 承载 React。纯 Rust GUI（egui / gpui / iced）能把内存再压到 30–60 MB，但需要自研 WYSIWYG 编辑器、文本排版、输入法与无障碍支持，成本极高。本项目采用与 Textus 相同的混合架构，但把比 Textus 更多的逻辑（Markdown 渲染、搜索）下沉到 Rust，因此 WebView 侧更轻。

## 应用数据目录

**所有运行时数据只放在应用自己的目录里**，不写入原 Joplin profile、项目源码目录或 exe 所在目录。

| 路径（Windows） | 内容 |
|---|---|
| `%APPDATA%\joplin-rs\profile.json` | 本应用库配置（数据库 / 附件路径） |
| `%APPDATA%\joplin-rs\sync.json` | WebDAV 同步配置（含明文密码） |
| `%APPDATA%\joplin-rs\notes.sqlite` | 本地库：首次启动自动创建为**空库**，内容只由 WebDAV 同步写入 |
| `%APPDATA%\joplin-rs\resources\` | 附件二进制，**只来自 WebDAV 下载** |
| `%APPDATA%\joplin-rs\startup.log` | 启动与同步诊断日志 |

- 非 Windows：`$XDG_DATA_HOME/joplin-rs/`（默认 `~/.local/share/joplin-rs/`）或
  `~/Library/Application Support/joplin-rs/`。
- 设环境变量 `JOPLIN_RS_DATA_DIR` 可把整个数据目录改到别处（便携运行 / 隔离测试），
  「工具 → 同步设置…」底部会显示当前生效的目录。
- Rust 侧唯一路径入口是 `src-tauri/src/paths.rs`；配置、数据库、附件、日志路径全部由它派生，
  业务代码不硬编码 `APPDATA` / `HOME`。
- **数据只有一个来源：WebDAV。** 删掉整个数据目录后再启动，得到的仍是一个空库，
  不会从本机 Joplin profile 复制任何笔记、附件或同步凭据。
- 旧版本 `profile.json` 里若仍指向数据目录之外的数据库 / 附件目录，会在启动时**自动归一化**
  回来；旧工作副本会被**复制**而非移动，不会丢失已保存数据（含同步密码）。

## 数据安全策略

**不读取、也不写入本机 Joplin 的任何文件。数据来源只有 WebDAV。**

- 首次启动（或数据目录被删除后）：在自有目录里创建一个空的 Joplin 兼容库
  （`notes` / `folders` / `resources` / `tags` / `note_tags` / `deleted_items` + 索引），
  日志会写 `empty library created (nothing imported; WebDAV is the only data source)`。
- 笔记、笔记本、标签、附件全部由「立即同步」从 WebDAV 下载；附件落在自有 `resources/`，
  读取时也只读这个目录（缺附件时提示先同步，不再回头找 Joplin）。
- 新建 / 编辑 / 删除只写本地库；删除采用 Joplin 同款软删除（`deleted_time`），
  再由同步传播到 WebDAV。
- 本机 Joplin 客户端可以同时开着，两者互不干扰、互不读取对方目录。
- 想从 Joplin 迁移：两端配置同一个 WebDAV 目标，先在 Joplin 里同步上去，本应用再同步下来。
- 同步完成后会重建搜索索引与附件映射，新下载的笔记立即可搜。首次同步前建议先「同步预演」。

## 已实现

- 笔记本树（层级展开）、标签、全部笔记视图
- 笔记列表（摘要、更新时间）、中文友好的全文搜索
- Markdown 双模式编辑：CodeMirror 6 源码 + Milkdown Crepe 打字机；自动保存（900 ms 防抖）、Ctrl+S
- Rust 侧渲染：字数 / 字符统计、标题大纲（独立预览面板已移除，仍复用 pulldown-cmark）
- Joplin `:/<32位ID>` 附件与内部笔记链接解析；附件按需经 IPC 读取并转为 Blob URL
- 源码 / 打字机两种视图（Ctrl+E 切换，「视图」菜单可直达）
- 新建笔记 / 新建笔记本 / 重命名 / 软删除
- 关于面板（显示「数据源：WebDAV（唯一来源）」、数据目录与上次同步时间）
- 快捷键：Ctrl+N 新建 · Ctrl+S 保存 · Ctrl+F 搜索 · Ctrl+E 切换源码 / 打字机
- 原生菜单栏：文件 / 视图 / 工具 / 帮助；菜单事件经 `menu://action` 转发到前端
- **WebDAV 同步**：同步设置、连接测试、预演（dry-run）、立即同步、自动同步、进度与结果
- 应用自包含数据目录（`paths.rs` 统一路径，附件只来自 WebDAV，支持 `JOPLIN_RS_DATA_DIR` 覆盖）
- 两个编辑器均按视图懒加载（Textus 式启动预算）；22 个 Rust 单元测试覆盖渲染、链接改写、
  HTML 消毒、中文搜索、摘要清洗、base64、WebDAV XML、Joplin 同步文件协议、条目类型识别、
  同步引擎与数据目录布局

## 双编辑模式：打字机 + 源码

Textus 式 Typora 风格编辑由 **Milkdown Crepe 7**（ProseMirror + remark）提供，源码模式仍是
CodeMirror 6。现在只保留「源码」与「打字机」两种视图：编辑区右上角是一个 iOS 风格开关
（关闭 = 打字机，打开 = 源码，默认关闭），「视图」菜单与 Ctrl+E 也可直达。**「打字机」只是显示名称**：它是原来的所见即所得编辑器，编辑与
光标行为完全不变，没有额外的居中滚动或 caret 逻辑。

**架构**：`activeNote.body` 这段 Markdown 是唯一真相。两个编辑器互斥挂载、各自 `lazy()`
按需加载，切换时销毁一个、用当前字符串初始化另一个，不同步光标 / 选区 / 撤销历史。
保存链路与原来完全一致（`onDraftChange` → 900 ms 防抖 → Rust `save_note` → 工作副本 → WebDAV）。

**数据保真（重要）**：Milkdown 用 remark 解析再序列化，会规范化列表符号、空行等排版，
也可能丢失它不认识的构造。为避免把损坏内容写进数据库并同步到手机 / 其它电脑：

- 只有在打字机里**真实产生输入**后才写回；仅切换模式、打开笔记不会改动原文。
- 挂载后立即做一次往返检查（`getMarkdown()` 对比原文）。检测到 **YAML front matter、
  原始 HTML、Joplin `:/id` 链接数量变化、内容显著变短** 时自动设为只读并提示
  「切回源码」，绝不写回。（remark 的转义与列表规范化只会让文本略微变长，属无害差异。）
- 只是排版规范化差异时允许编辑，顶部提示会说明「会规范化 Markdown 排版」。
- `![图片](:/id)` 与 `[笔记](:/id)` 在 ProseMirror 文档里保留原 token，仅在 DOM 层把图片换成
  `read_resource` 返回的 base64、把内链点击接管为应用内跳转，序列化不会把 base64 写进笔记。

**源码模式彩色渲染**：参考 Textus 的 `src/editor/modes/source-highlight.ts`，用
`@codemirror/language` 的 `HighlightStyle` + `@lezer/highlight` 的 tag 给 Markdown token 上色：
标题、加粗、斜体、删除线、引用、列表、链接、URL、行内代码、标记符号各有独立颜色，
围栏代码块还会按语言懒加载对应语法（`codeLanguages`）。所有颜色都读 `styles.css` 的
`--src-*` 变量（带兜底值），换主题只改一处；`basicSetup` 自带的默认高亮作为 fallback。

**打字机（WYSIWYG）样式**：Crepe 的 `theme/classic.css` **只有配色变量**，组件样式
（`position:absolute`、`[data-show=false]{display:none}`、去掉 contenteditable 焦点框）
在 `theme/common/*.css`，必须单独导入。早期版本只导了 classic.css，于是编辑区出现黑色
焦点框，工具栏 / slash 菜单 / 链接气泡 / 块手柄全部裸排进文档流。现在按已启用 feature
逐个导入，并跳过 `latex.css` / `ai.css` / `diff.css`（LaTeX 与 AI 已关闭），避免把整套
KaTeX 字体（约 1 MB）打进包。

**体积与内存**：Milkdown 相关代码是独立 chunk（约 1.08 MB / gzip 342 KB，另有一个 427 KB /
gzip 138 KB 的 ProseMirror + remark 共享 chunk），只在首次切到打字机时加载；
首屏仍约 250 KB / gzip 79 KB，源码模式完全不加载。源码模式为围栏代码块
按语言生成约 100 个按需小 chunk（dist 合计 3.3 MB），只有真正打开对应语言的代码块才会
请求，不占常驻内存。AI 与 LaTeX feature 已关闭。

**已知差异**：打字机由 remark 渲染，字数 / 大纲由 Rust `pulldown-cmark` 计算（不再挂载
独立预览视图）；公式与 Mermaid 暂不支持可视化编辑。

## 界面：深蓝科技主题与新图标

- 配色仍是原来的**浅色底**（同样的亮度、留白与文字对比度），只把橙 / 暖棕这一族颜色整体换成
  深蓝：`--accent #1d4ed8`、`--accent-soft #e3ecfb`、`--bg-active #dde8fa`，中性色同步冷调化
  （`--bg #fdfeff`、`--bg-side #f1f4fa`、`--border #dde4ee`）。`styles.css` 的 `:root`
  集中定义，CodeMirror 与 Milkdown 的 `--crepe-*` / `--src-*` 变量都从这里取值。
- 顶栏整体移除，界面只剩三栏：左侧导航、笔记列表、编辑区。
- 笔记列表头部只有「搜索框 + ＋ 新建笔记」：原先的「＋ 新建笔记」大按钮从侧栏顶部搬到了这里，
  笔记本名与笔记数量那两行文字已删除（当前笔记本由侧栏高亮表示）。
- 侧栏「同步」按钮带一个刷新图标（同步中旋转），**点击直接触发后台同步**，不再弹对话框；
  按钮右侧一条 148px 的窄条滚动显示本次同步的最新里程碑，例如
  `已获取条目：1240/1240。已新建本地条目：3。已完成`（`sync://progress` 事件驱动，
  同一阶段的新消息替换旧消息、最多保留 3 条；文字超出宽度才滚动，**恒速 30px/s 只滚一次**，
  滚到末尾即停住不再循环（`animation-iteration-count: 1` + 按溢出差值算出的位移）；
  右键按钮打开同步设置）。
- 新建笔记本改用应用内输入框（`.prompt-dialog`）。原来的 `window.prompt()` 由 WebView2
  渲染成系统对话框，标题是「tauri.localhost 显示」，既不可控也不好看。
- 右下角状态栏左侧显示保存状态与字数，右侧显示上次同步时间与版本号：
  `上次同步 刚刚 · v0.1.0`，时间格式为 `刚刚 / 12 分钟前 / 3 小时前 / 11月9日 14:20`
  （超过一天转为具体时间，悬停可看完整时间戳），从未同步时显示「尚未同步」；
  相对时间每 60 秒刷新一次，同步完成后立即更新。版本号来自 `CARGO_PKG_VERSION`，
  由 `bootstrap` 的 `version` 字段下发，随 `tauri.conf.json` / `Cargo.toml` 一起升。
- 编辑区标题行右侧以非强调样式（400 / 11.5px / `--text-faint`）显示**笔记所属笔记本**与
  **最后更新时间**（`今天 21:05` / `11月9日 21:05` / `2025年11月9日`，悬停看完整时间戳），
  笔记本名前带一个 📖 图标；「源码模式」开关也从单独一行挪到了这一行的最右端，
  标题行高度因此从 93px 降到 60px（没有标签的笔记不再显示空的标签行）。
  标题与信息垂直居中对齐，笔记本名过长时省略；保存成功后立即把 `updatedTime` 同步到界面，
  不必重新打开笔记。
- **笔记可直接拖到侧栏的笔记本上完成归类**：卡片 `draggable`，拖拽数据用自定义 MIME
  `application/x-joplin-rs-note` 携带笔记 id，笔记本行在 `dragover` 时高亮（`.drop-target`），
  松手调用 `move_note(id, folderId)`：更新 `parent_id` 并 bump `updated_time`，
  这样下一次 WebDAV 同步会把新归属推上去；目标笔记本不存在时直接报错而不是产生孤儿笔记。
  注意 `tauri.conf.json` 里必须 `"dragDropEnabled": false`，否则 Windows 上 WebView2 的
  系统级拖放处理会吞掉页面内的 HTML5 拖拽事件。
- **笔记列表支持右键菜单**（`.context-menu`，渲染到 `<body>`，靠近视口边缘会自动翻转）：
  - **导出为 PDF**：先弹系统保存对话框选路径，然后把整个界面换成一张 A4 排版的
    `PrintSheet`（标题 / 📖 笔记本 / 更新时间 / 标签 / 正文），等附件解码与字体就绪后，
    由 Rust 侧调 WebView2 的 `ICoreWebView2_7::PrintToPdf` 直接把当前 DOM 打成 PDF。
    文字是矢量的（可选中、可搜索），代码块/表格/引用的浅色底也会打印出来
    （`ShouldPrintBackgrounds(true)`）。`window.print()` 只会弹出 Edge 的打印预览，
    Tauri 自带的 `print()` 又只有 macOS 支持，所以才走 COM 接口，见 `src-tauri/src/print.rs`。
    页边距（上下 20mm、左右 18mm）设在 PrintSettings 里而不是 CSS：元素的 padding 只会
    生效一次，用 CSS 做页边距会让第 2 页起紧贴纸边（踩过这个坑）。
  - **删除笔记**：先弹应用内确认框，确认后走 Joplin 自己的墓碑逻辑
    （`deleted_time` + `deleted_items` + `joplin_rs_pending_deletes`），下次同步把删除推到云端，
    需要时仍可在官方客户端恢复。删的是当前打开的笔记时，编辑区自动跳到相邻的一篇。
- `list_notes` 不再偷偷截断（旧实现默认 300 / 前端 500），`limit <= 0` 即「全部」；
  搜索同样不再封顶 120 条。列表计数已从界面上移除，但全量加载仍然是必须的。
- 左侧导航栏使用比笔记列表深一档的表面色（`--bg-rail #e5ecf7`，列表 `#f7f9fd`），
  与主编辑区分出三层层次；栏内悬停 / 选中态单独调过，保证在更深的底色上依然清晰。
- 新图标：`public/app-icon.svg` 为矢量源（深蓝圆角方块 + 折角文档 + 发光光标 + 电路线），
  前端品牌区与 favicon 直接使用；`src-tauri/icons/` 下的多尺寸 PNG 与 `icon.ico` 由该 SVG
  光栅化生成，作为窗口 / exe / 安装包图标。
- **替换图标后必须让构建脚本重跑**：Tauri 把图标编进 Windows 资源（`resource.lib`），
  而 Cargo 只在 build script 的声明输入变化时才重跑它，`icons/` 默认不在监听列表里，于是
  「换了 ico 但 exe 还是旧图标」会一直存在。`src-tauri/build.rs` 现在显式声明
  `cargo:rerun-if-changed=icons` 与 `tauri.conf.json`；若仍看到旧图标，多半是资源管理器 /
  任务栏的图标缓存，按 F5 刷新或把 exe 复制到另一个路径即可确认。

## 菜单栏与 WebDAV 同步

### 入口

| 位置 | 动作 |
|---|---|
| 文件 → 新建笔记 / 退出 | 新建、关闭应用 |
| 视图 → 源码 / 打字机 | 切换两种编辑视图（同编辑区右上角开关与 Ctrl+E；菜单动作 id 仍为 `view_wysiwyg`） |
| 工具 → 立即同步 | 执行双向同步 |
| 工具 → 同步预演（不写入） | 只计算计划，不改本地也不改服务器 |
| 工具 → 同步设置… | 打开设置对话框 |
| 帮助 → 关于 Joplin RS | 关于面板 |
| 左侧导航底部「同步」 | 立即在后台同步（图标 = 刷新）；未配置 WebDAV 时改为打开设置对话框 |
| 左侧导航底部「同步」右键 | 打开同步设置对话框（关于面板改从菜单打开） |

### 首次使用

1. 首次启动会看到一个空库引导页：本应用只从 WebDAV 取数据，点「配置 WebDAV 同步」。
2. 填 WebDAV 地址、用户名、密码（**全部手动填写**，不会从本机 Joplin 读取）。
   配置保存在 `%APPDATA%\joplin-rs\sync.json`，**目前密码为明文**，请勿把该文件外传。
3. 点「测试连接」→「保存」→「预演」。空库首次预演应当是「下载 = 远端条目数、上传 0」。
4. 点「立即同步」，全部笔记与附件会下载到本地。之后可用自动同步（应用内定时器，仅运行时生效）。

### 与官方 Joplin 互通的协议

远端是扁平文件仓库，和官方 Joplin 的 WebDAV 同步目标同一套格式：

```
<sync root>/
├── info.json              sync version 3, e2ee=false（新目标时自动创建，已有则只读校验）
├── <32位ID>.md            每个笔记 / 笔记本 / 标签 / 附件元数据一个文件
└── .resource/<32位ID>     附件二进制
```

`.md` 为三段纯文本：`title`、`body`（仅笔记）、`key: value` 属性块；时间字段序列化为
`YYYY-MM-DDTHH:mm:ss.SSSZ`；`type_` 取 1 note / 2 folder / 4 resource / 5 tag / 6 note_tag。
传输只用到 WebDAV 的 `PROPFIND` / `GET` / `PUT` / `DELETE` / `MKCOL` + Basic Auth
（`reqwest` + rustls，不依赖 OpenSSL）。

同步判定：同一 ID 比较本地与远端的 `updated_time`，新者胜；本地删除写入
`joplin_rs_pending_deletes` 队列并传播到服务器；远端删除只在同步状态证明本地副本
自上次同步后未被修改时才软删除本地；附件二进制独立于 `.md` 元数据补齐，
即使上次只成功上传了元数据也会在后续同步中补传。状态存于工作副本的
`joplin_rs_sync_state` / `joplin_rs_sync_meta` 表。

**不支持的条目类型**：官方 Joplin 还会在同步目录里放笔记修订历史（`type_=13`）等
本应用没有对应模型的条目。这类文件是纯属性块、值里带引号 JSON diff，格式与笔记不同。
引擎在完整解析之前先读取末尾的 `type_`，遇到未实现的类型就跳过并计入结果里的
「忽略 N 个不支持的条目」，**不会报错，也不会下载、修改或删除这些远端文件**。
所以「远端条目 1236 · 本地条目 1180 · 忽略 56」是正常状态，官方客户端仍可正常使用这些文件。

**删除标记（墓碑）**：Joplin 删除笔记时不会移走文件，而是把 `deleted_time` 写成非 0 的
「墓碑」。引擎现在把这类条目视为「远端已不存在」（`SyncItem::is_tombstone()`），不再当作
需要下载的笔记。此前它会被当成新条目反复 apply —— 本地软删除的行被 `deleted_time = 0`
过滤掉，`(None, Some)` 分支于是每次都重新写一遍，表现为**空转同步也报「下载 3」**。
修复后无变化时同步结果是 `下载 0 · 跳过 1180`。

### 限制（重要）

- **不支持端到端加密**：远端 `info.json` 显示 `e2ee.value=true` 时直接拒绝同步，避免写出无法解密的数据。
- 只支持 sync version 3；不支持 Joplin Server / Dropbox / OneDrive / S3。
- 未实现 Joplin 的同步锁、冲突笔记、master key、revision 完整逻辑；修订历史等
  `type_=13` 条目被安全跳过（保留在服务器，不下载也不删除）。
- 采用「新者胜」，同一笔记在两端同时编辑时后写者覆盖先写者，不会生成冲突副本。
- 自动同步是应用内定时器，应用关闭后不运行。
- 首次连接真实服务器前务必先「预演」；完整写入回归只在本地 mock WebDAV 上跑过，
  线上目标目前只做过只读预演（1236 个远端条目、56 个 `type_=13` 忽略、`errors=0`）。

## 验证结果

```
cargo test --lib  →  26 passed; 0 failed
npm run build       →  TypeScript 0 error, 首屏 250 KB / gzip 79 KB
                      WYSIWYG 独立 chunk 1,083 KB / gzip 342 KB（按需加载）
                      dist 3.3 MB / 124 文件（约 100 个为围栏代码块按需语言 chunk）
npm run build:app   →  release exe（内嵌前端资源）
```

本地 mock WebDAV + CDP 端到端回归（不影响真实同步目标）：

```
sync_preview    → 计划上传 1180 条、附件 143（预演不写入）
sync_run #1     → 上传 1181（含 info.json）、附件 143，errors=[]；
                  远端 .md=1180 / .resource=143 / info.json=True
format          → 三段式、type_/id、ISO 时间全部符合 Joplin 格式
sync_run #2     → 注入"手机端"新笔记 + 附件 → 下载 2 条、附件 ↓1，
                  本地 SQLite 出现该笔记，read_resource 读回 PNGDATA
sync_run #3     → 本地删除该笔记 → 远端删除 1，远端 .md 消失，队列清空
sync_run #4     → 再次同步不复活该笔记（downloaded=0）
sync_run #5/6   → 注入后再删除远端笔记 → 本地软删除 1
全部 6 轮 errors=[]；测试后 sync.json 恢复真实配置，同步状态表清空
```

双模式编辑器在 `JOPLIN_RS_DATA_DIR` 隔离目录里用 CDP 做端到端回归（不触碰真实笔记），14 项检查全通过：

```
视图按钮 / Milkdown chunk 按需挂载 / 两个编辑器互斥
Input.insertText 编辑 → 900 ms 后写回工作副本 SQLite
切回源码内容一致 / Ctrl+E 切换 / 12 篇往返抽样 0 篇被误判为只读
含 :/id 的笔记 → 图片解析为 data:image/png，且库中无 base64 泄漏
```

编辑器界面修复后用 release exe + WebView2 CDP 复验（真实笔记，只读检查）：

```
WYSIWYG → .ProseMirror outline=none / border=0 / box-shadow=none（黑框消失）
           toolbar / slash-menu / link-preview / link-edit / block-handle
           全部 position=absolute，未激活时 display:none，visibleWidgets=[]
拖选文本 → toolbar data-show=true、display=flex，浮在选区上方
           （x=537 y=302 w=241 h=44），其余浮层保持隐藏
源码模式 → --src-* 变量全部解析；渲染出 4 种 token 颜色：
           标题 #bf5f31 / 标记符 #a49e93 / 行内代码 #a3461f / 正文 #302e2a
Runtime.exceptionThrown = []
```

「打字机」只是改名（显示文字变化，编辑器逻辑不变）后的回归：

```
开关关闭（默认）→ aria-checked=false、editor-body.mode-wysiwyg、.ProseMirror 挂载（正文 185 字符）
              padding 26px / 140px、outline=none、border=0、toolbar position=absolute
打开开关       → aria-checked=true、mode-source、knob translateX(15px)、CodeMirror 挂载、WYSIWYG 卸载
关掉开关       → 回到 mode-wysiwyg，.ProseMirror 重新挂载
光标移到文末   → .wysiwyg-host scrollTop 保持 0（无居中滚动副作用）
```

深蓝主题改为「浅色底 + 深蓝强调」后的复验（release exe + CDP，真实笔记只读）：

```
表面色   bg #fdfeff / sidebar #f1f4fa / list #f7f9fd / topbar #fdfeff
正文     #1f2733，对 bg 对比度 14.89（原暖色主题 13.43，只升不降）
强调     #1d4ed8，主按钮白字对比度 6.70；侧栏选中 #16368f on #dde8fa = 8.68
视图     ["源码", "打字机"]，active 为白底蓝字
对话框   同步设置 bg #fdfeff / text #1f2733 / 主按钮 #1d4ed8
```

测试目录与临时数据已清理，真实目录核验仍为 919 篇笔记、WebDAV 密码保留。

数据源改为「只有 WebDAV」后的验证（隔离 `JOPLIN_RS_DATA_DIR` + 本地 mock 目标）：

```
空数据目录启动 → notes.sqlite 77 KB；notes=0 folders=0 resources=0 tags=0
                 日志：empty library created (nothing imported; WebDAV is the only data source)
首次界面       → 空库引导页：0 张笔记卡、无 stat-pill，按钮「配置 WebDAV 同步」
同步对话框     → 无「从 Joplin 导入」按钮；提示明确写明只从 WebDAV 读
空库首次同步   → 下载 6 · 上传 0 · 跳过 0 · 附件 ↓1（预置 6 个远端条目全部落地）
同步后界面     → 2 篇笔记 / 云端笔记本 2 / 云端标签 1，状态栏右侧「上次同步 刚刚 · v0.1.0」
数据库         → 2 notes / 1 folder / 1 resource / 1 tag，无任何本机 Joplin 数据
附件           → read_resource 返回 image/png，base64 以 iVBORw0KGgo 开头
```

历史版本曾把 `%USERPROFILE%\.config\joplin-desktop\database.sqlite` 复制成工作副本
（`notes 919 / folders 52 / resources 143`，逐表一致）。该链路已整体删除：
现在删除数据目录只会得到一个空库，不再重新导入。

## 暂未实现（后续路线）

- WebDAV 同步的 E2EE 加解密、同步锁、冲突笔记、master key、revision
- Joplin Server / Dropbox / OneDrive / S3 同步目标
- 后台常驻自动同步（应用关闭后仍运行）、系统凭据里保存密码
- KaTeX 公式、Mermaid 图表的可视化编辑（打字机中按普通文本处理）
- 跨编辑模式保留光标位置、选区与撤销历史；打字机中编辑 raw HTML / front matter
- 插件系统、主题、版本历史、OCR
- 导出 PDF / HTML / JEX
- SQLite FTS5 索引（当前内存索引已足够快，大数据量时可切换）
- 纯 Rust GUI 版本（egui / gpui），进一步降到 ~50 MB

## 构建与运行

环境要求：Rust 1.85+、Node 20+、Windows 需 WebView2 Runtime（Win10/11 一般已内置）。

```bash
cd joplin-rs
npm install
npm run tauri dev          # 开发模式（Vite 提供 localhost:1420）
npm run build:app          # 产出可直接双击的 exe（内嵌前端，不打包安装包）
npm run build:installer    # 产出 msi / exe 安装包
```

产物：`src-tauri/target/release/joplin-rs.exe`。**这个 exe 可以单独复制到任何目录运行**，前端已编译进二进制，不依赖 Node、Vite 或 dist 目录。

> ⚠️ **不要用裸 `cargo build --release` 生成可运行 exe**。Tauri 2 靠 `custom-protocol` feature 区分开发/生产；不带该 feature 时，WebView 会去连 `devUrl`（`http://localhost:1420`），而双击 exe 时 Vite 并未启动，界面就会显示「localhost 拒绝连接」。
> 现在 `lib.rs` 里加了编译期防呆：release 且未启用该 feature 会直接 `compile_error!`，不会再产出这种坏 exe。若坚持用 cargo 命令，必须写成：
> `cd src-tauri && cargo build --release --features custom-protocol`

只做前端调试：

```bash
npm run dev                # http://localhost:1420（IPC 不可用，界面会显示加载状态）
```

Rust 侧单独检查：

```bash
cd src-tauri
cargo check
cargo test --lib          # 26 个单元测试
```

## 故障排查

双击 exe 若出现白屏或「localhost 拒绝连接」，查看 `%APPDATA%\joplin-rs\startup.log`，正常启动大致如下：

```
[2026-11-09T...] app data dir: C:\Users\DM\AppData\Roaming\joplin-rs (JOPLIN_RS_DATA_DIR override=false)
[2026-11-09T...] resources imported into app data dir: 146 files
[2026-11-09T...] startup mode: PRODUCTION / frontend embedded (custom-protocol=true)
[2026-11-09T...] initial webview url: about:blank
[2026-11-08T...] native menu installed (文件 / 视图 / 工具 / 帮助)
[2026-11-08T...] filter self-check: folder '某笔记本' -> 12 notes (sidebar 12, all 919): OK
[2026-11-08T...] frontend IPC bootstrap: ok=true notes=919 source=C:\Users\DM\.config\joplin-desktop
[2026-11-08T...] list_notes IPC: folder=None tag=None rows=500
[2026-11-08T...] list_notes IPC: folder=Some("a1b2...") tag=None rows=12
```

第二行是 setup 阶段窗口刚创建、尚未导航时的值，`about:blank` 属正常；关键看第一行是否为 `PRODUCTION`，以及 `frontend IPC bootstrap` 是否出现（它由前端 JS 通过 IPC 触发，出现即证明内嵌的 `index.html` + JS 已成功加载并执行）。最后两行分别对应「全部笔记」和「点击某笔记本」，可用于确认筛选确实生效。

| 现象 | 含义 / 处理 |
|---|---|
| 第一行是 `DEV / external Vite dev server` | 用了裸 `cargo build --release`，重新执行 `npm run build:app` |
| 只有前两行、缺第三行 | 前端 JS 未执行（JS 报错或内嵌资源缺失），用 `npm run tauri dev` 看控制台 |
| `ok=false` 或 `notes=-1` | 本地库无法打开；把数据目录移走后重启会自动重建空库，再从 WebDAV 同步 |
| 日志出现 `Failed to setup app` | WebView2 Runtime 异常，重装「Microsoft Edge WebView2 Runtime」 |
| 点笔记本后日志仍是 `folder=None` | JS 传了 `folder_id`；Tauri 2 默认要求 camelCase（`folderId` / `tagId` / `parentId`），见 `src/api.ts` |
| `filter self-check: ... MISMATCH` | 本地库数据异常；删掉数据目录重建空库后重新同步 |
| 缺 `native menu installed` | 原生菜单创建失败（多见于非 Windows/Linux 平台或无窗口环境） |
| `app data dir` 指向意外位置 | 检查是否设置了 `JOPLIN_RS_DATA_DIR`；同步设置底部也会显示当前目录 |
| `附件文件不存在: <id>` | 自有 `resources/` 被清空，执行一次 WebDAV 同步补齐（不再回读 Joplin） |

日志文件是追加写入的，每次启动会新增一组，便于对比。同步开始/结束也会写入：

```
[2026-11-08T...] sync start: dry=false url=https://…/ user=note
[2026-11-08T...] sync end: 同步完成：下载 2、上传 0、跳过 1180、…、附件 ↓1 ↑0
```

| 同步报错 | 含义 / 处理 |
|---|---|
| `WebDAV 认证失败 (HTTP 401)` | 密码没填或填错，到同步设置里补一次 |
| `远端启用了端到端加密 (E2EE)` | 该同步目录开了加密，本实现拒绝同步 |
| `远端同步格式版本为 N` | 只支持 sync version 3 |
| `尚未填写 WebDAV 密码` | Joplin 不把密码写进 settings.json，需手动输入 |
| `PROPFIND 网络错误` | 检查 URL（必须以 `http(s)://` 开头、指向同步根目录）、端口、证书；自签证书可勾「允许无效证书」 |
| 远端条目数明显多于本地条目 | 差额通常是 Joplin 修订历史（`type_=13`）；结果里会显示「忽略 N 个不支持的条目」，属正常 |
| 结果里出现「忽略 …」 | 这些条目本应用不实现，已原样保留在服务器；官方 Joplin 仍可读取 |

## 内存对比方法

1. 保持 Joplin 打开，任务管理器记录 `Joplin.exe` 全部进程内存之和。
2. 启动 `src-tauri/target/release/joplin-rs.exe`，记录 `joplin-rs.exe` 与其 WebView2 子进程内存之和。
3. 打开同一篇含图片的笔记，两边分别观察增量。

PowerShell 复测脚本：

```powershell
$joplin = Get-Process -Name Joplin
$rs     = Get-Process -Name joplin-rs
$wv     = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
          Where-Object { $_.CommandLine -match 'joplin-rs' }
$wvMB   = (($wv | ForEach-Object { (Get-Process -Id $_.ProcessId).WorkingSet64 } |
          Measure-Object -Sum).Sum) / 1MB
"Joplin: $([math]::Round((($joplin.WorkingSet64 | Measure-Object -Sum).Sum)/1MB,1)) MB"
"Joplin RS: $([math]::Round((($rs.WorkingSet64 | Measure-Object -Sum).Sum)/1MB + $wvMB,1)) MB"
```

`Cargo.toml` 的 release profile 已按 Textus 的做法调优：`opt-level = "s"`、fat LTO、单 codegen unit、strip symbols。

## 目录

```
joplin-rs/
├── index.html
├── package.json / vite.config.ts / tsconfig.json
├── src/
│   ├── api.ts                 # 类型化 IPC 层
│   ├── types.ts
│   ├── App.tsx
│   ├── styles.css
│   └── components/            # Sidebar / NoteList / MarkdownEditor / WysiwygEditor / NotePreview / SyncDialog
└── src-tauri/
    ├── Cargo.toml / build.rs / tauri.conf.json
    ├── capabilities/default.json
    └── src/
        ├── main.rs / lib.rs    # 应用入口、原生菜单、启动日志、编译期防呆
        ├── paths.rs            # 唯一路径入口：配置 / 数据库 / 附件 / 日志
        ├── db.rs               # SQLite 数据层、空库 schema、同步引擎写入
        ├── markdown.rs         # pulldown-cmark 渲染
        ├── search.rs           # 内存全文索引
        ├── state.rs            # 应用状态与 profile 同步
        ├── commands.rs         # Tauri IPC commands
        ├── sync/mod.rs         # WebDAV 同步配置、IPC 命令（唯一数据入口）
        ├── sync/webdav.rs      # 最小 WebDAV 客户端（PROPFIND/GET/PUT/DELETE/MKCOL）
        ├── sync/protocol.rs    # Joplin `<id>.md` 序列化 / 解析、info.json 校验
        ├── sync/engine.rs      # 双向同步引擎、状态表、进度事件
        ├── model.rs / error.rs
```

## 许可

MIT
