"""Small dependency-free Chrome DevTools Protocol client for localhost."""

from __future__ import annotations
from typing import Any


import base64
import hashlib
import json
import os
import socket
import struct
from urllib.parse import urlparse
from urllib.request import ProxyHandler, build_opener


class CdpError(RuntimeError):
    pass


def target_identity_localhost(port: int, timeout: float = 1.0) -> str:
    """Return a stable page identity without executing code in WorkBuddy."""
    target = _target_localhost(port, timeout)
    return str(target.get("id") or target["webSocketDebuggerUrl"])


def evaluate_localhost(port: int, expression: str, timeout: float = 4.0) -> Any:
    target = _target_localhost(port, timeout)

    client = _WebSocket(target["webSocketDebuggerUrl"], timeout)
    try:
        request_id = 1
        client.send_json({
            "id": request_id,
            "method": "Runtime.evaluate",
            "params": {
                "expression": expression,
                "awaitPromise": True,
                "returnByValue": True,
                "userGesture": False,
            },
        })
        while True:
            message = client.receive_json()
            if message.get("id") != request_id:
                continue
            if message.get("error"):
                raise CdpError(str(message["error"].get("message", "WorkBuddy bridge error")))
            evaluation = message.get("result", {})
            if evaluation.get("exceptionDetails"):
                detail = evaluation["exceptionDetails"].get("text", "WorkBuddy evaluation failed")
                raise CdpError(str(detail))
            remote = evaluation.get("result", {})
            return remote.get("value")
    finally:
        client.close()


def _target_localhost(port: int, timeout: float) -> dict:
    opener = build_opener(ProxyHandler({}))
    try:
        with opener.open(f"http://127.0.0.1:{port}/json/list", timeout=timeout) as response:
            targets = json.load(response)
    except (OSError, ValueError) as error:
        raise CdpError("WorkBuddy monitoring bridge is not running") from error

    pages = [target for target in targets if target.get("type") == "page" and target.get("webSocketDebuggerUrl")]
    if not pages:
        raise CdpError("WorkBuddy page is not available")
    return next(
        (page for page in pages if "workbuddy" in (page.get("title", "") + page.get("url", "")).lower()),
        pages[0],
    )


class _WebSocket:
    def __init__(self, url: str, timeout: float) -> None:
        parsed = urlparse(url)
        if parsed.scheme != "ws" or parsed.hostname not in ("127.0.0.1", "localhost"):
            raise CdpError("Only a localhost WorkBuddy bridge is allowed")
        self.socket = socket.create_connection((parsed.hostname, parsed.port or 80), timeout=timeout)
        self.socket.settimeout(timeout)
        key = base64.b64encode(os.urandom(16)).decode("ascii")
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        request = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {parsed.hostname}:{parsed.port or 80}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.socket.sendall(request.encode("ascii"))
        response = self._read_headers()
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        if " 101 " not in response.split("\r\n", 1)[0] or expected.lower() not in response.lower():
            self.close()
            raise CdpError("WorkBuddy bridge rejected the connection")

    def _read_headers(self) -> str:
        data = bytearray()
        while b"\r\n\r\n" not in data and len(data) < 65536:
            chunk = self.socket.recv(4096)
            if not chunk:
                break
            data.extend(chunk)
        return data.decode("latin-1", errors="replace")

    def send_json(self, value: dict[str, Any]) -> None:
        self._send_frame(0x1, json.dumps(value, separators=(",", ":")).encode("utf-8"))

    def receive_json(self) -> dict[str, Any]:
        chunks: list[bytes] = []
        while True:
            first, second = self._read_exact(2)
            final = bool(first & 0x80)
            opcode = first & 0x0F
            length = second & 0x7F
            if length == 126:
                length = struct.unpack("!H", self._read_exact(2))[0]
            elif length == 127:
                length = struct.unpack("!Q", self._read_exact(8))[0]
            masked = bool(second & 0x80)
            mask = self._read_exact(4) if masked else b""
            payload = self._read_exact(length)
            if masked:
                payload = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
            if opcode == 0x8:
                raise CdpError("WorkBuddy bridge closed the connection")
            if opcode == 0x9:
                self._send_frame(0xA, payload)
                continue
            if opcode in (0x0, 0x1):
                chunks.append(payload)
                if final:
                    return json.loads(b"".join(chunks).decode("utf-8"))

    def _send_frame(self, opcode: int, payload: bytes) -> None:
        mask = os.urandom(4)
        first = 0x80 | opcode
        length = len(payload)
        if length < 126:
            header = struct.pack("!BB", first, 0x80 | length)
        elif length < 65536:
            header = struct.pack("!BBH", first, 0x80 | 126, length)
        else:
            header = struct.pack("!BBQ", first, 0x80 | 127, length)
        masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
        self.socket.sendall(header + mask + masked)

    def _read_exact(self, size: int) -> bytes:
        data = bytearray()
        while len(data) < size:
            chunk = self.socket.recv(size - len(data))
            if not chunk:
                raise CdpError("WorkBuddy bridge disconnected")
            data.extend(chunk)
        return bytes(data)

    def close(self) -> None:
        try:
            self.socket.close()
        except OSError:
            pass
