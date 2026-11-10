# Third-party notices / 第三方声明

AICC's macOS app and Python bridge contain no third-party source code. The
account worker (`account-worker/`) **does** vendor one MIT-licensed crate —
`wb-switch-core` — which is reproduced below with its license. Two further
projects shaped the implementation and are credited here.

AICC 的 macOS 主程序与 Python 桥接不包含第三方源码。账号助手
（`account-worker/`）**确实内联**了一份 MIT 许可的 crate —— `wb-switch-core`，
其来源、改动与许可证见下。另有三个项目影响了实现，在此署名。

## changexbc/workbuddy-switch — vendored `wb-switch-core`

- https://github.com/changexbc/workbuddy-switch
- Vendored path: `account-worker/vendor/crates/wb-switch-core`
  (plus `account-worker/vendor/scripts/codebuddy-cli-helper.cjs`, which the crate
  pulls in with `include_str!`)
- Pinned commit: `22b318c1f44b335c2222e444abcc2d0f74e160f7` (`release/0.1.62`)
- MIT License, Copyright (c) 2026 wb-switch（全文见文件末尾）
- Bundled as source, not as a binary dependency. Only the pure-Rust core crate
  is taken; the upstream Tauri app, its UI, icons and session tooling are **not**
  included.
- Local modifications (all marked `AICC 补丁`, see `account-worker/vendor/VENDOR.md`):
  - `config::client_home_dir()` added so that `WB_SWITCH_HOME` isolates only the
    wb-switch data directory and does not relocate the WorkBuddy client's own
    auth file lookup;
  - `variant::auth_file_path()` uses it;
  - the matching unit test now asserts against `client_home_dir()`.

## 88lin/workbuddy-auto-signin — reference implementation

- https://github.com/88lin/workbuddy-auto-signin
- MIT License, Copyright (c) 88lin
- Used as a **reference**, no code copied:
  - `qoder_signin.py` defines the macOS Qoder CN credential format that
    `account-worker/src/providers/qoder.rs` reimplements in Rust (AES-128-CBC,
    IV = 16 spaces, PKCS7, key = PBKDF2-HMAC-SHA1 of the Keychain password with
    `saltysalt` / 1003 rounds / 16 bytes);
  - `signin.py` documents the WorkBuddy check-in and growth-centre endpoints and
    the AES-GCM auth envelope that PR1b will build on.

## peterroe/magpie-usage-display

- https://github.com/peterroe/magpie-usage-display
- MIT License, Copyright (c) peterroe
- Used as a **reference**, not as a dependency:
  - the usage metrics in `macos/MenuBarApp/Sources/Models/UsageModel.swift`
    (success rate, average TTFT, token sum, cache-hit ratio, top model, the
    "tightest window" rule) follow `companion/codex_display/metrics.py`;
  - the board widget's palette and proportions follow `src/ui.cpp`
    (480×480 layout, `#0B1210` surface, `#A8FF4F` accent, `205×82` cards).
- AICC reimplements both in Swift for a desktop widget; no C++ or Python code
  was copied.

```
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## yetone/magpie

- https://github.com/yetone/magpie
- Magpie is not bundled. AICC only reads its local HTTP API
  (`/v1/magpie/quotas`, `/v1/magpie/usage`, `/v1/magpie/usage/requests`) and
  links to its releases. See the Magpie repository for its license.

## License text — wb-switch-core (changexbc/workbuddy-switch)

Reproduced verbatim from `account-worker/vendor/crates/wb-switch-core/LICENSE`.

```
MIT License

Copyright (c) 2026 wb-switch

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
