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
ACCOUNT_REFRESH_SECONDS = max(30, min(300, int(os.environ.get("WORKBUDDY_REFRESH_SECONDS", "120"))))
BALANCE_ERROR_MESSAGES = {
    "bridge_unavailable": "WorkBuddy monitoring bridge is not running",
    "renderer_not_found": "WorkBuddy renderer page was not found",
    "account_api_unavailable": "WorkBuddy account usage API is unavailable",
    "account_api_invalid": "WorkBuddy account usage API returned no usable balance",
    "menu_trigger_not_found": "WorkBuddy account menu trigger was not found",
    "balance_text_not_found": "WorkBuddy balance text was not found",
    "balance_parse_failed": "WorkBuddy balance text could not be parsed",
    "timeout": "WorkBuddy balance read timed out",
}
_lock = threading.Lock()
_last_attempt = 0.0
_account_cache: dict[str, Any] | None = None
_account_target: str | None = None
_account_last_error = False
_account_last_error_code: str | None = None
_account_last_diag: dict[str, Any] | None = None

ACCOUNT_EXPRESSION = r"""
(async () => {
  const balanceLabels = ['积分余额', '剩余积分', '可用积分', '积分', '余额', 'Credits', 'Credit', 'Balance', 'Remaining'];
  const publicFields = [
    'usageLeft', 'usageTotal', 'usageUsed', 'balance', 'credit', 'credits', 'remaining',
    'remainingPoints', 'creditBalance', 'pointBalance', 'availableCredit', 'points',
    'totalPoints', 'usedPoints', 'refreshAt', 'resetAt'
  ];

  const safeNumber = (value) => {
    if (typeof value === 'number') return Number.isFinite(value) ? String(value) : null;
    if (typeof value !== 'string') return null;
    const normalized = value.replace(/[,，\s]/g, '');
    return /^\d+(?:\.\d+)?$/.test(normalized) ? normalized : null;
  };

  const firstNumber = (object, names) => {
    if (!object || typeof object !== 'object') return null;
    for (const name of names) {
      const value = safeNumber(object[name]);
      if (value !== null) return value;
    }
    return null;
  };

  const presentPublicFields = (object) => publicFields.filter((name) =>
    Object.prototype.hasOwnProperty.call(object || {}, name));

  const host = globalThis.workbuddyDesktop;
  const apiProbe = {
    page: (typeof location !== 'undefined' && location.href) || '',
    title: (typeof document !== 'undefined' && document.title) || '',
    hostKeys: host && typeof host === 'object'
      ? Object.keys(host).filter((key) =>
          /^(platform|appVersion|configDir|machineId|app|invoke|events|window|clipboard|shell|network|log|storage|product|ui|menu|fs|dialog|power|updater|oauth|protocol)/i.test(key)
        ).slice(0, 40)
      : []
  };
  let apiErrorCode = null;
  let apiError = null;
  // WorkBuddy 5.3.x reads account usage through the in-process daemon RPC:
  // the renderer opens a MessageChannel, the preload forwards it to the main
  // process, and the daemon replies with {kind:'open', sessionId}. Requests
  // are {id, type:'request', channel:'auth:getAccountUsage', args:[]} — the
  // in-process daemon registers auth handlers under their RPC channel names
  // (registry.handle('auth:getAccountUsage')). The IDE-only "__backend__"
  // channel is not registered in the desktop daemon.
  // Reusing that channel keeps the request inside WorkBuddy's own session and
  // never exposes authentication material to AICC.
  const readUsageViaDaemon = () => new Promise((resolve) => {
    if (typeof window === 'undefined' || typeof MessageChannel === 'undefined'
        || typeof window.postMessage !== 'function') {
      apiError = 'daemon transport is unavailable in this context';
      resolve(null);
      return;
    }
    try {
      const channel = new MessageChannel();
      const port = channel.port1;
      let sessionId = null;
      let settled = false;
      const timer = setTimeout(() => {
        if (settled) return;
        settled = true;
        try { port.close(); } catch (_) {}
        apiError = 'daemon rpc timed out';
        resolve(null);
      }, 3500);
      const finish = (value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        try { port.close(); } catch (_) {}
        resolve(value);
      };
      port.onmessage = (event) => {
        const payload = event.data;
        if (!payload || typeof payload !== 'object' || settled) return;
        if (payload.kind === 'open') {
          sessionId = payload.sessionId;
          const requestId = 'aicc-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8);
          port.postMessage({
            kind: 'message',
            json: { id: requestId, type: 'request', channel: 'auth:getAccountUsage', args: [] }
          });
          return;
        }
        if (payload.kind === 'message' && payload.json && payload.json.id
            && String(payload.json.id).startsWith('aicc-')) {
          if (payload.json.type === 'error' || payload.json.error) {
            apiError = String((payload.json.error && payload.json.error.message) || payload.json.error || 'daemon rpc error');
            finish(null);
            return;
          }
          finish(payload.json.result);
          return;
        }
        if (payload.kind === 'error') {
          apiError = String(payload.error || 'daemon transport error');
          finish(null);
          return;
        }
        if (payload.kind === 'close') {
          apiError = 'daemon transport closed';
          finish(null);
        }
      };
      port.onmessageerror = () => {
        apiError = 'daemon transport port error';
        finish(null);
      };
      port.start();
      window.postMessage(
        { type: 'workbuddy:open-local-daemon-transport-port', target: { transportType: 'local' } },
        '*',
        [channel.port2]
      );
    } catch (error) {
      apiError = String((error && error.message) || error);
      resolve(null);
    }
  });
  const readUsageViaHost = async () => {
    if (host && typeof host.invoke === 'function') {
      try {
        const value = await host.invoke({type: 'invoke', channel: 'auth:getAccountUsage'});
        if (value !== undefined && value !== null) return value;
        apiError = apiError || 'host invoke returned no value';
      } catch (error) {
        apiError = apiError || String((error && error.message) || error);
      }
    }
    if (host && typeof host.authGetAccountUsage === 'function') {
      try {
        const value = await host.authGetAccountUsage();
        if (value !== undefined && value !== null) return value;
        apiError = apiError || 'legacy api returned no value';
      } catch (error) {
        apiError = apiError || String((error && error.message) || error);
      }
    }
    return null;
  };
  let usage = await readUsageViaDaemon();
  if (usage && typeof usage === 'object' && usage.data !== undefined
      && usage.data !== null && typeof usage.data === 'object') {
    usage = usage.data;
  }
  if (usage === null || usage === undefined) {
    usage = await readUsageViaHost();
  }
  if (usage !== null && usage !== undefined) {
    try {
      const usageLeft = firstNumber(usage, [
        'usageLeft', 'balance', 'credit', 'credits', 'remaining', 'remainingPoints',
        'creditBalance', 'pointBalance', 'availableCredit', 'points'
      ]);
      if (usageLeft !== null) {
        return {
          usageLeft,
          usageTotal: firstNumber(usage, ['usageTotal', 'totalPoints', 'totalCredit', 'total']),
          usageUsed: firstNumber(usage, ['usageUsed', 'usedPoints', 'usedCredit', 'used']),
          refreshAt: usage?.refreshAt ?? usage?.resetAt ?? null,
          source: 'account-api',
          apiFields: presentPublicFields(usage)
        };
      }
      apiErrorCode = 'account_api_invalid';
    } catch (_) {
      apiErrorCode = 'account_api_unavailable';
    }
  } else {
    apiErrorCode = 'account_api_unavailable';
  }

  const isVisible = (element) => {
    if (!element) return false;
    const style = globalThis.getComputedStyle?.(element);
    const rect = element.getBoundingClientRect?.();
    return style?.display !== 'none' && style?.visibility !== 'hidden'
      && (!rect || (rect.width > 0 && rect.height > 0));
  };

  const textOf = (element) => {
    if (!element || ['INPUT', 'TEXTAREA', 'SCRIPT', 'STYLE'].includes(element.tagName)) return '';
    return String(element.innerText || element.textContent || '').replace(/\u00a0/g, ' ').trim();
  };

  const parseBalanceText = (text) => {
    const value = String(text || '').replace(/\r/g, '\n');
    const label = '(?:积分余额|剩余积分|可用积分|积分|余额|Credits?|Balance|Remaining)';
    const number = '((?:\\d{1,3}(?:[ ,，]\\d{3})+(?:\\.\\d+)?|\\d+(?:\\.\\d+)?))';
    const patterns = [
      new RegExp(`${label}\\s*(?:刷新|刷新余额|Refresh|更新|Update)?\\s*[:：]?\\s*${number}`, 'i'),
      new RegExp(`${number}\\s*${label}`, 'i')
    ];
    for (const pattern of patterns) {
      const match = pattern.exec(value);
      if (!match) continue;
      const numberText = match[1] || match[0].match(/\d[\d,，\s]*(?:\.\d+)?/)[0];
      const prefix = value.slice(Math.max(0, match.index - 10), match.index);
      if (/(?:今日|使用|已用|消耗|累计|总额|套餐|Used|Spent|Consumed|Total)/i.test(prefix)) continue;
      const normalized = safeNumber(numberText);
      if (normalized !== null) return normalized;
    }
    return null;
  };

  const balanceRoots = () => [
    ...document.querySelectorAll(
      '[role="menu"], [role="dialog"], [aria-label*="账户"], [aria-label*="account" i], '
      + '[class*="account-menu" i], [class*="user-menu" i], [data-testid*="account" i], '
      + '[data-testid*="menu" i]'
    )
  ].filter(isVisible);

  const readVisibleBalance = () => {
    const candidates = [];
    const add = (element) => {
      if (element && isVisible(element) && !candidates.includes(element)) candidates.push(element);
    };
    balanceRoots().forEach(add);
    document.querySelectorAll('button, [role="button"], [role="menuitem"], [aria-label], [data-testid]')
      .forEach((element) => {
        if (!isVisible(element)) return;
        const own = textOf(element);
        if (balanceLabels.some((label) => own.toLowerCase().includes(label.toLowerCase()))) {
          add(element);
          add(element.parentElement);
          add(element.nextElementSibling);
          add(element.parentElement?.nextElementSibling);
        }
      });
    for (const element of candidates) {
      const parsed = parseBalanceText(textOf(element));
      if (parsed !== null) return parsed;
      const nearby = [
        element,
        element.parentElement,
        element.nextElementSibling,
        element.parentElement?.nextElementSibling
      ].filter(Boolean).map(textOf).filter(Boolean).join(' ');
      const nearbyParsed = parseBalanceText(nearby);
      if (nearbyParsed !== null) return nearbyParsed;
    }
    return null;
  };

  const findMenuTrigger = () => {
    const selector = [
      '[aria-label*="账户"]', '[aria-label*="用户"]', '[aria-label*="account" i]',
      '[aria-label*="profile" i]', '[data-testid*="account" i]', '[data-testid*="user" i]',
      '[data-testid*="profile" i]', '[class*="user-menu-trigger" i]', '[class*="avatar" i]'
    ].join(',');
    const selected = [...document.querySelectorAll(selector)].find(isVisible);
    if (selected) return selected;
    return [...document.querySelectorAll('button, [role="button"]')].find((element) => {
      if (!isVisible(element)) return false;
      const value = `${element.getAttribute('aria-label') || ''} ${textOf(element)}`.trim();
      return /^(卡里|账户|用户|Account|Profile|User)$/i.test(value);
    }) || null;
  };

  const menuIsOpen = (trigger) => Boolean(
    balanceRoots().length || trigger?.getAttribute('aria-expanded') === 'true');

  const waitForBalance = async () => {
    for (let attempt = 0; attempt < 30; attempt += 1) {
      const value = readVisibleBalance();
      if (value !== null) return value;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    return null;
  };

  let usageLeft = readVisibleBalance();
  let openedByMonitor = false;
  const trigger = findMenuTrigger();
  if (usageLeft === null && trigger && !menuIsOpen(trigger)) {
    trigger.click();
    openedByMonitor = true;
    usageLeft = await waitForBalance();
  }
  if (openedByMonitor && trigger) {
    trigger.click();
  }
  if (usageLeft !== null) return {usageLeft, refreshAt: null, source: 'account-menu'};
  return {
    __errorCode: apiErrorCode || (trigger ? 'balance_text_not_found' : 'menu_trigger_not_found'),
    __error: 'WorkBuddy balance read failed',
    __apiProbe: apiProbe,
    __apiError: apiError ? String(apiError).slice(0, 240) : null
  };
})()
"""


