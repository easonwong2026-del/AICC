"""Automatically collect WorkBuddy balance and local usage without reading tokens."""

from __future__ import annotations
import json
import os
import sqlite3
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any

from services.cdp import CdpError, evaluate_localhost, target_identity_localhost

DEFAULT_DB_PATH = Path.home() / ".workbuddy" / "workbuddy.db"
DATA_ROOT = Path(os.environ.get("EINK_DATA_DIR", Path(__file__).resolve().parents[1] / "data")).expanduser()
ACCOUNT_CACHE_PATH = DATA_ROOT / "workbuddy_last_success.json"
DEBUG_PORT = int(os.environ.get("WORKBUDDY_DEBUG_PORT", "9223"))
ACCOUNT_REFRESH_SECONDS = max(30, min(300, int(os.environ.get("WORKBUDDY_REFRESH_SECONDS", "60"))))
_lock = threading.Lock()
_last_attempt = 0.0
_account_cache: dict[str, Any] | None = None
_account_target: str | None = None

ACCOUNT_EXPRESSION = """
(async () => {
  const api = globalThis.workbuddyDesktop;
  if (api && typeof api.authGetAccountUsage === 'function') {
    return await api.authGetAccountUsage();
  }

  // WorkBuddy 5.2.5 no longer exposes authGetAccountUsage in preload.
  // Read the same visible balance used by its account menu, then restore the
  // menu to its original state. No authentication data leaves the renderer.
  const readVisibleBalance = () => {
    const lines = (document.body?.innerText || '')
      .split('\\n')
      .map(value => value.trim())
      .filter(Boolean);
    const index = lines.findIndex(value => value.includes('\\u79ef\\u5206\\u4f59\\u989d'));
    if (index < 0 || index + 1 >= lines.length) return null;
    const value = lines[index + 1].replace(/[,，\\s]/g, '');
    return /^\\d+(?:\\.\\d+)?$/.test(value) ? value : null;
  };

  let usageLeft = readVisibleBalance();
  let openedByMonitor = false;
  const trigger = document.querySelector('.user-menu-trigger--workbuddy');
  if (!usageLeft && trigger) {
    trigger.click();
    openedByMonitor = true;
    await new Promise(resolve => setTimeout(resolve, 1800));
    usageLeft = readVisibleBalance();
  }
  if (openedByMonitor && trigger) {
    trigger.click();
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  return usageLeft
    ? {usageLeft, refreshAt: null, source: 'account-menu'}
    : {__error: 'WorkBuddy balance is not visible'};
})()
"""


def collect(fallback: dict, database_path: Path = DEFAULT_DB_PATH) -> dict:
    """Read balance through WorkBuddy itself and today's usage from its local DB."""
    result = fallback.copy()
    account = _get_account_usage()
    if account:
        result.update(account)

    local_usage = _read_today_usage(database_path)
    if local_usage:
        result.update(local_usage)

    sources = ["WorkBuddy account" if account else "Manual balance"]
    if local_usage:
        sources.append("local usage")
    result["usage_source"] = " + ".join(sources)
    return result


def _get_account_usage() -> dict[str, Any] | None:
    global _last_attempt, _account_cache, _account_target
    now = time.monotonic()
    with _lock:
        if _account_cache is None:
            _account_cache = _load_account_cache()
        if now - _last_attempt < ACCOUNT_REFRESH_SECONDS:
            return _with_stale_state(_account_cache)
        _last_attempt = now
        try:
            target = target_identity_localhost(DEBUG_PORT)
            if target == _account_target:
                return _with_stale_state(_account_cache)
            raw = evaluate_localhost(DEBUG_PORT, ACCOUNT_EXPRESSION)
            parsed = _normalise_account_usage(raw)
            if parsed:
                parsed["balance_updated_epoch"] = time.time()
                parsed["balance_updated_at"] = datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S")
                _account_cache = parsed
                _account_target = target
                _save_account_cache(parsed)
        except (CdpError, OSError, ValueError, TypeError):
            _account_target = None
        return _with_stale_state(_account_cache)


def _normalise_account_usage(raw: Any) -> dict[str, Any] | None:
    if not isinstance(raw, dict) or raw.get("__error"):
        return None
    left = _number(raw.get("usageLeft"))
    total = _number(raw.get("usageTotal"))
    used = _number(raw.get("usageUsed"))
    if left is None:
        return None
    result: dict[str, Any] = {"points": left, "balance_state": "Connected"}
    if total is not None:
        result["total_points"] = total
    if used is not None:
        result["cycle_used_points"] = used
    refresh_at = raw.get("refreshAt")
    if refresh_at:
        timestamp = float(refresh_at)
        if timestamp > 10_000_000_000:
            timestamp /= 1000
        result["reset_text"] = "Reset " + datetime.fromtimestamp(timestamp).astimezone().strftime("%Y-%m-%d %H:%M")
    else:
        result["reset_text"] = "Auto updated"
    return result


def _read_today_usage(database_path: Path) -> dict[str, Any] | None:
    if not database_path.exists():
        return None
    start_today_ms = int(datetime.now().astimezone().replace(hour=0, minute=0, second=0, microsecond=0).timestamp() * 1000)
    try:
        with sqlite3.connect(f"file:{database_path}?mode=ro", uri=True, timeout=1) as connection:
            rows = connection.execute(
                "select credit_json from session_usage where updated_at >= ? and credit_json is not null",
                (start_today_ms,),
            )
            total = 0.0
            records = 0
            for (raw,) in rows:
                try:
                    item = json.loads(raw)
                    if isinstance(item, dict):
                        total += sum(value for value in item.values() if isinstance(value, (int, float)))
                        records += 1
                except (TypeError, ValueError):
                    continue
        return {"auto_used_credits": round(total, 2), "usage_records": records}
    except sqlite3.Error:
        return None


def _number(value: Any) -> int | float | None:
    if value in (None, "", "unlimited"):
        return None
    if isinstance(value, str):
        value = value.replace(",", "").replace("，", "").strip()
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return int(number) if number.is_integer() else round(number, 2)


def _with_stale_state(value: dict[str, Any] | None) -> dict[str, Any] | None:
    if not value:
        return None
    result = value.copy()
    updated = float(result.get("balance_updated_epoch", 0) or 0)
    result["balance_age_seconds"] = max(0, round(time.time() - updated)) if updated else None
    result["balance_stale"] = not updated or result["balance_age_seconds"] > ACCOUNT_REFRESH_SECONDS * 3
    result["balance_state"] = "Cached" if result["balance_stale"] else "Connected"
    return result


def _load_account_cache() -> dict[str, Any] | None:
    try:
        value = json.loads(ACCOUNT_CACHE_PATH.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) and value.get("points") is not None else None
    except (OSError, ValueError):
        return None


def _save_account_cache(value: dict[str, Any]) -> None:
    temporary = ACCOUNT_CACHE_PATH.with_suffix(".json.tmp")
    try:
        ACCOUNT_CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
        try:
            existing = json.loads(ACCOUNT_CACHE_PATH.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            existing = {}
        stable_keys = ("points", "total_points", "cycle_used_points", "reset_text")
        unchanged = all(existing.get(key) == value.get(key) for key in stable_keys)
        recent = time.time() - float(existing.get("balance_updated_epoch", 0) or 0) < 900
        if unchanged and recent:
            return
        temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, ACCOUNT_CACHE_PATH)
    except OSError:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
