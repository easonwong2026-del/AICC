# AICC v2.13.0 瘦身基线（PR-A）

本文是 PR-A 的交付物：**只做测量与判断，不改任何业务行为**。
所有数字都能用仓库里的脚本复现，命令写在每一节标题下面。

- 基线版本：`v2.13.0` / build 25（tag `v2.13.0`，产物由构建输入 `b718cfb` 编译，
  见 `docs/releases/2.13.0.md`）；本文所有数字取自本机 `dist/mac/AICC.app`（同版本、同 build）
- 基线 DMG：`AICC-2.13.0.dmg`，**20,067,290 字节**，
  sha256 `3edbead9cbe3f1c4ec4209ec3acfa8f9501a4b6fcd8e94662e532a746e0559df`
- 测量主机：macOS 27.0.1 / arm64 / 只有 Command Line Tools（无 Xcode）

## 0. 结论先说

| 问题 | 答案 |
| --- | --- |
| 20 MB 由什么组成 | **Python 运行时占 67%**（压缩后 12.90 MB / 20.07 MB）；Rust worker 18%（3.61 MB）；Swift 主程序 10%（2.00 MB）；Widget 1.4%（0.28 MB） |
| 最大的单点浪费 | **Swift 主程序 7.01 MiB 里 4.41 MiB 是 `__LINKEDIT`（几乎全是符号表）**；Widget 1.06 MiB 里 0.61 MiB 同理。发布构建从没 strip 过 |
| 第二个浪费 | **DMG 用的是 `hdiutil` 默认 zlib 级别**（实测 ≈ level 1）。同一份内容换 `UDBZ` 立刻少 2.9 MB，一行脚本改动 |
| Rust worker | profile 已经 `lto=true`、`codegen-units=1`、`strip="symbols"`，很干净。但 `opt-level=3` 让 6.6 MB 全是代码；`opt-level="z"` + `panic="abort"` 后 **7.18 MB → 3.71 MB**，冒烟测试通过 |
| 能达到多少 | 三项叠加实测 **14,669,417 字节（-26.9%）**，优于 15 MB 目标。**但必须按 PR-B 逐项验收，本文不预先承诺** |
| 内存 | 进程组 footprint 上界 **74.8 MB**：Swift 43.2 MB、**Python 桥接 29.4 MB**、worker 5.8 MB。Python 桥接是第二大占用，比 worker 大 5 倍 |
| 空闲 CPU | 三次采样均为 **0.0%**，空闲没有明显忙等 |

> 一句话：**安装包的体积问题几乎全是「没有 strip」和「压缩级别用了默认值」，不是 Python 太大。**
> 真正的 Python 体积问题要等到要不要把桥接迁到 Rust 那一层才谈得上。

---

## 1. 方法与可复现性

```bash
bash macos/build-aicc-swiftui.sh            # 产出 dist/mac/AICC.app
bash scripts/audit-bundle.sh --dmg dist/AICC-2.13.0.dmg --dmg-breakdown
```

PR-A 新增 `scripts/audit-bundle.sh`，它做三件只读的事：

1. 用 `du` / `stat` / `otool` / `nm` 量磁盘体积与 Mach-O 段构成；
2. 对每个 Mach-O **在临时副本上真跑一次 `strip -x`**，报告「strip 后」而不是估算；
3. 用 `hdiutil` 重建 DMG 并逐个移除组件，量出**每个组件的边际压缩贡献**。

产物：`dist/audit/audit.md`、`audit.csv`、`audit.json`（`dist/` 不入库，
按上面命令可原样重建；报告里的每个数字都来自这三个文件）。

第 3 步的意义：磁盘上的 44.9 MiB 和用户下载的 20.07 MB 是两个不同的数，
一个在磁盘上很大但压缩后趋近于 0 的组件不值得动。

**口径（重要）**：第 3 步得到的是**边际贡献**——把某个组件整体删掉再重新压缩，
用两次 DMG 体积相减得到。压缩结果依赖布局，**这些差值不能相加**，也不等于该组件在
DMG 里的精确字节数；它只回答「动这个组件值不值」。可加总的只有脚本直接测到的 DMG 体积本身。

**可复现性证据**：脚本用同一份 App 重建出的 DMG 在 20,067,290 ± 400 字节区间内
（两次运行：20,067,407 / 20,067,646，差 117 / 356 字节，≤0.002%），差异来自 hdiutil 的
卷元数据与时间戳。这说明本报告的测量环境与发布构建是同一回事，不是另一台机器上的近似。

### 1.1 工具的安全边界（PR-A 复审后补）

诊断工具不能改变被诊断的系统。三条硬规则，都有测试守着：

