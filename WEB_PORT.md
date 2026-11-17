# LLM Wiki — 自托管 Web 版（单账号 / 单容器）

本分支把 Tauri 桌面应用 **nashsu/llm_wiki v0.6.13** 移植为可以用一个 Docker 容器部署的
Web 应用。目标是**尽量少改上游代码**，方便以后合并上游更新。

## 1. 架构

```
浏览器 (React 19 / Vite "web" 模式构建)
  │   @tauri-apps/* 被 vite alias 替换为 src/web-shims/*
  │   invoke  ──► POST /api/invoke/<cmd>      (JSON 参数，camelCase)
  │   listen  ──► GET  /api/events            (SSE，单连接，按事件名分发)
  │   store   ──► POST /api/store             (服务器端 JSON store)
  │   http    ──► ANY  /api/proxy             (替代 plugin-http，流式转发)
  │   dialog  ──► <input type=file> + POST /api/upload / 服务器端文件夹选择器
  │   asset   ──► GET  /api/asset?path=       (替代 convertFileSrc / asset://)
  ▼
llm-wiki-web (Rust, axum + tokio)               server/
  ├─ 认证中间件（单账号，argon2，HttpOnly 会话 cookie）
  ├─ 命令分发表  server/src/web/invoke.rs  ──►  上游 #[tauri::command] 函数（原样编译）
  ├─ 事件总线：AppHandle::emit → tokio broadcast → SSE
  ├─ 内部 clip server (127.0.0.1:19827，经 /api/clip/* 访问)
  └─ 可选内部 REST API (127.0.0.1:19828，经 /api/local-api/* 访问)
```

### 1.1 后端：复用上游命令，不依赖 Tauri 运行时

关键点：**`server/` 是一个独立 crate，它的 lib target 直接指向
`../src-tauri/src/lib.rs`**，但依赖表里的 `tauri` / `tauri-plugin-opener`
被换成了本仓库的小 shim（`server/shims/`）：

| shim | 作用 |
|---|---|
| `shims/tauri`（crate 名 `tauri`） | 提供 `AppHandle`、`State`、`Manager`（`state/manage/path()`）、`Emitter`（`emit` → broadcast）、`async_runtime`（tokio）、`Error/Result` |
| `shims/tauri-web-macros` | `#[tauri::command]` 的替代：保留原函数，再生成 `__web_cmd_<name>(app, args) -> Future<Result<Value,Value>>`，按 Tauri 规则（camelCase）反序列化参数，注入 `AppHandle`/`State<T>`，同步命令放到 blocking 线程池执行 |
| `shims/tauri-plugin-opener` | `open_path` 等返回“Web 版不可用”错误 |

因此桌面端的 ~35k 行 Rust 命令代码**一行不改**就能在无 GUI、无 WebKitGTK 的环境运行。
对上游文件的改动只有：

* `src-tauri/src/lib.rs`：4 个 `#[cfg(not(feature = "web"))]`（`mod tray`、`close_behavior`、
  `tray_available`、`run()`）+ 一个 `#[cfg(feature = "web")] #[path = "../../server/src/web/mod.rs"] pub mod web;`
* `src-tauri/Cargo.toml`：声明空 feature `web = []`（只为 check-cfg，桌面构建不受影响）
* `vite.config.ts`：仅在 `--mode web` 时加 alias；`package.json` 增加 `build:web` / `dev:web` 脚本

上游新增命令时，需要在 `server/src/web/invoke.rs` 的 `command_table!` 里补一行
（与 lib.rs 的 `generate_handler!` 列表一一对应；漏写只会导致该命令 404，写错名字则编译失败）。

### 1.2 前端 shim（`src/web-shims/`，只在 web 模式打包）