def initial_status(value: dict[str, Any]) -> dict[str, Any]:
    """Hide manual points until WorkBuddy has produced a real balance."""
    result = value.copy()
    if result.get("balance_updated_epoch") is not None:
        return result
    for key in ("points", "total_points", "cycle_used_points", "used_points", "reset_text"):
        result[key] = None
    result.update({
        "balance_state": "Unavailable",
        "balance_stale": True,
        "balance_updated_at": None,
        "balance_age_seconds": None,
        "usage_source": "WorkBuddy unavailable",
    })
    result.pop("balance_error_code", None)
    result.pop("balance_error", None)
    return result


def collect(fallback: dict, force: bool = False, *, database_path: Path = DEFAULT_DB_PATH) -> dict:
    """Read balance through WorkBuddy itself and today's usage from its local DB."""
    result = fallback.copy()
    account = _get_account_usage(force=force)
    if account:
        result.update(account)
    elif _has_successful_balance(result):
        result = _with_stale_state(result) or result
    else:
        for key in ("total_points", "cycle_used_points", "used_points", "reset_text"):
            result[key] = None
        # An old manually entered value is not a successful WorkBuddy read.
        # Keep the key explicit so the server cannot re-introduce that value
        # while merging the collector snapshot into status.json.
        result["points"] = None
        result["balance_state"] = "Unavailable"
        result["balance_stale"] = True
        result["balance_updated_at"] = None
        result["balance_updated_epoch"] = None
        result["balance_age_seconds"] = None
    if _account_last_error_code:
        result.update(_error_payload())

    local_usage = _read_today_usage(database_path)
    if local_usage:
        result.update(local_usage)

    sources = ["WorkBuddy account" if account or _has_successful_balance(result) else "WorkBuddy unavailable"]
    if local_usage:
        sources.append("local usage")
    result["usage_source"] = " + ".join(sources)
    return result


