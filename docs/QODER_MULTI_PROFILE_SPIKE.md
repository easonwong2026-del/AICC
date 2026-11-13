# Qoder CN / International 双版本技术验证（M0）

> PR #70 的前置证据。**只写本机能验证到的事实**；没装、没测的一律标注"未复验"。
> 这份文档改过一次结论：原本按 PRD 设想"每个账号一个 `--user-data-dir`"，
> 实测发现客户端根本不认这个参数，于是方案改成"快照切换"（见 §4）。

- 机器：`rakmacmini`（macOS / Apple Silicon）
- 采集时间：2026-11-13
- 采集方式：`Info.plist` / `lsregister` / 文件系统 / 钥匙串条目元数据 / 客户端自身状态文件 / 官方下载源 / 真实启动实验
- 一句话结论：**两个版本是独立产品（Bundle ID、数据目录、钥匙串全不同）；客户端忽略
  `--user-data-dir`，一个 OS 用户只有一份 userData，所以只能"退出 → 换认证快照 → 再启动"，
  不能并行多开。**

---

## 1. 客户端识别（实测）

| 项目 | Qoder CN | Qoder International |
| --- | --- | --- |
| 是否安装 | **未安装** | 已安装 |
| App 名称 | —（未安装） | `Qoder.app` |
| Bundle ID | `com.qodercn.app`（**来自既有实现与官方发行标识，未复验**） | `com.qoder.app`（实测） |
| 安装路径 | — | `/Applications/Qoder.app` |
| 客户端版本 | — | `0.5.7`（build `87859893`，commit `73fe8d7`，构建于 2026-11-09T09:23:57Z） |
| 可执行文件 | — | `Contents/MacOS/Qoder` |
| 用户数据目录 | `~/Library/Application Support/com.qodercn.app*`（未复验） | `~/Library/Application Support/com.qoder.app.stable`（实测） |
| 认证文件 | `auth.v1.dat`（未复验） | `auth.v1.dat`，531 字节（实测） |
| Keychain Service | `Qoder CN App Safe Storage`（未复验） | `Qoder App Safe Storage`（实测存在，创建于 2026-11-12T03:39:17Z） |
| Keychain Account | `Qoder CN App Key`（未复验） | `Qoder App Key`（实测，来自 `security find-generic-password` 元数据） |
| 最低系统 | — | macOS 12.0 |
| URL Scheme | — | `qoder` / `qoder-app` |

### 1.1 CN 未安装的证据

```
$ ls /Applications | grep -i qoder          → Qoder.app（仅一个）
$ ls ~/Library/Application\ Support | grep -i qoder
  com.qoder.app.stable
  com.qoder.installer
$ security find-generic-password -s "Qoder CN App Safe Storage"
  SecKeychainSearchCopyNext: The specified item could not be found in the keychain.
$ mdfind -name qodercn                      → 无结果
```

### 1.2 CN 参数的来源

1. `account-worker/src/providers/qoder.rs`（PR1a 已在 main 上）里的
   `AUTH_DIR_PREFIX = "com.qodercn.app"` 与钥匙串 `Qoder CN App Safe Storage` /
   `Qoder CN App Key`，来源是该 PR 记录的社区参考实现（88lin/workbuddy-auto-signin）。
2. 独立性旁证：<https://qoder.com.cn/download> 的 HarmonyOS 分发链接是
   `com.qodercn.app.stable`，即 CN 产品线使用 `com.qodercn.*` 命名空间。
3. **这些都没有在真机上跑过 `auth.v1.dat` 解密**，所以 CN 的
   `verified_on_this_machine = false`，界面与 RPC 都会如实显示"未复验"。

### 1.3 为什么不能按域名/名字判断版本

- `download.qoder.com` 与 `download.qoder.com.cn` 的 `latest-mac.yml` **内容完全一致**，
  指向同一个 `Qoder-mac-arm64.zip`（sha256 `e2025a10…9909`，347 MB）。
- 客户端 asar 内同时存在 `qoder.com`、`qoder.com.cn`、`qoder.sh` 的端点，
  还内置了"Qoder (Global) 与 Qoder CN 使用独立账号与数据体系"的多语言文案。
- 结论：**文件名、下载域名、界面语言都不能作为版本判据**，唯一可靠判据是
  `Contents/Info.plist` 的 `CFBundleIdentifier`。`providers::qoder_edition::find_app`
  就是按这个实现的——把 App 改名成 `Weird Name.app` 也能认出来（有单测覆盖）。

---

## 2. 认证格式

International 的 `auth.v1.dat` 头部实测：

```
00000000: 7631 303a e670 1036 9bdb 380f cf1d 7ebd  v10:.p.6..8...~.
```

即 `v10` + 16 字节对齐的密文，与 CN 的信封**形态一致**：