| 模块 | Web 实现 |
|---|---|
| `api/core` | `invoke` → `fetch('/api/invoke/<cmd>')`；错误时像 Tauri 一样 reject 出错误值；`convertFileSrc` → `/api/asset?path=` |
| `api/event` | `listen/once/emit`，共享一个 `EventSource('/api/events')` |
| `api/window` | 空实现（主题、窗口操作均 no-op） |
| `plugin-store` | 存在服务器 `DATA_DIR/app/<name>`（与桌面版同名同格式，Rust 端读取 `app-state.json` 的逻辑照常工作） |
| `plugin-http` | `fetch` → `/api/proxy`：目标 URL 放 `X-Proxy-Url`，原始请求头 base64(JSON) 放 `X-Proxy-Headers`，body 原样转发；响应流式返回（LLM SSE 流式输出可用），真实状态码放 `X-Proxy-Status`，重定向由 shim 逐跳跟随（每跳都经过服务器 SSRF 检查） |
| `plugin-dialog` | `open()`：文件 → 浏览器文件选择框，上传到 `DATA_DIR/uploads/<时间戳>/`，返回服务器路径，后续 `copy_file`/导入流程不变；`open({directory:true})` → 服务器端文件夹浏览器（可新建文件夹，也可“从本机上传文件夹”= `webkitdirectory` 上传）；`save()` → 返回 `DATA_DIR/exports/...` 路径，命令写完后自动触发浏览器下载；`message/ask/confirm` → `alert/confirm` |
| `plugin-opener` | `openUrl` → `window.open`；`openPath` → 新标签页打开 `/api/asset`；`revealItemInDir` → 报错 |
| `plugin-autostart` | no-op |
| `web-ui.ts` | 上传进度条、文件夹选择对话框、右下角“退出登录”按钮；把前端写死的 `http://127.0.0.1:19827/…`（clip server）和 `:19828`（REST API）请求改写到 `/api/clip/…`、`/api/local-api/…` |

### 1.3 认证（单账号）

* 首次访问且无账号 → `/login` 显示“设置密码”（≥ 8 位），argon2id 哈希写入 `DATA_DIR/auth.json`（0600）。
* 之后是登录表单；成功后发 `llmwiki_session` HttpOnly cookie（30 天滑动过期，服务器只存 token 的 SHA-256，`DATA_DIR/sessions.json`，重启不掉线）。
* HTTPS（`X-Forwarded-Proto: https` 或 `LLM_WIKI_SECURE_COOKIE=1`）时 cookie 为 `Secure; SameSite=None; Partitioned`，以便在 Hugging Face Spaces 的 iframe 中也能登录；HTTP 时为 `SameSite=Lax`。
* CSRF：所有非 GET 的 `/api/*` 必须带 `X-LLM-Wiki` 头（跨站表单无法设置）。
* 暴力破解限制：10 分钟内最多 10 次失败；失败响应延迟 0.7 s。
* 整站（SPA 静态文件、`/api/*`、SSE、asset）都在登录之后；未登录访问页面 → 302 到 `/login`，访问 API → 401。
* `GET /logout` 或右下角按钮退出；`POST /api/auth/password {current,new}` 修改密码（会让其它会话失效）。
* 设置 `LLM_WIKI_PASSWORD` 时密码由环境变量固定（不出现设置页面，也不能在网页中改）。

### 1.4 文件系统沙箱

* 所有 invoke 参数中“像路径”的字段（键名以 `path/paths/dir/directory/root` 结尾，或为 `source/destination/target/relTo` 等，递归检查嵌套对象）必须解析到 `DATA_DIR` 内（先词法规范化 `..`，再对已存在的最长前缀 `canonicalize` 防符号链接逃逸），否则 403。
* `/api/asset`、`/api/download`、`/api/upload?dir=`、`/api/fs/*` 同样限制在 `DATA_DIR`。
* 上传文件名逐段清洗（去掉 `..`、绝对路径、控制字符）。
* `/api/asset` 对 HTML/SVG/XML/JS 加 `CSP: sandbox`，防止上传的网页在本站源下执行脚本。
* 进程启动时 `HOME`/`XDG_*` 指向 `DATA_DIR/home`，工作目录设为 `DATA_DIR`，因此所有状态都在同一个卷里。