def _get_account_usage(force: bool = False) -> dict[str, Any] | None:
    global _last_attempt, _account_cache, _account_target, _account_last_error, _account_last_error_code, _account_last_diag
    now = time.monotonic()
    with _lock:
        if _account_cache is None:
            _account_cache = _load_account_cache()
        if not force and _last_attempt and now - _last_attempt < ACCOUNT_REFRESH_SECONDS:
            return _with_stale_state(_account_cache)
        _last_attempt = now
        try:
            target = target_identity_localhost(DEBUG_PORT)
            raw = evaluate_localhost(DEBUG_PORT, ACCOUNT_EXPRESSION)
            parsed = _normalise_account_usage(raw)
            if parsed:
                parsed["balance_updated_epoch"] = time.time()
                parsed["balance_updated_at"] = datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S")
                _account_cache = parsed
                _account_target = target
                _account_last_error = False
                _account_last_error_code = None
                _account_last_diag = None
                _save_account_cache(parsed)
            else:
                _account_target = None
                _account_last_diag = {
                    key: value for key, value in raw.items()
                    if key in ("__apiProbe", "__apiError")
                } if isinstance(raw, dict) else None
                _set_account_error(_error_code_from_raw(raw))
        except (CdpError, OSError, ValueError, TypeError) as error:
            _account_target = None
            _account_last_diag = None
            _set_account_error(_error_code_from_exception(error))
        return _with_stale_state(_account_cache)


