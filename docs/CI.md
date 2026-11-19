# CI 流水线设计（按变更范围触发）

改造目标只有一个：**改什么跑什么**，同时不牺牲任何安全相关测试。
旧版每个 PR、每次 push 都无条件跑 7 个 job，其中 macOS 两个 job（`swift-test` 9.6 min、
`macos-build` 9.6 min 量级）与改动内容完全无关，一个 Rust 单文件改动也要等全套。

## 1. 结构

```
changes (ubuntu, ~10s)  ← 唯一的分类器
  ├── test           python 单元测试（含 bridge / TLS）
  ├── lint           flake8
  ├── rust           fmt + check --all-targets + cargo test 全量 + worker smoke
  ├── swift-test     Widget/看板/控制台 离屏渲染 + swift test
  ├── macos-build    编译 App + 产物完整性校验（快速通道）
  ├── macos-dmg      DMG 打包 + hdiutil verify + 挂载校验（仅 main / 手动 / 发版）
  ├── android        gradle test + assembleRelease
  └── android-visual 模拟器渲染验收图
```

## 2. 路径 → job 映射

| 改动路径 | 触发 |
| --- | --- |
| `server.py` `bridge_tls.py` `collectors/**` `tests/**` `scripts/smoke-test-bundled-server.sh` | `test` `lint` `macos-build`（包内嵌 Python，必须验一次打包） |
| `account-worker/**` `scripts/smoke-test-account-worker.sh` | `rust` |
| `macos/**` `VERSION` `scripts/{smoke-test-widget*,smoke-test-board-visual,smoke-test-console-visual,smoke-test-dashboard-visual,smoke-test-display-snapshot,validate-widget-bundle,validate-version,validate-python-runtime,validate-account-worker-bundle,fetch-python-runtime,build-widget-preview}.sh` | `swift-test` `macos-build` |
| `android/**` `scripts/smoke-test-android-visual.sh` | `android` `android-visual` |
| `.github/workflows/**` | **全量**（改 CI 自身必须整条验证一遍，否则门控改坏了也永远是绿） |
| 其它（`docs/**` `README*` `CHANGELOG*` `*.md` …） | 无（`changes` 之外全 skip） |

两条**校验脚本**（`validate-python-runtime.sh`、`validate-account-worker-bundle.sh`）挂在
`swift` 上，而不是"它们是谁的周边"：这两个脚本只被 macOS 侧调用（前者由
`fetch-python-runtime.sh` / `build-aicc-swiftui.sh` 调，后者只在打包产物上跑），
如果让它们落到一个不跑 macOS 构建的分类里，就等于"改了闸门却从不通电"。

规则的可执行版本：`scripts/ci-filter-selftest.py`。它**直接解析 ci.yml 里的 filters 块**，
不另存一份规则，因此不会出现"自测和生产的规则各自漂移"。除分类回归外还做一项
**覆盖断言**：把每个 job 会执行的文件（含脚本之间的传递调用）求闭包，要求每个文件
至少能触发一个真正执行它的 job。`changes` job 里跑 `--check`，漏了就红。

## 3. 三个刻意的设计决定

### 3.1 路径过滤只做在 job 级，绝不写 `on.pull_request.paths`

工作流级 path 过滤会让没被触发的检查永远停在 **Expected**，配了 branch protection
的 PR 就永久卡住（GitHub 的既定行为）。这里所有过滤都落在 job 的 `if:` 上：
job 被 `if` 跳过时结论是 **skipped**，分支保护按通过处理，不会阻塞，也不会
凭空多出一个"从未运行"的 Required Check。

因此**所有 job 名保持不变**（`test` `lint` `rust` `swift-test` `macos-build` `android`
`android-visual`），只新增 `macos-dmg`。已经配好的 Required Checks 无需改动。

### 3.2 `changes` 失败时退化成跑全量，而不是假绿

下游条件写成 `needs.changes.outputs.x != 'false'` 而不是 `== 'true'`：

- 正常情况：分类器给 `true`/`false`，行为就是"改什么跑什么"；
- 分类器失败或没产出（`needs` 失败、dorny 报错）：输出是空串，空串 `!= 'false'` 为真
  → 所有 job 照跑。宁可慢一次，也不出现"分类器挂了、检查全 skip、PR 是绿的"。

### 3.3 DMG 与"编译校验"分离