- IV = 16 个空格（0x20）
- PKCS7 填充
- key = PBKDF2-HMAC-SHA1(钥匙串口令, `saltysalt`, 1003 轮, 16 字节)
- AES-128-CBC

差别不在算法，而在**口令来源**：两个版本各自一条钥匙串条目。
因此算法共用（`providers::qoder_crypto`），口令与路径**各自实现**
（`providers::qoder` / `providers::qoder_global`）。

> 未完成项：钥匙串条目 ACL 绑定创建者，AICC 第一次读会弹一次系统授权；
> 自动化环境里点不到那个弹窗，所以"用该口令真正解出明文"这一步留给验收时人工点一次，
> 之后才能补录令牌指纹与到期时间。算法本身有加密-解密往返单测。

---

## 3. 数据目录结构（International 实测）

```
~/Library/Application Support/com.qoder.app.stable/
  auth.v1.dat                   531 B    ← 会话（v10 信封）
  auth-profile-overlays.v1.dat  147 B    ← 同信封，账号 overlay（客户端自身多账号预留）
  auth.machine-id                37 B
  Local State / Preferences / Cookies …
  main.sqlite / qoder-data.v1.json（schemaVersion 22）
  logs/ crash-feedback-v1/ …
```

`~/.qoder/` 另有 CLI 侧状态：

```json
// ~/.qoder/.qoder-app-status.json
{"logged_in": true, "name": "Yi Wang", "email": "e***@gmail.com",
 "avatar_url": "https://qoder.com/users/…/avatars", "product": "qoder",
 "version": "0.5.7", "writer": "main"}
```

两点用途：

1. 身份（邮箱/昵称）客户端自己写在磁盘上，AICC 用它判断"现在登录的是谁"，
   但**不用它判定登录是否有效**（那要以能解出会话为准）。
2. `auth-profile-overlays.v1.dat` 说明客户端内部已经有多账号 overlay 的概念，
   但 AICC 不读不写它，避免和客户端自己的逻辑打架。

---

## 4. 双版本启动与隔离（关键结论）

### 4.1 `--user-data-dir` **无效**

PRD 原设想是"每个账号一个独立 userDataDir，可以并行"。实测否掉了这条路：

| 实验 | 结果 |
| --- | --- |
| 直接执行 `Qoder --user-data-dir=/tmp/qoder-manual` | 进程起来了，但 `/tmp/qoder-manual` **始终为空** |
| 同时观察默认目录 `com.qoder.app.stable` | mtime 持续变化 → 客户端写的还是默认目录 |
| `open -na Qoder --args --user-data-dir=…` | 同上：8 个进程正常跑，自定义目录仍为空 |
| 在 `app.asar` 里搜 `user-data-dir` | **一处都没有** → 客户端从不读这个参数 |
| `HOME=/tmp/qoder-home` 启动 | 只有 `~/.qoder` 跟着迁移，`Library/Application Support` **不迁移** |

结论：`userData` 路径由客户端写死（`<bundleId>.stable` 这种形态），
**一个 OS 用户只有一份**；第二个实例会被单实例锁挡回。

### 4.2 社区工具也是"快照切换"

公开的 Qoder 多账号工具（如 `qoder-account-manager`、`TraeWorkAssistant`）采用的都是
"备份当前登录态 → 恢复到目标账号快照 → 重启客户端"的快照方案，没有一个靠多开。
这与上面的实测一致。

### 4.3 采用的方案：快照切换

| 概念 | 位置 | 谁拥有 |
| --- | --- | --- |
| `live_dir` | 客户端真实数据目录 | 客户端 |
| `snapshot_dir` | `AICC-Dashboard/accounts/qoder/profiles/<profile_id>/` | AICC |

切换流程（`qoder.profiles.open`）：

```
1. 确认客户端没在跑（在跑且不是 AICC 起的 → 直接报错，请用户自己退出）
2. 把 live 认证按文件指纹存回它所属的 Profile（找不到归属就整份挪进 .live-backup/<时间戳>/）
3. 把目标 Profile 的快照写回 live（先写 .tmp 再 rename，避免读到半个文件）
   若目标还没有快照 → 把 live 认证挪走，客户端会进登录页
4. 启动客户端，核实真实进程存在
5. 用文件指纹比对 live 与快照：一致才算"登录的是目标账号"，否则如实报 requiresLogin
```

### 4.4 并行能力

| 版本 | 并行实例 | 依据 |
| --- | --- | --- |
| International | **不支持**（`no`） | 实测：自定义 userDataDir 被忽略，只有一份 userData |
| CN | **未知**（`unknown`） | 本机没装过，不能写成"已确认不支持" |

`qoder.editions.detect` 分别返回 `no` / `unknown`，界面显示"实测不支持并行"/"并行能力未复验"。

