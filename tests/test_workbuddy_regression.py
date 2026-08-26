"""Regression guard for the frozen WorkBuddy 2.4.4 live-balance chain.

The fixed adapter must not restructure or replace the WorkBuddy collector.
These tests pin the critical transport pieces so an accidental rewrite fails
CI immediately.
"""
import unittest

from collectors import workbuddy


class WorkBuddyChannelRegressionTests(unittest.TestCase):
    def test_localhost_cdp_bridge_port_is_unchanged(self):
        self.assertEqual(workbuddy.DEBUG_PORT, 9223)
        self.assertEqual(workbuddy.ACCOUNT_REFRESH_SECONDS, 120)

    def test_rpc_channel_and_public_fields_are_unchanged(self):
        expression = workbuddy.ACCOUNT_EXPRESSION
        self.assertIn("auth:getAccountUsage", expression)
        self.assertIn("MessageChannel", expression)
        self.assertIn("usageLeft", expression)
        self.assertIn("usageTotal", expression)
        self.assertIn("usageUsed", expression)
        self.assertIn("refreshAt", expression)
        self.assertIn("window.postMessage", expression)

    def test_public_field_whitelist_is_unchanged(self):
        for name in ("usageLeft", "usageTotal", "usageUsed", "refreshAt"):
            self.assertIn(name, workbuddy.ACCOUNT_EXPRESSION)
        for name in ("__apiProbe", "__apiError"):
            self.assertIn(name, workbuddy.ACCOUNT_EXPRESSION)

    def test_collector_expression_has_no_executable_escape_hatch(self):
        expression = workbuddy.ACCOUNT_EXPRESSION
        self.assertNotIn("require(", expression)
        self.assertNotIn("import(", expression)
        self.assertNotIn("child_process", expression)


if __name__ == "__main__":
    unittest.main()