`macos-build` 只做编译 + 产物校验（worker / widget bundle / 内嵌 server smoke），
**9.6 min → 5.9 min（-38%）**。DMG 打包（`hdiutil create` + verify + 挂载校验）
搬到 `macos-dmg`，条件：

```
github.event_name != 'pull_request' && needs.macos-build.result == 'success'
```

即"只在 main / 手动运行 / 发版出现"，且**前置构建失败时不再重复编译一遍**去打包一个
注定坏的产物。

`macos-dmg` 内部仍调用 `macos/build-dmg.sh`，而该脚本会重新编译一次 App。
这是刻意保留的：发版链路上的打包脚本一行不改，代价是 main 上多编译一次
（main 不受延迟影响），换来打包输入与发布链路完全一致。

## 4. 并发

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.event_name == 'pull_request' && format('pr-{0}', github.event.pull_request.number) || format('run-{0}', github.run_id) }}
  cancel-in-progress: true
```

组名用假三元：PR 走 `CI-pr-<编号>`，push / dispatch 走 `CI-run-<run_id>`（天然唯一，
永远不同组）。于是 `cancel-in-progress: true` 的作用范围恰好是"同一个 PR 的上一次运行"：
新提交取消它自己的旧运行，**不会**碰到别的 PR，也不会打断 main 上的运行。

**为什么不用看起来更直观的写法**：`group: ${{ github.workflow }}-${{ github.ref }}` +
`cancel-in-progress: ${{ github.event_name == 'pull_request' }}` 在本仓库实测**没有生效**——
新运行一直挂在 `pending`，旧运行照跑（`refs/pull/<n>/merge` 的组名是对的，但取消动作没发生）。

**另一个坑**：取消是延迟生效的。job 还停在 runner 队列里等 macOS 的时候，
`POST /runs/{id}/cancel` 返回 202，但状态会一直是 `queued`，要等能分配到 runner 才真正变
`cancelled`。所以"排队 + 取消"会叠加，别把这个误判成配置写错。

## 5. 手动全量

`workflow_dispatch` 带 `run_mode` 选择：

| 取值 | 行为 |
| --- | --- |
| `full`（默认） | 除 `macos-dmg` 的 PR 限制外，跑全部 job —— 等价于改造前的全套 CI |
| `only-python` / `only-rust` / `only-swift` / `only-android` | 只跑该领域（+ 其依赖的 macOS 构建），其余 skip，用于验证门控 |

不传参数直接 dispatch 也按 `full` 处理。

## 6. 覆盖没有变薄的地方

- **Rust**：`cargo fmt --check`、`cargo check --all-targets --locked`、`cargo test --locked`
  全量（不做按模块裁剪）、`smoke-test-account-worker.sh`（自检 + JSON-RPC + `WB_SWITCH_HOME` 隔离）
  全部保留，只要动了 `account-worker/**` 就整组运行。
- **Python TLS / 凭据**：`tests/test_bridge_tls.py`、`tests/test_bridge.py`、`tests/test_magpie.py`
  仍在 `test` job 里 `unittest discover` 全量执行；`bridge_tls.py` 与 `collectors/**` 都挂在触发路径上。
- **打包完整性**：`validate-account-worker-bundle.sh`（签名 / 权限）、
  `validate-python-runtime.sh`（内嵌 CPython 版本 / OpenSSL / TLS 1.3+1.2 双重握手）、
  `smoke-test-widget-bundle.sh`（Widget 沙箱）、`smoke-test-bundled-server.sh`
  （内嵌 CPython + TLS 1.3）仍然每个 macOS 构建都跑，且这两个 validate 脚本本身
  也在 `swift` 触发路径上——改校验脚本不会静默跳过校验。
- **视觉回归**：五套离屏渲染脚本一张图都没少，只是整体跟随 `macos/**` 与
  `scripts/smoke-test-*visual*.sh` 触发。

## 7. 已知取舍

1. **`macos-dmg` 会重复编译一次**。为了不动发版链路（`build-dmg.sh` 一行未改），
   main 上 `macos-build` 与 `macos-dmg` 各编译一次。main 不在延迟关键路径上，接受。
2. **`changes` 用一个第三方 action**（`dorny/paths-filter@v3`）。自己用 `git diff`
   写一遍要处理 fork、rename、首个 push、force push 等边界，收益不抵风险；该 action
   是事实标准，固定主版本号，与仓库已有的 `Swatinem/rust-cache`、`gradle/actions` 同类。
3. **非测试类脚本不在任何 job 的触发路径里**（如 `scripts/release-publish.sh`）。
   它们本来就没有任何 job 会执行，所以覆盖断言也不管它们
   ——断言只对"确实被某个 job 执行的文件"负责。`Validate macOS scripts` 那步仍留在
   `macos-build` 内，任何 `scripts/*.sh` 的语法仍会在 Swift/python 改动时被检查到。
   `scripts/audit-bundle.sh` 是例外：它被 `macos-build` 的 `Audit app size` 步骤直接执行，
   所以它和另外两个体积/资源诊断脚本都挂在了 `swift` 触发路径上。
4. **视觉验收图 artifact 只在 macOS 变更时才产出**。需要看某一版的图，改一下
   `macos/**` 或用 `run_mode: full` 手动跑。

## 8. 实测

### 改造前（run 43894780102，PR `feat/rewards-readonly`）

| job | 耗时 |
| --- | --- |
| macos-build | 9.6 min |
| swift-test | 5.5 min |
| rust | 1.7 min |
| android-visual | 2.0 min |
| android | 0.7 min |
| test | 0.3 min |
| lint | 0.2 min |
| **整体墙钟** | **15 min 33 s**（含 macOS runner 排队 5–10 min） |

关键路径是 `macos-build`：与改动内容无关的 macOS 构建 + DMG 打包，占了墙钟的六成以上。

### 改造后（同一批 runner 环境，2026-11-16 20:10Z 之后）

| 场景 | 触发方式 | 实际运行 | 墙钟 | 需要排队的 macOS job |
| --- | --- | --- | --- | --- |
| Android-only | `run_mode=only-android` | android 0.75 + android-visual 1.95，其余 6 个 skipped | **2.1 min** | 0 |
| Rust-only | `run_mode=only-rust` | rust 1.78，其余 7 个 skipped | 15.9 min | 1（14 min 是排队） |
| Swift-only | `run_mode=only-swift` | macos-build 7.05 + swift-test 5.65 + macos-dmg 6.67，其余 6 个 skipped | 28.9 min | 3 |
| 全量（PR，无 DMG） | PR 自身 | 8 个 job 全 success，macos-dmg skipped | 34.9 min | 3 |

全量那一行的明细（run 43954903320）：`macos-build` 5.85、`swift-test` 6.87、
`android-visual` 2.23、`rust` 1.97、`android` 1.03、`test` 0.33、`changes` 0.25、`lint` 0.15。

`macos-build` 自身：**9.6 min → 5.9/6.0 min**（-38%），其中 `Build macOS app` 315s → 242s，
DMG 那 134 s 整体搬走；剩下的是编译 + 三套产物校验 + 内嵌 server smoke，全部保留。

**关于墙钟**：这个时间窗口内 macOS runner 排队 10–15 min/job（`rust` 与 `macos-build`
各排了 14.5 min），墙钟被排队主导，所以在同一个 PR 上比较"墙钟绝对值"意义不大。
真正该比的指标是**一个改动要排队等几个 macOS job**：改造前无条件 3 个
（rust + swift-test + macos-build），改造后按范围是 0 / 1 / 2 / 3。Android-only 的 2.1 min
就是"零 macOS job"时的真实墙钟。

**注意**：本 PR 自身每次 push 都会跑全量——PR 的 diff 里带着 `ci.yml`，命中"改 CI 自身 →
全量"这条规则。合并之后，普通 PR 才会真正享受到按范围触发。

### 附：过滤逻辑的本地回归

`python3 scripts/ci-filter-selftest.py` 直接解析 ci.yml 的 filters 块跑分类，并做覆盖断言：

```
场景                     python rust  swift android macos
Rust-only              skip   run    skip   skip   skip
Swift-only             skip   skip   run    skip   run
Android-only           skip   skip   skip   run    skip
Python-only            run    skip   skip   skip   run
Python runtime 校验脚本  skip   skip   run    skip   run
Docs-only              skip   skip   skip   skip   skip
Rust+Swift             skip   run    run    skip   run
CI 自身                  run    run    run    run    run

覆盖断言通过：每个 job 执行的文件都在对应的触发路径里。
```

`--check` 只跑覆盖断言，`changes` job 里用这个模式；不带参数时额外打印历史提交的分类结果。

### 复现方式

- 全量：`workflow_dispatch` 选 `full`（缺省值），或 CLI `-f run_mode=full`
- 单域：`-f run_mode=only-rust` / `only-swift` / `only-android` / `only-python`
