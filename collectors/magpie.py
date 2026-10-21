"""Normalize Magpie's quota report into AICC's cross-platform snapshot.

Magpie owns provider sign-in, account management and quota reads. AICC only
reads the documented local gateway routes and never touches agent credentials:

    GET  /v1/magpie/quotas?kept=1      last read values, no vendor asked
    GET  /v1/magpie/quotas            read every vendor now (12s server timeout)
    POST /v1/magpie/quotas/refresh    read again, optionally ?provider=<id>

The normalization below is the single contract shared with the macOS Widget
(macos/MenuBarApp/Sources/Models/QuotaModel.swift) and the e-ink APK
(DashboardData.java).  Keep the three implementations semantically identical.
"""

import hashlib
import json
import math
import os
import tempfile
from datetime import datetime
from pathlib import Path
from urllib.parse import quote, urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

SCHEMA_VERSION = 3
DATA_ROOT = Path(os.environ.get("EINK_DATA_DIR", Path(__file__).resolve().parent.parent / "data"))

# One place for the refresh policy: the bridge poll loop, the Widget timeline and
# the APK refresh interval all read these instead of hard-coding their own.
REFRESH_SECONDS = 300
MAX_AGE_SECONDS = 300

CACHE_PATH = DATA_ROOT / "magpie_last_success.json"

# Display names only.  This is not an allow list: an unknown provider id still
# becomes a card, named by `provider_name()` from its own id.
PROVIDER_NAMES = {
    "codex": "Codex",
    "claude": "Claude",
    "anthropic": "Claude",
    "openai": "OpenAI",
    "gemini": "Gemini",
    "antigravity": "Antigravity",
    "workbuddy": "WorkBuddy",
    "workbuddy-ai": "WorkBuddy AI",
    "zcode": "ZCode",
    "kiro": "Kiro",
    "minimax-code": "MiniMax Code",
    "minimax-code-global": "MiniMax Code",
    "qoder": "Qoder",
    "qoder-cn": "Qoder CN",
    "devin": "Devin",
    "zed": "Zed",
    "factory": "Factory",
    "grok": "Grok",
    "cursor": "Cursor",
    "cline": "Cline",
    "trae-cn": "Trae",
    "trae-global": "Trae",
    "commandcode-plan": "Command Code",
    "mimo-app": "MiMo",
    "kimi-work": "Kimi Work",
    "opencode-zen-free": "OpenCode Zen",
    "opendesign": "Open Design",
    "raccoon": "Raccoon",
    "autoclaw": "AutoClaw",
    "autoclaw-cn": "AutoClaw CN",
}

KINDS = ("subscription", "plan", "balance")


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def provider_name(provider):
    """Human name for any provider id, known or not."""
    if not isinstance(provider, str) or not provider.strip():
        return "Unknown"
    known = PROVIDER_NAMES.get(provider)
    if known:
        return known
    words = provider.replace("_", "-").split("-")
    return " ".join(word[:1].upper() + word[1:] for word in words if word) or "Unknown"


def _number(value):
    """A finite JSON number, or None.  Bools and strings never count."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if not math.isfinite(value):
        return None
    return float(value)


def _percent(value):
    """Remaining share, only when Magpie clearly means a percentage."""
    number = _number(value)
    if number is None or not 0 <= number <= 100:
        return None
    return round(number, 2)


def timestamp(value):
    if not isinstance(value, str) or not value:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except (ValueError, OverflowError, OSError):
        return None
    if parsed.tzinfo is None:
        return None
    try:
        return parsed.timestamp()
    except (OverflowError, OSError, ValueError):
        return None


def clock(value):
    """Local reset text; None when Magpie gave no usable time."""
    if value is None:
        return None
    try:
        return datetime.fromtimestamp(value).astimezone().strftime("%Y-%m-%d %H:%M")
    except (OverflowError, OSError, ValueError):
        return None


def mask_identity(text):
    """Never carry an account's own identity into a snapshot.

    Magpie's `user` is often an email (`rak.wang@example.com · Team`).  The
    snapshot keeps a stable anonymous id instead, plus this deliberately lossy
    label: the first character of the local part and nothing else.
    """
    if not isinstance(text, str):
        return None
    value = text.strip()
    if not value:
        return None
    local = value.split("@", 1)[0].strip() if "@" in value else value
    local = local.strip(" <>()[]")
    if not local:
        return "***"
    return local[0] + "***"


def display_label(row):
    """A name fit to show: Magpie's own alias/seat when set, else masked."""
    for key in ("alias", "seat"):
        value = row.get(key)
        if isinstance(value, str):
            value = value.strip()
            if 0 < len(value) <= 24 and "@" not in value:
                return value
    return mask_identity(row.get("user")) or "***"


def _count(value):
    """A count as a person reads it: whole numbers, one decimal when small."""
    if value >= 100:
        return str(int(round(value)))
    return ("%.1f" % value).rstrip("0").rstrip(".")


