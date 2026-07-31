"""Provider Manifest v1: the safe display layer over raw provider status.

Raw provider status objects — including WorkBuddy account objects and
diagnostics — never cross this boundary.  Each provider is wrapped by a
manifest adapter that only emits the whitelisted presentation fields the UI
is allowed to see.

A built-in provider can plug into the manifest layer in two ways:

1. Implement a ``manifest(self, metadata)`` method on the provider class
   (the preferred plug-in interface for new providers); or
2. Register a named adapter in ``MANIFEST_ADAPTERS`` for providers whose
   collectors must stay untouched (codex, workbuddy, deepseek, system).

Providers without either receive a conservative generic manifest built from
whitelisted raw keys.
"""

from __future__ import annotations

import re
from typing import Any, Callable

SCHEMA_VERSION = 1

PROVIDER_ID_PATTERN = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")
ALLOWED_VALUE_TYPES = {"number", "percentage", "currency", "text", "status", "duration"}
ALLOWED_FORMATS = {"integer", "decimal", "percent", "currency", "plain", "compact"}
ALLOWED_STATES = {"connected", "cached", "unavailable", "error", "pending", "disabled", "unknown"}
ALLOWED_CATEGORIES = {"credits", "quota", "system", "generic"}
ALLOWED_CAPABILITIES = {"refresh", "reconnect", "diagnostics"}
ALLOWED_ACTION_KINDS = {"refresh", "reconnect", "diagnostics"}

MAX_STRING_LENGTH = 80
MAX_DISPLAY_NAME_LENGTH = 60
MAX_ICON_LENGTH = 40
MAX_TIMESTAMP_LENGTH = 32
MAX_METRICS = 12
MAX_ACTIONS = 8

STATIC_SORT_ORDERS = {
    "codex": 10,
    "workbuddy": 20,
    "deepseek": 30,
    "system": 90,
    "example": 200,
}
DEFAULT_SORT_ORDER = 100

ManifestAdapter = Callable[[dict[str, Any], dict[str, Any]], dict[str, Any]]


def _number(value: Any) -> int | float | None:
    if value is None or isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        number = float(value)
        if number != number or number in (float("inf"), float("-inf")):
            return None
        return int(number) if number.is_integer() else round(number, 6)
    if isinstance(value, str):
        cleaned = value.replace(",", "").replace("，", "").strip()
        if not cleaned:
            return None
        try:
            number = float(cleaned)
        except (TypeError, ValueError):
            return None
        return int(number) if number.is_integer() else round(number, 6)
    return None


def _string(value: Any, limit: int = MAX_STRING_LENGTH) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None
    return text[:limit]


def _state_label(raw: Any) -> str:
    """Normalize an adapter state to the whitelisted state vocabulary."""
    text = str(raw or "").strip().lower()
    aliases = {
        "online": "connected",
        "connected": "connected",
        "ready": "connected",
        "cached": "cached",
        "stale": "cached",
        "unavailable": "unavailable",
        "not configured": "unavailable",
        "notconfigured": "unavailable",
        "disabled": "disabled",
        "pending": "pending",
        "loading": "pending",
        "error": "error",
        "failed": "error",
        "timeout": "error",
        "unknown": "unknown",
    }
    return aliases.get(text, text if text in ALLOWED_STATES else "unknown")


def _sanitize_metric(metric: Any) -> dict[str, Any] | None:
    if not isinstance(metric, dict):
        return None
    key = _string(metric.get("key"), 40)
    label = _string(metric.get("label"), MAX_DISPLAY_NAME_LENGTH)
    if not key or not label:
        return None
    value_type = str(metric.get("value_type") or "number")
    if value_type not in ALLOWED_VALUE_TYPES:
        value_type = "text"
    fmt = str(metric.get("format") or "plain")
    if fmt not in ALLOWED_FORMATS:
        fmt = "plain"
    unit = _string(metric.get("unit"), 16)
    value: Any = metric.get("value")
    if isinstance(value, bool):
        pass  # booleans are allowed and displayed as text
    elif value is None:
        pass
    elif isinstance(value, (int, float)):
        if value != value or value in (float("inf"), float("-inf")):  # not finite
            value = None
    elif isinstance(value, str):
        value = value[:MAX_STRING_LENGTH]
    else:
        value = None
    return {
        "key": key,
        "label": label,
        "value": value,
        "value_type": value_type,
        "format": fmt,
        "unit": unit,
        "primary": bool(metric.get("primary")),
    }