### 4.5 另一个实测坑：`ELECTRON_RUN_AS_NODE`

从带 `ELECTRON_RUN_AS_NODE=1` 的环境（终端 / IDE / 某些宿主）里执行
`Qoder`，Electron 会退化成 node，直接报
`Qoder: bad option: --user-data-dir=…` 然后退出。
`qoder_profiles::process::spawn` 在启动前显式摘掉这个变量；否则用户从终端打开的
AICC 永远起不来客户端。

---

## 4.5 切换默认关闭（安全收尾后）

即使文件组事务已经做到原子，**只切换两个认证文件也不等于完整 Profile 隔离**，
见 §7。因此：

- `AICC_QODER_ENABLE_SWITCH=1` 且该版本 `verified_on_this_machine=true` 才允许真实切换；
- 默认只开放：扫描、导入、认证检查、保存快照；
- CN 永远不满足第二个条件，所以在复验前不可能发生真实写操作。

## 5. 进程与归属判定

1. 启动：`<app>/Contents/MacOS/Qoder`（**不传任何参数**），注入
   `AICC_QODER_PROFILE=<profile_id>`。
2. 探测是三态：`confirmed_stopped` / `confirmed_running` / `unknown`。
   `pgrep` 退出码 1 才算"确认没跑"；`pgrep` 不可用、退出码异常、或"报告有匹配但没给 PID"
   一律是 `unknown`，**禁止修改 live 认证**。
3. 核实：`spawn` 成功不算成功。用 `ps -p <pid> -E -ww -o args=` 读到
   `AICC_QODER_PROFILE=<profile_id>` 才算"归属确认"。读不到命令行时用 `kill -0`：
   只有拿到 ESRCH（No such process）才算"已退出"，其它情况都是 `unknown`。
4. 归属：**只认启动时注入的标记**。`argv[0]` 相同不再算证据（同一份 App 用户也能自己开）。
   没有标记 → 不发信号、不判 running，只报 `unknown` 并提示手动退出。
4. 重启对账：`qoder_runtime` 表里的 PID 每次都要重新核实；
   PID 被复用、进程已退出、可执行文件不匹配 → 直接丢弃记录。

---

## 6. 未解决 / 待补

1. **CN 全链路未复验**：装好 `Qoder CN` 后重跑 §1、§2、§4，确认
   `com.qodercn.app` 目录前缀、`Qoder CN App Safe Storage` 条目名、`--user-data-dir`
   是否同样被忽略。
2. **International 解密未见明文**：需要人工在钥匙串弹窗点一次"始终允许"，
   然后跑 `qoder.profiles.inspect` 补录 Token 指纹与到期时间。
3. `auth-profile-overlays.v1.dat` 的语义尚未逆推；当前实现不读不写。
4. **隔离未成立**：只换认证文件无法隔离 `main.sqlite` 里的账号数据（§7），
   所以真实切换默认关闭；要打开必须先解决 §7.2 的最小安全快照并重新验证。
5. 快照切换期间客户端必须处于退出状态：提交前会再 probe 一次，发现被外部启动就回滚；
   但无法阻止用户在"写文件的那几百毫秒"里抢先启动。

### 复验清单（装好 CN 后逐条打勾）

- [ ] `qoder.editions.detect` 同时报出两版，`appPath` 不串
- [ ] CN 的 `auth.v1.dat` 能被 `providers.qoder::read_session` 解出
- [ ] CN 钥匙串条目名确认为 `Qoder CN App Safe Storage` / `Qoder CN App Key`
- [ ] CN 的 `--user-data-dir` 同样无效（预期无效，需要实测确认）
- [ ] CN 快照切换后客户端能正常进入目标账号
- [ ] 两版本交替切换各 5 次，认证不互相污染
- [ ] 重启 AICC 后 PID 对账正确，且不误杀手动启动的客户端

---

## 7. 认证快照 ≠ 完整 Profile 隔离

> 这一节是"切换默认关闭"的依据。结论不是"大概有风险"，而是**在同一份
> `com.qoder.app.stable` 里读到了账号绑定数据**。

调查方法：只读打开客户端数据目录（客户端已退出），按文件/表统计，凭据字段只看长度。

