# 交接说明（HANDOFF）

给在另一台机器上接着做的人看的：这个仓库现在在哪、怎么跑、哪些地方还没验证。

- 分支：`feature/usage-board-widget`（PR [#58](https://github.com/easonwong2026-del/AICC/pull/58) → `main`）
- 版本基线：`VERSION` = 2.11.0；`main` 已发布 v2.11.0 / macOS Build 16，本分支的本地构建是 **Build 17**
- 这个分支要合的是「AICC 看板」小组件：四页轮播、手动翻页、字号重排

## 1. 现在的状态

| 项 | 状态 |
| --- | --- |
| 额度小组件（small/medium） | 已完成，v2.11.0 已发布 |
| AICC 看板（large，四页） | 功能、排版、手动翻页均已在真机通过（深链方案，见第 6 节） |
| 看板视觉 QA（33 张离屏渲染） | 通过 |
| bundle / 签名 / 版本一致性校验 | 通过 |
| Android 墨水屏 | 与本次改动无关，未动 |

**手动翻页已改为 Link + 深链方案**：`Button(intent:)` 在没有 Xcode 的手工打包下拿不到可用的
App Intents 元数据，三轮尝试后放弃，改成页脚走 `aicc-widget://board/next|previous`，主机 App 投递
翻页请求（见第 6 节）。深链这一半已在本机端到端验证通过，剩下只有「桌面上点 Link」这一步需要人眼确认。

## 2. 目录地图

```
macos/
  MenuBarApp/Sources/     主机 App（SwiftUI，菜单栏）：AICCApp.swift、设置窗、Magpie 读取、桥接
  MenuBarApp/Tests/       swift test 单测（用量解析、设置、更新服务等）
  Widget/                 Widget 扩展源码
    AICCWidget.swift/.Views.swift   额度小组件（small/medium）
    BoardWidget.swift               看板：TimelineProvider + BoardCursor + 翻页命令消费
    BoardWidgetViews.swift          看板四页 UI、配色、字号层级
    WidgetUsage.swift               看板的数据读取与缓存（60s）
    WidgetStatus.swift              额度读取、诊断、共享策略
    BoardVisualSmokeMain.swift      离屏渲染入口（QA）
  build-aicc-swiftui.sh   构建 App + Widget 扩展（swiftc 直编，不依赖 Xcode）
  entitlements.plist / Widget/entitlements.plist
android/poke-dashboard/   墨水屏 App（Gradle，gradlew 已带）
server.py                 AICC 本地服务（桥接、Web 看板等）
collectors/magpie.py      Magpie 额度采集
scripts/                  冒烟与校验脚本（见第 7 节）
docs/                     验收、迁移、性能、服务商兼容性
```

## 3. 环境要求

- macOS + **Command Line Tools**（本机 `swiftc 6.3.3`、SDK `MacOSX26.5.sdk`）；**不需要 Xcode**
- `python3`（iOS/Android 无关的脚本、元数据生成、App 图标都用它）
- Android 端：JDK + `android/poke-dashboard/gradlew`
- 数据源：本机跑着 [Magpie](https://github.com/yetone/magpie)，监听 `127.0.0.1:3425`。
  AICC 只读它的三个接口，**不碰供应商密钥**：
  - `GET /v1/magpie/quotas?kept=1`（额度）
  - `GET /v1/magpie/usage`（用量）
  - `GET /v1/magpie/usage/requests?limit=3`（最近请求）

## 4. 从另一台机器开始

```bash
git clone https://github.com/easonwong2026-del/AICC.git && cd AICC
git checkout feature/usage-board-widget
gh auth login                       # 需要 repo + workflow 权限
bash macos/build-aicc-swiftui.sh    # 产出 dist/mac/AICC.app（含 AICCWidget.appex）
```

装到本机（**不要**直接用 Finder 拖，装完要清临时文件，否则签名校验会失败）：

```bash
pgrep -x AICC && kill -TERM "$(pgrep -x AICC)"     # 先退
mv /Applications/AICC.app /tmp/AICC.app.bak-$(date +%Y%m%d-%H%M)   # 留一份回滚
ditto dist/mac/AICC.app /Applications/AICC.app
find /Applications/AICC.app -name ".BC.T_*" -delete   # 沙箱拷贝留下的临时文件
codesign --verify --deep --strict /Applications/AICC.app
open /Applications/AICC.app
```

装完 **必须把桌面上的「AICC 看板」移除再重新添加**：WidgetKit 会缓存扩展和快照，不重加看不到新代码。

## 5. 构建与验证命令

```bash
bash macos/build-aicc-swiftui.sh                  # App + Widget 扩展，末尾自动跑 bundle 校验
bash scripts/validate-widget-bundle.sh            # 单独校验：身份、版本、签名、权限、无残留元数据
bash scripts/smoke-test-board-visual.sh           # 看板离屏渲染 → /private/tmp/board_qa（33 张）
bash scripts/smoke-test-widget-visual.sh          # 额度小组件渲染 → /private/tmp/widget_qa（94 张）
cd macos/MenuBarApp && swift test                 # Swift 单测
python3 -m unittest discover -s tests -v          # Python 单测（CI 同款）
cd android/poke-dashboard && ./gradlew assembleDebug
```

渲染尺寸是真实瓦片 `338×354`（看板）与 `systemSmall/systemMedium`（额度），断言会检查「不是全空、
不是一片墨」。改排版时先看这几张图，比上机试快。

## 6. 看板的关键设计约束（改动前先读）

**轮播是时间线驱动的，不是手势**：桌面小组件没有滑动。`BoardWidgetProvider` 一次给出 8 个 entry
（4 页 × 2 轮，每页 15 秒），用完 `.after(120s)` 让 WidgetKit 重新读一次 Magpie。系统不保证准点，
所以是「近似轮播」。

**手动翻页 = 停住 5 分钟**：`BoardCursor` 在扩展自己的 `UserDefaults` 里存两个东西——
自动轮播的 anchor（page + date，用来推算「此刻屏幕上第几页」）和 hold（page + until）。
`‹ / ›` 把目标页写进 hold、`reloadTimelines`；provider 看到 hold 就每 60 秒给一条同页 entry，
到期后从停住那页继续轮播（不跳回总览）。页脚此时显示「手动」。

**页脚三个控件是 `Link`，不是 `Button(intent:)`**——这是被真机否掉一轮之后定下来的。
`Button(intent:)` 要靠 appex 里的 `Resources/Metadata.appintents` 才能把点击路由回来，而那个文件由
Xcode 的 `appintentsmetadataprocessor` 生成；本机只有 Command Line Tools，手写一份（试过三轮：
带参 intent、去掉 `.buttonStyle(.plain)`、逐字段照抄系统样例）系统都**静默丢弃**，表现就是按钮画出来、
点了没反应、扩展里连痕迹都没有（诊断键 `aicc.board.intent.last` 始终为空）。所以改成 Link：

```
‹ → aicc-widget://board/previous
› → aicc-widget://board/next
⟳ → aicc-widget://refresh
```

主机 App 在 `application(_:open:)` 里接这三条：翻页那条把 `{step, at}` 写进
`com.aieink.dashboard.widget-settings` 域的 `boardCommand`，然后 `reloadTimelines`。
扩展的自有容器主机写不进去，共享域它只有只读例外，所以只能这样「投递请求」：

- 请求带时间戳 `at`，扩展把自己处理过的最后一个 `at` 记在 `aicc.board.command.applied`
- `BoardWidgetProvider.timeline()` 先 `consume` 命令（幂等，rebuild 不会重复翻页），再走 hold 逻辑
- 代价：点一下会唤起 AICC（冷启动约 1 秒）。启动是 URL 触发的，所以走 `kAEGetURL` 分支不弹设置窗

禁用元数据是有意的：`scripts/validate-widget-bundle.sh` 现在断言 appex 里**不得**出现
`Metadata.appintents`；`macos/make-appintents-metadata.py` 已删除。如果以后装 Xcode 想换回
`Button(intent:)`，把手工脚本换回官方生成器再改回去，别只改一半。

**离屏 QA 的坑**：`ImageRenderer` 画不了 `Link`，整棵子树会被替换成一个「禁止」图形，33 张图会全废。
所以 `BoardView` 有个 `interactive` 开关，`BoardVisualSmokeMain` 传 `false`，渲染时画的是惰性控件——
Link 本身不带任何外观，图片仍然是真实排版。

**字号层级是固定点数**：瓦片固定 338×354，`s = min(w,h)/480` 只用于间距和图形。早期正文用
`size × s` 缩放，12pt 落到 9pt 下限，这是「字太小」的根因。现在 `BoardText` 里的数值就是屏幕上
的点数，改字号直接改那里。

## 7. 脚本索引

| 脚本 | 用途 |
| --- | --- |
| `macos/build-aicc-swiftui.sh` | 唯一构建入口（App + 扩展 + 签名 + 校验） |
| `macos/build-dmg.sh` / `build-release-mac.sh` | 打 dmg / 发版流程 |
| `scripts/validate-widget-bundle.sh` | 身份、版本号、URL scheme、沙箱权限、确认没有 App Intents 元数据 |
| `scripts/validate-version.sh` | `VERSION` 与构建产物、更新清单是否一致 |
| `scripts/smoke-test-board-visual.sh` | 看板 33 张离屏渲染 |
| `scripts/smoke-test-widget-visual.sh` | 额度小组件 94 张离屏渲染 |
| `scripts/smoke-test-bundled-server.sh` | 打包内置 server 后的冒烟 |
| `scripts/smoke-test-android-visual.sh` | 墨水屏界面渲染 |

## 8. 已知问题 / 风险

1. **手动翻页「桌面点 Link」这一步未验证**（见第 6 节）。深链、投递、消费、渲染各段都已在本机
   用 `open aicc-widget://board/next` 端到端跑通，剩下的只有 WidgetKit 把点击变成 `open` 的那一跳。
2. 手工打包的 App 是 ad-hoc 签名（`codesign --sign -`），换机器首次打开可能被 Gatekeeper 拦，
   需要「右键 → 打开」或 `xattr -dr com.apple.quarantine`。
3. WidgetKit 的刷新时间不保证，页内数字最多可能滞后约 2 分钟（120 秒一轮重新读）。
4. 额度小组件 compact 布局最小字号 9pt，超长服务商名仍会截断（设计上的取舍）。
5. 深色/浅色/变暗态（macOS 26 accented）三套都要过：变暗态只有白 + 透明度，卡片靠 `palette.edge` 描边。
6. `dist/`、`.attic/`、`outputs/`、`data/`、`.workbuddy/` 都在 `.gitignore` 里，别提交构建产物和本机数据。

## 9. 建议的下一步

按这个顺序做，一步一确认：

1. 装机后移除重加看板 → 点 `›`，看页脚是否出现「手动」且停在该页；同时看
   `~/Library/Containers/com.aieink.dashboard.menubar.widget/Data/Library/Preferences/*.plist`
   里有没有 `aicc.board.intent.last`（`previous@…` = intent 跑了）与 `aicc.board.hold.page.<port>`
  （有 = 渲染该更新；两个都空 = 路由问题）
2. 通过后跑一遍 `bash scripts/smoke-test-board-visual.sh` 与 `swift test`，再看 CI（PR #58）
3. 决定是否把「点整块翻页」作为冗余手势补上
4. 收尾：`VERSION` 定版（2.11.1 或 2.12.0）、`CHANGELOG` 补条目、走 `scripts/build-release-mac.sh` 发版

## 10. 隐私边界（别破坏）

- 快照只存匿名 id + 「首字符 + `***`」，邮箱、账号名、错误文本、凭据都不落盘
- 主机 App 不读供应商密钥；小组件只读共享设置域的显示项（沙箱 + 只读例外）
- 日志只记状态码与数量，`aicc.widget.diag` 用于排障
