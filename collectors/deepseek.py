"""Fetch DeepSeek API balance when a key is supplied through the environment."""

from __future__ import annotations

import json
import os
import platform
import subprocess
from datetime import datetime
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

try:
    import winreg
except ImportError:  # pragma: no cover - only available on Windows
    winreg = None

BALANCE_URL = "https://api.deepseek.com/user/balance"


def load_api_key() -> str:
    """Prefer the process variable, then the current user's persistent variable."""
    key = os.environ.get("DEEPSEEK_API_KEY", "").strip()
    if key:
        return key
    if winreg is not None:
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, "Environment") as environment:
                return str(winreg.QueryValueEx(environment, "DEEPSEEK_API_KEY")[0]).strip()
        except OSError:
            pass
    if platform.system() == "Darwin":
        try:
            return subprocess.check_output(
                ["security", "find-generic-password", "-a", os.environ.get("USER", ""),
                 "-s", "ai-eink-dashboard.deepseek", "-w"],
                text=True, timeout=5, stderr=subprocess.DEVNULL,
            ).strip()
        except (OSError, subprocess.SubprocessError):
            pass
    return ""


def update_usage(history_path: Path, balances: list[dict]) -> list[dict]:
    """Estimate today's spend from observed balance changes, never treating a top-up as usage."""
    now = datetime.now().astimezone()
    try:
        history = json.loads(history_path.read_text(encoding="utf-8"))
        snapshots = history.get("snapshots", []) if isinstance(history, dict) else []
    except (OSError, ValueError):
        snapshots = []
    today = now.date().isoformat()
    result = []
    changed = False
    for balance in balances:
        currency = balance["currency"]
        try:
            current = float(balance["total_balance"])
        except (TypeError, ValueError):
            continue
        earlier = [item for item in snapshots if item.get("date") == today and item.get("currency") == currency]
        baseline = float(earlier[0]["total"]) if earlier else current
        result.append({"currency": currency, "used_today": f"{max(0.0, baseline - current):.4f}"})
        previous = next((item for item in reversed(snapshots) if item.get("currency") == currency), None)
        try:
            previous_total = float(previous.get("total")) if previous else None
        except (TypeError, ValueError):
            previous_total = None
        if not previous or previous.get("date") != today or previous_total != current:
            snapshots.append({"at": now.isoformat(timespec="minutes"), "date": today, "currency": currency, "total": current})
            changed = True
    if changed or not history_path.exists():
        history_path.parent.mkdir(exist_ok=True)
        temporary = history_path.with_suffix(".json.tmp")
        temporary.write_text(json.dumps({"snapshots": snapshots[-720:]}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, history_path)
    return result


def collect(history_path: Path | None = None) -> dict:
    api_key = load_api_key()
    if not api_key:
        return {"status": "Not configured", "balances": [], "source": "Environment variable"}
    request = Request(BALANCE_URL, headers={"Authorization": f"Bearer {api_key}", "Accept": "application/json"})
    try:
        with urlopen(request, timeout=8) as response:
            payload = json.loads(response.read().decode("utf-8"))
        balances = payload.get("balance_infos", []) if isinstance(payload, dict) else []
        safe_balances = [{"currency": item.get("currency", ""), "total_balance": item.get("total_balance", ""), "granted_balance": item.get("granted_balance", ""), "topped_up_balance": item.get("topped_up_balance", "")} for item in balances if isinstance(item, dict)]  # noqa: E501
        usage = update_usage(history_path, safe_balances) if history_path else []
        return {"status": "Online" if payload.get("is_available") else "No balance", "balances": safe_balances, "usage": usage, "source": "Observed balance"}  # noqa: E501
    except HTTPError as error:
        return {"status": f"API error {error.code}", "balances": [], "source": "DeepSeek API"}
    except (URLError, TimeoutError, ValueError):
        return {"status": "Connection error", "balances": [], "source": "DeepSeek API"}