| 工具 / 模式 | 行为 | 守卫 |
| --- | --- | --- |
| `measure-resources.sh --mode sample` | **只读**：不 quit、不 open、不写 App | `scripts/test-measure-resources.sh` 断言动作日志为空 |
| `--mode launch` / `--mode exit` | 会退出正在运行的 AICC | 必须显式 `--allow-lifecycle-actions`；`launch` 还必须 `--allow-real-data` |
| `--mode exit` 的结论 | 退出没生效或仍有残留 = **失败** | 报表先落盘，然后 **exit 3**；不再出现"报告写着残留、命令返回 0" |
| 读不到进程表（`ps` 被拒） | **拒绝运行**，退出码 4 | 看不见 ≠ 没有：曾因沙箱下 `ps` 全部失败，守卫误判"无实例"而准备再次启动 |
| 启动新构建 | 用**真实**账号库 | 没有隔离模式：AICC 没有数据目录参数，`open` 也无法注入环境变量。脚本如实说明，不假装隔离 |
| 其他安装位置正在运行 | 默认**拒绝** | 检测所有同 Bundle ID 实例的实际路径；要退别人的日常构建必须 `--allow-foreign-instance-quit` |
| 退出失败 | **fail closed** | 同 Bundle ID 实例仍在 → 退出码 3，**绝不 `open`**，避免双实例抢端口/数据库 |
| `audit-python-imports.sh` | 跑真实 `server.py` | 子进程用 `env -i` 白名单启动：强制 loopback、关 LAN/discovery、临时数据与证书目录、额度流量指向本地 mock |

`audit-python-imports.sh` 的隔离不是声明，是断言：脚本要求 mock 至少收到一个
`POST /v1/magpie/quotas/refresh`，并在 `127.0.0.1:3425` 放一个 canary socket，命中必须为 0。

canary 的计数由 `scripts/canary_loopback_port.py` 负责，写法刻意避开"退出时才写"的坑：
`bind()+listen()` 成功后**立刻原子写 0**，之后每次 `accept()` 都原子重写计数
（临时文件 + `os.replace`），**不依赖 `atexit` 或信号处理**——`kill` 发的是 SIGTERM，
Python 不会跑 `atexit`，旧实现因此永远停在初始值，让"0 请求"变成"无法判断"。
端口被占用时写 `unavailable`，审计把它当作**合法的降级**并打印代价（只剩 mock 单侧证据）；
只有"既没有计数也不是 unavailable"才算观测缺失、直接失败。审计端会等 canary 就绪后才启动桥接，
避免"监听晚于流量"的假 0。
`scripts/test-audit-python-imports.sh` 会先用**被污染的环境**
（`EINK_HOST=0.0.0.0`、`EINK_DISCOVERY=1`、真实 `AICC_MAGPIE_URL`、`AICC_TOKEN_STDIN`、
`AICC_PARENT_PID`、`*_proxy`）跑一遍，证明这些变量没有被继承。

`scripts/test-audit-python-imports.sh` 先单独验 canary 的三种状态
（空闲 → 0、故意连一次 → 1、端口被占 → `unavailable`），再开启测试开关故意打一次 canary，
要求**审计非零退出**——这条把"命中必须失败"从注释变成了可执行断言。

两个测试都进 CI（`macos-build` 的 `Guard tests for the audit tools`），都不启动、不退出任何真实 App。

---

## 2. 体积拆解

### 2.1 磁盘构成

| 组件 | MiB | 占 App |
| --- | ---: | ---: |
| `Contents/Resources/Python` | 29.80 | 66.4% |
| `Contents/MacOS/AICC`（Swift） | 7.01 | 15.6% |
| `Contents/Helpers/aicc-account-worker`（Rust） | 6.83 | 15.2% |
| `Contents/PlugIns/AICCWidget.appex` | 1.09 | 2.4% |
| `Contents/_CodeSignature` | 0.18 | 0.4% |
| `Contents/Resources/Server`（桥接源码） | 0.06 | 0.1% |
| **合计** | **44.91** | 100% |

文件总数 **620**，符号链接 2，**没有任何重复内容**（sha256 全量比对，> 64 KB 无一重复）。

### 2.2 边际压缩贡献（不是可加总的"分区大小"）

删除该组件后重建 DMG，用两次体积相减得到。**不能相加**（见 §1 口径）。

| 组件 | 移除后的 DMG | 边际贡献 | 占 DMG（仅用于排序） |
| --- | ---: | ---: | ---: |
| Python 运行时（整体） | 6,543,519 | **13,524,127** | 67.4% |
| ├─ 解释器 `bin/python3.14` | 11,976,235 | 8,091,411 | 40.3% |
| └─ 标准库 `lib/python3.14` | 15,329,742 | 4,737,904 | 23.6% |
| Rust worker | 16,278,219 | 3,789,427 | 18.9% |
| Swift 主程序 | 17,966,870 | 2,100,776 | 10.5% |
| Widget 扩展 | 19,776,083 | 291,563 | 1.5% |

Python 解释器本体（18.6 MiB）压缩后仍有 **8.09 MB**：它把 OpenSSL 3.5.9 与全部扩展模块
静态链进去了，这是 `python-build-standalone` 的 `install_only` 形态的固有代价，
不是配置错误。**这一项不动运行时来源就动不了。**

### 2.3 符号表：最大的一块纯浪费

> 术语：`__LINKEDIT` 里**不只是**符号表，还包含 rebase/bind 信息、导出表、函数起始表、
> 代码签名等元数据。所以不能把 4.41 MiB 直接当成「可删除的符号字节」。

| 二进制 | 文件 MiB | `__TEXT` | `__LINKEDIT` | 符号数 | strip -x 后 | 可省 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `Contents/MacOS/AICC` | 7.01 | 2.47 | 4.41 | 23,711 | 2.81 | **4.19 MiB** |
| `AICCWidget` | 1.06 | 0.42 | 0.61 | 4,512 | 0.54 | **0.52 MiB** |
| `python3.14` | 18.58 | 14.75 | 2.11 | 69,527 | 16.75 | **1.83 MiB** |
| `aicc-account-worker` | 6.83 | 6.62 | 0.06 | 227 | 6.83 | 0（已 strip） |

