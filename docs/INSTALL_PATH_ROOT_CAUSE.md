# `/Applications` 启动卡在 `_dyld_start`：根因核查

结论先说：**和安装目录无关。** `_dyld_start` 是 Gatekeeper 对「未公证 + 带
`com.apple.quarantine`」的包做首次评估时，把进程挂起在 dyld 之前等待确认的表现。
确认框属于 `CoreServicesUIAgent`，只有「完成 / 移到废纸篓」两个按钮；AICC 是
`.accessory` 应用、没有 Dock 图标，所以用户看到的是「点开没反应 / 还是旧界面」。

核查日期：2026-11-22，本机 macOS 27.0.1 (26A434) / arm64。

## 1. 待验证的说法

> 本机 `~/Applications/AICC.app` 可运行，但从 `/Applications/AICC.app` 启动会永久卡在
> `_dyld_start`（2.12.x 也曾复现）→ 「本机 `/Applications` 环境问题」。

这个说法把**两个变量混在一起**：安装目录，和包上的 `com.apple.quarantine`。
历史上那次「对照」的做法是：

```bash
ditto dist/mac/AICC.app /Applications/AICC.app          # 只是复制
xattr -w com.apple.quarantine "0081;00000000;Safari;" /Applications/AICC.app   # 只有这一份被写了隔离属性
open /Applications/AICC.app                              # → 卡住
open dist/mac/AICC.app                                   # 另一份没有隔离属性 → 正常
```

也就是说，被比较的两份副本在**隔离属性**上不同，而不只是目录不同。

## 2. 复现方法

为了排除「这个 cdhash 之前被批准过」的缓存影响，用**全新 cdhash** 做四格对照：

```bash
ditto ~/Applications/AICC.app /tmp/aicc-probe/AICC.app
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 26' /tmp/aicc-probe/AICC.app/Contents/Info.plist
codesign --force --sign "AICC Local Code Signing" --preserve-metadata=entitlements,flags \
  /tmp/aicc-probe/AICC.app          # CDHash=784fbefe65d78286671222d63b11e6396ab73c04（此前从未运行）
```

每组都在启动 6 秒后取 `sample` 的第一帧与 footprint，并核对 `lsof` 里的真实二进制路径。

## 3. 结果矩阵

| # | 路径 | 隔离属性 | 转位 | `sample` 第一帧 | footprint | 结论 |
|---|---|---|---|---|---|---|
| A | `/Applications/AICC-probe.app` | 无 | 否 | `start (in dyld) + 6688` | 38.3 M | 正常 |
| B | `~/Applications/AICC-probe.app` | **有** | **是** | **`_dyld_start + 0`** | **96 K** | **卡住** |
| C | `/Applications/AICC-probe.app` | **有** | **是** | **`_dyld_start + 0`** | **112 K** | **卡住** |
| D | `~/Applications/AICC-probe.app` | 无（曾被拦过） | 否 | `_dyld_start + 0` | 112 K | 卡住（队列里还有未处理的确认框） |
| D2 | `/Applications/AICC-probe2.app` | 无 | 否 | `start (in dyld) + 6688` | 38.6 M | 正常 |
| E | `~/Applications/AICC-probe3.app` | 无 | 否 | `start (in dyld) + 6688` | 38.3 M | 正常 |
| F | `/Applications/AICC-probe.app`（关掉确认框后） | 无 | 否 | `start (in dyld) + 6688` | 38.5 M | 正常 |

关键读法：

- **B 与 C 都卡住，A 与 E 都正常** → 决定因素是隔离属性，不是目录。
- **D 卡住、F 正常**，两者路径相同、都没有隔离属性 → **不存在「路径被永久毒化」**；
  D 只是排在上一次未关闭的确认框后面。
- 卡住时的特征完全一致：`_dyld_start + 0`（dyld 第一条指令，一次都没执行）、
  footprint 96–112 KB、`Binary images description not available`（没有任何已加载镜像）、
  进程状态 `S`（在等，不是被 SIGSTOP）。

