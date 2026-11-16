# 桥接运行时：PR-B0 实测结论

决策基线：**CPython 3.14.10 + OpenSSL 3.5 LTS**，TLS 最低 1.2、优先 1.3、LAN 不保留明文 HTTP。
本文是 PR-B0（可行性阶段）的交付物：**运行时来源、真实体积、可重定位性、TLS 能力，全部实测**。

## 一句话结论

`python-build-standalone` 的 `install_only` 发行物可用：**25.9 MB 下载 → 70 MB 解包 → 29.8 MB 裁剪 → 26.3 MB 压缩**，
`ssl.OPENSSL_VERSION = OpenSSL 3.5.9`、`HAS_TLSv1_3 = True`，TLS 1.3 优先、TLS 1.2 保留、TLS 1.0/1.1 拒绝，
拷贝到任意路径都能直接跑。**App 增量约 +26 MB（DMG 5.5 MB → 约 32 MB）**。

## 1. 运行时来源

| 项 | 值 |
| --- | --- |
| 上游 | `astral-sh/python-build-standalone`（MPL-2.0 / PSF 组合，CPython 本身 PSF-2.0） |
| 发行物 | `cpython-3.14.10+20261110-aarch64-apple-darwin-install_only.tar.gz` |
| 版本 | CPython **3.14.10**（`main, Nov 11 2026`，Clang 23.1.3） |
| OpenSSL | **3.5.9**（`OpenSSL 3.5.9 29 Sep 2026`，静态链进解释器） |
| 校验 | sha256 `42031ed5f0d38777335252c8c77d64708c8010dd67c128246bdec25000672666`，取自该 release 的 `SHA256SUMS` |
| 平台 | macOS arm64（`aarch64-apple-darwin`） |

获取与校验脚本化，不靠手工下载：`bash scripts/fetch-python-runtime.sh`（幂等，sha256 不符直接中止）。

## 2. 为什么不是 python.org 的 framework 包

实测 `python-3.14.10-macos11.pkg`（72 MB，其中 66 MB 是 Documentation）：

| 项 | 结果 |
| --- | --- |
| `Python.framework/Versions/3.14` 解包后 | 128 MB |
| 删 test/Tcl/Tk/share/include/idlelib/ensurepip 后 | 65 MB |
| 再 `zip -9` | 33 MB |
| **能否嵌入** | **不能**：`bin/python3.14` 与全部 `lib-dynload/*.so` 按绝对路径链 `/Library/Frameworks/Python.framework/...`，改 install name 立即破坏 arm64 签名（实测进程被杀 `exit=137`），要逐个重签几十个文件 |

那份包是给"装到 `/Library`"设计的；`install_only` 是为嵌入构建的，`otool -L` 只剩 `@rpath` / `/usr/lib` / `/System/Library`。

## 3. 体积（实测，本机 arm64）

| 项目 | 体积 |
| --- | --- |
| 下载包 | **25.9 MB**（27,109,898 B） |
| 解包后 | **70 MB** |
| 裁剪后 | **29.8 MB**（30,504 KB） |
| 压缩后（`zip -9`） | **26.3 MB**（27,623,171 B） |
| App 增量 | **约 +26 MB**：现 DMG 5.5 MB → 预计约 32 MB（真实 DMG 增量在 PR-B 集成后复测，此处按压缩后体积估） |
| 动态库 | **9 个 → 1 个**：裁剪后只剩 1 个 `.so`（`_dbm`，293 KB），其余扩展全部静态链进 `bin/python3.14` |
| 文件数 | 604 个 |

压缩率只有 88%（29.8 → 26.3 MB）：19 MB 的 `python3.14` 可执行文件本身压不动，`.so`/`.dylib` 压缩率也低。
**别按"解包体积 × 30%"估算 DMG 增量。**

## 4. 裁剪清单（每一项都有依据）

删掉的都不是运行必需：

| 删除 | 体积 | 依据 |
| --- | --- | --- |
| `lib/libpython3.14.dylib` | **19.1 MB** | `bin/python3.14` 是静态链接（`otool -L` 只有 libSystem/libz/libedit/CoreFoundation），全树没有任何文件引用这个 dylib。它只服务于编译第三方扩展。删掉后 `validate-python-runtime.sh` 实跑通过：解释器启动、扩展导入、TLS 握手全部正常 |
| `lib/tcl9.0`、`lib/tk9.0`、`lib/tcl9`、`lib/tk9`、`lib/itcl4.3.8`、`lib/thread3.0.6`、`libtcl9.0.dylib`、`libtcl9tk9.0.dylib`、`lib/python3.14/lib-dynload/_tkinter.*.so` | 约 8 MB | 桥接是 HTTP 服务，不用 Tk |
| `lib/python3.14/{test,idlelib,ensurepip,pydoc_data,tkinter,turtledemo,turtle.py,lib2to3,config-3.14-darwin}` | 约 4 MB | 解释器自带测试/工具，`server.py` 不涉及 |
| `site-packages/{pip,setuptools,_distutils_hack,pkg_resources}`、`bin/{pip,pip3,idle3}`、`share`、`include`、`lib/pkgconfig` | 约 6 MB | 运行时不需要装包 |

