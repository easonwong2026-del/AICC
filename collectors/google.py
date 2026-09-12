"""Google Gemini windows from OpenCodex, shared by the app and widget."""

import json
import math
import os
import shutil
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path


def epoch(value):
    try:
        number = float(value)
        if number > 10_000_000_000:
            number /= 1000
        return number if math.isfinite(number) and 0 < number <= 253402300799 else None
    except (TypeError, ValueError, OverflowError):
        try:
            return datetime.fromisoformat(str(value).replace("Z", "+00:00")).timestamp()
        except (TypeError, ValueError, OverflowError, OSError):
            return None


def parse_quota(document):
    if not isinstance(document, dict) or not isinstance(document.get("reports"), list):
        raise ValueError("No data")
    report = next((r for r in document["reports"] if isinstance(r, dict)
                   and r.get("provider") == "google-antigravity"), {})
    quota = report.get("quota")
    if not isinstance(quota, dict):
        raise ValueError("No data")
    result = {"source": report.get("source"), "updated_epoch": epoch(quota.get("updatedAt")), "stale": False}
    windows = quota.get("customWindows")
    for window in windows if isinstance(windows, list) else []:
        if not isinstance(window, dict):
            continue
        label = window.get("label")
        key = {"Gem (Weekly)": "weekly", "Gem": "five_hour"}.get(label) if isinstance(label, str) else None
        if not key or key in result:
            continue
        try:
            used = float(window["percent"])
            if isinstance(window["percent"], bool) or not math.isfinite(used):
                continue
        except (KeyError, TypeError, ValueError, OverflowError):
            continue
        reset = epoch(window.get("resetAt"))
        result[key] = {"remaining": min(100, max(0, 100 - used)),
                       "reset": datetime.fromtimestamp(reset).astimezone().strftime("%Y-%m-%d %H:%M") if reset else None,
                       "duration_minutes": 10080 if key == "weekly" else 300}
    if "weekly" not in result and "five_hour" not in result:
        raise ValueError("No data")
    # Missing source time is not evidence of a fresh upstream reading.
    result["stale"] = result["updated_epoch"] is None or time.time() - result["updated_epoch"] >= 300
    return result


def resolve_executable():
    saved = ""
    if sys.platform == "darwin":
        try:
            saved = subprocess.run(["/usr/bin/defaults", "read", "com.aieink.dashboard.menubar", "ocxCustomPath"],
                                   capture_output=True, text=True, timeout=1).stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            pass
    candidates = [os.environ.get("AICC_OCX_PATH", ""), saved,
                  str(Path.home() / ".npm-global/bin/ocx"), str(Path.home() / ".local/bin/ocx"),
                  "/opt/homebrew/bin/ocx", "/usr/local/bin/ocx", shutil.which("ocx") or ""]
    return next((p for p in candidates if p and os.path.isfile(p) and os.access(p, os.X_OK)), None)


def collect(force=False):
    executable = resolve_executable()
    if not executable:
        raise ValueError("OpenCodex is not installed")
    env = os.environ.copy()
    env["PATH"] = os.pathsep.join([str(Path(executable).parent), "/opt/homebrew/bin", "/usr/local/bin",
                                  str(Path.home() / ".npm-global/bin"), env.get("PATH", os.defpath)])
    try:
        response = subprocess.run([executable, "provider", "quota", *(["--refresh"] if force else []), "--json"],
                                  capture_output=True, text=True, timeout=8, env=env)
    except subprocess.TimeoutExpired:
        raise ValueError("Temporarily unavailable") from None
    if response.returncode:
        message = "OpenCodex is not running" if "Proxy is not running" in response.stderr else "Temporarily unavailable"
        raise ValueError(message)
    try:
        return parse_quota(json.loads(response.stdout))
    except json.JSONDecodeError:
        raise ValueError("No data") from None
