# Changelog

[English](CHANGELOG.en.md)

## 2.7.1 - 2026-08-29

- **macOS Widget 重设计**：中尺寸 Widget 改为适合横向尺寸的左右布局；Codex 每周额度成为主视觉指标，居中展示大字号百分比与真实进度条、重置时间和 5 小时额度；WorkBuddy 积分与 DeepSeek 余额作为右侧次级指标对称排布。
- **Widget 信息收口**：删除中尺寸顶部冗余标题、底部更新时间、以及缓存年龄和连接状态等辅助文字；服务异常时保留稳定占位，不把诊断和错误信息暴露到桌面；右上角保留轻量化刷新按钮。
- **Widget 兼容性**：保留手动刷新、15 分钟 timeline、last-success 缓存与 stale fallback；向下兼容 2.7.0 保存的旧 Widget 快照缓存。
- **macOS runtime 收口**：正式安装统一使用 DMG App + SMAppService + ServerManager + bundled Python Server；退休 legacy LaunchAgent production runtime，并保留精确安全的一次性旧服务迁移。
- **版本**：AICC 2.7.1，macOS Build 10；Android/Poke4S 继续使用独立的 `1.2.5-pencil-home` / versionCode 11。

## 2.7.0 - 2026-08-28 (Release Candidate)

- **macOS 桌面 Widget**：新增 WidgetKit 小尺寸和中尺寸 Widget；小尺寸显示 Codex/WorkBuddy，中尺寸显示 Codex、WorkBuddy、DeepSeek 和 System
- **Widget 刷新与缓存边界**：支持手动刷新时间线；AICC App 启动或状态变化时通知 WidgetKit；Widget 只读取固定的 `/api/status`，Server 暂时不可达时保留最近成功数据并标记 stale，首次安装无缓存时显示占位符
- **Codex 额度修复**：按实时 `account/rateLimits/read` 的窗口时长识别 5 小时和 weekly 额度，保留多 bucket、稀疏 weekly 更新和无窗口时长时的旧字段回退
- **运行时收口**：DMG App 的正式本机端点固定为 `127.0.0.1:8765`；内置 Server 的 `aicc-server.log` 和 `aicc-server-error.log` 启动前限制为最多 1 MiB，日志处理失败不影响 Server 启动；App/Server build identity 校验继续防止旧后端复用
- **版本**：AICC 2.7.0，macOS Build 9；Android/Poke4S 继续使用独立的 `1.2.5-pencil-home` 版本

## 2.6.0 - 2026-08-27

- **OpenCodex 更新管理**：可检查已安装 OpenCodex 版本、查看新版本并执行一次性更新；更新后刷新版本和运行状态，运行中的 OpenCodex 更新前会提示可能短暂重启
- **macOS 稳定性**：优化菜单栏和 Settings 行为，保持 System、Codex、WorkBuddy、DeepSeek、OpenCodex 状态显示稳定
- **架构精简**：删除无实际使用价值的旧 Provider、Manifest 和 legacy compatibility 路径，同时保留真实数据采集及 Poke4S/Android `/api/status` 兼容
- **版本**：AICC 2.6.0，macOS Build 5；Android/Poke4S 本版本未变更，正式 Release 只发布 macOS 产物

## 2.5.0 - 2026-07-31

- **动态 Provider 架构（P1）**：新增 Provider Manifest v1 规范化层，所有采集器只负责原始数据，展示层只消费 Manifest；`GET /api/providers`、`GET /api/providers/<id>`、`POST /api/providers/<id>/refresh`、`POST /api/providers/<id>/actions/<kind>` 四个新端点；Action 由后端白名单映射（refresh / reconnect / diagnostics），Manifest 永远不能下发 endpoint、shell 命令、脚本路径或远程 URL
- **安全边界**：Provider ID 严格校验 `[a-z0-9][a-z0-9_-]{0,63}`；Manifest 剥离 Token、Cookie、API Key、WorkBuddy 原始账户对象和 `balance_diagnostic` 内部对象；错误信息截断脱敏；写操作仅限 localhost
- **WorkBuddy 2.4.4 链路冻结**：`127.0.0.1:9223` bridge、主 renderer 选择、DevTools/tdoc/MCP 排除、MessageChannel daemon transport、`auth:getAccountUsage`、`usageLeft/usageTotal/usageUsed/refreshAt`、120 秒自动更新、`force=true`、自动恢复、失败缓存全部保持不变；仅在原始结果外层增加 Manifest Adapter，并新增通道回归测试
- **额度字体与信息层级修复**：Codex Weekly 主数字放大至 30–38pt（进度条加粗、5 Hour 降为次级）；WorkBuddy 积分与 DeepSeek 余额主数字放大至 28–36pt 并与单位分层；新增统一 `DashboardTypography` 设计 Token，主数字按长度自适应且不会退化到小号正文
- **macOS 动态卡片**：新增通用 `ProviderCard`/`ProviderMetricView`/`ProviderStateBadge`/`ProviderActionMenu`，由 Manifest 数组驱动；新增内置 Provider 不再需要专属 Swift 模型、卡片或设置开关
- **设置迁移**：旧 `menuBarShowWorkBuddy`/`menuBarShowDeepSeek` 等开关自动迁移为 `provider_order` + `hidden_providers` 动态集合；设置页支持动态排序、显示/隐藏、单独刷新、诊断与恢复默认顺序
- **示例 Provider**：开发模式（`AICC_DEV_PROVIDERS=1`）内置示例额度 Provider，通过插件式 `manifest()` 接口自动出现在 API、设置页和动态卡片，默认不污染正式界面
- **Android/Poke4S 兼容**：`/api/status` 原字段完全保留，现有 APK 继续正常工作；动态化方案见 `docs/provider-architecture.md`
- **版本统一 2.5.0**：VERSION、PACKAGE.json、Info.plist（CFBundleShortVersionString、CFBundleVersion）同步更新

