import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LEGACY_LABEL = "com.aieink.workbuddy-monitor"


class AutostartScriptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.home = Path(self.temporary.name) / "home"
        self.bin = Path(self.temporary.name) / "bin"
        self.home.mkdir()
        self.bin.mkdir()
        self.command_log = Path(self.temporary.name) / "launchctl.log"
        self._write_executable(
            self.bin / "launchctl",
            """#!/bin/bash
printf '%s\\n' "$*" >> "$AICC_LAUNCHCTL_LOG"
case "${1:-}" in
  print|bootout) exit 1 ;;
  bootstrap|kickstart) exit 0 ;;
  *) exit 1 ;;
esac
""",
        )
        self._write_executable(self.bin / "plutil", "#!/bin/bash\nexit 0\n")
        self.environment = os.environ.copy()
        self.environment.update(
            HOME=str(self.home),
            PATH=f"{self.bin}:{self.environment.get('PATH', '')}",
            PYTHON_BIN="/fake/python3",
            AICC_LAUNCHCTL_LOG=str(self.command_log),
        )

    def tearDown(self):
        self.temporary.cleanup()

    def _write_executable(self, path, contents):
        path.write_text(contents, encoding="utf-8")
        path.chmod(0o755)

    def _run(self, script):
        return subprocess.run(
            ["bash", str(ROOT / script)],
            env=self.environment,
            capture_output=True,
            text=True,
        )

    def test_install_removes_legacy_monitor_without_registering_one(self):
        legacy = self.home / "Library/LaunchAgents" / f"{LEGACY_LABEL}.plist"
        legacy.parent.mkdir(parents=True)
        legacy.write_text("legacy", encoding="utf-8")

        result = self._run("macos/install-autostart.sh")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(legacy.exists())
        launchctl_calls = self.command_log.read_text(encoding="utf-8").splitlines()
        self.assertTrue(
            any(call.startswith("bootout ") and LEGACY_LABEL in call for call in launchctl_calls)
        )
        self.assertFalse(
            any(call.startswith("bootstrap ") and LEGACY_LABEL in call for call in launchctl_calls)
        )
        self.assertTrue((self.home / "Library/LaunchAgents/com.aieink.dashboard.plist").exists())
        self.assertTrue((self.home / "Library/LaunchAgents/com.aieink.log-maintenance.plist").exists())

    def test_uninstall_removes_legacy_monitor_residue(self):
        launch_agents = self.home / "Library/LaunchAgents"
        launch_agents.mkdir(parents=True)
        for name in ("com.aieink.dashboard", LEGACY_LABEL, "com.aieink.log-maintenance"):
            (launch_agents / f"{name}.plist").write_text("old", encoding="utf-8")

        result = self._run("macos/uninstall-autostart.sh")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(launch_agents.glob("*.plist")))
        self.assertIn("legacy WorkBuddy monitor", result.stdout)


if __name__ == "__main__":
    unittest.main()
