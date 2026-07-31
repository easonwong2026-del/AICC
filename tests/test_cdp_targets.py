import unittest

from services.cdp import _rank_targets


class CdpTargetSelectionTests(unittest.TestCase):
    def test_prefers_main_renderer_over_first_page(self):
        pages = [
            {"id": "docs", "type": "page", "title": "Tencent Docs", "url": "https://docs.qq.com/doc/abc"},
            {
                "id": "main",
                "type": "page",
                "title": "WorkBuddy",
                "url": "file:///Applications/WorkBuddy.app/Contents/Resources/renderer/index.html",
            },
        ]
        self.assertEqual(_rank_targets(pages)["id"], "main")

    def test_excludes_embedded_webviews_when_main_page_is_known(self):
        pages = [
            {"id": "webview", "type": "page", "title": "tdoc-import", "url": "https://docs.qq.com/import"},
            {"id": "mcp", "type": "page", "title": "MCP App", "url": "app://mcp-app/index.html"},
            {"id": "main", "type": "page", "title": "WorkBuddy", "url": "file:///.../renderer/index.html"},
        ]
        self.assertEqual(_rank_targets(pages)["id"], "main")

    def test_falls_back_to_workbuddy_named_page(self):
        pages = [
            {"id": "docs", "type": "page", "title": "Docs", "url": "https://docs.qq.com/doc/abc"},
            {"id": "wb", "type": "page", "title": "WorkBuddy", "url": "https://copilot.tencent.com/"},
        ]
        self.assertEqual(_rank_targets(pages)["id"], "wb")

    def test_all_excluded_targets_still_return_one(self):
        pages = [
            {"id": "webview", "type": "page", "title": "tdoc-import", "url": "https://docs.qq.com/import"},
            {"id": "devtools", "type": "page", "title": "DevTools", "url": "devtools://devtools/bundled/inspector.html"},
        ]
        self.assertIn(_rank_targets(pages)["id"], {"webview", "devtools"})

    def test_empty_list_returns_none(self):
        self.assertIsNone(_rank_targets([]))


if __name__ == "__main__":
    unittest.main()