## 2.4.4 - 2026-07-31

- **真实环境根因确认**：WorkBuddy 5.3.5 的账户菜单实际显示 `5,343.37`，但安装版 2.4.3 的 9223 CDP bridge 未监听，AICC 因而落回旧 `Manual balance`；本机 CodeBuddy CLI 是编码代理 CLI，不提供账户积分读取接口
- **安全余额读取增强**：账户 API 支持积分字段别名；API 字段无效或抛错时继续使用受限 DOM fallback，支持同一行、相邻节点、千位分隔和小数格式
- **诊断可见性**：WorkBuddy 返回安全的 `balance_error_code`/`balance_error`，区分 bridge、renderer、API、菜单触发器、解析和超时错误；不读取或传输认证信息
- **安装版重连修复**：将 `start-workbuddy-monitored.sh` 打包到 `AICC.app/Contents/Resources/Server/macos/`，并在构建与 bundled smoke test 中校验可执行性；仅用户主动重连时启动/重启 WorkBuddy
- **重连通道可靠性**：重连脚本改用进程级退出/启动（不再依赖 Apple Events 自动化权限，避免静默失败），启动后校验 9223 桥和主渲染页面，失败时返回具体错误码；卡片在未连接时显示“设置 → 重连 WorkBuddy”操作提示
- **CDP 目标选择**：优先连接 WorkBuddy 主渲染页面（`renderer/index.html`），避免误连文档/MCP 等内嵌 WebView 导致积分 API 不可用
- **自动修复读取通道**：恢复 2.3.x 的 WorkBuddy 监控行为（原 LaunchAgent，现内置在 AICC 服务器内）：发现 WorkBuddy 已运行但未开放本地桥时，每个进程自动重启一次并带上调试参数；WorkBuddy 未运行时绝不启动，可用 `WORKBUDDY_AUTO_HEAL=0` 关闭
- **版本统一 2.4.4**：VERSION、PACKAGE.json、Info.plist 同步更新

## 2.4.3 - 2026-07-31

- **修复 WorkBuddy 实时积分缓存**：同一 CDP 页面 target 在超过 120 秒后会重新读取余额，不再因为页面会话未变化而永久复用旧值
- **贯通手动刷新**：`POST /api/refresh` 将 `force=true` 传递到 WorkBuddy 采集器，手动刷新会立即绕过采集器缓存
- **安全的失败回退**：CDP 读取失败时保留最近成功积分，并返回连接状态、更新时间、缓存年龄和过期标记；不读取、保存或传输 Token/Cookie
- **跨端显示修复**：macOS 使用千位分隔和小数显示真实积分，显示连接/缓存及最近更新时间；Android/Poke4S 兼容新的状态字段
- **并发保护**：同一时间只允许一个 WorkBuddy 采集任务运行，菜单读取仍只在页面余额不可见时短暂打开并恢复用户菜单

## 2.4.2 - 2026-07-29

- **修复 OpenCodex 登录 Shell 环境**：`ocx ensure` 和 `ocx stop` 现在通过 `/bin/zsh -lc` 执行，确保用户的 `.zprofile`、Homebrew、npm-global 路径和 `CODEX_CLI_PATH` 可用
- **安全的命令注入保护**：检测到的 `ocx` 二进制路径通过专用的 `AICC_OCX_PATH` 环境变量传递，不直接拼接到 Shell 命令字符串中，避免空格、引号等特殊字符导致的注入风险
- **仅影响生命周期命令**：`ocx status --json` 和 `ocx --version` 继续直接执行，无需加载 Login Shell；操作后的状态确认逻辑不变
- **命令构造测试覆盖**：新增 `OCXCommandBuilder` 单元测试，覆盖 `/bin/zsh -lc` 参数、特殊字符路径和受限制的命令集
- **版本统一 2.4.2**：VERSION、PACKAGE.json、Info.plist（CFBundleShortVersionString、CFBundleVersion）同步更新

