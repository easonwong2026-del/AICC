# 本轮自动化验收记录

记录时间：2026-10-20 起，分支 `feature/magpie-codex-dashboard`；**PR #56 已于 2026-10-21 合并到 `main`，随 v2.11.0 发布**。
以下内容保留当时的执行原貌（包含当时"未合并/未发布"的状态描述）。
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

## 三·补、CI（GitHub Actions）

运行 [`39397512549`](https://github.com/easonwong2026-del/AICC/actions/runs/39397512549)（提交 `3643bef`）：
6 个 job 全部 success —— `test`、`lint`、`android`、`swift-test`、`macos-build`、`android-visual`。

- `swift-test` 在 CI 上跑通了本地跑不了的 `swift test --package-path macos/MenuBarApp`（本机缺 XCTest）。
- `android` 跑通了本地跑不了的 `./gradlew test assembleRelease`：`DashboardView` / `MainActivity` /
  `DiscoveryClient` 的编译与 11 个单元测试在 CI 通过。
- `android-visual` 跑通了模拟器 instrumentation：`DashboardVisualTest` 的 8 种场景渲染通过。
- `macos-build` 产出 `AICC-2.10.2.dmg` 与 App 工件；`hdiutil verify` 本地复验 CRC 有效。

> CI 的 APK 工件是 `app-release-unsigned.apk`：仓库 `build.gradle` 没有 signingConfig，
> 正式签名是发布时的仓库外步骤。因此该工件**不能直接安装**，只用于确认构建与单元测试通过。

## 三·补2、安装态问题与修复（2026-10-20 晚）

用户在 `/Applications/AICC.app`（2.10.2，17:07 安装）上遇到「小组件没有数值」。排查结论：

1. Widget 扩展进程 `AICCWidget` 是**升级前启动的旧进程**（映射的二进制 919 KB，磁盘上的新二进制 476 KB），
   Chronod 没有重启它；旧进程里只有 2026-10-15 的旧格式缓存，超过 24 小时后按设计显示占位符 → 全为 `—`。
2. 结束该进程后，新扩展启动并把 `aicc.widget.quota.3425` 写入容器：
   `state=Connected`、`providers=[codex, workbuddy, antigravity]`、5 张卡 —— 说明新链路本身是通的。
3. 同时发现一个真实设计缺陷：旧实现**先 POST 刷新再读取**，刷新慢或被 WidgetKit 时间线预算打断时，
   整个时间线回退到占位符，已经存在的 Magpie 数据反而不显示。

修复 1（取数顺序）：`WidgetStatusLoader.load` 改为**先读 `?kept=1` 并落盘，再做刷新**；有数据时刷新放到
后台任务，无数据或用户主动点击时才等待刷新。另加 `aicc.widget.diag` 诊断键（只记状态码与条数）。

修复 2（渲染）：截屏确认卡片底是浅灰、文字完全看不见。系统为 Dark Mode，Widget 没有
`containerBackground`，SwiftUI 的 `.primary` 在桌面小组件的 vibrancy 合成下与自绘半透明底撞色。
改为：整块 Widget 用 `.containerBackground(for: .widget) { windowBackgroundColor }`，
卡片底用 `controlBackgroundColor` + `separatorColor` 细边，文字统一 `labelColor`/`secondaryLabelColor`，
不再依赖 `.primary`/`.secondary` 的 vibrancy 解析。

重新构建并覆盖安装到 `/Applications/AICC.app`（Build 15，覆盖前备份到
`dist/backup-installed-2.10.2.app`），新扩展进程写入的容器缓存为
`state=Connected / providers=[codex, workbuddy, antigravity] / 5 张卡`，Codex 显示 `94%`、`86%`。

### 修复 3（外观）

第一版修完能看见数字后，用户反馈「丑 / 某种状态全白 / 对齐乱」。原因是仍在使用语义色：
`windowBackgroundColor` + `labelColor` 在桌面小组件的 vibrant 合成下并不保证同向。

改为**显式调色板**：`WidgetPalette` 按 `colorScheme` 返回固定 RGB（浅色纸白 / 深色近黑，卡片、
描边、墨色、次要色、警示色各一枚），不再依赖 `.primary`/`.secondary`/语义 `NSColor`。
排版重做：provider + plan 顶行、匿名账号次行、大号剩余值 + 窗口名、剩余比例细条、
第二窗口与重置时间同一行右对齐；单卡时放大并垂直居中；空状态居中并给出原因。
重置时间按距离缩写（`23:40` / `明天 09:00` / `周三 14:30` / `10-27 14:30`），小尺寸不再截断。

72 张离屏渲染全部通过并逐张查看（浅色、深色、1/2/4 卡、筛选、余额、空、缓存、长名称）。

### 安装态副作用（记录）

每次用 ad-hoc 签名重新构建并覆盖 `/Applications/AICC.app`，代码签名都会变，
macOS 会弹一次「AICC 想访问钥匙串」——这是桥接凭据的访问授权，不是应用缺陷。
点「始终允许」即可；正式发布用稳定 Developer ID 签名不会有这个提示。

## 三·补3、A 版式与"选择额度"（2026-10-21）

- 排版改成 3 行：`服务商 · 账号` 标题行（右侧套餐/状态）、剩余值 + 窗口名（右侧第二窗口或总量）、进度条 + 重置。
- 积分类窗口只报剩余：Python 新增 `remaining_text`（`limit - amount`，`amount` 缺失且用量为 0 时按满额），
  Swift/Java 同步；`credits`/`prompts` 单位在界面显示为「积分」「次」。
- 新增 `QuotaSlot` 与 `slots(selection:provider:limit:)`：选择粒度到窗口，key = `卡片ID|窗口名|pool`；
  同一账号最多两张选中的窗口合并成一张卡；key 全部失效时回退自动；余额卡没有窗口也保留。
- 设置窗口新增"选择额度…"面板：按服务商分组、勾选顺序即显示顺序、可一键清空回到自动。
- 验证：Python 33 项、Swift 契约冒烟（含窗口级选择、失效回退、剩余积分）、Android 12 项、84 张离屏渲染通过。

### 修复 4（变暗/单色态下变成白框）

点开别的应用后整块出现 4 个纯白圆角块、数字全部消失。这**不是**配色没设死，而是 macOS 26 的
「Dim widgets on desktop（默认 Automatically）」：任何窗口在前台时，桌面小组件会切到
**accented rendering mode**，系统会把所有内容**染成白色**、**移除背景**并换成玻璃，
只剩**不透明度**有效。

我们此前给卡片画的是不透明深色填充 → 被染色后变成**整块纯白**；文字同样被染白 →
白字叠白块，看起来就是「白框、无内容」。

修复：读取 `\.widgetRenderingMode`，非 `fullColor` 时换一套「只有白+透明度」的调色板
（卡片 `white 14%`、描边 `26%`、主文字 `100%`、次要 `66%`），背景交给系统玻璃，
不再画不透明块。`fullColor` 仍用原来的固定 RGB 深浅两套。

离屏 QA 增加 accented 变体（94 张），并新增断言：accented 渲染中「接近纯白（>0.9）像素」
占比不得 ≥18%，即不允许出现被染成白板的卡片。实机验证：切到 Finder 前台截屏，
卡片为半透明白wash、白色数字清晰可读。

### 修复 5（配色）

之前整块只有黑白灰。现在每个服务商一个色相（Codex 蓝、WorkBuddy 绿、Antigravity 紫、
ZCode 紫蓝、Kiro 琥珀、DeepSeek 蓝、MiniMax 红…），用于**服务商名**与**剩余进度条**；
未知 provider 用 id 的稳定哈希取色相，不会变成灰色。剩余 ≤15% 时进度条转琥珀告警色。

注意：变暗态（accented rendering）下系统会把所有颜色**染成白色**，所以那时仍是单色 —— 这是
系统行为，不是配色缺失。想一直彩色：系统设置 → 桌面与程序坞 → 小组件 → 变暗桌面小组件 → 永不。

顺带修掉小尺寸 `7 days 7…` 的截断：窗口名压成 `7天 / 5时 / 2周`。

### 真机验收（Poke4S，2026-10-21）

USB 连接 `46C8BEA3`（BOOX Poke4S，Android 11），用本机构建的 release APK（debug 证书签名，
与既有安装同签名，`adb install -r` 保留配对数据）覆盖安装，逐项实测：

| 项 | 结果 |
| --- | --- |
| 覆盖安装 / 升级 | 通过，包身份 `com.aieink.pokedashboard`、versionCode 12 未变 |
| UDP 发现（8766） | 通过，日志 `Candidate verified: http://192.168.0.18:8765` |
| 8 位配对码配对 | 通过（本机 `POST /api/pairing` 生成，设备输入后 `配对成功`） |
| 通用额度渲染 | 通过：`5 张 · Codex / WorkBuddy / Antigravity`，Codex 79%（5 hours，右 7 days 70%）、WorkBuddy 4650/4222/3717 积分、Antigravity 100%（Gemini 3.8 Flash，+5 个窗口） |
| 分页 | 通过：4 张/页，`1/2 · 点击翻页`，第 2 页正常 |
| 离线缓存 | 通过：断网重启后 `OFFLINE` + `离线 · 保留上次额度`，仍显示上次额度；恢复网络后回到 `SYNC` |
| 单位中文化 | 修复：`credits` 显示为「积分」（与 Mac 小组件一致），`prompts/requests` → 「次」 |

本轮同时发现并修掉：`credits` 在墨水屏上仍显示英文。其余仍待真机验证：BOOX 闪刷校准、休眠/唤醒、
长时功耗与残影。

### 墨水屏排版重做（方案 C，2026-10-21 下午）

按 `peterroe/magpie-usage-display` 的视觉语言重排 Poke4S 界面，逐条修掉上面审计的问题：

- 分隔线压「重置」、44pt 与 18pt 基线错位：旧版四卡布局整体废弃，不再有卡片内分隔线。
- 焦点卡：`最低剩余 · Codex 5 小时` + `65%`（`%` 做 44pt 上标）+ 16px 圆角条 + `2 时 34 分后重置 · 7 天 68%`。
  焦点按「剩余最少、并列取先重置」自动选，账号的第二个窗口写进同一行，不再丢失。
- 摘要行：`WorkBuddy · 卡***` / `4635 积分` / `共 4727` / 300px 短条 + 右侧倒计时，
  三行一屏；无重置时间的窗口不再刷「重置 未知」。
- 账号详情页：只给窗口 ≥3 的账号（当前 Antigravity 7 个窗口），每行 `名称 / 数值 / 条 / 倒计时`，
  同名窗口用 pool 区分——`+5 个窗口` 的死胡同消失。
- 页脚合并成一行（`5 张卡 · 3 服务商` + 电量），页码 `1/2` + 页点 + 操作提示；
  离线时状态点变空心圈、页脚显示「上次 12:20」。品牌由 `AI COMMAND` 改为 `AICC / MAGPIE`。
- 单位与窗口名中文化：`credits → 积分`、`5 hours → 5 小时`、`7 days → 7 天`。

真机（Poke4S）逐页截图确认：总览、Antigravity 详情、断网离线态。单测 26 项通过
（17 DashboardData + 9 DiscoveryClient）。

真机操作踩到的两个坑（记下来省下次的时间）：

1. 跑过 `./gradlew connectedDebugAndroidTest` 之后，`am start` 会报
   `Error type 3 / Activity class does not exist`，但 APK 与 dumpsys 组件表都正常——
   实际是应用被系统**置为 disabled**。`adb shell pm enable com.aieink.pokedashboard` 即恢复，
   不需要重启设备（本次先做了一次重启，白折腾）。
2. 焦点卡选中的可能正是账号的第二个窗口，所以 caption 不能再盲目取 `items.get(1)`，
   否则会把焦点自己再写一遍（`最低剩余 · Codex 7 天 64%` + `7 天 64%`）。
   现在取「不是焦点的那个窗口」，显示为 `5 天 22 时后重置 · 5 小时 95%`。

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
