# 交接说明（HANDOFF）

给在另一台机器（或新会话）接着做的人看的：仓库当前状态、构建发版方式、关键约束与后续 Backlog。

- **当前分支与版本**：`main` — **v2.14.1 / macOS Build 28**（含 PR #124 夜间轮换空闲探测 & 借用授权过期镜像修复，以及 PR #125 10 分钟静默刷新 & 控制台交互打磨）
- **运行环境**：macOS 14+ / Apple Silicon + Command Line Tools（Swift 6.3.3），**不需要 Xcode**；Rust `1.89+`（`account-worker`）；内置 Python 3.14 运行时。

---

## 1. 当前状态

| 模块 | 状态 | 说明 |
| --- | --- | --- |
| macOS 主控制台（UI V5） | 已上线（2.14.0 / 2.14.1） | 总览 / AI 账号 / 额度看板 / 任务中心 / 设置中心 / 自动计划中心 + 菜单栏退出 |
| macOS 桌面小组件 | 稳定 | Small / Medium 通用额度卡片 + Large 四页看板（深链翻页） |
| Rust `account-worker` | 稳定 | 账号、签到、成长、旅行、Credits、夜间轮换、凭据存储、TLS 证书管理 |
| 内置 Python 桥接 | 稳定 | 本机 `127.0.0.1:8765` HTTP；局域网 `0.0.0.0:8767` HTTPS（TLS 1.3 + SPKI Pinning） |
| Android 墨水屏（POKE4S） | 已发布（1.2.8 / Build 14） | 双模式 UI（瑞士网格 + 桌搭时钟）、HTTPS 8767 + SPKI Pinning，已由 `CN=AICC Release` 签名并挂入 `v2.14.1` Release |

---

## 2. 目录地图

```text
macos/
  MenuBarApp/Sources/     SwiftUI 主控制台、设置中心、Store、Worker/Server 管理器
  MenuBarApp/Tests/       swift test 单测（496+ 项，CI swift-test 运行）
  Widget/                 WidgetKit 扩展（额度小组件 + 四页看板）
  build-aicc-swiftui.sh   唯一 App + Widget 构建脚本（swiftc 直编 + strip + codesign）
  build-dmg.sh            UDBZ DMG 打包 + 卷内 dSYM/UUID/SHA-256 强制校验
account-worker/           Rust JSON-RPC 辅助进程（stdin/stdout，SQLite v29）
server.py & bridge_tls.py 本机 HTTP + 局域网 HTTPS 桥接（仅标准库）
collectors/magpie.py      Magpie 额度采集（schema_version: 3）
android/poke-dashboard/   墨水屏 Android 客户端（Gradle）
scripts/                  校验、离屏视觉回归（console/widget/board/settings）、release-publish.sh
docs/                     UI_V5、BRIDGE_TLS、RELEASING、SLIMMING-BASELINE 等文档
```

---

## 3. 构建、验证与发版

```bash
# 本地构建与打包（默认使用本机固定证书 "AICC Local Code Signing"，勿用 build-release-mac.sh 以免落回 ad-hoc）
bash macos/build-dmg.sh

# 校验与单测
bash scripts/validate-version.sh --dmg
bash scripts/validate-widget-bundle.sh
bash scripts/smoke-test-bundled-server.sh dist/mac/AICC.app/Contents/Resources/Server
bash scripts/verify-symbols.sh --app dist/mac/AICC.app --symbols dist/symbols/<ver>-<build> --dmg dist/AICC-<ver>.dmg
python3 -m unittest discover -s tests -v
cargo test --manifest-path account-worker/Cargo.toml
cd macos/MenuBarApp && swift test   # 需完整 Xcode；无 Xcode 时由 CI swift-test 执行

# 离屏视觉回归（可选）
bash scripts/smoke-test-console-visual.sh
bash scripts/smoke-test-widget-visual.sh
bash scripts/smoke-test-board-visual.sh
```

发版流程见 [docs/RELEASING.md](docs/RELEASING.md)：更新 `VERSION`、`Info.plist`（`CFBundleVersion`）与 `docs/releases/<ver>.md` → `bash macos/build-dmg.sh` → 提交并打 `v<ver>` tag 推送 → `bash scripts/release-publish.sh`。
**注意**：`dist/symbols/<ver>-<build>/` 是私有符号归档，绝不上传至公开 Release 或 Actions artifact。

---

## 4. 关键设计约束（改动前必读）

1. **签名与小组件扩展**：
   - 本地安装构建必须使用固定证书 `AICC Local Code Signing`（`ad-hoc` 会让钥匙串与授权每次重建失效）。
   - App 包内严禁写入运行时数据（`Contents/Resources/Server/data` 会破坏封包导致 Widget 全黑）；数据一律落在 `~/Library/Application Support/AICC-Dashboard/`。
2. **看板小组件（Large）翻页机制**：
   - 无 Xcode 环境下 `Button(intent:)` 的 App Intents 元数据无法稳定生效，页脚控件采用 `Link` 深链（`aicc-widget://board/next|previous|refresh`），`scripts/validate-widget-bundle.sh` 明确禁止出现 `Metadata.appintents`。
   - `BoardView(interactive: false)` 专供 `ImageRenderer` 离屏 QA 使用。
3. **隐私与安全边界**：
   - 快照只存稳定匿名 ID 与首字符脱敏标签；邮箱、原始账号名、凭据、错误原文不落盘。
   - 写操作仅限本机；局域网设备只能通过 HTTPS `8767` 读取脱敏快照，首次配对必须人工核对根证书 SPKI 指纹。
   - 签到/成长/活跃严格遵守「不伪造活跃、不确定不重发」原则。

---

## 5. 下一步 Backlog（P1 / P2）

1. **结构拆分（PR-D）**：按业务域拆分 `account-worker/src/handlers.rs`（7,200+ 行）与 Swift 大文件，降低后续变更回归成本。
2. **瘦身与内存收尾（PR-B3 / PR-C）**：评估 Rust `opt-level = "z"`，以及关闭桥接时不常驻 Python 进程（预计可省 24–29 MB 内存）。
3. **长期路线择一推进**：Developer ID 公证分发 / Android 通用客户端与手机小组件（#13）/ 新平台额度与账号接入（#14）。
