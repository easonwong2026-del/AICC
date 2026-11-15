//! 凭据写入的**唯一有序门禁**：进程内互斥 + 跨进程文件锁。
//!
//! ## 为什么必须是一把门
//!
//! 复审 P0-2（第七轮）：迁移持的是 `LocalJsonStore` 的跨进程文件锁，而 WorkBuddy
//! 的登录 / 刷新 / 删除只拿进程内的 `auth_write_guard`。两个锁域互不相干，于是
//! 下面这条交错**不需要任何恶意**就能发生：
//!
//! ```text
//! 迁移 A: 读 access = A0
//! 登录 B: 写 access = A1, refresh = R1（只拿进程内锁）
//! 迁移 A: 读 refresh = R1        ← 拿到的是 B 的那一半
//! 迁移 A: 写 JSON {A0, R1} 并提交成功
//! 登录 B: 提交落空（版本变了）
//! ```
//!
//! 结果：库里、迁移记录里都写着"成功"，盘上却是一组**混搭**令牌，刷新可能直接失败。
//!
//! ## 门禁的两个层次，顺序固定
//!
//! 1. 进程内 `Mutex`：同一进程的多条写入路径（扫码、导入、刷新、删除、迁移）排队；
//! 2. 跨进程 `flock`：另一个 AICC 进程（helper 与 App 各起一个是很常见的）也排队。
//!
//! **先进程内、后跨进程**，全代码只有这一个顺序，因此不存在锁序环。
//!
//! ## 同线程可重入
//!
//! 外层（例如 `persist_login`）已经进了门，内层（`write_own` → `LocalJsonStore::write`）
//! 还会再要一次锁。没有重入，内层会在自己的 flock 上等到超时（2s 后报
//! `lock_timeout`），登录直接失败。所以这里用线程局部深度计数：同一线程的第二次
//! `begin` 只是一个空守卫。文件锁本身也是同线程可重入的（见 `localjson::FileLock`）。

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};

use serde_json::json;

use crate::app::App;
use crate::credentials::localjson::{FileLock, LocalJsonStore};
use crate::rpc::{RpcError, RpcResult, E_SECURESTORE};

static GATE_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    /// 本线程已经进了几层门。>0 时再进门直接返回空守卫。
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// 一次凭据事务的守卫。**持有到提交 / 回滚结束**，Drop 时按相反顺序放锁。
///
/// 只有最外层真的持有锁；嵌套层是空守卫（`inner` 全为 `None`）。
pub struct CredentialGate {
    _local: Option<MutexGuard<'static, ()>>,
    _file: Option<FileLock>,
}

impl CredentialGate {
    /// 这一层是否真的持锁。测试用它区分"外层"与"嵌套空守卫"。
    pub fn is_outer(&self) -> bool {
        self._local.is_some()
    }
}

impl std::fmt::Debug for CredentialGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialGate")
            .field("outer", &self.is_outer())
            .finish()
    }
}

impl Drop for CredentialGate {
    fn drop(&mut self) {
        DEPTH.with(|cell| cell.set(cell.get().saturating_sub(1)));
    }
}

