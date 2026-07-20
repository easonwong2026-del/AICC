import unittest
from unittest.mock import patch

from collectors import workbuddy
from services.cdp import CdpError


class WorkBuddyPassiveTests(unittest.TestCase):
    def setUp(self):
        workbuddy._last_attempt = 0.0
        workbuddy._account_cache = None
        workbuddy._account_target = None

    def test_balance_is_probed_once_per_renderer_session(self):
        raw = {"usageLeft": "123.45", "refreshAt": None}
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", return_value=raw) as evaluate, \
             patch.object(workbuddy.time, "monotonic", side_effect=[100.0, 200.0]), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            first = workbuddy._get_account_usage()
            second = workbuddy._get_account_usage()

        self.assertEqual(first["points"], 123.45)
        self.assertEqual(second["points"], 123.45)
        evaluate.assert_called_once()

    def test_closed_bridge_keeps_cache_and_allows_next_session_probe(self):
        cache = {"points": 88, "balance_updated_epoch": 900.0}
        with patch.object(workbuddy, "_load_account_cache", return_value=cache), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            result = workbuddy._get_account_usage()

        self.assertEqual(result["points"], 88)
        self.assertIsNone(workbuddy._account_target)


if __name__ == "__main__":
    unittest.main()