### 1.5 出站代理的 SSRF 防护

`/api/proxy` 先解析 DNS，若任一地址是回环/私网/链路本地/CGNAT 等则拒绝（`LLM_WIKI_PROXY_ALLOW_PRIVATE=1` 可放开，例如要访问同一 compose 里的 Ollama）；随后把解析结果固定（`resolve_to_addrs`）防 DNS rebinding；服务器端不自动跟随重定向。
注意：嵌入向量（`embedding_fetch`）、Agent 对话、`web_search` 等是**后端命令直接发起**的请求，与桌面版行为一致，不经过这个检查（只有已登录的唯一用户能配置这些地址）。

## 2. 功能状态

**可用（已在沙箱 smoke test 验证的打 ✅）**

* ✅ 设置密码 / 登录 / 会话 / 退出 / 未登录 401
* ✅ 创建项目、打开项目（`create_project` / `open_project`）
* ✅ 上传 `.md`、`.pdf` 作为源文件并 `copy_file` 到项目 `raw/sources`
* ✅ 列目录、读文件（PDF 经 pdfium 抽取文本）、写文件、搜索（`search_project`）
* ✅ SSE 事件流连接（`/api/events`）
* ✅ plugin-store 持久化（`app-state.json`）
* ✅ `/api/proxy` 能把请求送到真实 LLM 服务商（无 key 时得到服务商自己的 401），SSRF 拦截生效
* 其余上游命令（文件历史、向量库 lancedb、文件监控/同步队列、项目导入导出、图片抽取、Agent 流式对话等）都通过同一分发表暴露，代码路径与桌面版相同，但**需要 LLM key 的流程（导入摄取、聊天、深度研究、嵌入）未实测**。

**在 Web 版中禁用 / 降级**

| 功能 | 处理 |
|---|---|
| Claude Code CLI / Codex CLI 传输 | `*_detect` 返回 `installed:false`，`*_spawn` 拒绝（`LLM_WIKI_ALLOW_CLI=1` 且容器里装了 CLI 才放开） |
| AnyTXT 本地全文检索 | 返回“Web 版不可用” |
| 在系统文件管理器中打开/定位（`open_project_folder`、`open_path_in_project`、`revealItemInDir`） | 返回错误提示 |
| 开机自启、关闭行为、托盘、窗口主题 | no-op |
| 浏览器扩展 Web Clipper | 扩展只会连用户本机的 127.0.0.1:19827，无法直接连到远程服务器；clip server 在容器内照常运行（供前端轮询），但扩展剪藏需要用户自行反代，未支持 |
| 内置 REST API（19828）/ MCP | 默认关闭；`LLM_WIKI_ENABLE_API_SERVER=1` 时在容器内 127.0.0.1:19828 启动，并在已登录的 `/api/local-api/*` 下可访问（其自身 token 校验仍然有效） |
| “导出项目”保存位置 | 文件写到 `DATA_DIR/exports/` 后由浏览器下载 |
| “导入文件夹 / 定时导入目录” | 选服务器上的文件夹（在 `DATA_DIR` 内），或用“从本机上传文件夹” |