def _sanitize_action(action: Any) -> dict[str, Any] | None:
    if not isinstance(action, dict):
        return None
    action_id = _string(action.get("id"), 40)
    label = _string(action.get("label"), MAX_DISPLAY_NAME_LENGTH)
    kind = str(action.get("kind") or "")
    if not action_id or not label or kind not in ALLOWED_ACTION_KINDS:
        return None
    # Only whitelisted keys survive; endpoint/shell_command/executable/
    # script_path/external_url are dropped even when an adapter includes them.
    return {
        "id": action_id,
        "label": label,
        "kind": kind,
        "local_only": bool(action.get("local_only", True)),
    }


def _finalize(provider_id: str, partial: dict[str, Any]) -> dict[str, Any]:
    """Validate and normalize an adapter-produced manifest."""
    display_name = _string(partial.get("display_name"), MAX_DISPLAY_NAME_LENGTH) or provider_id
    category = str(partial.get("category") or "generic")
    if category not in ALLOWED_CATEGORIES:
        category = "generic"
    icon = _string(partial.get("icon"), MAX_ICON_LENGTH)
    state = _state_label(partial.get("state"))
    available = bool(partial.get("available"))
    stale = bool(partial.get("stale"))
    updated_at = _string(partial.get("updated_at"), MAX_TIMESTAMP_LENGTH)
    try:
        sort_order = int(partial.get("sort_order") or DEFAULT_SORT_ORDER)
    except (TypeError, ValueError):
        sort_order = DEFAULT_SORT_ORDER

    capabilities = []
    for capability in partial.get("capabilities") or []:
        text = str(capability).strip()
        if text in ALLOWED_CAPABILITIES and text not in capabilities:
            capabilities.append(text)

    metrics = []
    primaries = 0
    for raw_metric in (partial.get("metrics") or [])[:MAX_METRICS]:
        metric = _sanitize_metric(raw_metric)
        if metric is None:
            continue
        if metric["primary"]:
            primaries += 1
            if primaries > 2:
                metric["primary"] = False
        metrics.append(metric)
    metrics.sort(key=lambda item: (0 if item["primary"] else 1, item["key"]))

    actions = []
    for raw_action in (partial.get("actions") or [])[:MAX_ACTIONS]:
        action = _sanitize_action(raw_action)
        if action is not None and action["kind"] not in (item["kind"] for item in actions):
            actions.append(action)

    return {
        "id": provider_id,
        "display_name": display_name,
        "category": category,
        "icon": icon,
        "state": state,
        "available": available,
        "stale": stale,
        "updated_at": updated_at,
        "sort_order": sort_order,
        "capabilities": capabilities,
        "metrics": metrics,
        "actions": actions,
    }


# --------------------------------------------------------------------------
# Built-in adapters (raw status -> safe manifest)
# --------------------------------------------------------------------------

def _updated_at(status: dict[str, Any], metadata: dict[str, Any], *keys: str) -> str | None:
    for key in keys:
        value = _string(status.get(key), MAX_TIMESTAMP_LENGTH)
        if value:
            return value
    return _string(metadata.get("last_success"), MAX_TIMESTAMP_LENGTH)


