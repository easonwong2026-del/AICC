# AICC 主控制台

打开 AICC 就是一个窗口：左侧四个分页，右上角齿轮进设置。
本文说明它读什么数据、哪些能力还没接、以及怎么验收。

```
AICC.app
├── 主控制台（新增）        总览 / AI 账号 / 额度看板 / 签到任务
├── 设置（保留）            齿轮进入，原有全部设置项不变
├── Helpers/aicc-account-worker   账号与凭据（PR1a）
├── Resources/Server        Python Magpie 桥接（未改动）
└── PlugIns/AICCWidget.appex      WidgetKit（未改动）
```

## 1. 入口与窗口行为

| 动作 | 行为 |
| --- | --- |
| 启动 AICC（Finder / Spotlight / `open`） | 直接打开主控制台，不再默认进设置 |
| 再次点击图标 / Dock 重开 | `applicationShouldHandleReopen` → 激活已有窗口，不新建 |
| 右上角齿轮 | 打开独立设置窗口；关掉后回到主控制台 |
| 关闭主窗口 | 沿用原策略：桥接开启时继续后台运行，未开启时退出 |
| 小组件刷新 / 翻页 URL | 只处理 URL，不弹主控制台 |

> 目前没有 `NSStatusItem`：这个 App 一直是 `.accessory` + 无菜单栏图标，
> 入口是启动/激活。PRD 里"点击菜单栏图标"当前等价于"点击 App 图标"。
> 真要常驻菜单栏图标，会改变关闭窗口后的退出语义，需要单独确认。

## 2. 四个分页读什么

| 分页 | 数据来源 | 未接入 |
| --- | --- | --- |
| 总览 | 账号库统计 + 额度快照前 3 条 + Magpie/助手状态 | 今日签到（显示"待接入"） |
| AI 账号 | `accounts.list`（真实 SQLite） | 扫码登录、备注编辑、删除（按钮禁用） |
| 额度看板 | `WidgetStatusLoader.snapshot` + `QuotaSnapshot.slots` | 无（完整复用 Widget 链路） |
| 签到任务 | 任务清单为静态定义；账号来自账号库 | 全部执行动作（禁用，标注待接入） |

**额度页与桌面 Widget 用同一份缓存和同一个加载器**，所以两边数字一致；
打开主窗口不会额外拉起任何 Magpie 服务。

**没有数据时只显示空状态**，不填演示数字。账号为空时总览的"账号总数"就是 0。

## 3. 与 PR1a 助手的关系

- `AccountWorkerManager.shared` 是唯一实例：主控制台与设置页共用，只起一个进程。
- `ConsoleStore` 负责取数：`refreshAccounts` 走 `accounts.list`，
  `refreshQuota` 走 `WidgetStatusLoader`；切页不重建任何东西。
- 助手没起来时，账号页显示具体原因（未启动 / 启动中 / 失败信息），不会静默变成"空账号库"。

## 4. 视觉

- 浅色为默认设计，深色跟随"设置 → 外观"（system / light / dark）。
- 画布 `#F5F6F7`、卡片 `#FFFFFF`、强调 `#328F80`、边框 `#E6E9EC`，深色各自有对应值，
  全部走 `NSColor` 动态 provider（`ConsoleTheme.swift`）。
- 窗口 1120×760，最小 960×640；卡片圆角 14、统一细边框、无投影。
- 三个分页的截图在 `docs/images/console/`（浅色 + 深色各四张）。

## 5. 验收怎么跑

```bash
bash macos/build-aicc-swiftui.sh      # 编译 App + 助手 + 签名
ditto dist/mac/AICC.app ~/Applications/AICC.app
open ~/Applications/AICC.app
```

逐项确认：

1. 启动即出主控制台，"总览"选中；
2. 左侧四页都能点，内容随页切换，助手进程不重启（设置里"检查助手"仍显示同一份状态）；
3. 账号为空时是空状态，不是假数据；
4. 额度页数字与桌面 Widget 一致，点"刷新额度"两处同步变化；
5. 齿轮进设置，原有 Magpie 端口 / Widget 选择 / 墨水屏桥接 / 配对码都在；
6. 桥接开启时关窗口，墨水屏仍能同步；未开启时关窗口即退出。

## 6. 截图用的 QA 钩子

`AICC_CONSOLE_PAGE=overview|accounts|quota|checkin` 指定初始分页，
只为截图/回归脚本服务，正常启动不带这个变量：

```bash
AICC_CONSOLE_PAGE=quota open -n ~/Applications/AICC.app
```

配套命令：先 `defaults write com.aieink.dashboard.menubar themeMode -string dark`
切外观，截完记得改回 `system`。