def _normalise_account_usage(raw: Any) -> dict[str, Any] | None:
    if not isinstance(raw, dict) or raw.get("__error"):
        return None
    left = _first_number(
        raw,
        (
            "usageLeft", "balance", "credit", "credits", "remaining", "remainingPoints",
            "creditBalance", "pointBalance", "availableCredit", "points",
        ),
    )
    total = _first_number(raw, ("usageTotal", "totalPoints", "totalCredit", "total"))
    used = _first_number(raw, ("usageUsed", "usedPoints", "usedCredit", "used"))
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
        value = "".join(value.split())
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return int(number) if number.is_integer() else round(number, 2)


def _first_number(value: dict[str, Any], names: tuple[str, ...]) -> int | float | None:
    for name in names:
        number = _number(value.get(name))
        if number is not None:
            return number
    return None


def _has_successful_balance(value: dict[str, Any]) -> bool:
    return value.get("points") is not None and value.get("balance_updated_epoch") is not None


def _error_payload() -> dict[str, Any]:
    code = _account_last_error_code or "account_api_unavailable"
    payload: dict[str, Any] = {
        "balance_error_code": code,
        "balance_error": BALANCE_ERROR_MESSAGES.get(code, "WorkBuddy balance read failed"),
    }
    if _account_last_diag:
        payload["balance_diagnostic"] = _account_last_diag
    return payload