def _codex_manifest(status: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
    weekly = status.get("weekly") if isinstance(status.get("weekly"), dict) else {}
    five_hour = status.get("five_hour") if isinstance(status.get("five_hour"), dict) else {}
    weekly_remaining = _number(weekly.get("remaining"))
    five_remaining = _number(five_hour.get("remaining"))
    available = bool(status.get("available"))
    stale = bool(status.get("stale"))
    state = "cached" if stale else "connected" if available else "unavailable"
    if not available and not weekly_remaining and not five_remaining:
        state = _state_label(status.get("state") or "unavailable")

    metrics: list[dict[str, Any]] = []
    if weekly_remaining is not None:
        metrics.append({
            "key": "weekly_remaining",
            "label": "Codex Weekly 剩余",
            "value": weekly_remaining,
            "value_type": "percentage",
            "format": "percent",
            "unit": "%",
            "primary": True,
        })
        metrics.append({
            "key": "weekly_reset",
            "label": "Weekly 重置",
            "value": _string(weekly.get("reset")) or "--",
            "value_type": "text",
            "format": "plain",
            "unit": None,
            "primary": False,
        })
    if five_remaining is not None:
        metrics.append({
            "key": "five_hour_remaining",
            "label": "5 Hour 剩余",
            "value": five_remaining,
            "value_type": "percentage",
            "format": "percent",
            "unit": "%",
            "primary": True,
        })
        metrics.append({
            "key": "five_hour_reset",
            "label": "5 Hour 重置",
            "value": _string(five_hour.get("reset")) or "--",
            "value_type": "text",
            "format": "plain",
            "unit": None,
            "primary": False,
        })
    return {
        "display_name": "Codex",
        "category": "quota",
        "icon": "chart.bar.fill",
        "state": state,
        "available": available,
        "stale": stale,
        "updated_at": _updated_at(status, metadata),
        "sort_order": STATIC_SORT_ORDERS["codex"],
        "capabilities": ["refresh"],
        "metrics": metrics,
        "actions": [{"id": "refresh", "label": "刷新 Codex", "kind": "refresh", "local_only": True}],
    }


def _workbuddy_manifest(status: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
    points = _number(status.get("points"))
    used = _number(status.get("auto_used_credits"))
    if used is None:
        used = _number(status.get("cycle_used_points"))
    if used is None:
        used = _number(status.get("used_points"))
    total = _number(status.get("total_points"))
    raw_state = _string(status.get("balance_state")) or ("Connected" if points is not None else "Unavailable")
    stale = bool(status.get("balance_stale")) or raw_state.lower() == "cached"
    if points is None:
        state = "error" if status.get("balance_error_code") else "unavailable"
    else:
        state = "cached" if stale else "connected"

    metrics: list[dict[str, Any]] = [{
        "key": "points",
        "label": "剩余积分",
        "value": points,
        "value_type": "number",
        "format": "decimal",
        "unit": "积分",
        "primary": True,
    }]
    if used is not None:
        metrics.append({
            "key": "used_today",
            "label": "今日使用",
            "value": used,
            "value_type": "number",
            "format": "decimal",
            "unit": "积分",
            "primary": False,
        })
    if total is not None:
        metrics.append({
            "key": "total_points",
            "label": "总积分",
            "value": total,
            "value_type": "number",
            "format": "decimal",
            "unit": "积分",
            "primary": False,
        })
    age = status.get("balance_age_seconds")
    if stale and points is not None and isinstance(age, (int, float)):
        metrics.append({
            "key": "cache_age",
            "label": "缓存年龄",
            "value": max(0, int(age)),
            "value_type": "duration",
            "format": "plain",
            "unit": "秒",
            "primary": False,
        })
    return {
        "display_name": "WorkBuddy",
        "category": "credits",
        "icon": "wand.and.stars",
        "state": state,
        "available": points is not None,
        "stale": stale,
        "updated_at": _updated_at(status, metadata, "balance_updated_at"),
        "sort_order": STATIC_SORT_ORDERS["workbuddy"],
        "capabilities": ["refresh", "reconnect", "diagnostics"],
        "metrics": metrics,
        "actions": [
            {"id": "refresh", "label": "刷新 WorkBuddy", "kind": "refresh", "local_only": True},
            {"id": "reconnect", "label": "重连 WorkBuddy", "kind": "reconnect", "local_only": True},
            {"id": "diagnostics", "label": "WorkBuddy 诊断", "kind": "diagnostics", "local_only": True},
        ],
    }


def _deepseek_manifest(status: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
    raw_status = _string(status.get("status")) or "Unavailable"
    balances = status.get("balances") if isinstance(status.get("balances"), list) else []
    balance = next((item for item in balances if isinstance(item, dict) and item.get("currency") == "CNY"), None)
    if balance is None and balances:
        balance = balances[0] if isinstance(balances[0], dict) else None
    balance_value = _number(balance.get("total_balance")) if balance else None
    currency = _string(balance.get("currency"), 8) if balance else "CNY"
    usage = status.get("usage") if isinstance(status.get("usage"), list) else []
    used_item = next((item for item in usage if isinstance(item, dict) and item.get("currency") == currency), None)
    used_value = _number(used_item.get("used_today")) if used_item else None

    if raw_status == "Online":
        state = "connected"
    elif raw_status == "Not configured":
        state = "unavailable"
    else:
        state = "error"
    metrics: list[dict[str, Any]] = []
    if balance_value is not None:
        metrics.append({
            "key": "balance",
            "label": "当前余额",
            "value": balance_value,
            "value_type": "currency",
            "format": "currency",
            "unit": currency,
            "primary": True,
        })
    if used_value is not None:
        metrics.append({
            "key": "used_today",
            "label": "今日使用",
            "value": used_value,
            "value_type": "currency",
            "format": "currency",
            "unit": currency,
            "primary": False,
        })
    if not metrics:
        metrics.append({
            "key": "status",
            "label": "状态",
            "value": raw_status,
            "value_type": "status",
            "format": "plain",
            "unit": None,
            "primary": True,
        })
    return {
        "display_name": "DeepSeek",
        "category": "credits",
        "icon": "brain.head.profile",
        "state": state,
        "available": balance_value is not None,
        "stale": False,
        "updated_at": _updated_at(status, metadata),
        "sort_order": STATIC_SORT_ORDERS["deepseek"],
        "capabilities": ["refresh"],
        "metrics": metrics,
        "actions": [{"id": "refresh", "label": "刷新 DeepSeek", "kind": "refresh", "local_only": True}],
    }


def _system_manifest(status: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
    raw_status = _string(status.get("status")) or "--"
    return {
        "display_name": "System",
        "category": "system",
        "icon": "desktopcomputer",
        "state": "connected" if raw_status == "Online" else "unavailable",
        "available": raw_status == "Online",
        "stale": False,
        "updated_at": _updated_at(status, metadata),
        "sort_order": STATIC_SORT_ORDERS["system"],
        "capabilities": [],
        "metrics": [{
            "key": "status",
            "label": "状态",
            "value": raw_status,
            "value_type": "status",
            "format": "plain",
            "unit": None,
            "primary": True,
        }],
        "actions": [],
    }


def _generic_manifest(status: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
    """Conservative fallback for providers without a dedicated adapter."""
    value = None
    for key in ("points", "balance", "remaining", "credits", "value", "total"):
        candidate = _number(status.get(key))
        if candidate is not None:
            value = candidate
            break
    raw_state = _string(status.get("state")) or _string(status.get("status"))
    state = "unavailable" if value is None and not raw_state else _state_label(raw_state or "connected")
    metrics: list[dict[str, Any]] = []
    if value is not None:
        metrics.append({
            "key": "value",
            "label": _string(status.get("label"), 30) or "Value",
            "value": value,
            "value_type": "number",
            "format": "decimal",
            "unit": _string(status.get("unit"), 12),
            "primary": True,
        })
    if raw_state:
        metrics.append({
            "key": "state",
            "label": "状态",
            "value": raw_state,
            "value_type": "status",
            "format": "plain",
            "unit": None,
            "primary": False,
        })
    return {
        "display_name": _string(status.get("display_name"), 40) or "Provider",
        "category": "generic",
        "icon": _string(status.get("icon"), MAX_ICON_LENGTH),
        "state": state,
        "available": value is not None,
        "stale": False,
        "updated_at": _updated_at(status, metadata),
        "sort_order": DEFAULT_SORT_ORDER,
        "capabilities": ["refresh"],
        "metrics": metrics,
        "actions": [{"id": "refresh", "label": "刷新", "kind": "refresh", "local_only": True}],
    }


MANIFEST_ADAPTERS: dict[str, ManifestAdapter] = {
    "codex": _codex_manifest,
    "workbuddy": _workbuddy_manifest,
    "deepseek": _deepseek_manifest,
    "system": _system_manifest,
}


def _adapter_for(provider: Any) -> ManifestAdapter | None:
    manifest_method = getattr(provider, "manifest", None)
    if callable(manifest_method):
        def plug_in_adapter(status: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
            return manifest_method(metadata)

        return plug_in_adapter
    name = getattr(provider, "name", None)
    return MANIFEST_ADAPTERS.get(name if isinstance(name, str) else "")


def render_manifest(provider_id: str, provider: Any, metadata: dict[str, Any] | None = None) -> dict[str, Any]:
    """Build a safe, schema-valid manifest for one provider."""
    if not isinstance(provider_id, str) or not PROVIDER_ID_PATTERN.fullmatch(provider_id):
        raise ValueError(f"invalid provider id: {provider_id!r}")
    raw_status = provider.status()
    if not isinstance(raw_status, dict):
        raw_status = {}
    adapter = _adapter_for(provider)
    metadata = metadata if isinstance(metadata, dict) else {}
    partial = adapter(raw_status, metadata) if adapter else _generic_manifest(raw_status, metadata)
    if not isinstance(partial, dict):
        partial = {}
    return _finalize(provider_id, partial)


def error_manifest(provider_id: str, error: Exception) -> dict[str, Any]:
    """Minimal manifest used when one provider fails without breaking the list."""
    return _finalize(provider_id, {
        "display_name": provider_id,
        "category": "generic",
        "state": "error",
        "available": False,
        "stale": False,
        "capabilities": [],
        "metrics": [],
        "actions": [],
    })