## 3. 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `PORT` | `7860` | 监听端口 |
| `BIND` | `0.0.0.0` | 监听地址 |
| `DATA_DIR` | `/data` | 唯一的持久化目录：`auth.json`、`sessions.json`、`app/`(store)、`projects/`、`uploads/`(24h 自动清理)、`exports/`、`home/` |
| `DIST_DIR` | `<exe 目录>/dist` 或 `./dist` | 前端构建产物 |
| `RESOURCE_DIR` | exe 所在目录 | 查找 `libpdfium.so` 等资源 |
| `PDFIUM_DYNAMIC_LIB_PATH` | 自动探测 `RESOURCE_DIR/libpdfium.so` | pdfium 动态库路径（Docker 镜像中为 `/app/libpdfium.so`） |
| `LLM_WIKI_PASSWORD` | 未设置 | 固定登录密码 |
| `LLM_WIKI_SECURE_COOKIE` | 未设置 | `1` = 总是发 `Secure` cookie（反代没传 `X-Forwarded-Proto` 时使用） |
| `LLM_WIKI_PROXY_ALLOW_PRIVATE` | `0` | `1` = 允许 `/api/proxy` 访问私网/回环地址 |
| `LLM_WIKI_ALLOW_CLI` | `0` | `1` = 允许 Claude/Codex CLI 命令 |
| `LLM_WIKI_ENABLE_CLIP_SERVER` | `1` | 容器内 clip server |
| `LLM_WIKI_ENABLE_API_SERVER` | `0` | 容器内 REST API (19828) |
| `LLM_WIKI_KEEP_HOME` | `0` | `1` = 不把 `HOME` 改到 `DATA_DIR/home` |
| `MAX_UPLOAD_MB` | `512` | 单次请求体上限 |
| `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` | — | 也可在设置页面配置（与桌面版相同，`set_proxy_env`） |

## 4. 构建与运行

### 4.1 本地开发

```bash
# 前端（web 模式）
npm ci
npm run build:web            # = tsc --build + vite build --mode web → dist/

# 后端
cd server
cargo build --release --bin llm-wiki-web     # 需要 protoc（lancedb）与 C 编译器
DATA_DIR=/tmp/llmwiki DIST_DIR=../dist \
PDFIUM_DYNAMIC_LIB_PATH=../src-tauri/pdfium/libpdfium.so \
  ./target/release/llm-wiki-web
# 打开 http://localhost:7860 → 设置密码
```

开发时也可以 `npm run dev:web`（Vite 1420 端口），但需要自己把 `/api` 反代到 7860（未内置）。

### 4.2 Docker

```bash
docker build -t llm-wiki-web .
docker run -d --name llm-wiki -p 7860:7860 -v llm-wiki-data:/data llm-wiki-web
# 或
docker compose up -d
```

Dockerfile 为多阶段：`node:22` 构建前端 → `rust:1-bookworm`（装 `protobuf-compiler`、cmake、clang）编译
`llm-wiki-web` → 按 `TARGETARCH` 选择并校验 `src-tauri/pdfium/libpdfium.so`（x86_64）或 `libpdfium-arm64.so`
（SHA256SUMS 校验）→ `debian:bookworm-slim` 运行时，uid 1000 用户、`EXPOSE 7860`、`VOLUME /data`、tini、健康检查。
构建机内存紧张时可 `--build-arg BUILD_JOBS=2`。

### 4.3 Hugging Face Spaces

* Space 类型选 Docker，仓库根目录放本项目（Dockerfile 在根目录）。README 头部需 `sdk: docker`、`app_port: 7860`。
* 建议开启 Persistent Storage（挂载到 `/data`），否则重启后数据丢失。
* 可在 Space Secrets 中设置 `LLM_WIKI_PASSWORD`；否则第一个打开页面的人设置密码——**部署后请立即访问并设置**。
* Spaces 通过 HTTPS 反代并带 `X-Forwarded-Proto`，cookie 自动使用 `SameSite=None; Secure; Partitioned`。

## 5. 已知限制

* 单用户、单实例；多个浏览器标签同时操作同一项目时与桌面版多窗口一样没有额外加锁。
* 后台任务（导入队列、Agent 运行）由前端驱动的部分与桌面版一样依赖浏览器标签保持打开。
* `npm run dev:web` 没有内置 `/api` 代理。
* 浏览器文件选择框无法返回“本机路径”，所以所有导入都是“先上传再导入”；超过 `MAX_UPLOAD_MB` 的批次需要分批。
* 上游的单元测试（`cargo test`）仍针对桌面 crate；server crate 只带了路径沙箱的测试。
* 新增上游命令需同步 `command_table!`；新增的 `@tauri-apps/*` 子模块需在 `vite.config.ts` 的 `TAURI_WEB_SHIMS` 与 `src/web-shims/` 中补 shim（否则 web 构建会把真正的 Tauri 模块打进去并在运行时报错）。
