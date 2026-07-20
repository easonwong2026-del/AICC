"""Cross-platform system collector using only the Python standard library."""

from __future__ import annotations

import os
import platform
import re
import shutil
import socket
import subprocess


def _memory_bytes() -> tuple[int, int] | None:
    system = platform.system()
    if system == "Windows":
        # ctypes is only needed on Windows; avoiding it on macOS trims the
        # long-lived server's import graph and resident memory.
        import ctypes

        class MemoryStatus(ctypes.Structure):
            _fields_ = [
                ("length", ctypes.c_ulong), ("memory_load", ctypes.c_ulong),
                ("total_phys", ctypes.c_ulonglong), ("avail_phys", ctypes.c_ulonglong),
                ("total_page", ctypes.c_ulonglong), ("avail_page", ctypes.c_ulonglong),
                ("total_virtual", ctypes.c_ulonglong), ("avail_virtual", ctypes.c_ulonglong),
                ("avail_extended_virtual", ctypes.c_ulonglong),
            ]

        memory = MemoryStatus()
        memory.length = ctypes.sizeof(MemoryStatus)
        if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(memory)):
            return int(memory.total_phys), int(memory.avail_phys)
        return None
    if system == "Darwin":
        try:
            sysctl = shutil.which("sysctl") or "/usr/sbin/sysctl"
            vm_stat = shutil.which("vm_stat") or "/usr/bin/vm_stat"
            total = int(subprocess.check_output(
                [sysctl, "-n", "hw.memsize"], text=True, timeout=2
            ).strip())
            output = subprocess.check_output([vm_stat], text=True, timeout=2)
            page_match = re.search(r"page size of (\d+) bytes", output)
            page_size = int(page_match.group(1)) if page_match else 4096
            pages: dict[str, int] = {}
            for line in output.splitlines():
                match = re.match(r"([^:]+):\s+(\d+)\.", line)
                if match:
                    pages[match.group(1)] = int(match.group(2))
            available_pages = sum(pages.get(name, 0) for name in (
                "Pages free", "Pages inactive", "Pages speculative", "Pages purgeable"
            ))
            return total, min(total, available_pages * page_size)
        except (OSError, subprocess.SubprocessError, ValueError):
            return None
    try:
        page_size = int(os.sysconf("SC_PAGE_SIZE"))
        total = int(os.sysconf("SC_PHYS_PAGES")) * page_size
        available = int(os.sysconf("SC_AVPHYS_PAGES")) * page_size
        return total, available
    except (AttributeError, OSError, ValueError):
        return None


def collect() -> dict:
    result = {
        "label": socket.gethostname(),
        "status": "Online",
        "cpu": f"{os.cpu_count() or 0} cores",
        "platform": platform.system(),
    }
    memory = _memory_bytes()
    if memory:
        total, available = memory
        result["ram"] = f"{(total - available) / 1024**3:.1f}/{total / 1024**3:.0f} GB"
    else:
        result["ram"] = "--"

    executable = shutil.which("nvidia-smi")
    if executable:
        try:
            output = subprocess.check_output(
                [executable, "--query-gpu=name,utilization.gpu,memory.used,memory.total",
                 "--format=csv,noheader,nounits"],
                text=True, timeout=2,
            ).splitlines()[0]
            name, usage, used, total = [part.strip() for part in output.split(",")]
            result["gpu"] = f"{name}: {usage}% · {used}/{total} MiB"
        except (OSError, subprocess.SubprocessError, IndexError, ValueError):
            pass
    elif platform.system() == "Darwin" and platform.machine().lower() in ("arm64", "aarch64"):
        result["gpu"] = "Apple Silicon integrated GPU"
    return result