## 4. 机制

1. 包上带 `com.apple.quarantine`（从浏览器下载的 DMG 拖出来就会带上）。
2. 签名是 `AICC Local Code Signing`（自签，未公证）→ `syspolicyd` 无法给出通过结论。
3. LaunchServices 仍然创建进程，但**先挂起**，等 Gatekeeper 结论；
   同时 `CoreServicesUIAgent` 弹出：

   ```
   未打开"AICC-probe"
   Apple 无法验证"AICC-probe"是否包含可能危害 Mac 安全或泄漏隐私的恶意软件。
   [帮助]  [完成]  [移到废纸篓]
   ```

   新版 macOS 的这个框**没有「打开」按钮**，要放行只能去
   「系统设置 → 隐私与安全性 → 仍要打开」。
4. 没人应答这个框，进程就一直停在 dyld 之前 —— 不超时、不退出、不写日志。
5. AICC 是 `.accessory` 应用、无 Dock 图标，确认框属于 `CoreServicesUIAgent` 而不是 AICC，
   很容易被其它窗口盖住；用户点两次图标只会再排一个等待项，看起来就是「永久卡死」。
6. 关闭确认框（上面表格的 F）后，同一路径立刻恢复正常，也不需要重装。

补充证据：同一份包在 `/Applications` 下**不带隔离属性**时，`codesign --verify --deep --strict`
通过、窗口 1120×760 正常出现、worker 与内置 Python 都起来；`spctl -a -vv` 对该自签包
始终是 `rejected`，这是自签未公证的必然结果，与能否启动无关。

## 5. 尚未成立的替代假设

| 假设 | 结论 |
|---|---|
| `/Applications` 权限或 `sunlnk` 标志导致 | 不成立：A、D2 在 `/Applications` 正常运行；`sunlnk` 是系统目录的常规标志 |
| 签名或 Bundle ID 不同 | 不成立：四组用同一份 `ditto` 副本、同一 cdhash、同一 Bundle ID |
| 二进制损坏 / 版本错 | 不成立：卡住组的 `shasum` 与正常组逐字节相同 |
| LaunchServices 注册损坏 | 不成立：`lsregister -u/-f` 重注册后仍卡；关掉确认框后不重注册也正常 |
| 路径被永久记住为「拒绝」 | 不成立：F 组证明同一路径可以恢复 |

## 6. 产品侧的修复

进程被挂在 dyld 之前，App 自己没有任何代码在执行，所以**这段无法用代码拦截**。
能修的是它周围的三件事：

1. **DMG 里写清放行步骤**（本轮新增 `macos/build-dmg.sh` 生成 `安装说明.txt`）：
   拖到 Applications → 首次被拦时去「系统设置 → 隐私与安全性 → 仍要打开」，
   或先执行 `xattr -dr com.apple.quarantine "<安装路径>"`。快捷方式仍指向 `/Applications`，
   因为该位置实测可用；`~/Applications` 同样受支持，说明里一并写明。
2. **首次成功启动后自动清隔离**（既有 `WidgetHealthService`）：之后不再触发评估。
3. **小组件自愈覆盖 `~/Applications`**（本轮修复 `WidgetInstallation.installRoots`）：
   原来只查 `/Applications`，装在 `~/Applications` 的机器转位启动时找不到安装副本，
   `xattr`/`pluginkit` 全打在只读副本上，小组件永远是黑块。现在按
   `/Applications` → `~/Applications` 顺序查找，并分别记录「找不到安装副本」
   与「清隔离失败」两种原因。

## 7. 给用户的一行处置

旧机器上已经卡过的副本不需要删：

```bash
# 1) 关掉可能还挂着的确认框：屏幕右上/中间找「未打开"AICC"」，点「完成」
# 2) 清掉隔离属性（路径换成实际位置）
xattr -dr com.apple.quarantine "/Applications/AICC.app"
# 3) 重新打开
open "/Applications/AICC.app"
```
