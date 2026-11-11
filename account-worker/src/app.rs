//! Worker 运行状态：路径、数据库句柄、凭据后端。
//!
//! 数据库是**懒打开**的：`health` 必须在数据库坏掉时仍能回答，
//! 所以打不开只记进 `db_last_error`，不让整个进程起不来。

use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};

use crate::paths::Paths;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecureStore;
use crate::storage::Db;
use crate::version;

pub struct App {
    pub paths: Paths,
    pub secure: SecureStore,
    started_at: Instant,
    db: Mutex<Option<Db>>,
    db_last_error: Mutex<Option<String>>,
    last_secure_probe: Mutex<Option<Value>>,
}

impl App {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            secure: SecureStore::from_env(),
            started_at: Instant::now(),
            db: Mutex::new(None),
            db_last_error: Mutex::new(None),
            last_secure_probe: Mutex::new(None),
        }
    }

    /// 测试用：内存凭据后端 + 指定数据目录。
    pub fn with_secure(paths: Paths, secure: SecureStore) -> Self {
        let mut app = Self::new(paths);
        app.secure = secure;
        app
    }

    pub fn uptime_ms(&self) -> u128 {
        self.started_at.elapsed().as_millis()
    }

    pub fn with_db<T>(&self, f: impl FnOnce(&mut Db) -> RpcResult<T>) -> RpcResult<T> {
        let mut guard = self
            .db
            .lock()
            .map_err(|_| RpcError::internal("数据库锁已中毒"))?;
        if guard.is_none() {
            match Db::open(&self.paths.db_path) {
                Ok(db) => {
                    *guard = Some(db);
                    if let Ok(mut err) = self.db_last_error.lock() {
                        *err = None;
                    }
                }
                Err(err) => {
                    if let Ok(mut slot) = self.db_last_error.lock() {
                        *slot = Some(err.message.clone());
                    }
                    return Err(err);
                }
            }
        }
        let db = guard.as_mut().expect("刚刚打开");
        f(db)
    }

    /// 不抛错的数据库快照，给 `health` 用。
    pub fn db_snapshot(&self) -> Value {
        match self.with_db(|db| db.status()) {
            Ok(status) => {
                let mut value = status;
                value["ok"] = json!(true);
                value
            }
            Err(err) => json!({
                "ok": false,
                "path": self.paths.db_path.to_string_lossy(),
                "error": err.message,
                "code": err.code,
            }),
        }
    }

    pub fn record_secure_probe(&self, report: Value) {
        if let Ok(mut slot) = self.last_secure_probe.lock() {
            *slot = Some(report);
        }
    }

    pub fn last_secure_probe(&self) -> Option<Value> {
        self.last_secure_probe
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    pub fn secure_snapshot(&self) -> Value {
        json!({
            "backend": self.secure.backend_name(),
            "service": self.secure.service_name(),
            "lastProbe": self.last_secure_probe(),
        })
    }

    pub fn base_health(&self) -> Value {
        json!({
            "status": "starting",
            "protocol": version::PROTOCOL_VERSION,
            // 签到 / 领取按北京时间结算，界面上的"今天"必须以这个为准。
            "today": crate::clock::beijing_date(),
            "worker": {
                "version": version::WORKER_VERSION,
                "aiccVersion": version::aicc_version(),
                "pid": std::process::id(),
                "uptimeMs": self.uptime_ms(),
                "executable": std::env::current_exe()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            "upstream": {
                "crate": "wb-switch-core",
                "version": version::UPSTREAM_CORE_VERSION,
                "commit": version::UPSTREAM_CORE_COMMIT,
            },
            "paths": {
                "dataDir": self.paths.data_dir.to_string_lossy(),
                "dbPath": self.paths.db_path.to_string_lossy(),
                "backupsDir": self.paths.backups_dir.to_string_lossy(),
                "logsDir": self.paths.logs_dir.to_string_lossy(),
                "wbSwitchHome": self.paths.wb_switch_home.to_string_lossy(),
                "wbSwitchHomeOwned": self.paths.wb_home_owned,
            },
        })
    }
}