| 状态 | 位置 | 是否账号绑定 | 证据 |
| --- | --- | --- | --- |
| 登录凭据 | `auth.v1.dat` | **是** | `v10` 信封，切换时会替换 |
| 账号 overlay | `auth-profile-overlays.v1.dat` | **是**（语义未逆推） | 同信封，147 B |
| 设备身份 | `auth.machine-id`、`main.sqlite:local_device_identity.device_id` | 否（机器级） | `device_id=c2aad4f9-…`，与账号无关 |
| Cookie | `Cookies` | 当前为空，但不能保证 | `select count(*) from cookies` = **0** |
| Chromium 偏好 | `Local State`、`Preferences` | 未逐键审计 | 主要含 `os_crypt` / 窗口状态，**无证据证明不含账号痕迹** |
| 应用数据 | `main.sqlite` | **是** | 见下 |
| CLI 状态 | `~/.qoder/.qoder-app-status.json` | **是**（切换后会被客户端重写） | 含当前账号 email |
| 插件/市场 | `~/.qoder/plugins`、`extension_installations` | 机器级为主 | 与账号无直接关联证据 |

`main.sqlite` 里直接读到账号维度的行：

```
account_profiles:      account_id = qoder:prod:global:053f2aa8…  (1 行)
agent_profiles:        account_id = 01a1afdc-c495-78ad-8a5c-9123e17e8708 (1 行)
agent_profile_details: account_id = 01a1afdc-c495-78ad-8a5c-9123e17e8708 (1 行)
collaboration_runtime_credentials: account_id = qoder:prod:global:053f2aa8…, encrypted_token(151 B)
```

也就是说：**只换 `auth.v1.dat` 之后，A 账号的 agent 定义、工作角色、
协作运行时凭据仍然留在同一个库里**。客户端按 `account_id` 读取，这些都是跨账号可见/可误用的。
仅凭两个认证文件无法证明"切到 B 之后不会看到 A 的数据"。

### 7.1 因此采取的措施

1. `qoder.profiles.open` / `activate` **默认拒绝**，返回原因；
2. 只有 `AICC_QODER_ENABLE_SWITCH=1` **且**该版本已在本机复验时才允许；
3. CN 永远不满足"已复验"，所以 CN 在装好并验证前不可能发生真实写操作；
4. 保留 `scan` / `import` / `inspect` / `capture`（都不写 live）。

### 7.2 最小安全快照（未来工作，不在本 PR）

要真正隔离，快照至少要覆盖"账号维度"的状态：

- 认证组：`auth.v1.dat` + `auth-profile-overlays.v1.dat`
- 数据组：`main.sqlite` 中按 `account_id` 分区的表（account_profiles / agent_profiles /
  agent_profile_details / collaboration_runtime_credentials / chat_* 等）
- 会话组：`Cookies`、`Local Storage`、`Session Storage`（一旦出现登录态就必须一起换）
- **不要**换：`auth.machine-id`、`local_device_identity`（机器级，换掉反而触发风控）

`main.sqlite` 在客户端运行时持续写入，整库快照需要"客户端完全退出 + 一致性备份（`.backup`）"两个前提，
且体积会随聊天记录增长。这部分要单独设计、单独验证，当前**没有实现**。

## 8. 切换事务与崩溃恢复

切换不再"两个文件 cp 一下"，而是一个带日志的文件组事务：

```
begin   备份 live 组（auth + overlays）到 switch-backups/<journal>，写 manifest.json（含 sha256）
        → 插入 qoder_switch_journal(phase=prepared)；同版本已有未完成 journal 时直接拒绝
stage   把快照组写成 live/.aicc-*.tmp，逐个校验 sha256
        → 目标快照的 sha256 必须等于库里记录的 snapshot_hash，否则判"快照损坏"并中止
        提交前再 probe 一次客户端，发现被外部启动就回滚
commit  逐个 rename 到正式位置，全组再校验一次 sha256 → phase=committed，删除 journal
失败    任一步 I/O 失败 → rollback：按 manifest 把 live 组恢复原样（缺的文件删掉）
崩溃    重启后 list/status/open 都会先跑 recover_pending：所有 prepared/staged 的 journal
        一律按备份回滚，然后删日志
```

要点：

- **备份先落盘**：`begin` 之前不会动 live 一个字节；
- 恢复证据来自 manifest 的 sha256，恢复后还要再校验一次；
- 回滚失败会如实报错（不会假装成功）；
- 备份目录保留最近 20 份，便于人工排查。

对应的故障注入测试（`qoder_profiles::switch` + `manager`）：

| 场景 | 断言 |
| --- | --- |
| pgrep 不可用 | 零写入：live 哈希不变、无 journal、无备份目录 |
| ps 读不到但进程活着 | 拒绝切换，live 不变 |
| 捕获身份不一致 | live 与已有快照都不变 |
| 捕获 I/O 失败 | 不进入 restore，live 不变 |
| 提交第二个文件失败 | 两个文件都回到原值 |
| 崩溃后重启 | 按备份恢复成功，journal 清空 |
| 目标快照损坏 | 覆盖前中止，live 不变 |
| 并发切换 | 唯一索引拒绝第二条 journal |
| 切换窗口被外部启动 | 回滚并报错，live 回到原账号 |
| CN 未复验 | 拒绝真实切换 |
