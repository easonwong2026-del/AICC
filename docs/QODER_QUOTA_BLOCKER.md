# Qoder 授权与 Credits 现状（OIDC 阻塞 ≠ 所有登录都不可用）

更新时间：**2026-11-14**。本文只回答"这一轮 AICC 里 Qoder 走了哪条路、哪条还开着"。
它**不是**"Qoder 无法登录"的结论。

本机环境：`Qoder.app` = `com.qoder.app` v0.5.7（International），CN 客户端未安装，
`qoder_accounts` 表为空 —— 也就是**没有任何 Qoder 账号凭据**。

## 1. 三条独立的路，别混为一谈

| 路径 | 状态 | 依据 |
| --- | --- | --- |
| **CLI 设备授权**（浏览器登录 + 设备码轮询） | **已实现，待在真实账号上验收** | PR **#73** `feature/qoder-device-auth-pr73`（13 文件 / +1845） |
| **官方 PAT**（用户在账户设置里生成） | 未实现，规划中 | 国际 `qoder.com/account/integrations`、国内 `qoder.cn/account/integrations` |
| **OIDC `/oauth/authorize`**（本文实测的那条） | **不可用**，仅此一条 | 见 §2 |

> **2026-11-14 更新（PR #75）**：CN 侧已经接上 `campaigns` 查询/领取适配器，
> 看板行改为展示"当日奖励"（`credits` 块），余额仍然留空——没有已证实的余额接口。
> International 维持零请求、`unverified`。当前状态与验收方法见
> `docs/QODER_CREDITS.md`。下面 §1～§4 保留为当时的阻塞记录。

## 2. OIDC 这条路为什么走不通（实测）

两版各有一套官方 OIDC：

| | International | 国内 |
| --- | --- | --- |
| discovery | `https://qoder.com/.well-known/openid-configuration` | `https://qoder.com.cn/.well-known/openid-configuration` |
| issuer | `https://qoder.com` | **`https://qoder.cn`** |
| authorization | `https://qoder.com/oauth/authorize` | `https://qoder.com.cn/oauth/authorize` |
| token / userinfo | `https://qoder.com/oauth/{token,userinfo}` | **`https://qoder.cn/oauth/{token,userinfo}`** |

两版共有 `authorization_code` + `refresh_token`、PKCE `S256`、CIMD 声明支持。
但：

1. `token_endpoint_auth_methods_supported` 只有 `client_secret_post` / `private_key_jwt`，
   **没有 `none`** —— 不是给桌面公开客户端准备的；
2. 拿不到 Qoder 侧注册的 `client_id`，客户端 `app.asar` 里也没有硬编码的；
3. 把 `client_id` 设成 https 元数据文档（CIMD）后请求 `/oauth/authorize`，返回
   `401 {"error":"invalid_client","error_description":"client configuration unavailable"}`，
   换两个不同 URL 结果相同。

注意国内版 issuer 是 **`qoder.cn`** 而不是 `qoder.com.cn`：同一版本跨两个域，
信任列表必须同时包含，否则自己的校验会挡掉 token 交换。

## 3. 为什么 Credits 仍然算"未验证"

- Qoder 的 Credits / 每日领取 / 成长任务属**账号级业务接口**，与 OIDC userinfo 是两套东西。
- `88lin/workbuddy-auto-signin` 的 `qoder_signin.py` 已有 Qoder CN 的**活动查询与领取**流程，
  这是下一轮的明确参考（此前把它归为"完全没有接口依据"是错的）。
- 社区报错记录里出现过 `openapi.qoder.sh/api/v1/deviceToken/poll`（Qoder 论坛），
  与 #73 的设备授权路线吻合，但**不是 AICC 成功换票的证明**。
- "有参考实现"≠"本机跑通"：AICC 侧还没有一份带凭据的成功响应，所以看板不给数字。

## 4. 恢复条件（任一条即可解阻）

1. #73 的设备授权在真实账号上完成换票，拿到可用 access token；
2. 用户生成 PAT 并确认它对账号查询 / Credits 查询有效（**不能假定 CLI 能用就等于全部接口都能用**）；
3. 拿到一份真实成功的 Credits 响应（URL + 头部 + 结构）。

解阻后只需在 `account-worker/src/quota/qoder.rs` 把 `unverified_row` 换成真实适配器，
看板与界面都不用改。
