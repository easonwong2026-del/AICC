"""Read Magpie's documented quota report without accessing agent credentials."""

import hashlib
import json
import os
import shutil
from datetime import datetime
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from collectors.codex import DATA_ROOT, _percent, mark_stale

CACHE_PATH = DATA_ROOT / "magpie_last_success.json"
MAX_AGE_SECONDS = 300


def installed():
    return bool(os.environ.get("AICC_MAGPIE_URL") or shutil.which("magpie") or Path("/Applications/magpie.app").is_dir())


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def timestamp(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        return parsed.timestamp() if parsed.tzinfo else None
    except (ValueError, OverflowError, OSError):
        return None


def parse_report(document, now=None):
    if not isinstance(document, dict) or document.get("object") != "list" or not isinstance(document.get("data"), list):
        raise ValueError("Invalid Magpie quota report")
    now = datetime.now().timestamp() if now is None else now
    accounts = []
    seen = set()
    for row in document["data"]:
        if not isinstance(row, dict) or row.get("provider") != "codex" or row.get("kind") != "subscription":
            continue
        user = row.get("user")
        origin = row.get("from", "")
        if not isinstance(user, str) or not user or not isinstance(origin, str):
            raise ValueError("Invalid Magpie account identity")
        identity = hashlib.sha256((origin + "\0" + user.casefold()).encode()).hexdigest()[:12]
        if identity in seen:
            raise ValueError("Duplicate Magpie account")
        seen.add(identity)
        read_at = timestamp(row.get("readAt")) or timestamp(row.get("asOf"))
        stale = bool(row.get("error") or row.get("asOf") or read_at is None
                     or now - read_at > MAX_AGE_SECONDS or read_at > now + 60)
        windows = row.get("windows")
        if not isinstance(windows, list):
            raise ValueError("Invalid Magpie quota windows")
        account = {"id": "magpie-" + identity, "label": "Codex · " + identity[:6],
                   "plan": row.get("plan") if row.get("plan") in ("free", "plus", "pro", "team", "business", "enterprise", "edu") else None,
                   "active": False, "needs_reauth": False, "stale": stale, "read_at": read_at}
        for name, key, minutes in (("5 hours", "five_hour", 300), ("7 days", "weekly", 10080)):
            matches = [window for window in windows if isinstance(window, dict) and window.get("name") == name]
            if len(matches) > 1:
                raise ValueError("Duplicate Magpie quota window")
            window = matches[0] if matches else {}
            reset = timestamp(window.get("resetsAt"))
            remaining = None if window.get("unlimited") else _percent(window.get("used"))
            account[key] = {"remaining": remaining, "duration_minutes": minutes,
                            "reset": datetime.fromtimestamp(reset).astimezone().strftime("%Y-%m-%d %H:%M") if reset else None}
            if reset is not None and reset <= now:
                account["stale"] = True
        accounts.append(account)
    stale = bool(accounts) and all(account["stale"] for account in accounts)
    reads = [account["read_at"] for account in accounts if account["read_at"] is not None]
    return {"schema_version": 2, "source": "Magpie", "available": bool(accounts),
            "state": "Cached" if stale else ("Connected" if accounts else "No Codex accounts"),
            "stale": stale, "selection_mode": "auto", "active_account_id": None, "accounts": accounts,
            "updated_at": datetime.fromtimestamp(min(reads)).astimezone().strftime("%Y-%m-%d %H:%M:%S") if reads else None,
            "weekly": accounts[0]["weekly"] if len(accounts) == 1 else None,
            "five_hour": accounts[0]["five_hour"] if len(accounts) == 1 else None,
            "refresh_seconds": 120}


def load_cache():
    try:
        snapshot = json.loads(CACHE_PATH.read_text(encoding="utf-8"))
        if snapshot.get("source") == "Magpie" and snapshot.get("schema_version") == 2 and isinstance(snapshot.get("accounts"), list):
            return mark_stale(snapshot, "Cached")
    except (OSError, ValueError, AttributeError):
        pass
    return None


def _request(path, method="GET"):
    base = os.environ.get("AICC_MAGPIE_URL", "http://127.0.0.1:3425")
    try:
        parsed = urlsplit(base)
        valid = (parsed.scheme == "http" and parsed.hostname in ("127.0.0.1", "::1")
                 and not parsed.username and not parsed.password and parsed.path in ("", "/")
                 and not parsed.query and not parsed.fragment and parsed.port is not None)
    except ValueError:
        valid = False
    if not valid:
        raise ValueError("Magpie URL must be a local HTTP address with a port")
    try:
        opener = build_opener(ProxyHandler({}), NoRedirect())
        with opener.open(Request(base.rstrip("/") + path, method=method), timeout=35 if method == "POST" else 15) as response:
            if response.geturl() != base.rstrip("/") + path:
                raise ValueError("Unexpected Magpie redirect")
            raw = response.read(1024 * 1024 + 1)
            if len(raw) > 1024 * 1024:
                raise ValueError("Magpie quota report too large")
            return json.loads(raw)
    except Exception:
        raise ValueError("Magpie temporarily unavailable") from None


def collect(force=False):
    _request("/v1/magpie/quotas/refresh?provider=codex", "POST")
    snapshot = parse_report(_request("/v1/magpie/quotas?kept=1"))
    try:
        CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
        temporary = CACHE_PATH.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(snapshot, ensure_ascii=False) + "\n", encoding="utf-8")
        os.replace(temporary, CACHE_PATH)
    except OSError:
        pass
    return snapshot
