"""OpenCodex Codex account pool quota; only safe CLI account fields enter AICC."""

import json
import math
import os
import subprocess
from datetime import datetime
from pathlib import Path

from collectors.google import epoch, ocx_env, resolve_executable

DATA_ROOT = Path(os.environ.get("EINK_DATA_DIR", Path(__file__).resolve().parents[1] / "data")).expanduser()
CACHE_PATH = DATA_ROOT / "codex_last_success.json"


def _percent(value):
    if isinstance(value, bool):
        return None
    try:
        number = float(value)
        return min(100, max(0, 100 - number)) if math.isfinite(number) else None
    except (TypeError, ValueError, OverflowError):
        return None


def _window(quota, percent_key, reset_key, minutes):
    reset_epoch = epoch(quota.get(reset_key))
    return {"remaining": _percent(quota.get(percent_key)),
            "reset": datetime.fromtimestamp(reset_epoch).astimezone().strftime("%Y-%m-%d %H:%M") if reset_epoch else None,
            "duration_minutes": minutes}


def parse_pool(document, current):
    if not isinstance(document, dict) or not isinstance(document.get("accounts"), list):
        raise ValueError("No data")
    if not isinstance(current, dict):
        raise ValueError("No active account data")
    active_id = current.get("activeId")
    if active_id is not None and not isinstance(active_id, str):
        raise ValueError("Invalid active account")
    if any(isinstance(row, dict) and isinstance(row.get("quotaRefresh"), dict)
           and row["quotaRefresh"].get("status") not in (None, "ok") for row in document["accounts"]):
        raise ValueError("Temporarily unavailable")
    accounts = []
    for row in document["accounts"]:
        if not isinstance(row, dict):
            continue
        quota = row.get("quota") if isinstance(row.get("quota"), dict) else {}
        short_key = "shortPercent" if "shortPercent" in quota else "fiveHourPercent"
        short_reset = "shortResetAt" if "shortResetAt" in quota else "fiveHourResetAt"
        account = {"id": row.get("id") if isinstance(row.get("id"), str) else None,
                   "label": row.get("label") if isinstance(row.get("label"), str) else None,
                   "email": (row.get("email") if isinstance(row.get("email"), str)
                             and any(marker in row["email"] for marker in ("*", "•")) else None),
                   "plan": row.get("plan") if isinstance(row.get("plan"), str) else None,
                   "priority": (row.get("priority") if isinstance(row.get("priority"), int)
                                and not isinstance(row.get("priority"), bool) else None),
                   "active": bool(active_id and row.get("id") == active_id),
                   "needs_reauth": row.get("needsReauth") is True,
                   "stale": False,
                   "five_hour": _window(quota, short_key, short_reset, 300),
                   "weekly": _window(quota, "weeklyPercent", "weeklyResetAt", 10080)}
        if "monthlyPercent" in quota or "monthlyResetAt" in quota:
            account["monthly"] = _window(quota, "monthlyPercent", "monthlyResetAt", 43200)
        accounts.append(account)
    active = next((row for row in accounts if row["active"]), None)
    result = {"schema_version": 2, "available": bool(accounts), "state": "Connected", "source": "OpenCodex",
              "stale": False, "updated_at": datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S"),
              "selection_mode": "auto" if active_id is None else "pinned",
              "active_account_id": active_id, "accounts": accounts,
              "weekly": active["weekly"] if active else None,
              "five_hour": active["five_hour"] if active else None}
    return result


def load_cache(path=CACHE_PATH):
    try:
        snapshot = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if (not isinstance(snapshot, dict) or snapshot.get("schema_version") != 2
            or snapshot.get("source") != "OpenCodex" or not isinstance(snapshot.get("accounts"), list)):
        return None
    return mark_stale(snapshot, "Cached")


def mark_stale(snapshot, state):
    result = {**snapshot, "stale": True, "state": state}
    result["accounts"] = [{**row, "stale": True} for row in snapshot.get("accounts", []) if isinstance(row, dict)]
    return result


def _command(executable, args, env):
    try:
        response = subprocess.run([executable, "account", *args, "--json"], capture_output=True, text=True,
                                  timeout=5 if args[0] == "current" else 60, env=env)
    except (subprocess.TimeoutExpired, OSError):
        raise ValueError("Temporarily unavailable") from None
    if response.returncode:
        # Never echo stdout/stderr: OpenCodex errors can contain credentials.
        raise ValueError("OpenCodex is not running" if "Proxy is not running" in response.stderr else "Temporarily unavailable")
    try:
        return json.loads(response.stdout)
    except json.JSONDecodeError:
        raise ValueError("No data") from None


def collect(force=False):
    executable = resolve_executable()
    if not executable:
        raise ValueError("OpenCodex not installed")
    env = ocx_env(executable)
    document = _command(executable, ["refresh", "openai"] if force else ["list", "openai", "--quota"], env)
    if not isinstance(document, dict) or not isinstance(document.get("accounts"), list):
        raise ValueError("No data")
    current = _command(executable, ["current", "openai"], env) if document["accounts"] else {"activeId": None}
    snapshot = parse_pool(document, current)
    try:
        CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
        temporary = CACHE_PATH.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(snapshot, ensure_ascii=False) + "\n", encoding="utf-8")
        os.replace(temporary, CACHE_PATH)
    except OSError:
        pass
    return snapshot