保留：`encodings`（1.7 MB）、`lib/python3.14` 全量 stdlib、`lib-dynload/_dbm`。

**裁剪不是盲删**：`scripts/validate-python-runtime.sh` 会在裁剪后立刻实跑一遍，
删错任何一个必需模块都会让构建失败，不会带着坏运行时打包。

## 5. 硬校验（`scripts/validate-python-runtime.sh`）

构建流程里必须先跑这个脚本，任何一项不过就中止：

| 检查 | 做法 | 当前结果 |
| --- | --- | --- |
| 版本 | 实跑 `sys.version` | `3.14.10` |
| OpenSSL | 实跑 `ssl.OPENSSL_VERSION` 必须是 `OpenSSL 3.5*` | `OpenSSL 3.5.9 29 Sep 2026` |
| TLS 1.3 编译进来 | `ssl.HAS_TLSv1_3` | `True` |
| 必需扩展 | 导入 `_ssl _hashlib json sqlite3 hmac secrets socket threading uuid http.server urllib.request` | 通过 |
| 依赖干净 | 遍历全树 Mach-O，`otool -L` 每条依赖只允许 `@…` / `/usr/lib/` / `/System/Library/` | 2 个 Mach-O、7 条依赖，全部合规 |
| 可重定位 | 把运行时**拷贝到临时目录**，清空 `PYTHONHOME`/`PYTHONPATH` 后实跑 | 通过 |
| 真实握手 | `tests/runtime/tls_selfcheck.py`，服务端 + 客户端都在这个解释器里 | 12/12 通过（见下） |

TLS 自检输出（`tests/runtime/tls_selfcheck.py`）：

```
CASE openssl_is_3_5                     => PASS: 3.5.9
CASE tls13_compiled_in                  => PASS: True
CASE tls13_preferred                    => PASS: TLSv1.3
CASE tls12_accepted                     => PASS: TLSv1.2
CASE tls10_rejected                     => PASS: rejected: NO_PROTOCOLS_AVAILABLE
CASE tls11_rejected                     => PASS: rejected: NO_PROTOCOLS_AVAILABLE
CASE no_verify_is_test_side_only        => PASS: verify_mode NONE still connects (test scaffolding)
CASE unknown_ca_rejected                => PASS: rejected: CERTIFICATE_VERIFY_FAILED
CASE wrong_hostname_rejected            => PASS: rejected: Hostname mismatch, certificate is not valid for 'wrong.example'.
CASE spki_pin_stable                    => PASS: 44 字符 base64（SHA-256 over SPKI）
CASE spki_pin_differs_for_other_cert    => PASS: second cert has a distinct pin
CASE https_get_200                      => PASS: HTTP/1.0 200 OK
SUMMARY pass=12 fail=0
```

两端都在同一个解释器里跑，所以结论不受本机 Homebrew OpenSSL、macOS LibreSSL（3.3.6）或开发者 CA 库影响。

## 6. 桥接本身在这个运行时下能跑

用内置运行时直接启动现有 `server.py`（`EINK_HOST=127.0.0.1 EINK_PORT=8801`）：

```
GET /api/health/live  -> 200 {"protocol":"aicc","version":"2.12.2","status":"live",...}
GET /api/status       -> 401（无 Bearer，符合预期）
启动开销（import ssl + 监听） 0.12–0.15 s
```

说明换运行时不影响现有本机管理 API 的行为。

## 7. 打包与签名纪律

- 放 `AICC.app/Contents/Resources/Python/`。**实测**：放进 `Contents/Frameworks/` 时 `codesign` 会把
  `lib/python3.14` 当嵌套 bundle 校验并拒绝封签（`bundle format unrecognized ... In subcomponent: lib/python3.14`），
  换到 `Resources` 下就正常；两个 Mach-O 单独签完，`codesign --verify --deep --strict` 通过。
  `Resources/Server/data` 那条限制只针对运行期写进 bundle 的数据，与运行时无关。
