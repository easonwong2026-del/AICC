# 交接说明（HANDOFF）

给在另一台机器上接着做的人看的：这个仓库现在在哪、怎么跑、哪些地方还没验证。

- 分支：`feature/usage-board-widget`（PR [#58](https://github.com/easonwong2026-del/AICC/pull/58) → `main`）
- 版本基线：`VERSION` = 2.11.0；`main` 已发布 v2.11.0 / macOS Build 16，本分支的本地构建是 **Build 17**
- 这个分支要合的是「AICC 看板」小组件：四页轮播、手动翻页、字号重排、App Intents 元数据

## 1. 现在的状态

| 项 | 状态 |
| --- | --- |
| 额度小组件（small/medium） | 已完成，v2.11.0 已发布 |
| AICC 看板（large，四页） | 功能与排版完成，**手动翻页按钮未在真机验证通过** |
| 看板视觉 QA（33 张离屏渲染） | 通过 |
| bundle / 签名 / 版本一致性校验 | 通过 |
| Android 墨水屏 | 与本次改动无关，未动 |

**唯一未闭环的问题**：看板页脚 `‹` `›` `⟳` 点击后没有反应。已定位为手工打包缺少 `Metadata.appintents`，
本轮补了手写的元数据生成器，但需要装机验证（见第 6 节）。

## 2. 目录地图

```
macos/
  MenuBarApp/Sources/     主机 App（SwiftUI，菜单栏）：AICCApp.swift、设置窗、Magpie 读取、桥接
  MenuBarApp/Tests/       swift test 单测（用量解析、设置、更新服务等）
  Widget/                 Widget 扩展源码
    AICCWidget.swift/.Views.swift   额度小组件（small/medium）
    BoardWidget.swift               看板：TimelineProvider + BoardCursor + 两个 App Intent
    BoardWidgetViews.swift          看板四页 UI、配色、字号层级
    WidgetUsage.swift               看板的数据读取与缓存（60s）
    WidgetStatus.swift              额度读取、诊断、共享策略
    BoardVisualSmokeMain.swift      离屏渲染入口（QA）
  build-aicc-swiftui.sh   构建 App + Widget 扩展（swiftc 直编，不依赖 Xcode）
  make-appintents-metadata.py       生成 Metadata.appintents（见第 6 节）
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
bash scripts/validate-widget-bundle.sh            # 单独校验：身份、版本、签名、权限、App Intents 元数据
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

**App Intents 元数据是手写的，这是本项目最脆的一环**：Xcode 的 `appintentsmetadataprocessor`
会在 appex 里生成 `Resources/Metadata.appintents`，系统靠它把 `Button(intent:)` 的点击路由回来。
本机只有 Command Line Tools，所以由 `macos/make-appintents-metadata.py` 生成（构建脚本在签名前调用）：

- mangled 名不是猜的，是从二进制符号核对的：`10AICCWidget15BoardStepIntentV` / `10AICCWidget18BoardRefreshIntentV`
- 格式对照系统自带小组件（如 `WorldClockWidget.appex`）的样例；改 Intent 名字要同步改这个脚本
- `CFBundleVersion` 从 16 提到 17，是为了让系统重新索引扩展（不然还认旧缓存）
- `scripts/validate-widget-bundle.sh` 里已从「不得存在元数据」改为「必须存在且恰好这两个 intent」

如果点击仍然无效，按顺序试：① 移除重加小组件；② 提高 `CFBundleVersion` 再重装；
③ 退路是点整块小组件走 `aicc-widget://` 深链让主机 App 翻页（代价：会唤起 AICC，冷启动约 1 秒）；
④ 装 Xcode 后改用官方 `appintentsmetadataprocessor` 生成，删掉手写脚本。

**字号层级是固定点数**：瓦片固定 338×354，`s = min(w,h)/480` 只用于间距和图形。早期正文用
`size × s` 缩放，12pt 落到 9pt 下限，这是「字太小」的根因。现在 `BoardText` 里的数值就是屏幕上
的点数，改字号直接改那里。

## 7. 脚本索引

| 脚本 | 用途 |
| --- | --- |
| `macos/build-aicc-swiftui.sh` | 唯一构建入口（App + 扩展 + 签名 + 校验） |
| `macos/build-dmg.sh` / `build-release-mac.sh` | 打 dmg / 发版流程 |
| `scripts/validate-widget-bundle.sh` | 身份、版本号、URL scheme、沙箱权限、App Intents 元数据 |
| `scripts/validate-version.sh` | `VERSION` 与构建产物、更新清单是否一致 |
| `scripts/smoke-test-board-visual.sh` | 看板 33 张离屏渲染 |
| `scripts/smoke-test-widget-visual.sh` | 额度小组件 94 张离屏渲染 |
| `scripts/smoke-test-bundled-server.sh` | 打包内置 server 后的冒烟 |
| `scripts/smoke-test-android-visual.sh` | 墨水屏界面渲染 |

## 8. 已知问题 / 风险

1. **手动翻页按钮真机未验证**（最重要，见第 6 节末）。
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
   里有没有 `aicc.board.hold.page.<port>`（有 = intent 执行了，问题在渲染；没有 = 还是路由问题）
2. 通过后跑一遍 `bash scripts/smoke-test-board-visual.sh` 与 `swift test`，再看 CI（PR #58）
3. 决定是否把「点整块翻页」作为冗余手势补上
4. 收尾：`VERSION` 定版（2.11.1 或 2.12.0）、`CHANGELOG` 补条目、走 `scripts/build-release-mac.sh` 发版

## 10. 隐私边界（别破坏）

- 快照只存匿名 id + 「首字符 + `***`」，邮箱、账号名、错误文本、凭据都不落盘
- 主机 App 不读供应商密钥；小组件只读共享设置域的显示项（沙箱 + 只读例外）
- 日志只记状态码与数量，`aicc.widget.diag` 用于排障
