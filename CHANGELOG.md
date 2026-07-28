# Changelog

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
- Add a generated monochrome macOS app icon and a reproducible `macos/build-menubar-app.sh` build path.
- Add `macos/build-dmg.sh` for a self-contained drag-to-Applications DMG whose bundled server stores runtime data in Application Support and can publish into a separate releases directory.
- Ship Android 1.2.5 Pencil Home UI with the final Poke4S home layout, larger quota typography, corrected reset-credit expiry fallback, and fixed charging status text.
- Document that the menu bar app is built into `dist/mac/AICC.app` first, with `/Applications` installation kept as an explicit user action.

## 2.2.2 - 2026-07-18
