# Joplin RS

用 **Rust + Tauri 2** 重写的 Joplin 桌面客户端，目标是解决 Electron 版内存占用过大的问题。技术路线参照 [Textus](https://github.com/CatVinci-Studio/Textus)：Rust 原生外壳 + 系统 WebView + 轻量 React/TypeScript 前端。

## 为什么

本机实测（2026-11-08，Joplin 与 Joplin RS 同时运行，均为 release 版）：

|                | Joplin (Electron)  | Joplin RS              |
| -------------- | ------------------ | ---------------------- |
| 渲染引擎       | 内置完整 Chromium  | 系统 WebView2          |
| 进程数         | 6                  | 6（1 主 + 5 WebView2） |
| 常驻内存       | **821.7 MB**       | **308.1 MB**           |
| 数据/搜索/渲染 | JS (Node + React)  | **全部 Rust**          |
| 可执行文件大小 | ~200 MB+ 安装目录 | **12.5 MB**            |

**降幅 62.5%**。明细：Rust 主进程 28.4 MB + 5 个 WebView2 子进程 279.7 MB。
debug 构建实测 345 MB（主进程 31.5 MB + 6 个 WebView2 进程）。

*"A window takes about 110 MB, and each further window about 55 MB more."*

## 架构
**关键取舍**：用 WebView 承载 React。纯 Rust GUI（egui / gpui / iced）能把内存再压到 30–60 MB，但需要自研 WYSIWYG 编辑器、文本排版、输入法与无障碍支持，成本极高。本项目采用与 Textus 相同的混合架构，但把比 Textus 更多的逻辑（Markdown 渲染、搜索）下沉到 Rust，因此 WebView 侧更轻。

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

## 进程
Rust 主进程只拉起一个 WebView2 browser 进程，其余 4 个 WebView2 子进程都由 browser 进程派生，应用代码并不能直接创建或关闭它们。

| # | 进程 | 命令行类型 | WS | 作用 |
|---|---|---|---|---|
| 1 | `joplin-rs.exe` | Tauri 宿主 | 36.1 | **Rust 主进程**：创建 Win32 窗口与原生菜单（文件/视图/工具/帮助），注册 27 个 IPC command（`lib.rs:176`），用 rusqlite 读写 `notes.sqlite`、用 reqwest 做 WebDAV 同步（`spawn_blocking` 放进后台线程）、Markdown 渲染/大纲/PDF 导出、数据目录与启动日志、菜单事件 `emit`。**不含浏览器引擎**，所以只有 36MB。 |
| 2 | `msedgewebview2.exe` | 无 `--type`（browser） | 117.3 | **WebView2 浏览器主进程**：唯一由 Rust 主进程直接拉起的进程。管理 profile（`%LOCALAPPDATA%\app.joplinrs.desktop\EBWebView`）、缓存/Cookie/配额、站点隔离与权限、WebView 生命周期，并派生下面 4 个子进程；也是前端 ↔ Rust IPC 的中转枢纽。 |
| 3 | `msedgewebview2.exe` | `--type=renderer` | 150.0 | **渲染进程**：Blink + V8，加载 `tauri://localhost` 的 React 前端，跑所有 TS/JS、DOM/CSS/布局/事件，Milkdown/CodeMirror 编辑器包也在它里面（已 `lazy()` 懒加载）。CDP 实测 V8 堆仅 12MB、919 张笔记卡片 5207 个元素，其余是引擎固有开销。**唯一随前端内容明显增长的进程**。 |
| 4 | `msedgewebview2.exe` | `--type=gpu-process` | 71.2 | **GPU 进程**：D3D 合成、图层光栅化、字体/纹理、WebGL，滚动与窗口动画都经它。私有提交（约 162MB）远大于工作集，因为大量是显存与资源映射。 |
| 5 | `msedgewebview2.exe` | `--type=utility` | 55.1 | **沙箱服务进程**（`network.mojom.NetworkService` / `storage.mojom.StorageService`）：处理 WebView2 自身的资源请求、自定义协议、磁盘缓存与本地存储读写。注意 **WebDAV 同步不走这里**，那是 Rust 主进程用 reqwest 直连，所以同步时它不会变大。 |
| 6 | `msedgewebview2.exe` | `--type=crashpad-handler` | 12.5 | **崩溃处理**：注册异常回调、在崩溃时写 minidump。常驻待命，几乎不耗 CPU，但不会自己退出。 |

## 应用数据目录

**所有运行时数据只放在应用自己的目录里**，不写入原 Joplin profile、项目源码目录或 exe 所在目录。

| 路径（Windows） | 内容 |
|---|---|
| `%APPDATA%\joplin-rs\profile.json` | 本应用库配置（数据库 / 附件路径） |
| `%APPDATA%\joplin-rs\sync.json` | WebDAV 同步配置（含明文密码） |
| `%APPDATA%\joplin-rs\notes.sqlite` | 本地库：首次启动自动创建为**空库**，内容只由 WebDAV 同步写入 |
| `%APPDATA%\joplin-rs\resources\` | 附件二进制：WebDAV 下载的，以及本机插入的图片 |
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
- 笔记、笔记本、标签、已有附件全部由「立即同步」从 WebDAV 下载；附件落在自有 `resources/`，
  读取时也只读这个目录（缺附件时提示先同步，不再回头找 Joplin）。
- 本机粘贴 / 拖入 / 选取的图片同样写入 `resources/` 并登记为 Joplin 资源，
  正文只存 `![](:/<32位ID>)`；同步时先 PUT `.resource/<id>` 二进制，再 PUT `<id>.md` 元数据。
- 新建 / 编辑 / 删除只写本地库；删除采用 Joplin 同款软删除（`deleted_time`），
  再由同步传播到 WebDAV。
- 两端同时改同一篇笔记时**不会互相覆盖**：`updated_time` 较新的一份保留原笔记，另一份另存为
  「标题（冲突副本 时间）」并标记 `is_conflict = 1`，与 Joplin 官方客户端行为一致；
  同步结果面板会列出本次产生了哪些冲突副本。
- 本机 Joplin 客户端可以同时开着，两者互不干扰、互不读取对方目录。
- 想从 Joplin 迁移：两端配置同一个 WebDAV 目标，先在 Joplin 里同步上去，本应用再同步下来。
- 同步完成后会重建搜索索引与附件映射，新下载的笔记立即可搜。首次同步前建议先「同步预演」。

## 已实现

- 笔记本树（层级展开）、标签、全部笔记视图
- 笔记列表（摘要、更新时间）、中文友好的全文搜索
- Markdown 双模式编辑：CodeMirror 6 源码 + Milkdown Crepe 打字机；自动保存（900 ms 防抖）、Ctrl+S
- Rust 侧渲染：字数 / 字符统计、标题大纲（独立预览面板已移除，仍复用 pulldown-cmark）
- Joplin `:/<32位ID>` 附件与内部笔记链接解析；附件按需经 IPC 读取并转为 Blob URL
- 剪贴板粘贴 / 本地文件拖入 / 加号菜单选图，自动创建 Joplin 资源并随 WebDAV 同步上传
- 源码 / 打字机两种视图（Ctrl+E 切换，「视图」菜单可直达）
- 新建笔记 / 新建笔记本 / 重命名 / 软删除
- 关于面板（显示「数据源：WebDAV（唯一来源）」、数据目录与上次同步时间）
- 快捷键：Ctrl+N 新建 · Ctrl+S 保存并同步 · Ctrl+F 搜索 · Ctrl+E 切换源码 / 打字机 · F12 / Ctrl+Shift+I 开发者工具
- 原生菜单栏：文件 / 视图 / 工具 / 帮助；菜单事件经 `menu://action` 转发到前端
- **WebDAV 同步**：同步设置、连接测试、预演（dry-run）、立即同步、自动同步、进度与结果
- 应用自包含数据目录（`paths.rs` 统一路径，附件只来自 WebDAV，支持 `JOPLIN_RS_DATA_DIR` 覆盖）
- **PDF 导出**（右键笔记卡片）：Windows 走 WebView2，macOS 走 WKWebView；见下节
- 两个编辑器均按视图懒加载（Textus 式启动预算）；33 个 Rust 单元测试覆盖渲染、链接改写、
  HTML 消毒、中文搜索、摘要清洗、base64、WebDAV XML、Joplin 同步文件协议、条目类型识别、
  同步引擎、双端冲突判定、数据目录布局与 PDF 数据/捕获尺寸校验

## PDF 导出

右键笔记卡片 →「导出为 PDF」。前端先把整个窗口换成只含笔记内容的打印页（`PrintSheet` + `@media print`），
再让平台 WebView 渲染当前 DOM，所以文字是矢量、可选中、可搜索，不是截图。

| 平台 | 实现 | 输出 |
|---|---|---|
| Windows | `print.rs` → WebView2 `ICoreWebView2_7::PrintToPdf` | A4 分页、20/18 mm 页边距、背景图形、标题书签 |
| macOS | `print_macos.rs` → WKWebView `createPDFWithConfiguration:completionHandler:` | A4 宽、内容完整的长页矢量 PDF，不按 A4 高度分页 |
| Linux | 暂无实现 | 返回「当前平台暂不支持导出 PDF」 |

**macOS 的完整内容捕获**：`WKPDFConfiguration` 默认用 null rect，含义是「当前可见视口」，
而打印页是 `position: fixed` + 内部滚动，所以早期版本只导出一屏、长笔记被从页底截断。
现在前端在导出前给 `body` 加 `pdf-capture`（仅 `@media screen` 生效，不影响 Windows 的打印
媒体），把打印页收窄到 210mm、量出 `.print-sheet` 的完整 `scrollWidth/scrollHeight`，经 IPC
传给 Rust；`print_macos.rs` 用它设置从 `(0, 0)` 起的显式 `rect`。这与 Textus 的离屏
WKWebView 用的是同一个关键点，但省掉了它的分页/切页逻辑。

**`createPDF` 用的是屏幕媒体**：本机实测 PDF 里保留 `@media screen` 内容、不含 `@media print`
内容，也就是说下面那组打印规则在 macOS 上完全不生效，真正决定导出样式的是 `pdf-capture` 这组
屏幕规则。因此它们额外做了两件事：隐藏 portal 到 `<body>` 的 `.context-menu` / `.toast` /
`.modal-backdrop`（`body.printing #root` 只能藏住渲染在 `#root` 里的浮层），以及把打印页的
滚动条宽度清零，让 `clientWidth === scrollWidth === 794`，避免 9px 滚动条造成横向裁切。
Windows 走打印媒体，看不到这组规则，不受影响。

本机 macOS 14.7.1 的 WKWebView 实测：120 段测试页默认只有 1652 字符且缺尾段，显式 rect 后
得到 **794 × 4888 pt、7943 字符、首段到末段齐全**；1320 pt 宽窗口下收窄纸张同样完整，进度气泡
不进 PDF。用**项目真实编译出的 CSS** 加真实 `PrintSheet` 结构复验 150 段（含表格、代码块、
首尾标题）：794 × 5506 pt 单页，第 1/75/150 段与尾部标记全部保留，应用浮层全部不进 PDF。
超长笔记（600 段 / 23608 pt）WebKit 会在 14400 pt 处自行拆成两页，但内容不丢。

剩下的平台差异：`createPDF` 忽略 `@page { size, margin }`，长页不按 A4 高度分页；要纸张尺寸的
分页结果需改用 `printOperationWithPrintInfo:` + `NSPrintSaveJob`。另外 WebKit 不会像 Chromium
那样把 `<a href="#id">` 写成 PDF 的 `/Dests`，所以 `outline::apply` 在 macOS 上是 best-effort：
找不到命名目标就跳过书签，不会让导出失败。

实现位于 `src-tauri/src/commands.rs`（`render_pdf` 的平台分叉 + 共用的书签后处理）与
`print.rs` / `print_macos.rs`。macOS 专属依赖固定在 `Cargo.toml` 的
`[target.'cfg(target_os = "macos")'.dependencies]`，版本与 Tauri/wry 使用的 objc2 保持一致，
否则 `NSData` / `NSError` 等 extern 类型对不上，回调签名无法编译。

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
cargo test --lib          # 30 个单元测试
```

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