可删除的字节以**实测**为准——下表最后一列是临时副本上真跑 `strip -x` 之后减少的体积，
合计 **6.54 MiB 磁盘**：

- `build-aicc-swiftui.sh` 用 `swiftc -O` 直接产出，**没有 strip 步骤**。
  Xcode 的 release 构建默认 `STRIP_INSTALLED_PRODUCT=YES`，这里是手写脚本，漏了。
- Rust 侧 `strip = "symbols"` 已经生效（只剩 227 个动态符号），所以没有收益。

`strip -x` 只删本地符号，不动动态符号表、不动 `__TEXT`，
对 Swift 运行时反射和内建模块查找没有影响。**但必须在 `codesign` 之前做**：
签名后再改二进制会让内核拒绝执行（本次实测：strip 后未重签的 `python3.14` 直接静默退出）。

### 2.4 strip 之后还能不能跑（实测）

不能只凭"Apple 的 release 也这么做"就下结论。本次真的做了一遍：

1. 复制 `dist/mac/AICC.app` 到临时目录；
2. 对 `AICC`、`AICCWidget`、`python3.14`、`_dbm.so` 执行 `strip -x`（App 44.9 → 38.4 MiB）；
3. 用**与发布相同的固定证书** `AICC Local Code Signing` 按"先内层后外层"重签；
4. `codesign --verify --deep --strict` → 通过；
5. 启动：Swift 宿主、Rust worker、Python 桥接三个进程都起来了，主窗口正常渲染
   （9 个账号、签到状态、平台分布、Magpie 状态全部正常显示，无空白页、无错误提示）；
6. 回环接口 `GET http://127.0.0.1:8765/` 返回 `404` + `{"error":"Pair this device in AICC settings"}`
   —— 这是未配对设备的正常响应，说明桥接服务本身在正常应答；
7. `../Helpers/aicc-account-worker` **没有重签**（它本来就没被 strip），
   所以钥匙串 ACL 绑定的 cdhash 不变，不会触发额外的授权弹窗。

结论：strip 这条路在**代码层面**是安全的。剩余风险只在打包顺序
（必须先 strip 再 codesign），这一点在 PR-B 里用构建脚本固化，不靠人工记得。

---

## 3. Python 运行时审计

```bash
bash scripts/fetch-python-runtime.sh                     # CPython 3.14.10，sha256 固定
bash scripts/audit-python-imports.sh                     # 新增：导入图
bash scripts/validate-python-runtime.sh <runtime>        # 12 项 TLS 硬门
```

### 3.1 构成

| 项 | 值 |
| --- | --- |
| 版本 / OpenSSL | 3.14.10 / OpenSSL 3.5.9（静态链入） |
| 解释器 | 18.6 MiB，`LC_SYMTAB` 有 69,527 个符号 |
| 标准库 | 11.1 MiB，553 个 `.py`、3 个 `.pyc`、1 个 `.so`（`_dbm`） |
| `site-packages` | 0.23 MiB（pip 及其 vendored 依赖） |
| 重复库 | **无**。整个 bundle 没有 sha256 相同的文件 |

没有 `.pyc`/`.py` 双份，这点做得对：`.pyc` 反而更大，而且源码压缩率更好。

### 3.2 实际用到哪些模块（实测，不是猜）

`scripts/audit-python-imports.sh` 用 `sys.addaudithook` 记录 `import` 事件，
并合并 `sys.modules`（`encodings` 在 site 之前就被解释器导入，只靠 hook 会漏），
跑真实 `server.py` 6 秒后 SIGINT 落盘（隔离方式见 §1.1）：

- 有磁盘文件的模块：**114 个**
- 标准库未加载：**447 个 `.py`，7.3 MiB**

未加载的按目录（节选）：

| 目录 | KiB | 文件数 | 判断 |
| --- | ---: | ---: | --- |
| 顶层 `.py`（pydoc/pdb/doctest/tarfile/…） | 3,329 | 100 | 多数可删，需逐个确认 |
| `encodings/` | 1,352 | 119 | **不要删**：编码是按需导入的，删错就是运行时崩溃 |
| `asyncio/` | 523 | 35 | 桥接是 `ThreadingHTTPServer`，用不到 |
| `multiprocessing/` | 302 | 23 | 用不到 |
| `xml/` | 297 | 22 | 用不到（`xmlrpc` 也在里面） |
| `unittest/` | 257 | 13 | 用不到 |
| `_pyrepl/` | 222 | 25 | 用不到（无交互 REPL） |
| `email/` | 186 | 14 | **部分加载**：`urllib` 依赖 `email.parser`，不能整目录删 |
| `logging/` | 186 | 3 | 桥接只用 `print` 到 stderr |
| `wsgiref`/`venv`/`tomllib`/`zoneinfo` | ~150 | — | 用不到 |

