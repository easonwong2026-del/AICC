# 本轮自动化验收记录

记录时间：2026-10-20，分支 `feature/magpie-codex-dashboard`（PR #56，未合并）。
本机：macOS，Magpie **0.1.1296** 在 `127.0.0.1:3425` 运行，3 个服务商 / 5 个账号真实在线。

**只记录实际执行过的命令与结果。** 没跑过的项在「未完成」里列出，不用 CI 预期结果代替本地结果。

## 一、Python（本机执行）

```bash
python3 -m unittest tests.test_magpie -v   # Ran 23 tests — OK
python3 -m unittest tests.test_bridge -v   # Ran 8 tests — OK
python3 -m unittest discover -s tests       # Ran 31 tests — OK
flake8 --max-line-length=140 --exclude=__pycache__,.git,dist,android,web .
                                           # 无输出（0 findings）
```

覆盖：Codex 订阅、WorkBuddy Credits（`display`/`amount`/`limit`/`unit`）、自定义窗口名套餐、
API Key 余额与拆分、多 Provider 多账号混合、未知 Provider、缺失字段/空窗口/无限额度、
过期与 `error`/`readDeferred`/`asOf`、重复账号、异常百分比、无效时间戳、坏信封与坏行隔离、
刷新请求次数与 provider 失败隔离、匿名化、配对/权限/限速/凭据轮换、`format=2` 兼容层、缓存原子写入与 schema 闸门。

真实链路（本机 Magpie，非夹具）：

```bash
python3 - <<'PY'
from collectors import magpie
s = magpie.collect()
print(s["state"], s["providers"], len(s["accounts"]))
PY
# Connected ['codex', 'workbuddy', 'antigravity'] 5
# 强制刷新走 POST /v1/magpie/quotas/refresh，返回 Connected / 5 张卡
```

## 二、macOS（本机执行）

```bash
bash scripts/smoke-test-widget.sh
# Widget contract: providers, plan/balance/subscription, masking, capacity, freshness, config, cache — passed

bash scripts/smoke-test-display-snapshot.sh
# Widget loader: refresh policy, provider coverage, kept reads, redaction, expiry, offline and recovery passed

bash scripts/smoke-test-display-snapshot.sh --live
# Live local Magpie direct read passed; providers: ["codex","workbuddy","antigravity"] accounts: 5

bash scripts/smoke-test-widget-visual.sh
# Visual QA rendered 72 widget PNGs to /private/tmp/widget_qa
# （7 种状态 × 3 种配置 × 2 种尺寸 × 明暗 = 72）

bash scripts/smoke-test-dashboard-visual.sh
# Settings visual smoke passed: /private/tmp/aicc-dashboard-qa

BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh
# /Users/easonwong/Documents/Codex-Projects/AICC/dist/mac/AICC.app
# PASS: signed Widget bundle, refresh URL, matching versions and narrow read-only settings permission

bash scripts/smoke-test-widget-bundle.sh      # PASS（含「Broken installed refresh routing is rejected」）
bash scripts/smoke-test-bundled-server.sh dist/mac/AICC.app/Contents/Resources/Server
# Bundled bridge: stdin credential, authentication, offline Magpie and no dashboard passed
bash macos/build-dmg.sh                        # 候选 DMG 产出（见交付报告）
```

Swift 单元测试（`swift test --package-path macos/MenuBarApp`）**本地未执行**：本机只有 CommandLineTools，
没有 XCTest 模块（`no such module 'XCTest'`）。该步骤由 CI 的 `swift-test` job 在 macOS runner 上执行。

## 三、Android（本机部分执行）

本机没有 Android SDK，`./gradlew` 无法运行，因此：

- **已执行**：用 JDK 17 + Gradle 缓存中的 `org.json 20240303` + `junit 4.13.2`
  直接编译 `DashboardData.java` 与 `DashboardDataTest.java` 并运行：

  ```
  Tests run: 11, Failures: 0  — OK
  ```

  覆盖：三种 `kind`、Credits `display`、余额不转百分比、每页 4 张与翻页、1/2/全部数量、
  未知 Provider 保留、未知 `kind` 跳过、缺失额度未知、非法百分比、`used` 推导剩余、
  状态文案、仅真实变化才重绘、旧 `codex` 结构显式兼容、坏 JSON 拒绝。
- **未执行**：`./gradlew test`、`assembleRelease`、`connectedDebugAndroidTest`（模拟器截图）。均由 CI 执行。
  `DashboardView.java` / `MainActivity.java` / `DiscoveryClient.java` 的编译与渲染验证只在 CI 中完成。

## 四、未完成（真机/安装态/上游）

1. **macOS 安装态**：未把候选 DMG 安装到系统并观察桌面 Widget 的真实调度、深浅色、多桌面场景。
2. **Poke4S 真机**：没有设备，未验证配对、IP 迁移、残影、功耗、唤醒刷新与在 e-ink 上的实际可读性。
3. **上游服务商覆盖**：本机只登录了 Codex / WorkBuddy / Antigravity。Kiro、ZCode(GLM)、MiniMax、
   Command Code、API Key 余额类都只有源码级证据，没有真实额度行，见兼容性矩阵。
4. **Magpie 版本兼容**：只验证了 0.1.1296；更早/更晚版本的字段漂移未做回归。
5. **APK 编译**：本机无 Android SDK，未产出本地 APK；候选 APK 由 CI 构建。

## 五、辅助验证（人工查看过的渲染结果）

- `/private/tmp/widget_qa/small_mixed_auto_light.png`：2 张卡 +「+4」溢出提示，账号脱敏为 `a***`。
- `/private/tmp/widget_qa/medium_mixed_filtered_dark.png`：服务商筛选生效（仅 WorkBuddy），深色可读。
- `/private/tmp/aicc-dashboard-qa/settings-bridge-light.png`：设置页「显示数量 / 服务商」两项可见。

这些是离屏渲染，不等于真机安装态验收。
