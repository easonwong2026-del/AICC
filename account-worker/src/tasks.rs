//! 统一的"今天到底做了什么"汇总（PR #76）。
//!
//! 这个模块**只读本地记录**：一次网络请求都不发。界面顶部的几个数字必须来自
//! 真实记录，不能靠推测，也不能在没查过的时候显示 0——那会让人以为"今天没任务"。
//!
//! 三个刻意的设计：
//!
//! 1. **不支持的账号不进分母**。WorkBuddy AI 没有国内版签到 / 成长 / 旅行，
//!    把它算成"待完成"会让汇总永远差几个。
//! 2. **`null` ≠ 0**。Qoder 的"今天能领多少"要联网才知道，汇总里给 `null`，
//!    界面写"未查询"，而不是"0 个"。
//! 3. **失败单独计数**。失败和未完成是两件事：前者要人处理，后者等时间。

use serde_json::{json, Value};

use crate::app::App;
use crate::rpc::RpcResult;
use crate::storage::AccountFilter;

/// RFC3339（UTC）→ 北京时间日期。解析不了就不算进"今天"。
fn beijing_date_of(raw: &str) -> Option<String> {
    let offset = chrono::FixedOffset::east_opt(8 * 3600).expect("UTC+8 固定偏移");
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| at.with_timezone(&offset).format("%Y-%m-%d").to_string())
}