def _item(window):
    """One window as the clients render it, or None when unreadable."""
    if not isinstance(window, dict):
        return None
    name = window.get("name")
    if not isinstance(name, str) or not name.strip():
        return None
    name = name.strip()[:40]
    unlimited = window.get("unlimited") is True
    used = _percent(window.get("used"))
    remaining = None if unlimited else _percent(window.get("remaining"))
    if remaining is None and not unlimited and used is not None:
        # Magpie's contract carries both shares of the same window; derive the
        # other one rather than showing an unknown for a known window.
        remaining = _percent(100 - used)
    display = window.get("display")
    if not isinstance(display, str) or not display.strip():
        display = None
    else:
        display = display.strip()[:32]
    unit = window.get("unit")
    unit = unit.strip()[:16] if isinstance(unit, str) and unit.strip() else None
    pool = window.get("pool")
    pool = pool.strip()[:32] if isinstance(pool, str) and pool.strip() else None
    resets_at = timestamp(window.get("resetsAt"))
    amount, limit = _number(window.get("amount")), _number(window.get("limit"))
    # What is left, in the vendor's own unit when it counts in one: a credits
    # window reads "4720", not "2.27 / 4723" (used / total).
    remaining_text = None
    if not unlimited:
        if limit is not None and unit:
            # Magpie omits `amount` when it is 0, so a 0% window still means
            # "nothing used", not "unknown".
            used_count = amount
            if used_count is None and (remaining == 100 or used == 0):
                used_count = 0.0
            if used_count is not None:
                remaining_text = _count(max(0.0, limit - used_count))
        if remaining_text is None:
            if display:
                remaining_text = display
            elif remaining is not None:
                remaining_text = str(int(round(remaining))) + "%"
    return {
        "name": name,
        "used": None if unlimited else used,
        "remaining": remaining,
        "remaining_text": remaining_text,
        "display": display,
        "amount": amount,
        "limit": limit,
        "unit": unit,
        "unlimited": unlimited,
        "resets_at": resets_at,
        "reset": clock(resets_at),
        "pool": pool,
    }


def _parts(value):
    if not isinstance(value, list):
        return []
    parts = []
    for part in value:
        if not isinstance(part, dict):
            continue
        text = part.get("text")
        if not isinstance(text, str) or not text.strip():
            continue
        label = part.get("label")
        parts.append({"label": label.strip()[:24] if isinstance(label, str) and label.strip() else None,
                      "text": text.strip()[:32],
                      "percent": _percent(part.get("percent"))})
    return parts


def _resets(value):
    if not isinstance(value, dict):
        return {"count": None, "until": None, "each": []}
    each = []
    for reset in value.get("each") or []:
        if isinstance(reset, dict):
            at = timestamp(reset.get("until"))
            if at is not None:
                each.append(at)
    count = value.get("count")
    return {"count": count if isinstance(count, int) and not isinstance(count, bool) and count >= 0 else None,
            "until": timestamp(value.get("until")),
            "each": each[:16]}


def normalize(row, now):
    """One quota row to one card, or None when the row can't be read."""
    if not isinstance(row, dict):
        return None
    provider = row.get("provider")
    kind = row.get("kind")
    if not isinstance(provider, str) or not provider.strip():
        return None
    provider = provider.strip()[:40]
    if kind not in KINDS:
        return None
    user = row.get("user") if isinstance(row.get("user"), str) else ""
    origin = row.get("from") if isinstance(row.get("from"), str) else ""
    identity = hashlib.sha256(f"{origin}\0{provider}\0{kind}\0{user.casefold()}".encode()).hexdigest()[:12]

    windows = row.get("windows")
    items = []
    if isinstance(windows, list):
        for window in windows:
            item = _item(window)
            if item is not None:
                items.append(item)
    balance = row.get("balance")
    balance = balance.strip()[:32] if isinstance(balance, str) and balance.strip() else None
    parts = _parts(row.get("balanceSplit"))

    error = row.get("error")
    failed = bool(error) if not isinstance(error, (dict, list)) else bool(error)
    deferred = row.get("readDeferred") is True
    as_of = timestamp(row.get("asOf"))
    read_at = timestamp(row.get("readAt"))
    if read_at is None:
        read_at = as_of
    stale = bool(failed or deferred or as_of is not None or read_at is None)
    if read_at is not None and (now - read_at > MAX_AGE_SECONDS or read_at > now + 60):
        stale = True
    reset_at = [item["resets_at"] for item in items if item["resets_at"] is not None]
    if any(reset <= now for reset in reset_at):
        stale = True

    plan = row.get("plan")
    renew = row.get("renew")
    last_served = timestamp(row.get("lastServedAt"))
    resets = _resets(row.get("resets"))
    return {
        "id": "magpie-" + identity,
        "provider": provider,
        "provider_name": provider_name(provider),
        "account_label": display_label(row),
        "kind": kind,
        "plan": plan.strip()[:32] if isinstance(plan, str) and plan.strip() else None,
        "items": items,
        "balance": balance,
        "balance_parts": parts,
        "until": timestamp(row.get("until")),
        "renew": renew if renew in ("auto", "off") else None,
        "resets_count": resets["count"],
        "resets_until": resets["until"],
        "last_served_at": last_served,
        "last": row.get("last") is True,
        "from": origin[:32] or None,
        "failed": bool(failed),
        "deferred": bool(deferred),
        "stale": stale,
        "read_at": read_at,
    }


