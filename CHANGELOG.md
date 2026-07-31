# Changelog

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
