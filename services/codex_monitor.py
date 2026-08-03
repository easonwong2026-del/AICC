"""Codex app-server JSON-RPC monitor for account rate limits."""

from __future__ import annotations
from typing import Any


import json
import os
import platform
from pathlib import Path
import shutil
import subprocess
import threading
import time
from datetime import datetime

DATA_ROOT = Path(os.environ.get("EINK_DATA_DIR", Path(__file__).resolve().parents[1] / "data")).expanduser()


class CodexMonitor:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._io_lock = threading.Lock()
        self._process: subprocess.Popen[str] | None = None
        self._started = False
        self._restarting = False
        self._restart_attempts = 0
        self._request_id = 0
        self._initialize_request_id: int | None = None
        self._rate_limit_request_id: int | None = None
        self._last_request = 0.0
        self._last_access = 0.0
        self._last_success_epoch = 0.0
        self._refresh_seconds = max(30, min(300, int(os.environ.get("CODEX_REFRESH_SECONDS", "60"))))
        self._idle_seconds = max(30, min(600, int(os.environ.get("CODEX_IDLE_SECONDS", "30"))))
        self._fresh_event = threading.Event()
        self._status: dict[str, Any] = {"available": False, "state": "Not started", "source": "Codex app-server"}
        self._cache_path = DATA_ROOT / "codex_last_success.json"
        self._load_cache()

    def status(self) -> dict[str, Any]:
        self.start()
        with self._lock:
            wait_for_first_result = self._status.get("state") == "Connecting"
        if wait_for_first_result:
            self._fresh_event.wait(timeout=3)
        with self._lock:
            result = self._status.copy()
            if self._last_success_epoch:
                result["age_seconds"] = max(0, round(time.time() - self._last_success_epoch))
                result["stale"] = result.get("state") != "Connected" or result["age_seconds"] > max(180, self._refresh_seconds * 3)
            else:
                result["stale"] = result.get("state") != "Connected"
            return result

    def start(self) -> None:
        with self._lock:
            self._last_access = time.monotonic()
            if self._started:
                return
            self._started = True
            self._fresh_event.clear()
            self._launch_locked()

    def _launch_locked(self) -> None:
        self._fresh_event.clear()
        executable, source = self._resolve_cli()
        if not executable:
            self._status.update(
                state="ChatGPT desktop Codex CLI not found",
                detail="Open ChatGPT once, or set CODEX_CLI_PATH.",
            )
            self._fresh_event.set()
            self._schedule_restart()
            return
        try:
            self._process = subprocess.Popen(
                [executable, "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                text=True, encoding="utf-8", bufsize=1, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
        except OSError as error:
            self._status.update(state=f"Unable to start Codex: {error.strerror or error}")
            self._fresh_event.set()
            self._schedule_restart()
            return
        self._status.update(state="Connecting", source=source)
        self._rate_limit_request_id = None
        self._last_request = 0.0
        process = self._process
        # app-server can answer immediately. Send initialize before starting
        # the reader so its response cannot race with request-id assignment.
        # The old order could leave the monitor stuck in Connecting forever.
        self._initialize_request_id = self._send(
            "initialize",
            {"clientInfo": {"name": "ai-eink-dashboard", "version": "2.1"}, "capabilities": {}},
        )
        threading.Thread(target=self._read_stdout, args=(process,), name="codex-rate-limits", daemon=True).start()
        threading.Thread(target=self._refresh_loop, args=(process,), name="codex-rate-limits-refresh", daemon=True).start()

    @classmethod
    def _resolve_cli(cls) -> tuple[str | None, str]:
        configured = os.environ.get("CODEX_CLI_PATH")
        if configured and os.path.isfile(configured):
            return configured, "Configured Codex CLI"

        # On macOS the supported standalone command is normally already on
        # PATH. Prefer it so updates and authentication follow the local Codex
        # installation instead of depending on a private app bundle layout.
        if platform.system() != "Windows":
            command = shutil.which("codex")
            if command:
                return command, "Standalone Codex CLI"

        desktop_cli = cls._find_desktop_cli()
        if desktop_cli:
            extracted = cls._sync_desktop_cli(desktop_cli)
            if extracted:
                return extracted, "ChatGPT desktop"

        if platform.system() == "Windows":
            cached_desktop_cli = Path(__file__).resolve().parents[1] / ".runtime" / "codex-desktop.exe"
            if cached_desktop_cli.is_file():
                return str(cached_desktop_cli), "ChatGPT desktop (cached)"

        if platform.system() == "Windows":
            pnpm_cli = os.path.join(os.environ.get("LOCALAPPDATA", ""), "pnpm", "bin", "codex.cmd")
            if os.path.exists(pnpm_cli):
                return pnpm_cli, "Standalone Codex CLI"
        command = shutil.which("codex")
        return (command, "Standalone Codex CLI") if command else (None, "Codex app-server")

    @staticmethod
    def _find_desktop_cli() -> str | None:
        """Locate the CLI bundled with the currently installed ChatGPT desktop app."""
        configured = os.environ.get("CHATGPT_CODEX_PATH")
        if configured and os.path.isfile(configured):
            return configured

        if platform.system() == "Darwin":
            applications = [Path("/Applications"), Path.home() / "Applications"]
            relative_candidates = [
                Path("ChatGPT.app/Contents/Resources/codex"),
                Path("ChatGPT.app/Contents/Resources/codex-cli"),
                Path("Codex.app/Contents/Resources/codex"),
            ]
            for root in applications:
                for relative in relative_candidates:
                    candidate = root / relative
                    if candidate.is_file() and os.access(candidate, os.X_OK):
                        return str(candidate)
            return None

        if platform.system() != "Windows":
            return None

        powershell = shutil.which("powershell") or shutil.which("powershell.exe")
        if not powershell:
            return None
        script = (
            "$p=Get-Process ChatGPT -ErrorAction SilentlyContinue | "
            "Where-Object {$_.Path} | Select-Object -First 1 -ExpandProperty Path; "
            "if($p){Join-Path (Split-Path $p) 'resources\\codex.exe'}"
        )
        try:
            result = subprocess.run(
                [powershell, "-NoProfile", "-NonInteractive", "-Command", script],
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=10,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
        except (OSError, subprocess.SubprocessError):
            return None
        candidate = result.stdout.strip().splitlines()[-1] if result.stdout.strip() else ""
        return candidate if candidate and os.path.isfile(candidate) else None

    @staticmethod
    def _sync_desktop_cli(source: str) -> str | None:
        """Copy the packaged CLI outside WindowsApps so normal programs may execute it."""
        if platform.system() != "Windows":
            return source
        runtime_dir = Path(__file__).resolve().parents[1] / ".runtime"
        destination = runtime_dir / "codex-desktop.exe"
        temporary = runtime_dir / "codex-desktop.exe.new"
        try:
            runtime_dir.mkdir(parents=True, exist_ok=True)
            source_stat = os.stat(source)
            if destination.exists():
                destination_stat = destination.stat()
                if (
                    destination_stat.st_size == source_stat.st_size
                    and destination_stat.st_mtime_ns == source_stat.st_mtime_ns
                ):
                    return str(destination)
            shutil.copy2(source, temporary)
            os.replace(temporary, destination)
            return str(destination)
        except OSError:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                pass
            return str(destination) if destination.is_file() else None

    def _refresh_loop(self, process: subprocess.Popen[str]) -> None:
        while process is self._process:
            time.sleep(5)
            if time.monotonic() - self._last_access >= self._idle_seconds:
                with self._lock:
                    if process is not self._process:
                        return
                    self._process = None
                    self._started = False
                    if self._status.get("available"):
                        self._status.update(state="Cached", stale=True)
                    else:
                        self._status.update(state="Idle", stale=True)
                self._stop_process(process)
                return
            if process.poll() is not None:
                with self._lock:
                    if process is self._process:
                        self._process = None
                        self._status.update(state="Reconnecting", stale=True)
                        self._schedule_restart()
                return
            if time.monotonic() - self._last_request >= self._refresh_seconds:
                self._request_limits()

    @staticmethod
    def _stop_process(process: subprocess.Popen[str]) -> None:
        try:
            if process.stdin:
                process.stdin.close()
            process.terminate()
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
        except OSError:
            pass
        for stream in (process.stdout, process.stderr):
            try:
                if stream:
                    stream.close()
            except OSError:
                pass

    def _schedule_restart(self) -> None:
        if self._restarting:
            return
        self._restarting = True
        threading.Thread(target=self._restart_after_delay, name="codex-restart", daemon=True).start()

    def _restart_after_delay(self) -> None:
        self._restart_attempts += 1
        time.sleep(min(60, 5 * self._restart_attempts))
        with self._lock:
            self._restarting = False
            if time.monotonic() - self._last_access >= self._idle_seconds:
                self._started = False
                return
            if self._process and self._process.poll() is None:
                return
            self._launch_locked()

    def _request_limits(self) -> None:
        self._last_request = time.monotonic()
        request_id = self._send("account/rateLimits/read", None)
        if request_id is not None:
            self._rate_limit_request_id = request_id

    def _send(self, method: str, params: Any = None, notification: bool = False) -> int | None:
        failed = False
        request_id = None
        with self._io_lock:
            process = self._process
            if not process or not process.stdin:
                return None
            try:
                payload: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
                if not notification or params is not None:
                    payload["params"] = params
                if not notification:
                    self._request_id += 1
                    payload["id"] = self._request_id
                process.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
                process.stdin.flush()
                request_id = payload.get("id")
            except OSError:
                failed = True
        if failed:
            with self._lock:
                self._status.update(state="Codex app-server connection lost")
            return None
        return request_id

    def _read_stdout(self, process: subprocess.Popen[str]) -> None:
        assert process.stdout
        for line in process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if "error" in message:
                error = message["error"]
                detail = error.get("message", "Unknown app-server error") if isinstance(error, dict) else "Unknown app-server error"
                if "authentication required" in str(detail).lower():
                    detail = "ChatGPT login required"
                with self._lock:
                    has_cache = bool(
                        self._status.get("five_hour") or self._status.get("weekly")
                        or self._status.get("limit_buckets")
                    )
                    self._status.update(state=str(detail), available=has_cache, stale=True, detail="Open ChatGPT and sign in; the dashboard will reconnect automatically.")  # noqa: E501
                self._fresh_event.set()
                continue
            if message.get("method") == "account/rateLimits/updated":
                self._apply_limits(message.get("params", {}))
            elif "result" in message:
                if message.get("id") == self._initialize_request_id:
                    self._send("initialized", notification=True)
                    self._request_limits()
                elif message.get("id") == self._rate_limit_request_id:
                    self._rate_limit_request_id = None
                    self._apply_limits(message["result"])

    def _apply_limits(self, payload: Any) -> None:
        windows = self._find_windows(payload)
        five_hour = self._normalise_window(windows.get("five_hour"))
        weekly = self._normalise_window(windows.get("weekly"))
        limit_buckets = self._extract_limit_buckets(payload)
        reset_credits_present = isinstance(payload, dict) and (
            "rateLimitResetCredits" in payload or "rate_limit_reset_credits" in payload
        )
        reset_credits_value = None
        if reset_credits_present:
            reset_credits_value = payload.get("rateLimitResetCredits", payload.get("rate_limit_reset_credits"))
        reset_credits = self._normalise_reset_credits(reset_credits_value) if reset_credits_present else None
        if not five_hour and not weekly and not limit_buckets and not reset_credits_present:
            return
        with self._lock:
            source = self._status.get("source", "ChatGPT desktop")
            if not five_hour:
                five_hour = self._status.get("five_hour")
            if not weekly:
                weekly = self._status.get("weekly")
            if not limit_buckets:
                limit_buckets = self._status.get("limit_buckets", [])
            if not reset_credits_present:
                reset_credits = self._status.get("reset_credits", self._normalise_reset_credits(None))
            self._status = {
                "available": True,
                "state": "Connected",
                "source": source,
                "five_hour": five_hour,
                "weekly": weekly,
                "limit_buckets": limit_buckets,
                "reset_credits": reset_credits,
                "refresh_seconds": self._refresh_seconds,
                "updated_at": datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S"),
            }
            self._last_success_epoch = time.time()
            self._restart_attempts = 0
            snapshot = self._status.copy()
            snapshot["updated_epoch"] = self._last_success_epoch
        self._save_cache(snapshot)
        fresh_event = getattr(self, "_fresh_event", None)
        if fresh_event:
            fresh_event.set()

    def _load_cache(self) -> None:
        try:
            snapshot = json.loads(self._cache_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return
        reset_credits = snapshot.get("reset_credits") if isinstance(snapshot, dict) else None
        if not isinstance(snapshot, dict) or not (
            snapshot.get("five_hour") or snapshot.get("weekly") or snapshot.get("limit_buckets")
            or (isinstance(reset_credits, dict) and reset_credits.get("provided"))
        ):
            return
        self._last_success_epoch = float(snapshot.pop("updated_epoch", 0) or 0)
        snapshot.setdefault("limit_buckets", [])
        snapshot.setdefault("reset_credits", self._normalise_reset_credits(None))
        snapshot.update(available=True, state="Cached", stale=True)
        self._status = snapshot

    def _save_cache(self, snapshot: dict[str, Any]) -> None:
        temporary = self._cache_path.with_suffix(".json.tmp")
        try:
            self._cache_path.parent.mkdir(parents=True, exist_ok=True)
            try:
                existing = json.loads(self._cache_path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                existing = {}
            ignored = {"updated_at", "updated_epoch", "age_seconds", "stale", "state"}
            stable_existing = {key: value for key, value in existing.items() if key not in ignored}
            stable_snapshot = {key: value for key, value in snapshot.items() if key not in ignored}
            recent = time.time() - float(existing.get("updated_epoch", 0) or 0) < 900
            if stable_existing == stable_snapshot and recent:
                return
            temporary.write_text(json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
            os.replace(temporary, self._cache_path)
        except OSError:
            try:
                temporary.unlink(missing_ok=True)
            except OSError:
                pass

    def _find_windows(self, value: Any) -> dict[str, dict[str, Any]]:
        found: dict[str, dict[str, Any]] = {}
        aliases = {"five_hour": "five_hour", "fiveHour": "five_hour", "weekly": "weekly", "week": "weekly"}
        if isinstance(value, dict):
            if "usedPercent" in value or "used_percent" in value:
                duration = value.get("windowDurationMins", value.get("window_duration_mins"))
                try:
                    duration = int(duration)
                except (TypeError, ValueError):
                    duration = None
                if duration == 300:
                    found["five_hour"] = value
                elif duration == 10080:
                    found["weekly"] = value
            for key, child in value.items():
                if key in aliases and isinstance(child, dict):
                    found[aliases[key]] = child
                elif key == "primary" and isinstance(child, dict):
                    duration = child.get("windowDurationMins", child.get("window_duration_mins"))
                    if duration in (None, 300, "300"):
                        found.setdefault("five_hour", child)
                elif key == "secondary" and isinstance(child, dict):
                    found.setdefault("weekly", child)
                found.update({name: item for name, item in self._find_windows(child).items() if name not in found})
        elif isinstance(value, list):
            for child in value:
                found.update({name: item for name, item in self._find_windows(child).items() if name not in found})
        return found

    def _extract_limit_buckets(self, payload: Any) -> list[dict[str, Any]]:
        if not isinstance(payload, dict):
            return []
        candidates: list[tuple[str, dict[str, Any]]] = []
        by_id = payload.get("rateLimitsByLimitId", payload.get("rate_limits_by_limit_id"))
        if isinstance(by_id, dict):
            candidates.extend(
                (str(limit_id), snapshot) for limit_id, snapshot in by_id.items() if isinstance(snapshot, dict)
            )
        historical = payload.get("rateLimits", payload.get("rate_limits"))
        if not candidates and isinstance(historical, dict):
            candidates.append((str(historical.get("limitId") or historical.get("limit_id") or "codex"), historical))
        if not candidates and (isinstance(payload.get("primary"), dict) or isinstance(payload.get("secondary"), dict)):
            candidates.append((str(payload.get("limitId") or payload.get("limit_id") or "codex"), payload))

        buckets: list[dict[str, Any]] = []
        for fallback_id, snapshot in candidates:
            windows: list[dict[str, Any]] = []
            seen: set[tuple[Any, Any]] = set()
            for raw in (snapshot.get("primary"), snapshot.get("secondary")):
                normalised = self._normalise_window(raw if isinstance(raw, dict) else None)
                if not normalised:
                    continue
                identity = (normalised.get("duration_minutes"), normalised.get("reset"))
                if identity in seen:
                    continue
                seen.add(identity)
                windows.append(normalised)
            buckets.append({
                "id": str(snapshot.get("limitId") or snapshot.get("limit_id") or fallback_id),
                "name": str(snapshot.get("limitName") or snapshot.get("limit_name") or fallback_id),
                "plan_type": snapshot.get("planType", snapshot.get("plan_type")),
                "windows": windows,
            })
        return buckets

    @staticmethod
    def _normalise_reset_credits(value: Any) -> dict[str, Any]:
        if not isinstance(value, dict):
            return {
                "provided": False,
                "available_count": None,
                "detail_count": None,
                "next_expiry": None,
            }
        count = value.get("availableCount", value.get("available_count"))
        try:
            count = max(0, int(count))
        except (TypeError, ValueError):
            return {
                "provided": False,
                "available_count": None,
                "detail_count": None,
                "next_expiry": None,
            }
        details = value.get("credits")
        detail_count = len(details) if isinstance(details, list) else None
        expirations: list[int] = []
        if isinstance(details, list):
            for item in details:
                if not isinstance(item, dict) or item.get("status") not in (None, "available"):
                    continue
                expires = item.get("expiresAt", item.get("expires_at"))
                try:
                    if expires is not None:
                        expirations.append(int(expires))
                except (TypeError, ValueError):
                    continue
        next_expiry = None
        if expirations:
            next_expiry = datetime.fromtimestamp(min(expirations)).astimezone().strftime("%Y-%m-%d %H:%M")
        return {
            "provided": True,
            "available_count": count,
            "detail_count": detail_count,
            "next_expiry": next_expiry,
            "details_limited": detail_count is not None and detail_count < count,
        }

    @staticmethod
    def _normalise_window(value: dict[str, Any] | None) -> dict[str, Any] | None:
        if not value:
            return None
        remaining = value.get("remaining_percent", value.get("remainingPercentage"))
        if remaining is None and value.get("used_percent", value.get("usedPercent")) is not None:
            remaining = 100 - float(value.get("used_percent", value.get("usedPercent")))
        if remaining is None and value.get("usedPercentage") is not None:
            remaining = 100 - float(value["usedPercentage"])
        try:
            remaining = max(0, min(100, round(float(remaining))))
        except (TypeError, ValueError):
            return None
        reset = value.get("reset_time", value.get("reset_at", value.get("resetAt", value.get("resetsAt", value.get("reset", "--")))))
        if isinstance(reset, (int, float)) or (isinstance(reset, str) and reset.isdigit()):
            reset = datetime.fromtimestamp(int(reset)).astimezone().strftime("%Y-%m-%d %H:%M")
        result: dict[str, Any] = {"remaining": remaining, "reset": str(reset)}
        duration = value.get("windowDurationMins", value.get("window_duration_mins"))
        try:
            duration = int(duration)
        except (TypeError, ValueError):
            duration = None
        if duration is not None:
            result["duration_minutes"] = duration
            if duration == 300:
                result["label"] = "5小时"
            elif duration == 10080:
                result["label"] = "1周"
            elif duration % 1440 == 0:
                result["label"] = f"{duration // 1440}天"
            elif duration % 60 == 0:
                result["label"] = f"{duration // 60}小时"
            else:
                result["label"] = f"{duration}分钟"
        return result


monitor = CodexMonitor()
