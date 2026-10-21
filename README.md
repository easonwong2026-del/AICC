# AICC · Magpie 多服务商额度伴侣

[English](README.en.md)

AICC 提供 macOS 桌面小组件和 Android 墨水屏的**额度显示**，覆盖 Magpie 里所有服务商与账号。
账号登录、模型路由和额度采集都由 [Magpie](https://github.com/yetone/magpie) 负责。AICC 没有菜单栏图标、
弹出看板或 Web 看板，不启动代理、不读取供应商密钥，也不再自带 Codex / WorkBuddy / DeepSeek 采集器。

本分支为改版候选，尚未发布。沿用正式版 **2.10.2 / Build 15** 的版本及 Widget 身份；
不能按版本号区分候选包和正式安装包。当前正式版仍在 [Releases](https://github.com/easonwong2026-del/AICC/releases)。

## 显示什么

Magpie 的额度接口有三种数据类型，AICC 全部支持，且不按服务商写死：

| 类型 | 例子 | 显示方式 |
| --- | --- | --- |
| 订阅额度 `subscription` | Codex 的 `5 hours`/`7 days`、WorkBuddy 的 `Credits`、Antigravity 的 `Gemini · 7 days` | 每个窗口一条，窗口名原样保留 |
| 套餐额度 `plan` | GLM Coding Plan、Command Code 套餐 | 同上，另显示套餐名与到期/续费 |
| 余额 `balance` | API Key / 账户余额与拆分 | 直接显示金额，**不**换算成百分比 |

服务商不是白名单：Magpie 返回什么就显示什么；新装一个社区插件，只要 Magpie 能读到额度，AICC 无需升级适配。
真实窗口名会变（Antigravity 刷新后会从 `Gemini · 7 days` 变成 `Gemini 3.8 Flash (High)`），也会重名，
所以模型里没有 `five_hour`/`weekly` 这种固定字段。

账号标识脱敏：快照只保留稳定的匿名 id 和「首字符 + `***`」的标签，邮箱、原始账号名、错误文本与凭据都不落盘。

逐服务商的验证状态见 [服务商兼容性矩阵](docs/PROVIDER-COMPATIBILITY.md)：**只有 Codex、WorkBuddy、Antigravity 在本机用真实额度验证过**。

## 桌面小组件

1. 安装候选 AICC App，保持 Magpie 正在运行；默认本机端口为 3425。
2. 桌面右键 → 编辑小组件 → 添加 AICC 小尺寸或中尺寸。
3. 在 AICC 设置里选「显示数量」（自动/1/2/4/全部）与「服务商」（全部或某一个）。
4. 可以关闭 AICC。Widget 直接读取本机 Magpie，不依赖 Python 桥接或宿主 App 常驻。

积分类额度只显示**还剩多少**（例如 `4694 积分`，总量放在右侧 `共 4727`），不再显示"已用 / 总量"。

设置里可以"**选择额度**"：按服务商 → 账号 → 窗口逐项勾选（例如只显示 Gemini 的 7 天额度），勾选顺序就是显示顺序；
不勾选＝自动。窗口名被上游改了导致旧勾选失效时，会跳过失效项而不是让整块空白。

小尺寸最多 2 张额度卡，中尺寸最多 4 张；每张卡是一个服务商账号，展示前两个额度窗口，窗口更多时显示 `+N`，
整屏装不下时右下角显示剩余张数。没有额度、离线、缓存、读取失败、未知服务商都有独立状态，不会因为缺 Codex 就整块不可用。
点击小组件会让宿主执行一次**全服务商**刷新并重载时间线，不再只刷 Codex。

自动时间线以 5 分钟为目标；同一 Widget 进程对同一端口限制刷新频率，实际调度由 WidgetKit 决定——
这是「请求重载」的节奏，不是实时定时器。请求失败保留同端口的匿名缓存并明确标记缓存；
源读数超过 5 分钟也标记缓存；超过 24 小时的本地快照不再当作额度显示。

桌面小组件在「有窗口在前台」时会变暗成单色（macOS 26 的 *Dim widgets on desktop*，默认自动）：
这是系统行为，AICC 已针对该渲染模式单独配色，数字仍然清晰。想让小组件一直保持全彩，可在
系统设置 → 桌面与程序坞 → 小组件 → **变暗桌面小组件** 里改成「永不」。

升级 AICC 后如果桌面小组件仍是空白，先把小组件移除再重新添加一次：macOS 会沿用升级前已经启动的
Widget 扩展进程，旧进程里是旧代码和过期缓存。

> 注：本轮沿用静态配置（`StaticConfiguration`）+ 共享设置域，因为当前构建方式下 App Intents 动态配置
> 无法稳定注册；Widget 种类标识与升级兼容性保持不变。

## 墨水屏 APK

1. 打开 AICC 设置，启用“墨水屏桥接”。Mac 与墨水屏连接同一可信局域网。
2. APK 长按打开设置，自动发现或输入 Mac 地址；桥接使用 HTTP 8765、UDP 发现 8766。
3. 在 Mac 点击“生成配对码”，在 APK 输入 8 位码并配对。有效期 2 分钟，只能使用一次，
   最多尝试 5 次。APK 不需要 Magpie 网关 Key 或任何供应商凭据。
4. 可关闭 Mac 设置窗口；启用桥接时 AICC 留在后台，停用时关闭窗口即退出。登录启动可单独开启。

APK 显示同样的通用额度：多服务商、多账号，每个账号的订阅窗口/积分/套餐/余额，以及数据状态。
默认每 5 分钟同步，前台或唤醒时立即更新，后台或熄屏时暂停；显示 1/2/全部，全部模式每页 4 张卡、点击翻页；
内容变化时才重绘，保留既有 BOOX 闪刷校准与缓存减写。

升级 Mac 端时请一并升级 APK：桥接默认按 schema 3 回答，新版 APK 会显式声明 `X-AICC-Schema: 3`，
旧版 APK 不会声明版本，需要靠 `?format=2` 兼容层（由调用方显式指定）。

桥接是独立的只读权限：网络设备只能读匿名额度快照，不能刷新上游、管理账号、改路由或关闭服务。
客户端用 `X-AICC-Schema: 3` 显式协商版本；旧 `codex` 结构由同一份快照派生（`?format=2`），不做字段级猜测。
配对凭据在 Mac 钥匙串保存，通过标准输入交给 Python；Android 用 Keystore 加密，关闭备份。
当前 LAN HTTP 传输没有加密，因此只适合可信局域网，不要开放到公网或公共 Wi-Fi。

## 开发与验收

接口证据、数据契约、刷新与安全边界见 [迁移说明](docs/MAGPIE-MIGRATION.md)；
本轮实际执行过的测试与未完成项见 [验收记录](docs/ACCEPTANCE.md)。
Python 桥接只依赖标准库；Swift 使用原生 WidgetKit；Android 不增加运行依赖。

```bash
python3 -m unittest discover -s tests -v
bash scripts/smoke-test-widget.sh
bash scripts/smoke-test-display-snapshot.sh
bash scripts/smoke-test-widget-visual.sh
bash scripts/smoke-test-dashboard-visual.sh
swift test --package-path macos/MenuBarApp
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh
bash scripts/smoke-test-bundled-server.sh dist/mac/AICC.app/Contents/Resources/Server
bash macos/build-dmg.sh
./android/poke-dashboard/gradlew -p android/poke-dashboard test assembleRelease
```

开发桥接：`bash macos/start-dashboard.sh`；默认产生进程内临时只读凭据，重启后需要重新配对。
`AICC_MAGPIE_URL` 只接受带端口的 loopback HTTP 地址，禁止代理、重定向、远程地址和 URL 凭据。
没有 `AICC_CODEX_SOURCE` 之类的来源切换开关，AICC 永远只读 Magpie。

PR CI 验证 Python、桥接安全边界、Swift 数据与配置、原生 Widget/设置界面、DMG、Android 单元测试与模拟器截图，
以及 macOS 打包后的 Widget 沙箱与刷新路由。CI 候选包不代表已安装或正式发布。
保留原有用户数据目录及旧偏好，升级时只清理历史 AICC 安装器拥有的精确 LaunchAgent 身份。
