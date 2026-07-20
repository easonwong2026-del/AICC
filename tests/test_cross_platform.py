import os
import unittest
from unittest.mock import patch

from collectors import deepseek
from collectors import system
from collectors.system import collect as collect_system
from services.codex_monitor import CodexMonitor


class CrossPlatformTests(unittest.TestCase):
    def test_deepseek_process_environment_has_priority(self):
        with patch.dict(os.environ, {"DEEPSEEK_API_KEY": "test-key"}, clear=False):
            self.assertEqual(deepseek.load_api_key(), "test-key")

    def test_deepseek_reads_macos_keychain(self):
        with patch.dict(os.environ, {"DEEPSEEK_API_KEY": "", "USER": "tester"}, clear=False), \
             patch.object(deepseek.platform, "system", return_value="Darwin"), \
             patch.object(deepseek, "winreg", None), \
             patch.object(deepseek.subprocess, "check_output", return_value="mac-key\n") as check:
            self.assertEqual(deepseek.load_api_key(), "mac-key")
            self.assertEqual(check.call_args.args[0][0], "security")

    def test_macos_prefers_codex_on_path(self):
        with patch.dict(os.environ, {}, clear=True), \
             patch("services.codex_monitor.platform.system", return_value="Darwin"), \
             patch("services.codex_monitor.shutil.which", return_value="/opt/homebrew/bin/codex"):
            self.assertEqual(
                CodexMonitor._resolve_cli(),
                ("/opt/homebrew/bin/codex", "Standalone Codex CLI"),
            )

    def test_system_collector_returns_portable_contract(self):
        result = collect_system()
        self.assertEqual(result["status"], "Online")
        self.assertIn("label", result)
        self.assertIn("cpu", result)
        self.assertIn("ram", result)
        self.assertIn("platform", result)

    def test_macos_memory_tools_have_absolute_fallbacks(self):
        outputs = {
            "/usr/sbin/sysctl": "17179869184\n",
            "/usr/bin/vm_stat": "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 100.\n",
        }
        with patch.object(system.platform, "system", return_value="Darwin"), \
             patch.object(system.shutil, "which", return_value=None), \
             patch.object(system.subprocess, "check_output", side_effect=lambda args, **_: outputs[args[0]]) as check:
            total, available = system._memory_bytes()
        self.assertEqual(total, 17179869184)
        self.assertEqual(available, 100 * 16384)
        self.assertEqual(check.call_args_list[0].args[0][0], "/usr/sbin/sysctl")


if __name__ == "__main__":
    unittest.main()
