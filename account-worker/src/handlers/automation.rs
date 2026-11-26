use std::sync::Arc;

use serde_json::{json, Value};
use wb_switch_core::modules::variant::WbVariant;

use super::{log_note, optional_string_list, require_bool, require_str, string_list};
use crate::app::App;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::AccountFilter;
use crate::travel::{scheduler as travel_scheduler, service::EventDraft};
use crate::workbuddy_switch;

pub(super) async fn dispatch(app: &Arc<App>, method: &str, params: &Value) -> RpcResult<Value> {
    match method {
        "travel.summary" => Ok(travel_summary(app)),

        "travel.events" => {
            let id = require_str(params, "id")?;
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .min(200) as u32;
            app.with_db(|db| Ok(json!({ "events": db.travel_events(&id, limit)? })))
        }

        "travel.setEnabled" => {
            let id = require_str(params, "id")?;
            let enabled = require_bool(params, "enabled")?;
            app.with_db(|db| {
                if db.get_account(&id)?.is_none() {
                    return Err(RpcError::not_found(format!("账号不存在：{id}")));
                }
                db.set_travel_enabled(&id, enabled)?;
                Ok(json!({ "id": id, "travelEnabled": enabled }))
            })
        }

        // 刷新是**纯只读**：只查状态，看到「已到达」也不提交领取。
        "travel.refresh" => {
            let id = require_str(params, "id")?;
            travel_sync(app, &id, SyncIntent::Observe).await
        }

        // 每个按钮只做一件事：派出不顺手领奖，领奖不顺手派出。
        "travel.depart" => {
            let id = require_str(params, "id")?;
            travel_sync(app, &id, SyncIntent::Depart).await
        }

        "travel.claim" => {
            let id = require_str(params, "id")?;
            travel_sync(app, &id, SyncIntent::Claim).await
        }

        "travel.auto.status" => Ok(travel_scheduler::auto_status(app)),

        "travel.auto.tick" => Ok(travel_scheduler::run_tick(app).await),

        // ---- 成长中心（PR #67：只读）----
        //
        // 一次调用扫完（或扫指定）账号，返回每个账号的任务、积分、能量、连签
        // 与旅行状态。**这里没有任何写接口**：接受任务、领奖在 #68，
        // 每个都会有自己的开关与验收。
        //
        // 与签到 / 旅行共用同一把凭据锁：扫描虽然只读，但读的是同一份 access，
        // 不能与「正在轮换令牌」的写入撞在一起。
        "growth.scan" => {
            let ids = optional_string_list(params, "accountIds")?;
            let scans = crate::growth::scan_fleet(app, &ids).await;
            let overview = crate::growth::overview(&scans);
            // 扫描日志：只记结论，不记令牌与任务正文。
            let summary: Vec<String> = scans
                .iter()
                .map(|scan| {
                    format!(
                        "{}={}(tasks {}, claimable {}, errors {})",
                        scan.account_id,
                        scan.state,
                        scan.counts.total,
                        scan.counts.claimable,
                        scan.errors.len()
                    )
                })
                .collect();
            log_note(&format!("成长中心扫描：{}", summary.join("; ")));
            Ok(json!({
                "scans": scans,
                "overview": overview,
                "scannedAt": crate::clock::now_rfc3339(),
                "readOnly": true,
            }))
        }

        // ---- 成长中心自动管理（PR #68）----
        //
        // 只有三个方法，而且开关默认关闭：没打开时 tick 连只读都不做。
        "growth.auto.status" => Ok(crate::growth::scheduler::auto_status(app)),

        "growth.auto.setEnabled" => {
            let enabled = require_bool(params, "enabled")?;
            crate::growth::scheduler::set_enabled(app, enabled)?;
            log_note(&format!(
                "成长中心自动管理{}",
                if enabled { "已开启" } else { "已关闭" }
            ));
            Ok(crate::growth::scheduler::auto_status(app))
        }

        // `force`：用户点「立即检查」时传 true，绕过间隔判断。
        // 但写操作仍受总开关约束：开关关着就是一次只读检查。
        "growth.auto.tick" => {
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = crate::growth::scheduler::run_tick(app, force).await;
            Ok(result)
        }

        // 执行记录：每个任务取最近一条 + 累计到账。
        "growth.runs" => {
            let id = require_str(params, "accountId")?;
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .min(200) as u32;
            app.with_db(|db| {
                let latest = db.latest_growth_run_per_task(&id)?;
                let (credit, energy) = db.growth_totals(&id)?;
                Ok(json!({
                    "accountId": id,
                    "latestByTask": latest,
                    "recent": db.growth_runs(&id, limit)?,
                    "grantedCredit": credit,
                    "grantedEnergy": energy,
                }))
            })
        }

        // ---- 成长活跃（PR #103）----
        //
        // 只读：每账号两个成长 GET（`/heatmap` + `/streak`），外加本地当天的签到记录；
        // 本地拿不出成功记录时，再发一次**只读语义的 POST**（签到状态接口，
        // 仅在 404 时换一次路径）。上限：4 账号 × (2 GET + 1 POST)。
        // 签到与活跃是两个独立字段，**不互相推导**：实测四个账号连续签到 5 天，
        // 其中三个热力墙仍是 0（docs/activity-rules-2026-11-17.md）。
        "growth.activity.status" => {
            let ids = optional_string_list(params, "accountIds")?;
            let fleet = crate::growth::activity::fleet(app, &ids).await?;
            let summary: Vec<String> = fleet
                .accounts
                .iter()
                .map(|account| {
                    format!(
                        "{}={}(active {:?}, streak {:?}, next {})",
                        account.account_id,
                        account.diagnosis.code,
                        account.today_active,
                        account.streak_days,
                        account
                            .next_tier_remaining
                            .map(|days| days.to_string())
                            .unwrap_or_else(|| "—".to_string())
                    )
                })
                .collect();
            log_note(&format!("成长活跃查询：{}", summary.join("; ")));
            Ok(serde_json::to_value(fleet).unwrap_or(Value::Null))
        }

        // ---- 奖励状态（PR #96）----
        //
        // 只读：三档兑换状态、抽奖次数、补签卡余额与可补签日期，外加账本里
        // "还没核验"的条数。**这里没有抽奖、兑换、补签的写入口** —— 它们要等
        // AICC 自己拿到 A 级接口证据（见 docs/growth-rewards-protocol-2026-11-16.md）。
        "growth.rewards.status" => {
            // 类型不对就报错：静默当成"全部账号"是一次意外访问。
            let ids = optional_string_list(params, "accountIds")?;
            // 账号列表读不到时返回错误，不返回空数组（空 = "你没有账号"，
            // 那是完全不同的结论）。
            let states = crate::growth::reward_states(app, &ids).await?;
            let summary: Vec<String> = states
                .iter()
                .map(|state| {
                    format!(
                        "{}={}(claimable {}, chances {:?}, makeup {} candidates, awaiting {:?})",
                        state.account_id,
                        state.state,
                        state.tiers.iter().filter(|tier| tier.claimable).count(),
                        state.lottery_chances,
                        state.makeup.candidates.len(),
                        state.awaiting_verification
                    )
                })
                .collect();
            log_note(&format!("奖励状态查询：{}", summary.join("; ")));
            Ok(json!({
                "accounts": states,
                "queriedAt": crate::clock::now_rfc3339(),
                "readOnly": true,
            }))
        }

        // ---- 奖励取证（PR #100，dev-only）----
        //
        // 只在 debug 构建里存在：release 的 match 里没有这两条，调用方拿到的是
        // "未知方法"。probe 会发真实 POST，所以额外要求 confirm=true ——
        // 它不是"又一个写入口"，账本、凭据门禁、plan() 全都走同一套。
        #[cfg(debug_assertions)]
        "growth.rewards.evidence" => {
            let _gate = crate::credentials::gate::begin(app)?;
            if !require_bool(params, "confirm")? {
                return Err(RpcError::invalid_params(
                    "confirm 必须为 true：这会真的向服务端发 4 个只读请求",
                ));
            }
            let account_id = require_str(params, "accountId")?;
            let report = crate::growth::capture_evidence(app, &account_id).await?;
            Ok(serde_json::to_value(report).unwrap_or(Value::Null))
        }

        #[cfg(debug_assertions)]
        "growth.rewards.probe" => {
            let _gate = crate::credentials::gate::begin(app)?;
            let account_id = require_str(params, "accountId")?;
            let action_raw = require_str(params, "action")?;
            let action =
                crate::growth::probe::ProbeAction::parse(&action_raw).ok_or_else(|| {
                    RpcError::invalid_params("action 只能是 lottery_draw / redeem / makeup_card")
                })?;
            let target = params.get("target").and_then(Value::as_str);
            let command = crate::growth::probe::command(action, target)?;

            // `confirm` 不是布尔就当没确认；没确认**只给预览**，绝不下手。
            // 预览本身也是只读的：账号、动作、当前余额、预计消耗。
            let confirmed = params
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if confirmed {
                crate::growth::probe::execute(app, &account_id, &command).await
            } else {
                crate::growth::probe::preview(app, &account_id, &command).await
            }
        }

        // ---- 正式奖励写操作（PR #101）----
        //
        // 三个动作三个方法，而不是一个 `growth.rewards.execute`：审计日志一眼能看出
        // 用户点了什么。共同点：`confirm` 不为 true 时只返回预览（账号、动作、余额、
        // 预计消耗），不会写账本也不会发请求。
        "growth.rewards.draw" => {
            let _gate = crate::credentials::gate::begin(app)?;
            let account_id = require_str(params, "accountId")?;
            let confirm = params
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::growth::rewards_write::run(
                app,
                &account_id,
                crate::growth::probe::ProbeAction::LotteryDraw,
                None,
                confirm,
            )
            .await
        }

        "growth.rewards.redeem" => {
            let _gate = crate::credentials::gate::begin(app)?;
            let account_id = require_str(params, "accountId")?;
            let tier = require_str(params, "target")?;
            let confirm = params
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::growth::rewards_write::run(
                app,
                &account_id,
                crate::growth::probe::ProbeAction::Redeem,
                Some(&tier),
                confirm,
            )
            .await
        }

        "growth.rewards.makeup" => {
            let _gate = crate::credentials::gate::begin(app)?;
            let account_id = require_str(params, "accountId")?;
            let date = require_str(params, "target")?;
            let confirm = params
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::growth::rewards_write::run(
                app,
                &account_id,
                crate::growth::probe::ProbeAction::MakeupCard,
                Some(&date),
                confirm,
            )
            .await
        }

        // 回查：把 unknown 的记录拿服务端当前状态比一次，能收敛就收敛。
        // 只读服务端（GET），只写本地账本。
        "growth.rewards.recheck" => {
            let _gate = crate::credentials::gate::begin(app)?;
            let account_id = require_str(params, "accountId")?;
            let op_id = params.get("opId").and_then(Value::as_str);
            crate::growth::rewards_write::recheck(app, &account_id, op_id).await
        }

        // ---- WorkBuddy 每日签到（PR #69）----
        //
        // 三个只读、三个显式写、一个调度。规则：
        // - `checkin.status` 只查状态，**不提交**；`cached` 时连网络都不碰。
        // - `checkin.run` / `checkin.runPending` 是用户点的按钮，不受自动开关约束，
        //   但仍然先查服务端状态，已签到就返回 already，不会重复提交。
        // - 返回里没有任何令牌，也不接受外部 URL。
        "checkin.status" => {
            let ids = string_list(params, "accountIds");
            let cached = params
                .get("cached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(crate::checkin::fleet_status(app, &ids, cached).await)
        }

        "checkin.run" => {
            let id = require_str(params, "accountId")?;
            let outcome = crate::checkin::run_one(app, &id).await?;
            log_note(&format!(
                "手动签到 {}：{}",
                id,
                outcome.get("state").and_then(Value::as_str).unwrap_or("?")
            ));
            Ok(outcome)
        }

        "checkin.runPending" => {
            let ids = string_list(params, "accountIds");
            let result = crate::checkin::run_pending(app, &ids).await;
            log_note(&format!(
                "批量签到：{}",
                result.get("summary").and_then(Value::as_str).unwrap_or("?")
            ));
            Ok(result)
        }

        "checkin.setEnabled" => {
            let id = require_str(params, "accountId")?;
            let enabled = require_bool(params, "enabled")?;
            // 专用写入：不走 accounts.upsert，所以不动 auth_epoch（PRD §2）。
            crate::checkin::scheduler::set_enabled(app, &id, enabled)?;
            log_note(&format!(
                "账号 {id} 的自动签到{}",
                if enabled { "已开启" } else { "已关闭" }
            ));
            Ok(json!({
                "accountId": id,
                "enabled": enabled,
                "auto": crate::checkin::scheduler::auto_status(app),
            }))
        }

        "checkin.auto.status" => Ok(crate::checkin::scheduler::auto_status(app)),

        // `force` 只跳过"到点没到点"，**不绕过账号开关**。
        "checkin.auto.tick" => {
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(crate::checkin::scheduler::run_tick(app, force).await)
        }

        "checkin.records" => {
            let id = params.get("accountId").and_then(Value::as_str);
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .min(200) as u32;
            crate::checkin::records(app, id, limit)
        }

        // ---- 四平台额度看板（M4）----
        //
        // 只读：不改账号、不改授权状态、不写 last_used_at。唯一写入是快照缓存。
        // Qoder 两版在这里是 `unverified` 且**零请求**，不是"暂时取不到"。
        "quota.board" => {
            let parsed: crate::quota::BoardParams = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("额度参数非法：{e}")))?;
            crate::quota::board(app, parsed).await
        }

        "quota.history" => {
            let parsed: crate::quota::HistoryParams = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("额度参数非法：{e}")))?;
            crate::quota::history(app, parsed)
        }

        // ---- WorkBuddy 客户端账号切换（WB-SWITCH-1）----
        //
        // 只支持国内版；`switch` / `restore` 会**关闭并重启 WorkBuddy 客户端**，
        // 所以它们的 RPC 超时必须远大于这里的预算（Swift 侧 120 秒）。
        "workbuddy.client.status" => {
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            workbuddy_switch::status(app, &control, &env)
        }

        "workbuddy.client.switch" => {
            let account_id = require_str(params, "accountId")?;
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            let request = workbuddy_switch::SwitchRequest {
                account_id,
                // 默认必须重启：客户端运行时会用内存里的登录态覆盖刚写进去的文件。
                restart: params
                    .get("restart")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                // 切换前后各查一次目标账号的服务端活跃状态（只读，失败不影响切换）。
                observe_activity: params
                    .get("observeActivity")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                // 切换后用只读探针确认服务端是否接受这份 access；失败只记 unknown。
                verify_server: params
                    .get("verifyServer")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            };
            // 手动切换与自动轮换共用同一把客户端锁：两边不能同时关客户端、
            // 互相覆盖认证文件。拿不到就明确拒绝，不排队。
            let _gate = workbuddy_switch::rotation::begin_manual_client_write(app, &env)?;
            workbuddy_switch::switch(app, &request, &control, &env).await
        }

        "workbuddy.client.restore" => {
            let journal_id = params
                .get("journalId")
                .and_then(Value::as_str)
                .map(str::to_string);
            // 用户手动换过账号时，恢复会覆盖新登录态：默认拒绝，确认后由界面带 force 再来。
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            let _gate = workbuddy_switch::rotation::begin_manual_client_write(app, &env)?;
            workbuddy_switch::restore(app, journal_id.as_deref(), force, &control, &env)
        }

        // ---- 夜间账号轮换（WB-ROTATE-1）----
        "workbuddy.rotation.status" => {
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            workbuddy_switch::rotation::status(
                app,
                &control,
                &env,
                &workbuddy_switch::rotation::SystemClock,
            )
        }

        "workbuddy.rotation.dryRun" => {
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            workbuddy_switch::rotation::dry_run(
                app,
                &control,
                &env,
                &workbuddy_switch::rotation::SystemClock,
            )
        }

        "workbuddy.rotation.save" => {
            let mut plan = app.with_db(|db| db.rotation_plan_or_default())?;
            apply_rotation_params(&mut plan, params)?;
            workbuddy_switch::rotation::validate_plan(&plan).map_err(RpcError::invalid_params)?;
            app.with_db(|db| db.save_rotation_plan(&plan))?;
            // 开关单独走一条：保存一次时间不该顺手把计划打开或关掉。
            if let Some(enabled) = params.get("enabled").and_then(Value::as_bool) {
                app.with_db(|db| db.set_rotation_enabled(enabled))?;
            }
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            workbuddy_switch::rotation::status(
                app,
                &control,
                &env,
                &workbuddy_switch::rotation::SystemClock,
            )
        }

        "workbuddy.rotation.setEnabled" => {
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| RpcError::invalid_params("缺少 enabled"))?;
            if enabled {
                // 一个连时区都不合法的计划不该被打开，更不能在半夜才开始报错。
                let plan = app.with_db(|db| db.rotation_plan_or_default())?;
                workbuddy_switch::rotation::validate_plan(&plan)
                    .map_err(RpcError::invalid_params)?;
                if plan.account_ids.is_empty() {
                    return Err(RpcError::invalid_params(
                        "先选择至少一个参与轮换的账号，再打开自动轮换",
                    ));
                }
            }
            app.with_db(|db| db.set_rotation_enabled(enabled))?;
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            // 关掉开关不只是"别再开新轮次"：还在跑的那一轮必须立刻安全收尾，
            // 否则客户端会停在一个轮换账号上过夜。这里同步执行一次 stop，
            // 让界面能马上看到"已恢复原账号"，而不是等下一次心跳。
            let stop_result = if enabled {
                Value::Null
            } else {
                workbuddy_switch::rotation::stop(app, &control, &env)?
            };
            let mut payload = workbuddy_switch::rotation::status(
                app,
                &control,
                &env,
                &workbuddy_switch::rotation::SystemClock,
            )?;
            if !enabled {
                payload["stopResult"] = stop_result;
            }
            Ok(payload)
        }

        "workbuddy.rotation.runNow" => {
            // 会立刻关掉并重启客户端：必须显式确认，避免误点。
            if params.get("confirm").and_then(Value::as_bool) != Some(true) {
                return Err(RpcError::invalid_params(
                    "立即执行会关闭并重启 WorkBuddy，需要 confirm=true",
                ));
            }
            workbuddy_switch::rotation::run_tick(app, true).await
        }

        "workbuddy.rotation.stop" => {
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            workbuddy_switch::rotation::stop(app, &control, &env)
        }

        "workbuddy.rotation.restoreOrigin" => {
            let run_id = params.get("runId").and_then(Value::as_str);
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let control = workbuddy_switch::SystemControl::for_variant(WbVariant::Cn);
            let env = workbuddy_switch::SwitchEnv::system(app);
            workbuddy_switch::rotation::restore_now(app, &control, &env, run_id, force)
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

// ---------------------------------------------------------------------------
// Buddy 旅行
// ---------------------------------------------------------------------------

/// 本地快照汇总，**不发任何请求**。
///
/// 界面轮询这个（几秒一次也不会打接口），要更新状态时显式调 `travel.refresh`。
fn travel_summary(app: &App) -> Value {
    let accounts = app
        .with_db(|db| {
            db.list_accounts(&AccountFilter {
                platform: Some("workbuddy".to_string()),
                ..Default::default()
            })
        })
        .unwrap_or_default();
    let states = app.with_db(|db| db.travel_states()).unwrap_or_default();
    let by_id: std::collections::HashMap<&str, &crate::storage::travel::TravelState> =
        states.iter().map(|s| (s.account_id.as_str(), s)).collect();

    let rows: Vec<Value> = accounts
        .iter()
        .map(|account| {
            let state = by_id.get(account.id.as_str()).map(|s| (*s).clone());
            json!({
                "accountId": account.id,
                "uid": account.uid,
                "nickname": account.nickname,
                "authStatus": account.auth_status,
                "authSource": account.auth_source,
                "travelEnabled": account.travel_enabled,
                "tokenExpiresAt": account.token_expires_at,
                "travel": state,
            })
        })
        .collect();

    json!({
        "today": crate::clock::beijing_date(),
        "auto": travel_scheduler::auto_status(app),
        "accounts": rows,
    })
}

/// 一次手动同步的意图。三个动作互不越界，避免「点刷新却把奖励领了」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]

enum SyncIntent {
    /// 只查状态，不派出、不领取。
    Observe,
    /// 派出（是否真的能派由服务端每日上限决定）。
    Depart,
    /// 领取（只有 arrived 会真的提交）。
    Claim,
}

impl SyncIntent {
    fn flags(self) -> (bool, bool) {
        match self {
            Self::Observe => (false, false),
            Self::Depart => (true, false),
            Self::Claim => (false, true),
        }
    }
}

/// 单账号执行一次（查询 / 派出 / 领取）。
///
/// 返回值只含快照与事件，**不含令牌**；事件文案在写库前已经过 `sanitize`。
async fn travel_sync(app: &Arc<App>, account_id: &str, intent: SyncIntent) -> RpcResult<Value> {
    let (allow_depart, allow_claim) = intent.flags();
    let outcome =
        travel_scheduler::sync_one(app, account_id, allow_depart, allow_claim, true).await?;
    let events: Vec<Value> = outcome
        .events
        .iter()
        .map(|event: &EventDraft| {
            json!({
                "kind": event.kind,
                "status": event.status,
                "message": event.message,
                "credits": event.credits,
            })
        })
        .collect();
    Ok(json!({
        "accountId": account_id,
        "travel": outcome.state,
        "events": events,
        "auto": travel_scheduler::auto_status(app),
    }))
}

/// 把 RPC 参数合并进现有计划。只覆盖出现的字段——界面改一个时间不该把
/// 账号顺序、停留时长一起冲掉。
fn apply_rotation_params(
    plan: &mut crate::storage::rotation::RotationPlan,
    params: &Value,
) -> RpcResult<()> {
    if let Some(timezone) = params.get("timezone").and_then(Value::as_str) {
        plan.timezone = timezone.to_string();
    }
    if let Some(days) = params.get("weekdays").and_then(Value::as_array) {
        plan.weekdays = days
            .iter()
            .filter_map(Value::as_u64)
            .filter(|day| (1..=7).contains(day))
            .map(|day| day as u8)
            .collect();
    }
    if let Some(value) = params.get("startMinutes").and_then(Value::as_i64) {
        plan.start_minutes = value;
    }
    if let Some(value) = params.get("endMinutes").and_then(Value::as_i64) {
        plan.end_minutes = value;
    }
    if let Some(ids) = params.get("accountIds").and_then(Value::as_array) {
        plan.account_ids = ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    if let Some(value) = params.get("dwellSeconds").and_then(Value::as_i64) {
        plan.dwell_seconds = value;
    }
    Ok(())
}