**诚实结论**：**未加载 ≠ 可删除**。这份清单只覆盖「跑起来会走到的路径」，
错误分支、罕见编码、证书校验失败路径都不会出现。可以安全删的保守集合
（`asyncio` + `multiprocessing` + `unittest` + `_pyrepl` + `xml`/`xmlrpc` + `wsgiref` + `venv` + `site-packages/pip`）
约 **2.0 MiB 磁盘、压缩后约 0.6–0.9 MB DMG**。收益中等、风险中等，
**优先级低于 strip 与压缩格式，放在 PR-B 的最后一项做**，并且必须配一组行为测试。

### 3.3 TLS 与签名边界

strip 后重签，`validate-python-runtime.sh` **12/12 通过**：

```
CASE tls13_preferred       => PASS: TLSv1.3
CASE tls12_accepted        => PASS: TLSv1.2
CASE tls10_rejected        => PASS: rejected
CASE tls11_rejected        => PASS: rejected
CASE unknown_ca_rejected   => PASS: CERTIFICATE_VERIFY_FAILED
CASE wrong_hostname_rejected => PASS: Hostname mismatch
CASE spki_pin_stable       => PASS: 1rOqYlToKp8vmtmgLNGoNuxPo9gaf7/nJurrMbByFZY=
CASE https_get_200         => PASS: HTTP/1.0 200 OK
SUMMARY pass=12 fail=0
```

SPKI pin 不变是关键：pin 由证书派生，与二进制无关，所以 strip 不影响已配对设备。

### 3.4 要不要把桥接迁到 Rust

**结论：本阶段不建议做，建议先做完 PR-B 的零风险项再评估。**

| 维度 | 评估 |
| --- | --- |
| 收益上限（实测推得） | 省掉整个 Python 树 = **DMG -12.90 MB**（20.07 → 7.2 MB）。这是唯一能到 10 MB 以下的路径 |
| 实际收益 | 真要复刻 TLS（含 LAN 证书签发、固定 pin、mDNS 发现）+ 墨水屏 HTTP/HTTPS 双栈 + Magpie 快照轮询，Rust 侧需引入 `hyper`+`rustls`+`rcgen`+`x509-parser`（已在 worker 里），**净增约 1.5–2.5 MB**，净省约 10 MB |
| 风险 | 高。桥接是**已交付且被真实墨水屏设备依赖**的协议面：TLS 指纹、固定 pin、配对身份、mDNS 广播、`EINK_PORT`/`LAN_PORT` 行为全部要逐位复刻。任何一处偏差 = 用户设备连不上，且**离线设备无法远程修复** |
| 工作量 | 约 1600 行 Python（`server.py` 614 + `bridge_tls.py` + `collectors/magpie.py`）重写 + 双栈并行灰度 + 设备端回归。估计 3–4 个 PR，且需要真机 |
| 兼容成本 | 迁移期 App 内要同时存在两套实现，体积先涨 12 MB 再降，中间版本会更大 |
| 建议 | **不做现在**。先拿到 PR-B 的 -27%；如果之后仍要压到 10 MB 以下，再单独立项（PR-E），并且必须先有一台可回滚的真机验证流程 |

---

## 4. Rust worker

```bash
cd account-worker
cargo build --release --locked \
  --config 'profile.release.opt-level="z"' \
  --config 'profile.release.panic="abort"' \
  --target-dir /tmp/probe
bash scripts/smoke-test-account-worker.sh /tmp/probe/release/aicc-account-worker
```

现状 profile（`account-worker/Cargo.toml`）：`lto = true`、`codegen-units = 1`、`strip = "symbols"`。
这已经是最优的常规组合，符号表也清干净了（227 个动态符号）。剩下的只有代码本身。

实测三档：

| 档位 | 文件字节 | gzip -9 | 相对基线 |
| --- | ---: | ---: | ---: |
| `opt-level=3`（现状） | 7,184,416 | 3,652,148 | — |
| `opt-level="s"` + `panic="abort"` | 4,624,608 | 2,476,075 | -35.6% |
| `opt-level="z"` + `panic="abort"` | **3,711,424** | **2,235,946** | **-48.3%** |

`opt-level="z"` 版本跑 `smoke-test-account-worker.sh` **全部通过**
（自检、JSON-RPC 往返、未知方法报错、`WB_SWITCH_HOME` 隔离）。
依赖侧已经没有可削的：`rustls` 用的是 `ring`（不是体量大得多的 `aws-lc-rs`），
`libsqlite3-sys` 是 bundled（必需的），没有 `openssl`。

风险与代价：

| 项 | 说明 |
| --- | --- |
| `panic="abort"` | 进程 panic 直接 abort，不再 unwind。已确认 `src/` 内**没有 `catch_unwind`、没有 `should_panic` 测试**；父进程 `AccountWorkerManager` 本来就有重启逻辑。代价是崩溃时没有 backtrace 栈展开（仍有 panic message + 位置） |
| `opt-level="z"` | 牺牲运行速度换体积。worker 是网络与 IO 密集（RPC、HTTP、SQLite），实测热点不在纯计算；**但必须在 PR-B 里实测一次完整签到/额度/成长中心链路**，不能只看冒烟 |
| 测试 | `cargo test` 走 dev/test profile，不受 `[profile.release]` 影响，CI 无需改动 |
| 回滚 | 删两行 profile 即可，无代码改动、无 schema 改动 |

