# 局域网 HTTPS 与证书身份（PR-B）

本机管理接口永远是 `127.0.0.1:8765` 明文 HTTP；网络设备只走 HTTPS。两份监听共用同一个进程、
同一个只读快照，但**路由表不同**。

| 端口 | 协议 | 范围 | 谁能访问 |
| --- | --- | --- | --- |
| 8765 | HTTP | 仅 127.0.0.1 | 本机进程（App、调试） |
| 8767 | HTTPS，TLS 1.3 优先 / 1.2 保留 | `0.0.0.0`，需用户显式开启 | 局域网设备，只有 health / pair / status |
| 8766 | UDP | `0.0.0.0`，需用户显式开启 | 发现广播（无认证能力） |

关闭「允许局域网设备连接」后 8767 与 8766 一起停，8765 仍只对本机可用。

## 为什么是「根证书 pin」而不是「叶证书 pin」

设备记住的是**根证书的 SPKI**，不是当前叶证书的。叶证书短命、会随地址变化或临近过期被重签，
一旦 pin 叶证书，每次续期都要用户重新配对。当前设计：

| 事件 | 根 | 叶 | pin | 是否要重新配对 |
| --- | --- | --- | --- | --- |
| 正常启动 | 不变 | 复用 | 不变 | 否 |
| 换网络（IP 变了） | 不变 | 重签（SAN 覆盖新地址） | 不变 | 否 |
| 叶证书进入 30 天窗口 | 不变 | 重签 | 不变 | 否 |
| 根证书临近过期（< 365 天） | **同密钥**重签 | 不变 | 不变 | 否 |
| 用户点「重置身份」 | 新密钥 | 重签 | **改变** | **是** |

`identity.json` 里的 `not_after` 永远等于 `server.pem` 里那张证书的真实 notAfter：
签发决策看的是**磁盘上的证书**，不是 JSON。任何「今天启动一次就把到期日往后推一天」的漂移都会让
30 天续期窗口永远不触发，直到真实证书过期、设备突然连不上。

## 证书目录

`~/Library/Application Support/AICC-Dashboard/certs/`（`0700`）：

| 文件 | 内容 | 权限 |
| --- | --- | --- |
| `ca.pem` | 自签根，10 年 | 0644 |
| `ca.key` | 根私钥 | **0600** |
| `server.pem` | 叶证书 + 根（完整链） | 0644 |
| `server.key` | 叶私钥 | **0600** |
| `identity.json` | spki / not_after / generation / sans | 0644 |
| `identity.lock` | `flock` 用，跨进程串行化 | 0600 |

写入一律「临时文件 + `rename`」，私钥文件创建时就带 `0600`。私钥**不进 RPC、不进日志、不进 Git、不进 DMG 之外的任何产物**。

## 生成与轮换

Rust worker（`account-worker/src/tls_certs.rs`，`rcgen` + ECDSA P-256）负责生成，Python 只
`load_cert_chain` 读文件。RPC：`tls.identity` / `tls.ensure` / `tls.reset`，三者都不返回私钥。

- `ensure` 是幂等的，每次 App 启动都可以调用；并发调用由 `File::try_lock` 串行化 —— 两个 worker
  同时启动不能各自生成一个根，否则界面显示的指纹和端口实际用的身份会不一致。
- SAN 自动包含：`127.0.0.1`、`::1`、`localhost`、主机名、主机名`.local`、路由表给出的主网卡地址。
  额外地址可由调用方通过 `sans` 参数补充。证书**只覆盖实际地址**，san 不匹配时 Android 会直接拒绝。
- `generation` 只在根变化时递增，是设备用来判断「要不要让用户重新确认指纹」的信号。

## 首次信任

UDP 广播没有认证能力，所以它只负责「这里有个服务器」。广播里带的 SPKI 指纹只是**给人看的候选值**：
用户必须在 Mac 设置页和设备界面上把完整指纹对一遍，确认后设备才保存 pin。
禁止：Trust-All、忽略 hostname、校验失败回退 HTTP、凭广播或 IP 自动建立信任。

## 已验证

| 项 | 证据 |
| --- | --- |
| TLS 1.3 优先、1.2 保留、1.0/1.1 拒绝 | `tests/test_bridge_tls.py`、`tests/runtime/tls_selfcheck.py`、POKE4S probe |
| 未知 CA / 错 hostname / 明文被拒 | 同上 |
| 根 SPKI pin 匹配、错 pin 不匹配 | POKE4S probe `root_pinning_match` / `wrong_pin_rejected` |
| 叶续期后 pin 不变 | POKE4S probe `renewal_keeps_the_pin`；`twenty_nine_days_before_expiry_*` |
| 根重置后旧 pin 被拒 | POKE4S probe `reset_root_changes_the_pin` |
| 元数据不漂移、过期证书被换掉 | `metadata_never_drifts_*`、`an_expired_leaf_is_replaced_*` |
| 并发只产生一个身份 | 4 线程单测 + 6 进程实测（同一 pin） |
| ECDSA P-256 真机兼容 | `docs/verification/poke4s-ecdsa-probe.txt`（13/13） |

真机记录在 `docs/verification/poke4s-ecdsa-probe.txt`。**该记录只覆盖 USB + `adb reverse`**：
UDP 自动发现与真实 Wi-Fi 端到端仍未验证（PR-B4）。

## 还没做

- Swift 客户端：开关语义、指纹展示、`tls.ensure` 调用、内置解释器优先、重置入口。
- Android 正式客户端：HTTPS 发现、首次确认 UI、SPKI pinning、禁明文、证书异常处理。
- 真实 Wi-Fi 下的发现 → 确认 → 配对 → 读额度 → 断网重连。
