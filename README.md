# AICC · Magpie 显示伴侣

[English](README.en.md)

AICC 提供 macOS 桌面小组件和 Android 墨水屏 Codex 额度显示。账号登录、模型路由和调用由
[Magpie](https://github.com/yetone/magpie) 管理。AICC 没有菜单栏图标、弹出看板或 Web 看板，
不启动代理、不读取供应商密钥、不独立采集 WorkBuddy、DeepSeek、Google 或系统状态。

本分支为改版候选，尚未发布。沿用正式版 **2.10.2 / Build 15** 的版本及 Widget 身份；
不能按版本号区分候选包和正式安装包。当前正式版仍在 [Releases](https://github.com/easonwong2026-del/AICC/releases)。

## 桌面小组件

1. 安装候选 AICC App，保持 Magpie 正在运行；默认本机端口为 3425。
2. 桌面右键 → 编辑小组件 → 添加 AICC 小尺寸或中尺寸。
3. 右键 AICC → 编辑小组件，可独立选择 Magpie 端口及自动/1/2/全部账号。
4. 可以关闭 AICC。Widget 直接读取本机 Magpie，不依赖 Python 桥接或宿主 App 常驻。

默认页面只显示 Codex：单账号并列周与 5h 剩余，多账号分别显示，小尺寸最多 2 个、中尺寸最多
4 个，超过容量显示数量提示。原有非默认指标配置继续解码，但已停用的余额/供应商卡片显示未知，
不会继续展示旧来源缓存或自行推算额度。恢复默认指标配置即可使用 Codex 专用布局。

自动时间线以 5 分钟为目标；同一 Widget 进程对同一端口限制 Codex 专项刷新频率，实际调度由
WidgetKit 决定。右下角按钮直接请求 Magpie 的 Codex 刷新，再读取标准报告。请求失败保留
同端口的匿名缓存，并明确标记缓存；源读数超过 5 分钟也标记缓存。超过 24 小时的本地快照隐藏。

## 墨水屏 APK

1. 打开 AICC 设置，启用“墨水屏桥接”。Mac 与墨水屏连接同一可信局域网。
2. APK 长按打开设置，自动发现或输入 Mac 地址；桥接使用 HTTP 8765、UDP 发现 8766。
3. 在 Mac 点击“生成配对码”，在 APK 输入 8 位码并配对。有效期 2 分钟，只能使用一次，
   最多尝试 5 次。APK 不需要 Magpie 网关 Key 或任何供应商凭据。
4. 可关闭 Mac 设置窗口；启用桥接时 AICC 留在后台，停用时关闭窗口即退出。登录启动可单独开启。

APK 默认每 5 分钟同步，前台或唤醒时立即更新，后台或熄屏时暂停。显示 1/2/全部账号，
全部账号每页最多 4 个，点击切页；额度内容变化时才重绘，保留既有 BOOX 闪刷校准。
Mac 在设置中可撤销全部设备，随后需要重新配对。IP 变化会通过稳定主机身份重新发现；
地址被其他主机占用时不会向其发送已配对凭据。

桥接是独立的只读权限：网络设备只能读匿名 Codex 数据，不能刷新上游、管理账号、路由或关闭服务。
配对凭据在 Mac 钥匙串保存，通过标准输入交给 Python；Android 用 Keystore 加密，关闭备份。
当前 LAN HTTP 传输没有加密，因此只适合可信局域网，不要开放到公网或公共 Wi-Fi。

## 开发与验收

Magpie API、版本边界、插件调查和验收限制见 [迁移说明](docs/MAGPIE-MIGRATION.md)。
Python 桥接只依赖标准库；Swift 使用原生 WidgetKit/App Intents；Android 不增加运行依赖。

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
不再支持 `AICC_CODEX_SOURCE` 或旧供应商采集选项，不自动切换来源。

PR CI 验证 Python、安全边界、Swift 数据与配置、原生 Widget/设置界面、App/Widget 注册、DMG、
Android 单元测试和模拟器截图。CI 候选包不代表已安装或正式发布。
保留原有用户数据目录及旧偏好，升级时只清理历史 AICC 安装器拥有的精确 LaunchAgent 身份。