def _set_account_error(code: str) -> None:
    global _account_last_error, _account_last_error_code
    _account_last_error = True
    _account_last_error_code = code if code in BALANCE_ERROR_MESSAGES else "account_api_invalid"


def _error_code_from_raw(raw: Any) -> str:
    code = raw.get("__errorCode") if isinstance(raw, dict) else None
    return code if isinstance(code, str) and code in BALANCE_ERROR_MESSAGES else "account_api_invalid"


def _error_code_from_exception(error: Exception) -> str:
    message = str(error).lower()
    if "timeout" in message or "timed out" in message:
        return "timeout"
    if "page" in message or "target" in message or "renderer" in message:
        return "renderer_not_found"
    if isinstance(error, (CdpError, OSError)) or "bridge" in message or "connect" in message:
        return "bridge_unavailable"
    if isinstance(error, (ValueError, TypeError)):
        return "account_api_invalid"
    return "account_api_unavailable"


def _with_stale_state(value: dict[str, Any] | None) -> dict[str, Any] | None:
    if not value:
        return None
    result = value.copy()
    try:
        updated = float(result.get("balance_updated_epoch", 0) or 0)
    except (TypeError, ValueError):
        updated = 0
    result["balance_age_seconds"] = max(0, round(time.time() - updated)) if updated else None
    result["balance_stale"] = not updated or result["balance_age_seconds"] > ACCOUNT_REFRESH_SECONDS * 3
    result["balance_state"] = "Cached" if result["balance_stale"] or _account_last_error else "Connected"
    if _account_last_error_code:
        result.update(_error_payload())
    else:
        result.pop("balance_error_code", None)
        result.pop("balance_error", None)
    if updated:
        result.setdefault(
            "balance_updated_at",
            datetime.fromtimestamp(updated).astimezone().strftime("%Y-%m-%d %H:%M:%S"),
        )
    else:
        result.setdefault("balance_updated_at", None)
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
        try:
            existing_epoch = float(existing.get("balance_updated_epoch", 0) or 0)
        except (TypeError, ValueError):
            existing_epoch = 0
        recent = time.time() - existing_epoch < 900
        if unchanged and recent:
            return
        temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, ACCOUNT_CACHE_PATH)
    except OSError:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
