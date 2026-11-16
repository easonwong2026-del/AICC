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
- `generation` 只在根变化时递增，是设备用来判断「要不要让用户重新确认指纹」的信号。并发 `reset`
  也在锁内读计数器，两个同时重置不会都写出同一个编号（4 个并发 reset 实测得到 +4）。
- 证书是启动时读入内存的：`tls.reset` 之后**必须重启监听进程**才换上新身份，否则端口上仍是旧证书。
  Swift 在调用 reset 后要重启桥接，这一步在 PR-B3 里接。

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

## 真实 Wi-Fi 端到端（PR-B3，2026-11-16 实测）

Mac `192.168.0.14`（AICC 2.12.2 自带运行时 + 根证书 gen 2）／POKE4S `192.168.0.8`，
不经 USB、不经 `adb reverse`，两端都在 Wi-Fi 上。

| # | 场景 | 结果 | 证据 |
| --- | --- | --- | --- |
| 1 | UDP 发现 | `Candidate found: https://192.168.0.14:8767` | logcat `AICC Discovery` |
| 2 | 首次信任 | 弹「核对指纹」，显示 `SAeN 58Qk lqPm 10pm …`，与 Mac 设置页一致后才继续 | 界面操作、截图 |
| 3 | HTTPS 配对 | 200，token 保存，随后 `Candidate verified` | logcat |
| 4 | 额度读取 | 面板显示真实额度（SYNC 15:31） | `docs/verification/poke4s-wifi-paired-online.png` |
| 5 | App 重启 | 新进程重新 `verified`，无需重新配对 | logcat 15:28:46 |
| 6 | 熄屏唤醒 | 唤醒后立即 `verified` 并刷新 SYNC | logcat 15:29:51 |
| 7 | AICC 重启 | LAN 关闭→开启各一次，重启后均自动恢复 | logcat 15:31:07 |
| 8 | 关闭局域网 | `8767`/`8766` 无监听；设备 `ConnectException` → 面板 `OFFLINE` + 「上次 15:30」 | `poke4s-lan-off-offline.png` |
| 9 | 根证书重置 | 设备 `SSLHandshakeException` 拒绝，面板 OFFLINE，**未发送 token**；确认新指纹后重新配对恢复 | `poke4s-root-reset-refused.png` |
| 10 | HTTP 回退 | 不存在：URL 解析拒绝 `http://`、`usesCleartextTraffic=false`、失败即报错 | 代码 + 单测 |

两点如实说明：

- 「Mac 重启」这一项本轮是用 **AICC 进程重启 + 局域网开关循环**验证的，没有真的重启整台 Mac。
- 失败时不做任何"清数据重来"：根证书重置那条是故意制造的负向场景，现场先是 OFFLINE + 旧数据，
  确认新指纹后才恢复，没有用重置/清空去掩盖问题。

真机探针记录（USB + `adb reverse`，ECDSA 13/13）在 `docs/verification/poke4s-ecdsa-probe.txt`，
与上面的 Wi-Fi 验收是两件事，不能互相替代。

## 还没做

- Swift 客户端：开关语义、指纹展示、`tls.ensure` 调用、内置解释器优先、重置入口。
- Android 正式客户端：HTTPS 发现、首次确认 UI、SPKI pinning、禁明文、证书异常处理。
- 真实 Wi-Fi 下的发现 → 确认 → 配对 → 读额度 → 断网重连。
