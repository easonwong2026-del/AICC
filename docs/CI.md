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
| `account-worker/**` `scripts/smoke-test-account-worker.sh` `scripts/validate-account-worker-bundle.sh` | `rust` |
| `macos/**` `VERSION` `scripts/{smoke-test-widget*,smoke-test-board-visual,smoke-test-console-visual,smoke-test-dashboard-visual,smoke-test-display-snapshot,validate-widget-bundle,validate-version,fetch-python-runtime,build-widget-preview}.sh` | `swift-test` `macos-build` |
| `android/**` `scripts/smoke-test-android-visual.sh` | `android` `android-visual` |
| `.github/workflows/**` | **全量**（改 CI 自身必须整条验证一遍，否则门控改坏了也永远是绿） |
| 其它（`docs/**` `README*` `CHANGELOG*` `*.md` …） | 无（`changes` 之外全 skip） |

规则的可执行版本：`scripts/ci-filter-selftest.py`。改过滤规则后先本地跑一遍，
它会用真实历史提交回归分类结果。

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
约 9.6 min → **7.4 min**。DMG 打包（`hdiutil create` + verify + 挂载校验）
搬到 `macos-dmg`，条件 `github.event_name != 'pull_request'`，只在 main、手动运行、
发版流程出现。

`macos-dmg` 内部仍调用 `macos/build-dmg.sh`，而该脚本会重新编译一次 App。
这是刻意保留的：发版链路上的打包脚本一行不改，代价是 main 上多编译一次
（main 不受延迟影响），换来打包输入与发布链路完全一致。

## 4. 并发

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

`github.ref` 在 PR 上是 `refs/pull/<n>/merge`，天然按 PR 隔离：同一个 PR 推新提交
会取消它自己上一次的运行，**不会**影响其它 PR。`push` / `workflow_dispatch` 不取消
（`cancel-in-progress` 为 false），避免 main 上的运行被后来的运行打断。

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
  `smoke-test-widget-bundle.sh`（Widget 沙箱）、`smoke-test-bundled-server.sh`
  （内嵌 CPython + TLS 1.3）仍然每个 macOS 构建都跑。
- **视觉回归**：五套离屏渲染脚本一张图都没少，只是整体跟随 `macos/**` 与
  `scripts/smoke-test-*visual*.sh` 触发。

## 7. 实测

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

### 改造后

见 PR 描述里的对照表（每个场景都附了实际的 run 链接）。
