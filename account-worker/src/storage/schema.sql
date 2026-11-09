-- AICC 账号中心初始结构（PR1a）
-- 只放骨架表：PR2 会补分组 / 账号字段的完整 CRUD，PR3/PR4 补任务执行细节。
-- 迁移必须只做增量：已发布的库不会被重建。

CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS account_groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts (
    id               TEXT PRIMARY KEY,
    platform         TEXT NOT NULL CHECK (platform IN ('workbuddy', 'qoder_cn')),
    uid              TEXT NOT NULL,
    nickname         TEXT NOT NULL DEFAULT '',
    email            TEXT,
    group_id         TEXT REFERENCES account_groups(id) ON DELETE SET NULL,
    auth_ref         TEXT,
    profile_ref      TEXT,
    auth_status      TEXT NOT NULL DEFAULT 'unknown'
                     CHECK (auth_status IN ('unknown', 'valid', 'expired', 'invalid', 'denied')),
    checkin_enabled  INTEGER NOT NULL DEFAULT 0 CHECK (checkin_enabled IN (0, 1)),
    last_checkin_at  TEXT,
    last_result      TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    -- 账号唯一性是 platform + uid：不同平台即使 UID 相同也不是同一账号。
    UNIQUE (platform, uid)
);

CREATE INDEX IF NOT EXISTS idx_accounts_platform ON accounts (platform);
CREATE INDEX IF NOT EXISTS idx_accounts_group ON accounts (group_id);

CREATE TABLE IF NOT EXISTS task_definitions (
    id         TEXT PRIMARY KEY,
    platform   TEXT NOT NULL,
    kind       TEXT NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    schedule   TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (platform, kind)
);

CREATE TABLE IF NOT EXISTS task_executions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id    TEXT REFERENCES accounts(id) ON DELETE CASCADE,
    platform      TEXT NOT NULL,
    task_kind     TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed', 'skipped')),
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    credits       INTEGER,
    error_kind    TEXT,
    error_detail  TEXT,
    next_retry_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_task_executions_account ON task_executions (account_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_task_executions_started ON task_executions (started_at DESC);

CREATE TABLE IF NOT EXISTS checkin_records (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    platform   TEXT NOT NULL,
    -- 本地日期（北京时间），一账号一天一条，重复执行幂等。
    local_date TEXT NOT NULL,
    status     TEXT NOT NULL CHECK (status IN ('claimed', 'already', 'failed', 'skipped')),
    credits    INTEGER,
    checked_at TEXT NOT NULL,
    expires_at TEXT,
    UNIQUE (account_id, local_date)
);

CREATE TABLE IF NOT EXISTS credit_snapshots (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    platform    TEXT NOT NULL,
    credits     INTEGER,
    expires_at  TEXT,
    captured_at TEXT NOT NULL,
    source      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_credit_snapshots_account ON credit_snapshots (account_id, captured_at DESC);

CREATE TABLE IF NOT EXISTS app_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