- 签名由内到外：`.so`/`.dylib` → `Contents/Frameworks/Python`（`--deep` 对该目录）→ 主程序 → App bundle → Widget → DMG。
  本项目 ad-hoc 环境没有稳定证书，不能用带 entitlements 的 `codesign --deep` 一把过。
- 好消息是嵌套组件少：裁剪后只有 **1 个 `.so`**，签名面比 python.org 路线小两个数量级。
- 签完必须再跑一次 `validate-python-runtime.sh`（对 App 内的路径），因为打包过程会改动文件属性。

## 8. 解释器选择要改

```swift
// 现在：环境变量 → Homebrew → /usr/local → 系统 /usr/bin/python3
// 目标：内置优先，只有开发/诊断才允许覆盖
let candidates = [
    Bundle.main.bundleURL.appendingPathComponent("Contents/Frameworks/Python/bin/python3").path,
    isDevelopment ? environment["AICC_PYTHON_PATH"] : nil,
]
```

`AICC_PYTHON_PATH` 只在 `AICC_DEV=1` 时生效，否则正式包里的行为会因为开发者机器装了 Homebrew 而不同。
另外要把 Swift 注入的 `PATH`（现在含 `/opt/homebrew/bin`）一并收敛：内置运行时不需要它。

## 9. POKE4S 设备侧已验证的事实（PR #91 探针，已并入 main）

设备：Poke4S（Onyx）/ Android 11（API 30）/ arm64-v8a / 758×1024 @212dpi / serial `46C8BEA3`，
构建 `2025-04-02_08-15_v4.0-rel`。探针代码与复现步骤在 `android/tls-probe/`，**测试私钥和测试 CA 不进正式产品**。

```
CASE tls12_handshake              => PASS: TLSv1.2
CASE tls13_handshake              => PASS: TLSv1.3
CASE https_health_200             => PASS: status 200
CASE https_status_401_without_token => PASS: status 401
CASE untrusted_server_rejected    => PASS: rejected: SSLHandshakeException
CASE pinning_match                => PASS: Si8yl4K5Gvxq14fACg95+3MJ2QMTCXRC3dM5DJ9sI1Y=
CASE pinning_mismatch_detected    => PASS: wrong pin does not match
CASE cleartext_blocked            => PASS: blocked: IOException
SUMMARY pass=8 fail=0 elapsedMs=512
```

三条结论直接约束 PR-B：

1. **设备支持 TLS 1.3，服务端不支持**——App 侧用 `/usr/bin/python3`（3.9.6 / LibreSSL 2.8.3 / `HAS_TLSv1_3 == False`）。
   所以 1.3 必须靠本文的自带运行时，不能默认"设备支持就行"。
2. **现有 APK 连不上自签 HTTPS**——`poke-dashboard` 没有 `networkSecurityConfig`、没有自定义 `TrustManager`，
   走系统信任库，而 Android 7+ 不信任用户 CA。PR-B 必须同时改两端。
3. **负向用例要设计对**：最初用"系统信任库连我们的服务器期望失败"，第一轮竟然成功了——差点得出"这套配置在 trust-all"
   的错误结论。改成"第二套未安装的 CA 提供的服务器，期望被拒"后才站得住（不依赖设备信任库里恰好有什么）。

探针只覆盖 `127.0.0.1` + `adb reverse`、RSA-2048 单链；**LAN、UDP 发现、ECDSA、证书轮换都没验**，这些是 PR-B 的活。

## 10. 遗留与风险

| 问题 | 状态 |
| --- | --- |
| 真实 DMG 增量 | 未测，PR-B 集成后复测（当前估算 +26 MB） |
| 证书生成方案 | 未定。运行时只有 OpenSSL 库、没有 `openssl` CLI；候选：Rust `rcgen`（account-worker 已在用 Rust）生成 PEM，Python 只负责 `load_cert_chain`。PR-B 内定 |
| 首次启动解压/校验开销 | 无（运行时随 DMG 分发，不在用户机上加解压步骤） |
| x86_64 Mac | 未覆盖。上游有 `x86_64-apple-darwin` 资产，若要支持需再加一条构建线 |
| 本机网络 | GitHub release 大文件会中途断流（本次分 26 段续传才下完）。`fetch-python-runtime.sh` 内置断点续传：缓存摘要不符会先丢弃再重下，下载没到全量字节不会把 `.part` 提升为正式文件（否则会把坏文件固化进缓存，之后每次都校验失败且无法自愈） |
| 许可证合规 | CPython PSF-2.0、OpenSSL Apache-2.0、Tcl 不在包内（已删）。发布时需附第三方声明 |
