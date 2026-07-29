# AICC

面向 macOS 的 AI 状态中心，显示 Codex、WorkBuddy、DeepSeek 和系统状态。支持 Poke4S 墨水屏显示。服务只依赖 Python 标准库。

## 使用地址

- Mac：`http://localhost:8765`
- 设置：SwiftUI 原生设置窗口（菜单栏 → Settings）
- Poke4S：`http://<Mac 当前 Wi-Fi 地址>:8765/?kiosk=1`

Mac 与 Poke4S 需要连接同一 Wi-Fi。原生 Android 客户端也可以通过 UDP 自动发现 Mac。

## macOS 启动与维护

首次部署见 [MAC-MIGRATION.md](MAC-MIGRATION.md)。常用操作：

```bash
bash macos/start-dashboard.sh              # 前台启动
bash macos/install-autostart.sh            # 安装自动启动与监控
bash macos/uninstall-autostart.sh          # 移除自动启动
bash macos/set-deepseek-key.sh             # 更新 DeepSeek 密钥
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh  # 构建自包含 SwiftUI 菜单栏 App（含 Python 服务）
bash macos/build-dmg.sh  # 构建自包含 AICC DMG 安装包
```

安全更新会先跑测试并完整备份，同时保留当前额度历史和缓存：

```bash
bash macos/update-from-directory.sh /path/to/new-dashboard
bash macos/rollback-from-backup.sh /path/to/backup
```

日志位于 `~/Library/Logs/AICC-Dashboard/`，每天自动限制体积。

SwiftUI 菜单栏 App（AICC）会生成在 `dist/mac/AICC.app`。`SERVER_ROOT` 不填时
默认管理当前项目目录，填入生产目录时可让安装版 App 直接管理日常运行的服务。
`build-dmg.sh` 会打包同一套 SwiftUI App，并生成自包含安装包，默认输出到 `dist/`；传入 `RELEASE_DIR` 后可输出到
`/Users/easonwong/AICC/releases/mac/` 这类成品目录。DMG 里的 App 自带服务器代码，
可拖入 `/Applications`。自包含 App 的运行数据写入
`~/Library/Application Support/AICC-Dashboard/data/`，不会写进 App 包本身。
App 运行需要 macOS 14 或更高版本、Apple Silicon，以及可执行的 Python 3（建议 Python 3.10+）；DMG 不包含 Python 解释器。
没有 Developer ID 时生成的是 ad-hoc 测试包，首次在其他 Mac 安装需要右键选择"打开"。
菜单栏 App 只负责状态显示、启动/停止内部数据服务、手动控制 OpenCodex、打开原生设置窗口、
打开日志和配置开机自启；额度采集仍由现有 Python 服务完成。内部数据服务监督只访问
`/api/health/live`，状态面板读取 `/api/status` 缓存，不会额外触发 Provider 刷新。

### 2.4.1 OpenCodex 控制变更

- AICC **不负责打开 Codex Desktop 或 ChatGPT Desktop**。
- AICC 只负责查看 OpenCodex 状态、启动/停止 OpenCodex、打开 OpenCodex 仪表盘。
- OpenCodex 开关反映真实运行时状态（通过 `ocx status --json` CLI 发现），支持动态端口。
- OpenCodex 状态轮询仅在菜单栏面板打开时进行（约 9 秒间隔），面板关闭后停止。
- 不再保留"随 Codex Desktop 启动 OpenCodex"等联动设置。

## 数据采集与内存策略

- Codex：按需启动 `codex app-server`，读取账户额度；面板停止访问 30 秒后结束子进程并使用最后成功缓存。
- WorkBuddy：只连接 `127.0.0.1:9223` 的本机调试桥。应用关闭时绝不主动启动；你手动打开后，每个 WorkBuddy 会话只读取一次界面可见余额，随后持续使用最后成功缓存。本机数据库中的当日用量仍可被动更新，不保存令牌。
- DeepSeek：密钥只从环境变量或 macOS 钥匙串读取，不写入项目目录。
- 系统：使用 macOS 自带命令读取内存；没有第三方运行依赖。

四个采集器并行且互相隔离。某个服务超时不会拖死整个面板；并发请求会共用正在执行的采集任务。缓存仅在数据变化或超过保存周期时写盘。

局域网设备只能读取面板。刷新、修改和 WorkBuddy 重连接口只允许 Mac 本机调用。

## Poke4S

网页模式打开 kiosk 地址后，点一次"进入墨水屏模式"。页面每 5 分钟取数，并保留最近成功数据。

原生客户端 V1.2.2 Optimized C 安装包位于 `dist/Poke4S-AI-Dashboard-v1.2.2-optimized.apk`。它保留 V1.2 的 AI COMMAND 风格和长按设置弹窗，将 Codex 额度重置时间、重置机会次数和机会到期时间分组对应，并把 WorkBuddy 与 DeepSeek 调整为双栏。所有现有服务器字段均保留；DeepSeek 当日使用量会去掉无意义的尾随零。数据层继续兼容最新服务器字段，并保留单实例、缓存减写、回调保护、自动发现、R8 和低内存 Canvas 渲染等优化。

`android/poke-dashboard/` 是可重复构建的源码和 Gradle wrapper，不属于 Mac 后台运行时。客户端不使用 AndroidX、图片库或第三方运行依赖；release 构建启用代码与资源瘦身。

## 验证

```bash
# Python 单元测试
python3 -B -m unittest discover -s tests -v

# SwiftPM 核心包测试
swift test --package-path macos/MenuBarApp

# 无 XCTest 环境也可运行的核心 smoke test
bash scripts/smoke-test-swift-core.sh

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
```

健康检查：`http://localhost:8765/api/health`。完整状态：`http://localhost:8765/api/status`。

## 本地数据

- `data/status.json`：WorkBuddy 手工兜底值
- `data/codex_last_success.json`：Codex 最近成功额度
- `data/workbuddy_last_success.json`：WorkBuddy 最近成功余额
- `data/deepseek_history.json`：用于计算当日消耗的余额快照

这些文件不包含 DeepSeek、Codex 或 WorkBuddy 密钥。
