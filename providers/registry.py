"""Provider registry used by the HTTP server and CollectorManager."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable

from providers.base import CacheStore, Provider
from providers.codex import CodexProvider
from providers.deepseek import DeepSeekProvider
from providers.system import SystemProvider
from providers.workbuddy import WorkBuddyProvider


class ProviderRegistry:
    def __init__(self, providers: dict[str, Provider]) -> None:
        self._providers = providers

    def get(self, name: str) -> Provider:
        return self._providers[name]

    def items(self):
        return self._providers.items()

    def values(self):
        return self._providers.values()

    def as_dict(self) -> dict[str, Provider]:
        return self._providers.copy()


def build_provider_registry(
    data_root: Path,
    fallback: dict[str, Any],
    fallback_loader: Callable[[], dict[str, Any]],
) -> ProviderRegistry:
    cache = CacheStore(data_root)
    return ProviderRegistry({
        "codex": CodexProvider(cache, fallback.get("codex", {})),
        "deepseek": DeepSeekProvider(cache),
        "workbuddy": WorkBuddyProvider(
            cache,
            lambda: fallback_loader().get("workbuddy", {}),
            fallback.get("workbuddy", {}),
        ),
        "system": SystemProvider(cache),
    })