**不建议**动的：vendored `wb-switch-core` 的模块裁剪。它固定在上游 commit，
删模块等于改上游行为，而且省不下多少（crate 源码 2.4 MiB，但只有被调用的那部分进二进制）。
这一项收益低、维护成本高（每次上游更新都要重做），**不做**。

---

## 5. DMG 压缩格式：被忽略的 2.9 MB

`macos/build-dmg.sh` 用的是 `hdiutil create -format UDZO`，**没有指定压缩级别**。
实测：App 磁盘内容是 47,087,616 字节，DMG 只有 20,067,290 字节（**压到 42.6%**）；
而把同一份**未压缩镜像**（58,008,576 字节）直接 `gzip -9` 是 15,403,356 字节（**26.6%**）。
也就是说 `hdiutil` 默认级别明显没榨干可压缩空间。

同一份 App（未 strip 的发布内容），只换压缩格式：

| 格式 | DMG 字节 | 相对基线 |
| --- | ---: | ---: |
| `UDZO`（现状，默认级别） | 20,066,915 | — |
| `UDZO -imagekey zlib-level=5` | 17,430,241 | -13.1% |
| `UDZO -imagekey zlib-level=9` | 17,296,107 | -13.8% |
| `ULFO`（lzfse） | 17,243,561 | -14.1% |
| **`UDBZ`（bzip2）** | **17,150,001** | **-14.5%** |

再看叠加效应（越往下越激进）：

| 方案 | DMG 字节 | 相对 20,067,290 |
| --- | ---: | ---: |
| 发布基线（UDZO 默认） | 20,066,915 | — |
| 只换 `UDBZ` | 17,150,001 | **-14.5%** |
| strip + `UDBZ` | 16,020,432 | **-20.2%** |
| strip + worker `opt-level=z` + `UDBZ` | **14,669,417** | **-26.9%** |

**反直觉但必须记录的一点**：如果保持 `UDZO` 而只换更小的 worker，
DMG 反而**变大** 217 KB（18,713,552 → 18,930,575，三次重测一致，且有「同内容重写」对照）。
用 `gzip -9` 直接压整个未压缩镜像时，同一对输入是 15.40 MB → 14.01 MB，**正常变小**。
所以这是 `hdiutil` 默认 zlib 级别下的打包行为，不是二进制的问题 ——
**这恰好说明先换压缩格式、再谈二进制精简，顺序不能颠倒是必要的**，
否则会出现「优化了 3.4 MB 二进制，用户下载反而更大」的荒谬结果。

`UDBZ` 的兼容性：UDIF 的 bzip2 压缩自 macOS 10.4 起就支持，
`hdiutil attach` / `verify` 都能处理；本项目是自签分发、不做公证，不涉及兼容性限制。

**B1 已落地实测**（`macos/build-dmg.sh` 只改格式，其余一行未动）：

| 项 | UDZO（基线） | UDBZ（现在） |
| --- | ---: | ---: |
| DMG 体积（同一份 App） | 20,067,116 | **17,150,065**（-14.54%） |
| `hdiutil verify` | VALID | VALID |
| 首次挂载 | 0.15s | 0.15s |
| 首次拖拽安装（复制出 App，冷） | 0.16s | 0.67s |
| 再次安装（热） | 0.09s | 0.09s |
| 从 DMG 复制出的 App `codesign --verify --deep --strict` | OK | OK |
| 创建 DMG 的 CPU 时间 | 5.0s | 7.0s（一次发版） |

正式产物 `dist/AICC-2.13.0.dmg`：**17,149,777 字节**，sha256 `4758bafd…`；
DMG 内的 `AICC.app` 与构建产物**逐文件 sha256 一致**（620 个文件全等），
说明这一步确实只换了包装，没有碰任何内容。

---

## 6. 内存与空闲资源基线

```bash
# 只读观察：不需要任何授权
bash scripts/measure-resources.sh --mode sample --label idle-30min --warmup 1800 --samples 3

# 冷启动/退出：会退出正在运行的 AICC，必须显式授权
bash scripts/measure-resources.sh --mode launch --label cold-start --settle-timeout 120 \
  --allow-lifecycle-actions --allow-real-data
bash scripts/measure-resources.sh --mode exit --label quit-check --allow-lifecycle-actions
```

`--mode launch` 会用**真实**账号库启动（AICC 没有数据目录参数，`open` 也无法注入环境变量），
所以脚本要求 `--allow-real-data` 并打印这条说明；如果正在运行的是**另一个安装位置**的 AICC，
默认直接拒绝，而不是替你退掉日常在用的那个。

PR-A 重写了 `scripts/measure-resources.sh`：原来只覆盖 Swift + Python 两个进程、
只输出到屏幕；现在覆盖**整个进程组**（Swift 宿主 + Rust worker + Python 桥接 + 系统拉起的 Widget 扩展），
每个进程记 RSS / **macOS footprint** / CPU / 线程 / FD，并输出 CSV + JSON + Markdown，
每次运行追加一行到 `baseline.csv` 便于前后对比。

> 口径说明：RSS 与 footprint 都是**按进程**统计的。多进程相加会把共享页重复计算，
> 所以"进程组合计"只能作为**上界**，不能宣称是"整组占用的物理内存"。
> 需要精确值时以 `footprint` 的单进程数字为准。

（本次实测数据见 §6.1，环境与场景标注见每张表下方。）

