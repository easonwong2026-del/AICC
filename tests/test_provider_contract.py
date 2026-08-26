import tempfile
import unittest
from pathlib import Path

from providers.base import CacheStore, CallableProvider


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
            self.assertEqual(provider.refresh(force=True)["status"], "Online")
            self.assertEqual(provider.health()["state"], "Online")
            self.assertEqual(provider.cache().root, Path(directory))


if __name__ == "__main__":
    unittest.main()