## 2.4.1 - 2026-07-29

- **OpenCodex 控制简化**：删除 Codex Desktop / ChatGPT 客户端启动和联动功能；AICC 不再启动或监听 codex/chatgpt 应用
- **OpenCodex 真实状态**：状态检测基于 CLI 运行时发现，支持动态端口和仪表盘 URL；面板可见时 8-10 秒轻量检查，面板关闭后停止轮询
- **OpenCodex 控制**：菜单栏显示真实状态（未安装/检查中/已停止/启动中/运行中/停止中），开关反映真实运行状态；新增独立"打开 OpenCodex 仪表盘"操作行
- **移除系统健康面板项**：菜单栏面板和设置中删除重复的系统健康行；Python 后台 `/api/health/*` 接口完整保留用于服务监督
- **移除 Codex 客户端联动设置**：删除三项桌面客户端联动偏好及相关实现
- **移除 Swift 端重复健康轮询**：删除 APIService.startHealthRefresh 和独立 health Task；ServerManager 的 60 秒监督保持不变
- **版本统一 2.4.1**：VERSION、PACKAGE.json、Info.plist 同步更新；About 页面动态读取 CFBundleShortVersionString
- **SwiftPM 核心包**：新增 `macos/MenuBarApp/Package.swift`，支持编译状态模型和 ProcessRunner 并运行单元测试
- **CI 增强**：新增 Swift 核心测试步骤；新增静态引用检查确保已删除符号无残留
- **资源测量脚本**：新增 `scripts/measure-resources.sh`，支持可重复的内存/RSS/CPU/FD 基线测量
- **清理死文件**：删除 main.m.bak、main.m.bak2、build-menubar-app.sh

## 2.4.0 - 2026-07-28

- SwiftUI 进程级单实例保护，防止重复菜单栏入口
- 新增 PythonServiceSupervisor：自动恢复、指数退避、5 次失败熔断
- Provider 统一超时（默认 8 秒）与失败隔离
- 新增 `/api/health/live`、`/api/health/ready` 分层健康状态
- 刷新周期默认 120 秒；菜单栏沿用只读缓存
- 升级脚本增加版本 manifest 校验和健康检查后自动回滚
- 统一 DMG 命名为 `AICC-2.4.0.dmg`
- Developer ID 签名与 Apple Notarization 支持

## 2.3.1 - 2026-07-27

- 修复：`CODEX_CLI_PATH` 环境变量未传递导致 codex_monitor 无法连接 Codex CLI，使用旧 Manual 兜底数据
- 修复：`status.json` 只初始化一次不更新，菜单栏始终显示旧数据；新增 `_periodic_save()` 后台线程每 30 秒持久化收集器数据
- 修复：Python 3.9 下 `mkdir(exist_ok=True)` 无法创建嵌套目录，改为 `mkdir(parents=True, exist_ok=True)`
- 修复：app 启动不再依赖外部 `macos/start-dashboard.sh` 脚本，直接通过 NSTask 调用 python3 server.py
- 移除：旧 LaunchAgent 配置（com.aieink.dashboard / com.aieink.workbuddy-monitor / com.aieink.log-maintenance），服务完全由 app 管理
- macOS: 新增 `BUNDLE_SERVER=1` 构建模式，server.py + collectors + services + web 全部内置到 app bundle

## 2.3.0 - 2026-07-20

- Add a native 128 KB macOS menu bar app for starting, stopping, restarting, and monitoring the local dashboard service.
- Show cached Codex quota directly in the menu bar without triggering a quota refresh or collector run.
- Add a generated monochrome macOS app icon and a reproducible build path (historically `macos/build-menubar-app.sh`, superseded by `build-aicc-swiftui.sh`).
- Add `macos/build-dmg.sh` for a self-contained drag-to-Applications DMG whose bundled server stores runtime data in Application Support and can publish into a separate releases directory.
- Ship Android 1.2.5 Pencil Home UI with the final Poke4S home layout, larger quota typography, corrected reset-credit expiry fallback, and fixed charging status text.
- Document that the menu bar app is built into `dist/mac/AICC.app` first, with `/Applications` installation kept as an explicit user action.

## 2.2.2 - 2026-07-18