### 6.1 实测（本机 macOS arm64，真实构建）

> 过程披露：§6.1 的数据是在复审提出 P0-A **之前**采的。当时 `--mode launch` 会按 Bundle ID
> 退出正在运行的 App，本轮确实退出过一次 `~/Applications` 的 2.12.2（测完已恢复启动）。
> 现在加上 `--allow-lifecycle-actions` / `--allow-real-data` 与"外来实例默认拒绝"之后，
> 同样的测量必须由人明确授权，且不会再退掉另一个安装位置的构建。
> **数据本身不受影响**（测的是 2.13.0/25 的 dist 构建，与 2.12.2 无关）。

测量环境，每次运行都会写进 `summary.json` 的 `context` 段，前后必须一致才可比：

| 条件 | 值 |
| --- | --- |
| App | `dist/mac/AICC.app` 2.13.0 / build 25 |
| 主机 | macOS 27.0.1 arm64 |
| Magpie | **未运行**（快照刷新会快速失败，不影响进程内存） |
| 墨水屏桥接 | **开启**（`bridgeEnabled=1`、`bridgeLANEnabled=1`） |
| 旅行自动任务 | **开启**（`aicc.travelAutoEnabled=1`） |
| Widget | 未添加（无扩展进程） |

**冷启动**（`--mode launch`，先退出旧实例再启动）：

| 里程碑 | 秒（两次运行） |
| --- | ---: |
| Swift 主进程出现 | 0.09 / 0.09 |
| Rust worker 出现 | 1.31 / 1.30 |
| Python 桥接出现 | 1.36 / 1.36 |
| CPU 连续 3 次 <2%（视为启动完成） | **4.85 / 7.16** |
| 启动期间峰值 footprint（单进程最大） | 43.84 MB |
| 启动期间峰值 RSS（单进程最大） | 102.1 MB |

启动耗时两次差异（4.85s vs 7.16s）来自"连续 3 次 CPU<2%"的判定抖动，不是启动变慢：
三个进程出现的时刻两次都稳定在 0.09s / 1.30s / 1.36s。

**各场景稳定态**（footprint 单位 MB，取三次采样中位数）：

| 场景 | Swift | worker | Python | 组合计（上界） |
| --- | ---: | ---: | ---: | ---: |
| 冷启动后（总览打开） | 43.22 | 5.75 | 29.38 | **74.8** |
| 总览页 | 41.91 | 5.54 | 24.33 | 68.5 |
| 账号页 | 41.86 | 5.54 | 24.33 | 68.4 |
| 额度页 | 41.85 | 5.54 | 24.33 | 68.4 |
| 签到页 | 41.89 | 5.54 | 24.33 | 68.4 |
| 关闭主窗口 15s | 36.85 | 5.54 | 24.33 | 63.5 |
| 关闭主窗口 120s | 37.70 | 5.54 | 25.12 | 65.2 |
| 连续刷新 20 次（窗口开） | 42.83 | 5.69 | 28.95 | 73.9 |
| **空闲 30 分钟后**（窗口开） | 40.72 | 7.31 | 28.49 | **73.0** |

对应 RSS（KB，便于和旧文档 `PERFORMANCE-2.4.1.md` 对齐）：Swift 99–107 MB、
worker 15 MB、Python 42–44 MB。RSS 远大于 footprint 是因为共享的只读页（尤其 18 MB 的解释器
`__TEXT`）被算进了每个进程——**这正是不能把 RSS 相加当总内存的原因**。

三条可以直接拿去做 PR-C 的结论：

1. **翻页不涨内存**。账号 / 额度 / 签到 / 总览四页的 Swift footprint 差 <0.06 MB，
   说明四个页面共享同一份 store、切换时既不重建也不释放。所谓"页面越开越占"不成立。
2. **关窗口释放约 5 MB**（41.9 → 36.9 MB），2 分钟后稳定在 37.7 MB。
   窗口关闭不会退出进程（桥接开着），符合设计。
3. **连续刷新 20 次没有增长**：73.9 MB（刷新后）< 74.8 MB（冷启动后），
   FD 从 65 降到 63，线程数不变。没有泄漏迹象，也没有进程异常。
   Python 的 footprint 在 24.3–29.4 MB 之间浮动，是快照缓存的生命周期，不是单调增长。
4. **空闲 30 分钟后 73.0 MB、CPU 0.0%**，与刚启动时基本持平；
   唯一变化是 worker 的 footprint 从 5.5 MB 爬到 7.3 MB（SQLite 页缓存 + 一次快照），
   属于正常缓存行为，30 分钟内没有继续增长的趋势，但**只有一次样本，不足以证明收敛**，
   PR-C 若要动 worker 必须先补 2 小时以上的采样。

**退出**（`--mode exit`）：退出后**无残留进程**，worker 与 Python 桥接都随宿主结束。

> 测量踩坑记录：第一版脚本用 `pgrep -f` 按路径找进程，结果把**脚本自己的 `grep` 进程**
> 也算了进去（它的命令行里含同样的字符串）。现在一律改成 `ps` 快照里按 `argv[0]` 精确匹配，
> 并按父子关系遍历进程树。同理，退出测试期间若手工起过一个同名测试实例，
> 会被误判成"残留"——报告口径必须写清"哪个进程属于谁"。

