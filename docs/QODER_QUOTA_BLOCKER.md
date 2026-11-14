# Qoder 额度 / Credits 阻塞报告

实测时间：**2026-11-14**。本机 `Qoder.app` = `com.qoder.app` v0.5.7（International），
Qoder CN 客户端未安装。**没有任何 Qoder 账号凭据**，`qoder_accounts` 表为空。

结论：**Qoder 两版的额度与 Credits 无法在本版实现**，原因是拿不到 access token，
且 Credits 业务接口没有可复现的成功样本。因此额度看板对两版一律返回
`status = "unverified"`，**零网络请求**，额度字段是 `null`（不是 0）。

## 1. 已确认的事实

两个版本各有一套官方 OIDC：

| | International | 国内 |
| --- | --- | --- |
| discovery | `https://qoder.com/.well-known/openid-configuration` | `https://qoder.com.cn/.well-known/openid-configuration` |
| issuer | `https://qoder.com` | **`https://qoder.cn`** |
| authorization | `https://qoder.com/oauth/authorize` | `https://qoder.com.cn/oauth/authorize` |
| token | `https://qoder.com/oauth/token` | **`https://qoder.cn/oauth/token`** |
| userinfo | `https://qoder.com/oauth/userinfo` | **`https://qoder.cn/oauth/userinfo`** |

两版共有：`grant_types = authorization_code + refresh_token`、PKCE `S256`、
`client_id_metadata_document_supported = true`、`scopes = openid profile`。

注意：**国内版的 issuer 域名是 `qoder.cn`，不是 `qoder.com.cn`**。同一版本跨了两个域，
任何域名白名单都必须同时包含两者，否则 token 交换会被自己的校验挡掉。

## 2. 为什么拿不到 token

1. `token_endpoint_auth_methods_supported` 只有 `client_secret_post` 与 `private_key_jwt`，
   **没有 `none`**。这是给机密客户端（有 secret）准备的，不是桌面 App 的公开客户端模型。
2. AICC 没有 Qoder 侧注册的 `client_id`。公开渠道没有注册入口，客户端里也没有硬编码的
   client_id（`app.asar` 中 `client_id` 命中全部来自通用 OAuth 库）。
3. 试过「用 URL 当 client_id」（CIMD）：把 `client_id` 设成一个 https 元数据文档地址后请求
   `/oauth/authorize`，服务端返回
   `401 {"error":"invalid_client","error_description":"client configuration unavailable"}`。
   换成两个不同的 URL（含一个真实存在的 JSON）结果相同 → 该路径在本机无法走通。
4. 客户端登录走的是自家登录页（`/account/login` → `/users/sign-in`），不暴露可复用的
   `client_id`；登录窗是「显示 loginUrl + 等服务端状态变 signed-in」的模型
   （`app.asar` 里可见 `loginUrl` / `onCancelLogin` / `auth.waiting`），但没有公开文档说明
   轮询端点，且逆向混淆代码不属于可接受的实现依据。

## 3. 为什么连 Credits 也不能猜

- Qoder 的 Credits / 每日领取 / 成长任务属于**账号级业务接口**，与 OIDC userinfo 是两套东西。
- `api.qoder.com` 存在（无鉴权时 `401 missing authorization header`），但没有任何公开文档
  说明路径；`changexbc/workbuddy-auto-signin` 一类参考只覆盖 WorkBuddy，不覆盖 Qoder。
- 任务书要求「不允许为了完成任务模拟用户行为」，也不允许「反复猜测端点」。
  因此本版**不实现**，也不在界面上摆空任务。

## 4. 恢复条件（做到任一条即可解阻）

1. Qoder 官方开放桌面 App 的客户端注册（`none` 认证 + PKCE），或提供设备码流程；
2. 拿到一份**真实成功**的 Credits 请求样本（URL + 请求头 + 响应结构）；
3. 用户本机装上客户端并完成一次登录，用 `#70` 的本地快照能力读出会话，验证会话令牌
   能否直接用于业务接口。

在此之前，`account-worker/src/quota/qoder.rs` 只返回阻塞原因。恢复时只需在该文件里把
`unverified_row` 换成真实适配器，上层看板与界面都不用改。
