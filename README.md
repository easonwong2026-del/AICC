# AICC

[English](README.en.md)

面向 macOS 的 AI 状态中心，显示 Codex、WorkBuddy、DeepSeek 和系统状态。支持 Poke4S 墨水屏显示。服务只依赖 Python 标准库。

当前 macOS 版本为 AICC 2.7.1（Build 10）；Android/Poke4S 使用独立版本体系。

macOS 菜单栏使用固定的 Codex、WorkBuddy、DeepSeek、System 和 OpenCodex 状态卡片，只读取 `/api/status` 及固定操作接口。Python 服务端通过四个固定采集器提供 Codex、WorkBuddy、DeepSeek 和系统状态。

## 使用地址

- Mac：`http://localhost:8765`
- 设置：SwiftUI 原生设置窗口（菜单栏 → Settings）
- Poke4S：`http://<Mac 当前 Wi-Fi 地址>:8765/?kiosk=1`

Mac 与 Poke4S 需要连接同一 Wi-Fi。原生 Android 客户端也可以通过 UDP 自动发现 Mac。

## macOS 启动与维护

首次部署见 [MAC-MIGRATION.md](MAC-MIGRATION.md)。常用操作：

正式用户推荐下载 [GitHub Releases](https://github.com/easonwong2026-del/AICC/releases) 中的 DMG，拖入
`/Applications` 后由 AICC App 通过 `ServerManager` 启动内置 Python Server，并可在设置中使用
`SMAppService` 管理登录启动。这是 macOS 正式用户唯一支持的运行路径。

从旧版本升级时，AICC 首次启动会只针对旧版安装器创建的 3 个精确 LaunchAgent 身份执行一次清理，
卸载旧服务并移除对应 plist 后继续正常启动；清理失败不会让 App 崩溃，也不会扫描或杀掉其他服务/进程。

源码检出仅支持开发调试，不是生产安装方式：

```bash
bash macos/start-dashboard.sh              # DEV ONLY：前台启动源码 Server，不注册后台服务
bash macos/set-deepseek-key.sh             # 更新 DeepSeek 密钥
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh  # 构建自包含 SwiftUI 菜单栏 App（含 Python 服务）
bash macos/build-dmg.sh  # 构建自包含 AICC DMG 安装包
```

WorkBuddy 调试桥自动修复由 AICC Server 内部负责。正式用户通过设置中的“关于与更新”检查公开清单，
再下载并替换正式 DMG；源码目录不提供另一套生产更新或回滚机制。

DMG App 日志位于 `~/Library/Logs/AICC-Dashboard/`。启动内置 Server 前，
`aicc-server.log` 和 `aicc-server-error.log` 超过 1 MiB 时会保留最近 256 KiB；日志限制失败不会阻止 Server 启动。

SwiftUI 菜单栏 App（AICC）会生成在 `dist/mac/AICC.app`。`SERVER_ROOT` 仅用于本地开发/调试构建；
正式 DMG 始终使用 `BUNDLE_SERVER=1`，由 App 管理自带 Server，不指向源码目录。
`build-dmg.sh` 会打包同一套 SwiftUI App，并生成自包含安装包，默认输出到 `dist/`；传入 `RELEASE_DIR` 后可输出到
`/Users/easonwong/AICC/releases/mac/` 这类成品目录。DMG 里的 App 自带服务器代码，
可拖入 `/Applications`。自包含 App 的运行数据写入
`~/Library/Application Support/AICC-Dashboard/data/`，不会写进 App 包本身。
App 运行需要 macOS 14 或更高版本、Apple Silicon，以及可执行的 Python 3.10+；DMG 不包含 Python 解释器。
没有 Developer ID 时生成的是 ad-hoc 测试包，首次在其他 Mac 安装需要右键选择"打开"。
菜单栏 App 只负责状态显示、启动/停止内部数据服务、手动控制 OpenCodex、打开原生设置窗口、
打开日志和配置开机自启；固定额度采集仍由现有 Python 服务完成。内部数据服务监督只访问
`/api/health/live`，状态面板读取 `/api/status` 缓存，不会额外触发 collector 刷新。

### macOS 手动检查更新（可选）

设置 → 通用底部的“关于与更新”只在用户点击“检查更新”后请求一次公开清单，不会后台轮询、自动下载或安装。
发布版 App 已通过 `Info.plist` 配置公开更新清单：
`https://raw.githubusercontent.com/easonwong2026-del/AICC/main/updates/aicc-update.json`。
也可以在启动 App 的环境中设置 `AICC_UPDATE_MANIFEST_URL` 覆盖它。地址和清单中的下载/说明链接都必须使用 HTTPS；
清单缺失或未配置时会显示“更新源尚未配置”，并提供 GitHub Releases 页面入口。清单格式如下：

```json
{
  "version": "2.5.1",
  "build": "5",
  "minimumSystemVersion": "14.0",
  "downloadURL": "https://example.com/AICC-2.5.1.dmg",
  "releaseNotesURL": "https://example.com/aicc/releases/2.5.1",
  "publishedAt": "2026-08-03T00:00:00Z"
}
```

### 发布版本规则

- `VERSION` 是 AICC 产品版本唯一人工源。
- 新用户功能发布：minor 版本号加 1；bugfix 发布：patch 版本号加 1。
- 每个正式 macOS 发布都将 `CFBundleVersion` 加 1；普通开发 commit 不自动 bump 版本。
- 只有对应 GitHub Release 和 DMG 都已真实存在后，才将版本写入 `updates/aicc-update.json`。

### macOS 桌面 Widget

- 系统要求：macOS 14+、Apple Silicon；先安装并启动 AICC App。
- 添加方式：桌面右键 → 编辑 Widget → 搜索 “AICC”，选择小尺寸或中尺寸并添加。
- 小尺寸显示 Codex 和 WorkBuddy；中尺寸采用左右分栏布局显示 Codex（每周额度、进度、重置时间、5小时额度）以及 WorkBuddy（积分）和 DeepSeek（余额）。
- Widget 通过 `http://127.0.0.1:8765/api/status` 读取状态，正式端口固定为 `8765`，不会调用刷新接口或自行启动 Server。
- 点击右上角刷新按钮可手动刷新 Widget 时间线；AICC App 启动及状态变化时也会通知 Widget 更新。
- Server 暂时不可达时保留最近一次成功数据并标记为 stale；首次安装没有缓存时显示占位符 `—`。
- AICC App 负责启动和监督后端；App 重启后会重新加载 Widget 时间线并恢复实时数据。

### 2.4.1 OpenCodex 控制变更

- AICC **不负责打开 Codex Desktop 或 ChatGPT Desktop**。
- AICC 只负责查看 OpenCodex 状态、启动/停止 OpenCodex、打开 OpenCodex 仪表盘。
- OpenCodex 开关反映真实运行时状态（通过 `ocx status --json` CLI 发现），支持动态端口。
- OpenCodex 状态轮询仅在菜单栏面板打开时进行（约 9 秒间隔），面板关闭后停止。
- 不再保留"随 Codex Desktop 启动 OpenCodex"等联动设置。

## 数据采集与内存策略

- Codex：按需启动 `codex app-server`，读取账户额度；面板停止访问 30 秒后结束子进程并使用最后成功缓存。
- WorkBuddy：优先通过 `127.0.0.1:9223` 的本机调试桥读取账户余额；当前 WorkBuddy/CodeBuddy CLI 只提供编码代理能力，不作为账户积分接口。应用关闭时绝不主动启动；发现 WorkBuddy 已运行但未开放本地桥时，AICC 会在后台自动重启它一次以启用读取通道（恢复 2.3.x 的监控行为，每个 WorkBuddy 进程只重启一次，会话自动恢复），也可在设置中手动「重连 WorkBuddy」。启用后默认每 120 秒读取一次余额，同一页面 target 也会按时间重新读取，手动刷新会强制重新读取。本机数据库中的当日用量仍可被动更新，不保存或传输令牌、Cookie 等认证信息。
- DeepSeek：密钥只从环境变量或 macOS 钥匙串读取，不写入项目目录。
- 系统：使用 macOS 自带命令读取内存；没有第三方运行依赖。

四个采集器并行且互相隔离。某个服务超时不会拖死整个面板；并发请求会共用正在执行的采集任务。缓存仅在数据变化或超过保存周期时写盘。

局域网设备只能读取面板。刷新、修改和 WorkBuddy 重连接口只允许 Mac 本机调用。

## Poke4S

网页模式打开 kiosk 地址后，点一次"进入墨水屏模式"。页面每 5 分钟取数，并保留最近成功数据。

Android/Poke4S 当前源码版本为 `1.2.5-pencil-home`（versionCode 11），本次 macOS 2.7.1 保持不变。
不要使用仓库内不存在或过时的 APK 路径；请从 [GitHub Releases](https://github.com/easonwong2026-del/AICC/releases)
下载最新 Android APK。目前已发布的 1.2.5 APK 位于 [v2.5.0 Release assets](https://github.com/easonwong2026-del/AICC/releases/tag/v2.5.0)，也可直接[下载 APK](https://github.com/easonwong2026-del/AICC/releases/download/v2.5.0/Poke4S-AI-Dashboard-v1.2.5-pencil-home.apk)。
它保留 Poke4S 的 AI COMMAND 风格、长按设置、自动发现、缓存减写、R8 和低内存 Canvas 渲染，并兼容现有服务器字段。

`android/poke-dashboard/` 是可重复构建的源码和 Gradle wrapper，不属于 Mac 后台运行时。客户端不使用 AndroidX、图片库或第三方运行依赖；release 构建启用代码与资源瘦身。

## 验证

```bash
# Python 单元测试
python3 -B -m unittest discover -s tests -v

# SwiftPM 核心包测试
swift test --package-path macos/MenuBarApp

# Widget smoke test
bash scripts/smoke-test-widget.sh

# 完整构建
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh

# Shell 语法检查
for f in macos/*.sh scripts/*.sh; do [ -f "$f" ] && bash -n "$f"; done

# Bundled Server Smoke Test
bash scripts/smoke-test-bundled-server.sh

# DMG 构建
bash macos/build-dmg.sh

# 版本一致性
bash scripts/validate-version.sh

# Android 测试和 release 构建
( cd android/poke-dashboard && ./gradlew test assembleRelease )
```

健康检查：`http://localhost:8765/api/health`。完整状态：`http://localhost:8765/api/status`。

## 本地数据

- `data/status.json`：WorkBuddy 最近状态（旧版本的手工字段仅作兼容，不会在自动采集失败时冒充真实余额）
- `data/codex_last_success.json`：Codex 最近成功额度
- `data/workbuddy_last_success.json`：WorkBuddy 最近成功余额
- `data/deepseek_history.json`：用于计算当日消耗的余额快照

这些文件不包含 DeepSeek、Codex 或 WorkBuddy 密钥。
