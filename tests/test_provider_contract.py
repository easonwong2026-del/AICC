import tempfile
import unittest
from pathlib import Path

from providers.base import CacheStore, CallableProvider
from providers.registry import build_provider_registry


class ProviderContractTests(unittest.TestCase):
    def test_callable_adapter_exposes_provider_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            cache = CacheStore(Path(directory))
            provider = CallableProvider(
                "test",
                lambda: {"status": "Online"},
                60,
                {"status": "Pending"},
                cache,
            )
            self.assertEqual(provider.status()["status"], "Pending")
            self.assertEqual(provider.refresh()["status"], "Online")
            self.assertEqual(provider.health()["state"], "Online")
            self.assertEqual(provider.cache().root, Path(directory))

    def test_default_registry_contains_all_existing_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = build_provider_registry(Path(directory), {}, lambda: {})
            self.assertEqual(
                {name for name, _ in registry.items()},
                {"codex", "deepseek", "workbuddy", "system"},
            )
            for provider in registry.values():
                self.assertTrue(callable(provider.status))
                self.assertTrue(callable(provider.health))
                self.assertTrue(callable(provider.refresh))
                self.assertTrue(callable(provider.cache))
                self.assertEqual(provider.cache().root, Path(directory))


if __name__ == "__main__":
    unittest.main()
