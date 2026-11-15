//! 跨进程凭据竞争的离线 harness（PR-CRED-3a 复审）。
//!
//! 不联网、不使用真实令牌：两个**独立进程**共享同一个临时 SQLite 与 `secrets/` 目录，
//! 用文件握手把时序摆成竞争窗口，再由父进程断言最终状态。
//!
//! 为什么必须是真进程：`auth_write_guard()` 只是进程内的 Mutex，
//! `INSERT … ON CONFLICT DO NOTHING` 与 `UPDATE … WHERE version AND access_ref`
//! 的原子性只有在两条真实连接上才有意义。单进程内的"两个实例"测不到这一点。

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::auth::service;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const HARNESS_ENV: &str = "AICC_CRED_HARNESS";
    const FIRST_LOGIN: &str = "first_login";
    const LATE_LOGIN: &str = "late_login";
    const WAIT_LIMIT: Duration = Duration::from_secs(20);

    fn uid() -> &'static str {
        "harness-uid"
    }

    fn account_id() -> String {
        crate::storage::account_id("workbuddy", uid())
    }

    fn paths_at(root: &Path) -> Paths {
        let data = root.join("data");
        Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        }
    }

    fn app_at(root: &Path) -> Arc<App> {
        let app = Arc::new(App::with_secure(paths_at(root), SecureStore::memory()));
        crate::auth::consent::set_json_storage(&app, true).expect("consent");
        app
    }

    fn ensure_account(app: &App) {
        app.with_db(|db| {
            db.upsert_account(&crate::storage::AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid().to_string(),
                provider: Some("workbuddy".to_string()),
                ..Default::default()
            })
            .map(|_| ())
        })
        .expect("account");
    }

    fn touch(path: &Path) {
        std::fs::write(path, b"1").expect("touch");
    }

    fn wait_for(path: &Path) {
        let started = Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < WAIT_LIMIT,
                "等不到 {}（超时 {:?}）",
                path.display(),
                WAIT_LIMIT
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path)
            .unwrap_or_else(|_| panic!("读不到 {}", path.display()))
            .trim()
            .to_string()
    }

    fn spawn_child(root: &Path, test_name: &str, spec: &str) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().expect("current_exe"))
            .args(["--exact", test_name, "--test-threads", "1"])
            .env(HARNESS_ENV, spec)
            .current_dir(root)
            // 子进程是同一个 libtest 二进制，它的 "test result: ok" 只会污染日志。
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("spawn child")
    }

    /// 子进程入口：只跑一次场景，然后把结论写进文件。
    ///
    /// 这个函数永远不会返回给"父进程分支"——调用点在读环境变量的 `if` 里。
    fn child_run(spec: &str) {
        let parts: Vec<&str> = spec.split('|').collect();
        assert_eq!(parts.len(), 3, "harness spec 形状不对：{spec}");
        let (scenario, role, root) = (parts[0], parts[1], PathBuf::from(parts[2]));
        let app = app_at(&root);
        ensure_account(&app);
        let account_id = account_id();

        let outcome = match scenario {
            FIRST_LOGIN => {
                // 第一步：两份凭据都先落盘 —— 两边此刻都认为"还没有 own 行"。
                let write = service::write_own(
                    &app,
                    &account_id,
                    "workbuddy",
                    &format!("{role}-access"),
                    Some(&format!("{role}-refresh")),
                    None,
                )
                .expect("write own");
                touch(&root.join(format!("ready-{role}")));
                let peer = if role == "a" { "b" } else { "a" };
                wait_for(&root.join(format!("ready-{peer}")));

                // 第二步：竞争同一行的插入。
                match service::record_grant(
                    &app,
                    &account_id,
                    uid(),
                    "own",
                    None,
                    Some((write.access_ref(), write.refresh_ref())),
                    write.expectation(),
                ) {
                    Ok(_) => "won".to_string(),
                    Err(err) if crate::handlers::is_stale_snapshot(&err) => {
                        write.discard_unreferenced(&app);
                        "superseded".to_string()
                    }
                    Err(err) => format!("error:{}", err.message),
                }
            }
            LATE_LOGIN => {
                // 读到当前这一行、写进凭据，然后等父进程把引用挪走。
                let write = service::write_own(
                    &app,
                    &account_id,
                    "workbuddy",
                    "late-access",
                    Some("late-refresh"),
                    None,
                )
                .expect("write own");
                touch(&root.join("late-ready"));
                wait_for(&root.join("late-go"));

                match service::record_grant(
                    &app,
                    &account_id,
                    uid(),
                    "own",
                    None,
                    Some((write.access_ref(), write.refresh_ref())),
                    write.expectation(),
                ) {
                    Ok(_) => "won".to_string(),
                    Err(err) if crate::handlers::is_stale_snapshot(&err) => {
                        write.discard_unreferenced(&app);
                        "superseded".to_string()
                    }
                    Err(err) => format!("error:{}", err.message),
                }
            }
            other => panic!("未知场景：{other}"),
        };

        let name = if scenario == LATE_LOGIN {
            "outcome-late".to_string()
        } else {
            format!("outcome-{role}")
        };
        std::fs::write(root.join(name), outcome).expect("outcome");
    }

    fn secret_ids(app: &App) -> Vec<String> {
        crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
            .scan()
            .expect("scan")
            .secrets
            .into_iter()
            .map(|secret| secret.secret_id)
            .collect()
    }

    /// 并发首次登录：最终只有一条 own 引用，且**没有未被引用的有效凭据文件**。
    #[test]
    fn two_processes_racing_the_first_login_leave_one_reference_and_no_orphan() {
        if let Ok(spec) = std::env::var(HARNESS_ENV) {
            child_run(&spec);
            return;
        }

        let root = tempfile::tempdir().expect("tempdir");
        // 先把 schema 建好：两个子进程同时对全新库跑迁移会互相打架。
        let bootstrap = app_at(root.path());
        ensure_account(&bootstrap);
        drop(bootstrap);
        let name = "cred_harness::tests::two_processes_racing_the_first_login_leave_one_reference_and_no_orphan";
        let mut a = spawn_child(
            root.path(),
            name,
            &format!("{FIRST_LOGIN}|a|{}", root.path().display()),
        );
        let mut b = spawn_child(
            root.path(),
            name,
            &format!("{FIRST_LOGIN}|b|{}", root.path().display()),
        );
        assert!(a.wait().expect("wait a").success(), "子进程 a 失败");
        assert!(b.wait().expect("wait b").success(), "子进程 b 失败");

        let mut outcomes = vec![
            read(&root.path().join("outcome-a")),
            read(&root.path().join("outcome-b")),
        ];
        outcomes.sort();
        assert_eq!(
            outcomes,
            vec!["superseded".to_string(), "won".to_string()],
            "必须恰好一个赢、一个输：{outcomes:?}"
        );
        let winner = if read(&root.path().join("outcome-a")) == "won" {
            "a"
        } else {
            "b"
        };

        let app = app_at(root.path());
        let rows = app
            .with_db(|db| db.list_authorizations(&account_id()))
            .expect("rows");
        assert_eq!(rows.len(), 1, "只该有一条 own 授权：{rows:?}");
        assert_eq!(rows[0].storage_kind(), "localjson");

        let secrets = secret_ids(&app);
        assert_eq!(secrets.len(), 1, "输家没清理孤儿文件：{secrets:?}");
        assert_eq!(
            rows[0].access_ref,
            format!("localjson:v1:{}", secrets[0]),
            "被引用的文件必须就是留下的那一份"
        );

        // 留下的必须是赢家的令牌。
        let stored = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
            .load(&secrets[0])
            .expect("load");
        assert_eq!(
            stored.credentials.access_token.as_deref(),
            Some(format!("{winner}-access").as_str()),
            "留下的文件不是赢家的凭据"
        );
        assert_eq!(
            service::read_access(
                &app,
                &app.with_db(|db| db.get_account(&account_id()))
                    .expect("query")
                    .expect("account"),
                "own"
            )
            .expect("read")
            .as_deref(),
            Some(format!("{winner}-access").as_str())
        );
    }

    /// 迟到的登录（另一个进程）：引用被挪走之后，旧快照不许覆盖，孤儿文件要被清掉。
    #[test]
    fn a_late_login_in_another_process_does_not_overwrite_a_moved_reference() {
        if let Ok(spec) = std::env::var(HARNESS_ENV) {
            child_run(&spec);
            return;
        }

        let root = tempfile::tempdir().expect("tempdir");
        let app = app_at(root.path());
        ensure_account(&app);
        let account_id = account_id();

        // 基线：父进程先建一条正常授权。
        let first = service::write_own(
            &app,
            &account_id,
            "workbuddy",
            "first-access",
            Some("first-refresh"),
            None,
        )
        .expect("first");
        service::record_grant(
            &app,
            &account_id,
            uid(),
            "own",
            None,
            Some((first.access_ref(), first.refresh_ref())),
            first.expectation(),
        )
        .expect("grant");

        // 子进程读这一行、写凭据，然后停下等信号。
        let name = "cred_harness::tests::a_late_login_in_another_process_does_not_overwrite_a_moved_reference";
        let mut child = spawn_child(
            root.path(),
            name,
            &format!("{LATE_LOGIN}|a|{}", root.path().display()),
        );
        wait_for(&root.path().join("late-ready"));

        // 父进程把引用挪到另一份文件（模拟重新导入 / 迁移）。
        let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
        let other = store
            .create(crate::credentials::localjson::SecretDraft::oauth(
                crate::auth::ids::auth_id(&account_id, "own"),
                "workbuddy",
                "moved-access",
            ))
            .expect("other secret");
        let other_ref =
            crate::credentials::reference::local_json_ref(&other.secret_id).expect("ref");
        let row = app
            .with_db(|db| db.find_authorization(&account_id, "own"))
            .expect("query")
            .expect("row");
        let mut moved = row.clone();
        moved.access_ref = other_ref.clone();
        moved.refresh_ref = Some(other_ref.clone());
        assert!(app
            .with_db(|db| db.replace_authorization_cas(&moved, row.version, &row.access_ref))
            .expect("cas"));

        // 放行子进程：它手里的快照已经过期。
        touch(&root.path().join("late-go"));
        assert!(child.wait().expect("wait").success(), "子进程失败");
        assert_eq!(read(&root.path().join("outcome-late")), "superseded");

        let after = app
            .with_db(|db| db.find_authorization(&account_id, "own"))
            .expect("query")
            .expect("row");
        assert_eq!(after.access_ref, other_ref, "行必须还是挪过去的那份");
        assert_eq!(
            secret_ids(&app),
            vec![other.secret_id.clone()],
            "迟到者写的旧文件没有被清理：{:?}",
            secret_ids(&app)
        );
        assert_eq!(
            service::read_access(
                &app,
                &app.with_db(|db| db.get_account(&account_id))
                    .expect("query")
                    .expect("account"),
                "own"
            )
            .expect("read")
            .as_deref(),
            Some("moved-access")
        );
    }
}
