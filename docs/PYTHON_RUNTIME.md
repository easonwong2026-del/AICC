# 桥接运行时（PR-B 前置调研）

决策基线：**CPython 3.14.10 + OpenSSL 3.5 LTS**，TLS 最低 1.2、默认优先 1.3、LAN 不保留明文 HTTP。
本文只回答一件事：**这个运行时怎么装进 App，DMG 会大多少。**

## 为什么不能把 python.org 的安装包直接塞进去

实测（macOS，`python-3.14.10-macos11.pkg`，72 MB，Apple notary 已验证签名）：

| 项 | 结果 |
| --- | --- |
| 安装包 | 72 MB，其中 **66 MB 是 Documentation**，App 不需要 |
| `Python.framework/Versions/3.14`（解包后） | **128 MB** |
| 删掉 `test`(37M) / `Frameworks`(Tcl,Tk 14M) / `share`(4.7M) / `include`(2.6M) / `idlelib` / `ensurepip` / `pydoc_data` / `__pycache__` 后 | **65 MB** |
| 上述 65 MB 再 `zip -9` | **33 MB** |

**但 65 MB 这个数字不可用**，因为这份二进制不可重定位：

```
$ otool -L bin/python3.14
    /Library/Frameworks/Python.framework/Versions/3.14/Python
$ otool -L lib/python3.14/lib-dynload/_ssl.cpython-314-darwin.so
    /Library/Frameworks/Python.framework/Versions/3.14/lib/libssl.3.dylib
    /Library/Frameworks/Python.framework/Versions/3.14/lib/libcrypto.3.dylib
```

两个后果：

1. 每个扩展模块（`lib-dynload/*.so` 有几十个）都硬编码 `/Library/Frameworks/...` 绝对路径，
   嵌入 App 后要逐个 `install_name_tool -change`。
2. 改过二进制之后 **arm64 的代码签名立即失效**，进程直接被系统杀掉（实测 `exit=137`）。
   所以每个文件都得重新签 —— 用固定证书签几十个 `.so`，还要过 hardened runtime。

结论：python.org 的 framework 包是给"装到 /Library"设计的，不是给嵌入设计的。

## 建议方案：python-build-standalone（install_only）

`astral-sh/python-build-standalone` 的 `install_only` 包就是为嵌入准备的：
`bin/python3` 用相对路径、stdlib 已剔除 `test`/`tk`、体积通常只有 framework 包的一半左右。

**本轮未实测**：本机网络对 GitHub release 大文件反复断流（`Empty reply` / 协议错误），
python.org 的包也是分 12 段续传才下完。请在 PR-B 里用 CI 或正常网络跑一次同样的测量，
把真实数字补进来，不要沿用本文的 65 MB。

测量脚本（拿到 tarball 后直接可用）：

```bash
tar xzf cpython-3.14.x-*-aarch64-apple-darwin-install_only.tar.gz -C /tmp/py
du -sh /tmp/py/python                       # 解压体积
/tmp/py/python/bin/python3 -c "import ssl,sys; print(sys.version.split()[0], ssl.OPENSSL_VERSION, ssl.HAS_TLSv1_3)"
otool -L /tmp/py/python/bin/python3 | head -3     # 必须是 @executable_path / @rpath，不能有 /Library
zip -qr -9 /tmp/py.zip -j /dev/null /tmp/py/python && ls -lh /tmp/py.zip   # 压缩后 ≈ DMG 增量
```

## 打包与签名要点

- 运行时放 `AICC.app/Contents/Frameworks/`（与 `AICCWidget.appex`、`aicc-account-worker` 同级），
  **不要**放 `Contents/Resources/`：`Resources` 会被 2.12.2 起的 bundle 校验当成"运行数据"，
  而且 Framework 需要在 `Contents/Frameworks` 才会被 `codesign --deep` 正确覆盖。
- 签名的顺序是"先内层后外层"：所有 `.so` / `.dylib` → `python3` → 运行时目录 → App。
  `codesign --deep` 在带 entitlements 的宿主上不稳定，建议显式遍历。
- 构建脚本必须在签完之后**实跑一次** `ssl.OPENSSL_VERSION` 与 `ssl.HAS_TLSv1_3`，
  只看 Python 版本号不能证明 OpenSSL 是 3.5。
- `validate-version.sh` / `validate-widget-bundle.sh` 之外再加一条：运行时目录里
  不允许出现 `/Library/Frameworks` 的绝对依赖（`otool -L` 全量扫一遍）。

## 解释器选择要改

```swift
// 现在：环境变量 → Homebrew → /usr/local → 系统
// 目标：内置优先，只有开发才允许覆盖
let candidates = [
    Bundle.main.bundleURL.appendingPathComponent("Contents/Frameworks/Python/bin/python3").path,
    environment["AICC_PYTHON_PATH"],   // 仅当 AICC_DEV=1
]
```

同一个安装包的表现不能取决于用户机器上装没装 Homebrew。

## 体积结论（当前能给到的）

- python.org framework 路线：**65 MB 解压 / 33 MB 压缩**，且需逐个改写依赖 + 重签 —— 不推荐。
- python-build-standalone 路线：**待实测**，预期比上面小，且无需改依赖。
- 无论哪条路，DMG 增量都按"压缩后体积"估算，`.so`/`.dylib` 压缩率低，别按 30% 折算。