---

## 7. 前三大优化机会

按复审意见，顺序是**先固定压缩格式、再动二进制**，这样后面每一步都用同一个 UDBZ 基线比较：

| 步骤 | 机会 | 预计收益（DMG） | 风险 | 工作量 | 优先级 |
| --- | --- | ---: | --- | --- | --- |
| 1 | DMG 换 `UDBZ` | **-2.92 MB** | 极低（打包参数，不碰 App） | 1 行 | **P0** |
| 2 | 发布构建加 `strip -x`（Swift/Widget/Python），保留 dSYM 归档 | **-1.13 MB** | 低（Apple 标准做法，必须在签名前） | 3 行 + 归档步骤 | **P0** |
| 3a | worker `opt-level="z"`（单独） | 待测 | 中低（只影响速度） | 1 行 | **P1** |
| 3b | worker `panic="abort"`（单独） | 待测 | 中（崩溃语义变化：不再 unwind） | 1 行 + 故障恢复验证 | **P1** |
| 4 | 标准库裁剪（保守集合） | -0.6 ~ -0.9 MB | 中高（未加载≠可删） | 中 | P2 |
| 5 | 桥接迁 Rust | -10 MB（净） | 高（协议/设备兼容） | 大 | P3，先不立项 |

三项一起上的实测是 **-5.40 MB（-26.9%）**，即 20.07 MB → **14.67 MB**；
但 3a/3b 是**合并测量**的，PR-B 里必须分开评估，不能把两者的收益混在一起。

---

## 8. 后续 PR 的范围

### PR-B：安装包瘦身（依据本文第 2/4/5 节）

**分三步提交、逐步验收**（先固定压缩格式，后面每步都用同一个 UDBZ 基线比较收益）：

**B1 — 只换压缩格式（`build-dmg.sh`：`UDZO` → `UDBZ`）**

验收：DMG 字节数；`hdiutil verify`；`hdiutil attach` 后在 macOS 14+ 上实际拖拽安装一次
并记录耗时；确认卷名、`/Applications` 快捷方式、`Info.plist` 读取正常。
这一步不碰 App 内任何字节，**签名身份与 SPKI 必然不变**。

**B2 — Swift / Widget 符号裁剪（`build-aicc-swiftui.sh`）**

已实现：编译改成两步（`-O -g -wmo -c` + 链接），`dsymutil` 产出**带行号**的 dSYM，
归档到 `dist/symbols/<version>-<build>/`（含未裁剪原件），再对两个二进制执行
`strip -x -S`，**全部在 codesign 之前**完成。

为什么必须两步：单次 `swiftc ... -o bin` 会在结束时删掉中间 `.o`，dsymutil 拿不到
调试映射，只能产出"有符号名、没有行号"的残留 dSYM（实测 `--debug-line` 只有 3 行）。
拆开后 dSYM 有 51,125 行行号表，`.o` 留在 `dist/obj/`。

实测（v2.13.0 / build 25）：

| 二进制 | 裁剪前 | 裁剪后 | 省 | dSYM 行号 |
| --- | ---: | ---: | ---: | ---: |
| `AICC` | 7,080,632 | 2,660,424 | 4,316 KiB | 51,125 |
| `AICCWidget` | 1,078,040 | 516,744 | 548 KiB | 20,077 |

DMG：B1 的 17,149,777 → **16,470,283**（-679,494，-3.96%）；相对发布件 20,067,290
累计 **-17.92%**。磁盘上省 4.75 MiB 只换来 DMG 少 0.68 MB —— 符号表字符串压缩率很高，
这也是为什么"磁盘省了多少"不能直接当"下载省了多少"。

校验：`codesign --verify --deep --strict` 通过；固定身份 `AICC Local Code Signing` 不变；
`validate-widget-bundle.sh`、`validate-account-worker-bundle.sh`、
`validate-python-runtime.sh`（12 项 TLS、SPKI pin）全过；DMG 卷内二进制符号数
55,079 → 1,636，`__swift5` 元数据段 11 个全部保留（Swift 反射走元数据段，不走符号表）。

**符号与包的身份链**（复审后补）：只保留 dSYM 还不够，必须证明它对应**这一个**包。
`dist/symbols/<version>-<build>/manifest.txt` 记录每个二进制的 UUID 与**签名后**的 sha256；
`scripts/verify-symbols.sh` 逐项比对「App 内二进制 ↔ dSYM ↔ manifest」，并把 DMG 挂起来
核对 sha256。`build-dmg.sh` 在打包后自动跑它，任何一项不符就删掉 DMG 并失败。
清单每次构建前清空目录，避免同版本多次实验混用旧符号；`AICC_STRIP=0` 的调试构建
会删掉旧归档并明确声明"本次没有符号"。

**符号不上传**：仓库是 public，Actions artifact 对任何有读权限的用户可见，
所以 CI 只做校验并在日志里打印清单（UUID / sha256 / 版本），dSYM 与未裁剪二进制
留在构建机。已上传的历史 artifact 已删除（见 PR #108 评论）。

`scripts/test-verify-symbols.sh` 有 8 个用例，其中 5 个必须被拒绝：拿 Widget 的 dSYM
冒充 App 的、manifest 记的是另一次构建的 UUID、二进制被改动（UUID 不变 sha256 变）、
没有 manifest、拿另一个文件冒充 DMG。另有一例用 `atos` 把真实地址解析到源码行
（实测 `AccountWorkerProtocol.swift:143`），而不只是"表里有一百行"。