def _state(accounts):
    if not accounts:
        return "Empty"
    if all(account["stale"] for account in accounts):
        return "Cached"
    return "Connected"


def parse_report(document, now=None):
    """Normalize a whole Magpie report.

    Only the envelope is validated: a single unreadable row (an unknown kind, a
    window without a name, a truncated record) is dropped instead of taking
    every other provider down with it.
    """
    if not isinstance(document, dict) or document.get("object") != "list" or not isinstance(document.get("data"), list):
        raise ValueError("Invalid Magpie quota report")
    now = datetime.now().timestamp() if now is None else now
    accounts = []
    seen = set()
    for row in document["data"]:
        account = normalize(row, now)
        if account is None:
            continue
        if account["id"] in seen:
            # The same card twice: keep the newer reading, never both.
            existing = next(index for index, item in enumerate(accounts) if item["id"] == account["id"])
            old = accounts[existing]
            if (account["read_at"] or 0) > (old["read_at"] or 0):
                accounts[existing] = account
            continue
        seen.add(account["id"])
        accounts.append(account)
    reads = [account["read_at"] for account in accounts if account["read_at"] is not None]
    updated = max(reads) if reads else None
    stale = bool(accounts) and all(account["stale"] for account in accounts)
    providers = []
    for account in accounts:
        if account["provider"] not in providers:
            providers.append(account["provider"])
    return {
        "schema_version": SCHEMA_VERSION,
        "source": "Magpie",
        "available": bool(accounts),
        "state": _state(accounts),
        "stale": stale,
        "providers": providers,
        "accounts": accounts,
        "updated_at": clock(updated),
        "updated_epoch": updated,
        "refresh_seconds": REFRESH_SECONDS,
    }


def mark_stale(snapshot, state="Cached"):
    stale = {**snapshot, "stale": True, "state": state}
    stale["accounts"] = [{**account, "stale": True} for account in snapshot.get("accounts", [])]
    return stale


def load_cache():
    """Last successful snapshot, or None.  Older schemas are not reinterpreted."""
    try:
        snapshot = json.loads(CACHE_PATH.read_text(encoding="utf-8"))
        if (snapshot.get("source") == "Magpie" and snapshot.get("schema_version") == SCHEMA_VERSION
                and isinstance(snapshot.get("accounts"), list)):
            return mark_stale(snapshot, "Cached")
    except (OSError, ValueError, AttributeError):
        pass
    return None


def save_cache(snapshot):
    """Write the cache atomically and privately; never leave a half file."""
    try:
        CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
        handle, temporary = tempfile.mkstemp(dir=str(CACHE_PATH.parent), prefix=".magpie-", suffix=".json")
        try:
            with os.fdopen(handle, "w", encoding="utf-8") as output:
                json.dump(snapshot, output, ensure_ascii=False)
                output.write("\n")
                output.flush()
                os.fsync(output.fileno())
            os.chmod(temporary, 0o600)
            os.replace(temporary, CACHE_PATH)
        except BaseException:
            try:
                os.unlink(temporary)
            except OSError:
                pass
            raise
    except OSError:
        pass


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
        target = base.rstrip("/") + path
        request = Request(target, method=method, headers={"Accept": "application/json"})
        with opener.open(request, timeout=35 if method == "POST" else 15) as response:
            if response.geturl() != target:
                raise ValueError("Unexpected Magpie redirect")
            raw = response.read(1024 * 1024 + 1)
            if len(raw) > 1024 * 1024:
                raise ValueError("Magpie quota report too large")
            return json.loads(raw)
    except Exception:
        raise ValueError("Magpie temporarily unavailable") from None


def refresh(provider=None):
    """Ask Magpie to read again, every card or one provider's, and return its answer."""
    path = "/v1/magpie/quotas/refresh"
    if provider:
        path += "?provider=" + quote(provider, safe="")
    return _request(path, "POST")


def collect(force=False, provider=None):
    """Read the normalized snapshot, refreshing first when asked.

    A forced read uses the refresh answer itself, so one request serves both
    the refresh and the read.  Anything the refresh doesn't answer falls back
    to the kept read; Magpie's own 30 second per-card gap still applies.
    """
    document = None
    if force:
        try:
            document = refresh(provider)
        except ValueError:
            document = None
    if document is None:
        document = _request("/v1/magpie/quotas?kept=1")
    snapshot = parse_report(document)
    save_cache(snapshot)
    return snapshot