/// 进入凭据事务。**所有会 mutate 令牌的路径都必须先调它**：
///
/// | 路径 | 为什么 |
/// | --- | --- |
/// | 扫码 / 浏览器登录落库 | 写钥匙串或 JSON，再写授权行 |
/// | refresh 轮换 | 同上，且旧 refresh 立刻作废 |
/// | 删除账号 / 单份授权 | 删多把键，删一半最危险 |
/// | Magpie 重新同步 | 改授权行的引用与状态 |
/// | CRED-4 迁移 / 回滚 | 读整组钥匙串值后换引用 |
///
/// 拿不到跨进程锁时**失败**，不降级：宁可让这次登录/刷新报"稍后重试"，
/// 也不能在没有互斥的情况下改写凭据。
pub fn begin(app: &App) -> RpcResult<CredentialGate> {
    let depth = DEPTH.with(|cell| {
        let now = cell.get();
        cell.set(now + 1);
        now
    });
    if depth > 0 {
        // 同一线程已在事务里：文件锁与进程内锁都已被外层持有。
        return Ok(CredentialGate {
            _local: None,
            _file: None,
        });
    }

    let local = GATE_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    match LocalJsonStore::for_paths(&app.paths).lock_exclusive() {
        Ok(file) => Ok(CredentialGate {
            _local: Some(local),
            _file: Some(file),
        }),
        Err(err) => {
            // 这一层没建立起来，把深度还回去，否则这个线程以后会一直以为自己在事务里。
            DEPTH.with(|cell| cell.set(cell.get().saturating_sub(1)));
            drop(local);
            Err(RpcError::new(
                E_SECURESTORE,
                format!(
                    "另一个 AICC 进程正在写凭据，本次操作已放弃，请稍后重试（{}）",
                    crate::rpc::sanitize(&err.message)
                ),
            )
            .with_data(json!({
                "kind": err.kind.as_str(),
                "stage": "credentials/gate",
            })))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::securestore::SecureStore;

    fn app_with_temp_dir() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let paths = crate::paths::Paths {
            db_path: root.join("accounts.sqlite"),
            backups_dir: root.join("backups"),
            logs_dir: root.join("logs"),
            wb_switch_home: root.join("wb-switch-home"),
            data_dir: root.to_path_buf(),
            wb_home_owned: false,
        };
        (App::with_secure(paths, SecureStore::memory()), dir)
    }

    #[test]
    fn the_gate_is_reentrant_on_one_thread_and_holds_the_lock_once() {
        let (app, _dir) = app_with_temp_dir();
        let outer = begin(&app).unwrap();
        assert!(outer.is_outer());
        let inner = begin(&app).unwrap();
        assert!(!inner.is_outer(), "嵌套层必须是空守卫，不能自锁");
        drop(inner);
        // 外层仍然可用：文件锁没有被内层的 Drop 释放。
        assert!(LocalJsonStore::for_paths(&app.paths)
            .lock_exclusive()
            .is_ok());
        drop(outer);
    }

    #[test]
    fn dropping_the_outer_gate_releases_the_depth_counter() {
        let (app, _dir) = app_with_temp_dir();
        {
            let _gate = begin(&app).unwrap();
        }
        // 深度归零后再进一次，仍然是"外层"。
        let again = begin(&app).unwrap();
        assert!(again.is_outer());
    }

    /// 第七轮复审 P0-2 的第二半：WorkBuddy 的令牌写入点也在这道门后面。
    ///
    /// `auth::service::write_own` 是自有令牌唯一的落盘入口（登录 / 刷新 / 首次写 JSON
    /// 都经过它），它在最里面自己取门；所以即使某个调用点忘了取门，也不会和迁移交错。
    #[test]
    fn a_workbuddy_token_write_waits_for_the_same_gate() {
        let (app, _dir) = app_with_temp_dir();
        let other_paths = app.paths.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let gate = begin(&app).unwrap();
        let handle = std::thread::spawn(move || {
            let other = App::with_secure(other_paths, SecureStore::memory());
            // 账号行不存在，写入最终可能失败——这里要证明的是"它排队了"，
            // 不是"它成功了"。
            let _ = crate::auth::service::write_own(
                &other,
                "acc-gate",
                "workbuddy",
                "TOKEN",
                None,
                None,
            );
            tx.send(()).expect("send");
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "WorkBuddy 的令牌写入没有排队：它绕开了门"
        );
        drop(gate);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("放锁之后必须能继续");
        handle.join().unwrap();
    }

    /// 第七轮复审 P0-2 的第一半：门被一个写入者持着时，另一个写入者必须排队。
    ///
    /// 这里的第二个 `App` 用**同一组路径**构造，等价于"另一个进程"看到的锁域。
    /// 断言的是"200ms 内进不来"（排队），而不是"某时刻刚好失败"——锁生效就必然如此。
    #[test]
    fn a_second_writer_cannot_enter_while_the_gate_is_held() {
        let (app, _dir) = app_with_temp_dir();
        let other_paths = app.paths.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let gate = begin(&app).unwrap();
        let handle = std::thread::spawn(move || {
            let other = App::with_secure(other_paths, SecureStore::memory());
            let _gate = begin(&other).expect("第二个写入者最终应当拿到门");
            tx.send(()).expect("send");
        });
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "第二个写入者没有排队：门失效了"
        );
        drop(gate);
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("放锁之后必须能进");
        handle.join().unwrap();
    }
}