**B2b 实测结论：Python 的 strip 不采纳**（`python3.14` 19,580,280 → 17,648,536 字节，
磁盘省 1.84 MB，但压缩后的 DMG **反而变大**）：

| 构造（除 `python3.14` 外完全相同） | DMG 字节 |
| --- | ---: |
| 不 strip Python（B2 现状） | **16,468,800** |
| `strip -x` | 16,724,757（+255,957） |
| `strip -x -S` | 16,724,846（+256,046） |

两种 strip 变体的结果几乎一致（`-S` 没用：这个解释器本来就没有 `__DWARF` 段），
也就是说**省下 1.84 MB 磁盘会让用户多下载 250 KB**。原因和 worker 那次一样：
压缩器对符号表这类高度重复的内容压得极好，删掉它们反而破坏了可压缩性。
按"先测 DMG 字节再决定是否保留"的规矩，这一条不做，代码路径已回退，
`_dbm.so` 只有 52 KB，更没有意义。

**未做**：worker 的 `opt-level` / `panic` 属于 B3；标准库裁剪（估计 -0.6~-0.9 MB）
是另一条路，需要逐个模块的删除测试，不在 B2 范围。

**B3 — worker 代码生成（`Cargo.toml`）**

`opt-level = "z"` 与 `panic = "abort"` **分别提交、分别评估**：

- `panic="abort"` 改变崩溃语义（不再 unwind），不能只凭"没有 `catch_unwind`"就通过；
  要跑 release profile 编译、启动/退出、故障恢复（worker 被杀后父进程重建）和完整
  RPC/调度回归。
- 两者都要用同一 UDBZ 基线比字节，并附带一次真实签到/额度/成长中心链路。

回滚：每一步都是独立提交，revert 单个文件即可；无数据/schema 变化。

### PR-C：内存与空闲资源（依据 §6）

基线已经把几个猜测排除了。**PR-C 不以重写 `ConsoleStore` 缓存为优先目标**：
四页 footprint 差 <0.06 MB、刷新 20 次无增长，重写它只会增加状态不一致的风险而没有可量化收益。
范围收窄到下面三条：

| 方向 | 依据 | 预期收益 | 风险 |
| --- | --- | --- | --- |
| Python 桥接常驻 24–29 MB | 它是第二大占用，且只在"桥接开启"时才需要；关闭桥接时是否仍常驻需要验证 | 若能在关闭桥接时完全不启动，可省 **24–29 MB**（对这部分用户是唯一的大额内存收益） | 中：桥接关闭时墨水屏本就不可用，但要确认不影响配对入口与设置页状态 |
| Swift 43 MB 里的冷数据 | 关窗只释放 5 MB，说明大部分常驻内存不在窗口视图里 | 未知，必须先做 heap 快照（`vmmap`/`heap`）定位 | 中：`ConsoleStore` 有 67 个 `@Published`，改动容易引入状态不一致 |
| 空闲唤醒 | 三次采样 CPU 均为 0.0%，但这只说明"平均值低"，不等于"没有定时唤醒" | 需要 root 才能用 `powermetrics`，本次无法量化 | 低，但**没有数据就不动** |

**明确不做**的：为了省内存去降低快照刷新频率、缩短缓存寿命、把定时任务改成惰性。
这些都会影响签到、旅行、额度和墨水屏的时效性，收益却只有几 MB。

### PR-D：代码结构

按调用关系拆，不以行数为目标：

| 文件 | 行数 | 方向 |
| --- | ---: | --- |
| `account-worker/src/handlers.rs` | 6,338 | 按业务域拆 RPC 路由（账号/额度/签到/旅行/成长/凭据迁移） |
| `account-worker/src/cred_migrate.rs` | — | 迁移、校验、回滚分离 |
| `macos/.../ConsoleStore.swift` | 1,124 | 按业务拆加载与状态（当前 67 个 `@Published` 全在一个对象里） |
| `macos/.../AccountWorkerManager.swift` | 1,198 | 按模块拆 RPC 接口 |
| `macos/.../ConsoleAccountsView.swift` | 1,216 | 列表与详情拆分 |

**明确不计入内存或体积收益**：拆文件不改变二进制体积，也不改变常驻内存。

---

## 9. 本次没测的东西（以及为什么）

| 项 | 状态 | 原因 |
| --- | --- | --- |
| 空闲唤醒次数（wakeups） | unavailable | `powermetrics` 需要 root，本机不允许 |
| Widget 是否已添加 | unavailable | 系统不提供可靠查询；Widget 进程只在桌面上有实例时才出现 |
| Magpie 运行中的场景 | **未覆盖** | 测量期间 Magpie 未运行，所以"额度刷新拿到真实数据"这条路径的峰值没有测到；§6 的数字是"Magpie 离线"时的基线 |
| 墨水屏真机联调 | 未覆盖 | 本机没有设备；只验证了桥接进程与回环接口在跑 |
| Linux CI 内存对比 | 不做 | 本机是 macOS arm64，Linux runner 的数字没有可比性，不冒充 |

（其余未测量项见 §6 的说明。）