pub fn overview(app: &App) -> RpcResult<Value> {
    let today = crate::clock::beijing_date();
    let accounts = app.with_db(|db| db.list_accounts(&AccountFilter::default()))?;
    let qoder_accounts = app.with_db(|db| db.list_qoder_accounts(None))?;

    let mut needs_reauth = 0i64;
    let mut auto_checkin = 0i64;
    let mut auto_travel = 0i64;
    let mut checkin_done = 0i64;
    let mut checkin_pending = 0i64;
    let mut checkin_failed = 0i64;
    let mut growth_runs_today = 0i64;
    let mut growth_failed_today = 0i64;
    let mut travel_claimed_today = 0i64;
    let mut travel_pending = 0i64;
    let mut failures: Vec<Value> = Vec::new();

    for account in &accounts {
        if account.auth_status != "valid" {
            needs_reauth += 1;
        }
        // 只有国内版有签到 / 成长 / 旅行协议。国际版**不进分母**，
        // 也不进"自动任务开启数"——一个平台不支持的开关为 true 是历史遗留，
        // 把它算成"已开启的自动任务"等于承诺一件永远不会发生的事。
        if !account.supports_workbuddy_cn_apis() {
            continue;
        }

        if account.checkin_enabled {
            auto_checkin += 1;
        }
        if account.travel_enabled {
            auto_travel += 1;
        }

        match app.with_db(|db| db.checkin_record(&account.id, &today))? {
            Some(record) => match record.status.as_str() {
                "claimed" | "already" => checkin_done += 1,
                "failed" | "unauthorized" | "no_credential" => {
                    checkin_failed += 1;
                    failures.push(json!({
                        "platform": "workbuddy",
                        "accountId": account.id,
                        "nickname": account.nickname,
                        "task": "checkin",
                        "status": record.status,
                        "detail": record.error_kind,
                        "at": record.checked_at,
                    }));
                }
                _ => checkin_pending += 1,
            },
            None => {
                if account.checkin_enabled {
                    checkin_pending += 1;
                }
            }
        }

        for run in app.with_db(|db| db.growth_runs(&account.id, 20))? {
            if beijing_date_of(&run.executed_at).as_deref() != Some(today.as_str()) {
                continue;
            }
            growth_runs_today += 1;
            if run.outcome == crate::storage::growth::OUTCOME_FAILED {
                growth_failed_today += 1;
                failures.push(json!({
                    "platform": "workbuddy",
                    "accountId": account.id,
                    "nickname": account.nickname,
                    "task": format!("growth:{}", run.task_code),
                    "status": run.outcome,
                    "detail": run.error_kind.or(run.error_detail),
                    "at": run.executed_at,
                }));
            }
        }

        if let Some(state) = app.with_db(|db| db.travel_state(&account.id))? {
            let claimed_today = state
                .claimed_at
                .as_deref()
                .and_then(beijing_date_of)
                .as_deref()
                == Some(today.as_str());
            if claimed_today {
                travel_claimed_today += 1;
            } else if account.travel_enabled {
                travel_pending += 1;
            }
            if let Some(error) = state.last_error.filter(|text| !text.trim().is_empty()) {
                failures.push(json!({
                    "platform": "workbuddy",
                    "accountId": account.id,
                    "nickname": account.nickname,
                    "task": "travel",
                    "status": "failed",
                    "detail": error,
                    "at": state.updated_at,
                }));
            }
        }
    }

    let mut qoder_claimed_today = 0i64;
    let mut qoder_failed_today = 0i64;
    let mut qoder_auto = 0i64;
    for account in &qoder_accounts {
        // 未验证的版本即使残留开关也不计入：它根本不会执行。
        let supported = crate::providers::qoder_edition::QoderEdition::parse(&account.edition)
            .and_then(crate::providers::qoder_credits::capability)
            .is_some();
        if supported && app.with_db(|db| db.qoder_credits_auto(&account.id))? {
            qoder_auto += 1;
        }

        // **按活动去重**：同一个活动今天可能被查了好几次，日志会有多条
        // `already`；"今天领取成功几个"应该数活动，不该数执行次数。
        let mut claimed_campaigns: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for run in app.with_db(|db| db.list_qoder_credit_runs(Some(&account.id), 50))? {
            if beijing_date_of(&run.executed_at).as_deref() != Some(today.as_str()) {
                continue;
            }
            match run.outcome.as_str() {
                "ok" | "already" => {
                    let key = run
                        .campaign_id
                        .clone()
                        .unwrap_or_else(|| format!("run-{}", run.id));
                    claimed_campaigns.insert(key);
                }
                "rejected" | "forbidden" | "rate_limited" | "server_error" | "network"
                | "failed" => {
                    qoder_failed_today += 1;
                    failures.push(json!({
                        "platform": account.edition,
                        "accountId": account.id,
                        "nickname": account.nickname,
                        "task": "qoder_credits",
                        "status": run.outcome,
                        "detail": run.error_detail.or(run.error_kind),
                        "at": run.executed_at,
                    }));
                }
                _ => {}
            }
        }
        qoder_claimed_today += claimed_campaigns.len() as i64;
    }

    // 失败按执行时间**降序**取前 5 条：汇总条是用来"一眼看出现在该处理什么"的，
    // 不是日志。时间解析不了的排最后，而不是排最前。
    let failure_count = failures.len() as i64;
    failures.sort_by(|left, right| {
        let left = left.get("at").and_then(Value::as_str).unwrap_or("");
        let right = right.get("at").and_then(Value::as_str).unwrap_or("");
        right.cmp(left)
    });
    failures.truncate(5);

    Ok(json!({
        "today": today,
        "accounts": {
            "total": accounts.len() as i64 + qoder_accounts.len() as i64,
            "workbuddy": accounts.len() as i64,
            "qoder": qoder_accounts.len() as i64,
        },
        "checkin": {
            "done": checkin_done,
            "pending": checkin_pending,
            "failed": checkin_failed,
        },
        "growth": {
            "runs": growth_runs_today,
            "failed": growth_failed_today,
        },
        "travel": {
            "claimed": travel_claimed_today,
            "pending": travel_pending,
        },
        "qoderCredits": {
            "claimed": qoder_claimed_today,
            "failed": qoder_failed_today,
            "autoEnabled": qoder_auto,
            // 要联网才知道，汇总里如实为 null，界面写"未查询"而不是 0。
            "claimable": Value::Null,
        },
        "needsReauth": needs_reauth,
        "autoEnabled": auto_checkin + auto_travel + qoder_auto,
        "autoBreakdown": {
            "checkin": auto_checkin,
            "travel": auto_travel,
            "qoderCredits": qoder_auto,
        },
        "recentFailures": failures,
        "failureCount": failure_count,
        "note": "只统计本地记录；本轮没有发起任何网络请求",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::storage::AccountInput;

    fn test_app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        };
        (App::with_secure(paths, SecureStore::memory()), dir)
    }

    fn add(app: &App, provider: &str, uid: &str) -> String {
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: uid.into(),
                provider: Some(provider.into()),
                ..Default::default()
            })
        })
        .unwrap()
        .id
    }

    #[test]
    fn empty_install_reports_zero_without_guessing() {
        let (app, _dir) = test_app();
        let value = overview(&app).unwrap();
        assert_eq!(value["checkin"]["done"], 0);
        assert_eq!(value["checkin"]["pending"], 0);
        // Qoder 的"能领几个"必须是 null：本地记录回答不了。
        assert!(value["qoderCredits"]["claimable"].is_null());
        assert_eq!(value["needsReauth"], 0);
    }

    #[test]
    fn workbuddy_ai_is_excluded_from_cn_task_denominators() {
        let (app, _dir) = test_app();
        let id = add(&app, "workbuddy_ai", "u-ai");
        app.with_db(|db| db.set_checkin_enabled(&id, true)).unwrap();
        let value = overview(&app).unwrap();
        assert_eq!(value["accounts"]["total"], 1);
        // 开了开关但协议不支持：**不算待完成**，也不算"自动任务已开启"。
        assert_eq!(value["checkin"]["pending"], 0);
        assert_eq!(value["autoBreakdown"]["checkin"], 0);
        assert_eq!(value["autoEnabled"], 0);
    }

    #[test]
    fn a_claimed_record_counts_as_done_for_today() {
        let (app, _dir) = test_app();
        let id = add(&app, "workbuddy", "u-cn");
        app.with_db(|db| db.set_checkin_enabled(&id, true)).unwrap();
        app.with_db(|db| {
            db.upsert_checkin_record(crate::storage::checkin::NewCheckinRecord {
                account_id: &id,
                platform: "workbuddy",
                local_date: &crate::clock::beijing_date(),
                status: "claimed",
                credits: Some(10),
                error_kind: None,
                execution_source: "auto",
            })
        })
        .unwrap();
        let value = overview(&app).unwrap();
        assert_eq!(value["checkin"]["done"], 1);
        assert_eq!(value["checkin"]["pending"], 0);
    }

    #[test]
    fn needs_reauth_counts_non_valid_accounts() {
        let (app, _dir) = test_app();
        let id = add(&app, "workbuddy", "u-expired");
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "u-expired".into(),
                provider: Some("workbuddy".into()),
                auth_status: Some("expired".into()),
                ..Default::default()
            })
        })
        .unwrap();
        let value = overview(&app).unwrap();
        assert!(value["needsReauth"].as_i64().unwrap() >= 1, "id={id}");
    }

    #[test]
    fn qoder_runs_today_are_counted_and_failures_surface() {
        let (app, _dir) = test_app();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-q", Some("积分号"), None))
            .unwrap();
        app.with_db(|db| {
            db.record_qoder_credit_run(crate::storage::qoder_credits::RunInput {
                account_id: &account.id,
                campaign_id: Some("c-1"),
                campaign_key: Some("daily"),
                period_start: Some(1_700_000_000),
                period_end: Some(1_700_086_400),
                outcome: "ok",
                credits: Some(100),
                http_status: Some(200),
                auth_source: Some("own_pat"),
                error_kind: None,
                error_detail: None,
                trigger: "manual",
            })
        })
        .unwrap();
        app.with_db(|db| {
            db.record_qoder_credit_run(crate::storage::qoder_credits::RunInput {
                account_id: &account.id,
                campaign_id: Some("c-1"),
                campaign_key: Some("daily"),
                period_start: Some(1_700_000_000),
                period_end: Some(1_700_086_400),
                outcome: "rate_limited",
                credits: None,
                http_status: Some(429),
                auth_source: Some("own_pat"),
                error_kind: Some("rate_limited"),
                error_detail: Some("被限流（429）"),
                trigger: "auto",
            })
        })
        .unwrap();

        let value = overview(&app).unwrap();
        assert_eq!(value["qoderCredits"]["claimed"], 1);
        assert_eq!(value["qoderCredits"]["failed"], 1);
        assert_eq!(value["failureCount"], 1);
        assert_eq!(value["recentFailures"][0]["task"], "qoder_credits");
        assert!(value["qoderCredits"]["claimable"].is_null());
    }

    #[test]
    fn repeated_runs_for_the_same_campaign_count_once() {
        let (app, _dir) = test_app();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-dup", Some("积分号"), None))
            .unwrap();
        for outcome in ["already", "already", "ok"] {
            app.with_db(|db| {
                db.record_qoder_credit_run(crate::storage::qoder_credits::RunInput {
                    account_id: &account.id,
                    campaign_id: Some("c-1"),
                    campaign_key: Some("daily"),
                    period_start: Some(1_700_000_000),
                    period_end: Some(1_700_086_400),
                    outcome,
                    credits: Some(100),
                    http_status: Some(200),
                    auth_source: Some("own_pat"),
                    error_kind: None,
                    error_detail: None,
                    trigger: "auto",
                })
            })
            .unwrap();
        }
        let value = overview(&app).unwrap();
        // 三次执行、同一个活动：今日完成是 **1**，不是 3。
        assert_eq!(value["qoderCredits"]["claimed"], 1);
    }

    #[test]
    fn recent_failures_are_newest_first() {
        let (app, _dir) = test_app();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-fail", Some("积分号"), None))
            .unwrap();
        // 三条**今天**（北京时间）不同时刻的失败记录，用来验证排序。
        // 概览只统计今天，所以昨天的记录不该影响这两个数字。
        let today = crate::clock::beijing_date();
        let at_of = |utc_hour: u32| format!("{today}T{utc_hour:02}:00:00Z");
        for (uid, at) in [
            ("qc_old", at_of(1)),
            ("qc_new", at_of(3)),
            ("qc_mid", at_of(2)),
        ] {
            app.with_db(|db| {
                db.exec_raw(&format!(
                    "INSERT INTO qoder_credit_runs
                        (uid, account_id, campaign_id, campaign_key, outcome, credits,
                         http_status, auth_source, error_kind, error_detail, trigger, executed_at)
                     VALUES ('{uid}', '{}', 'c-1', 'daily', 'network', NULL, NULL,
                             'own_pat', 'network', '超时', 'manual', '{at}')",
                    account.id
                ))
            })
            .unwrap();
        }
        let value = overview(&app).unwrap();
        let first = value["recentFailures"][0]["at"].as_str().unwrap();
        assert_eq!(first, at_of(3), "最新的排最前");
        assert_eq!(value["failureCount"], 3);
    }
}
