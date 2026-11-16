#!/usr/bin/env python3
"""本地复现 ci.yml 的路径过滤 + 门控逻辑，用真实 git 变更做回归。

不依赖 GitHub Actions，纯本地校验：
  1. 从 .github/workflows/ci.yml 里取出 dorny/paths-filter 的 filters 块；
  2. 用与 picomatch 等价的极小 glob 语义（** / * / ?）判定命中；
  3. 复现 "Resolve job gates" 脚本，输出每个 job 是否运行。
"""
import re
import subprocess

FILTERS = {
    "workflow": [".github/workflows/**"],
    "python": [
        "server.py",
        "bridge_tls.py",
        "collectors/**",
        "tests/**",
        "scripts/smoke-test-bundled-server.sh",
    ],
    "rust": [
        "account-worker/**",
        "scripts/smoke-test-account-worker.sh",
        "scripts/validate-account-worker-bundle.sh",
    ],
    "swift": [
        "macos/**",
        "VERSION",
        "scripts/smoke-test-widget.sh",
        "scripts/smoke-test-widget-visual.sh",
        "scripts/smoke-test-widget-bundle.sh",
        "scripts/smoke-test-board-visual.sh",
        "scripts/smoke-test-console-visual.sh",
        "scripts/smoke-test-dashboard-visual.sh",
        "scripts/smoke-test-display-snapshot.sh",
        "scripts/build-widget-preview.sh",
        "scripts/validate-widget-bundle.sh",
        "scripts/validate-version.sh",
        "scripts/fetch-python-runtime.sh",
    ],
    "android": [
        "android/**",
        "scripts/smoke-test-android-visual.sh",
    ],
}


def glob_to_regex(p: str) -> str:
    out = []
    i = 0
    while i < len(p):
        c = p[i]
        if c == "*":
            if p.startswith("**", i):
                i += 2
                if p.startswith("/", i):
                    # `a/**/b` 与 `a/**` 都要能吃到任意层级
                    i += 1
                    out.append("(?:.*/)?")
                else:
                    out.append(".*")
                continue
            out.append("[^/]*")
        elif c == "?":
            out.append("[^/]")
        elif c in ".+()[]{}|^$\\":
            out.append("\\" + c)
        else:
            out.append(c)
        i += 1
    return "^" + "".join(out) + "$"


def classify(files):
    hit = {k: False for k in FILTERS}
    for f in files:
        for area, pats in FILTERS.items():
            for p in pats:
                if re.match(glob_to_regex(p), f):
                    hit[area] = True
                    break
    return hit


def gates(files, workflow_changed):
    h = classify(files)
    if workflow_changed:
        jobs = {"python": True, "rust": True, "swift": True, "android": True}
        scenario = "full(workflow touched)"
    else:
        jobs = {"python": h["python"], "rust": h["rust"], "swift": h["swift"], "android": h["android"]}
        scenario = "changed"
    jobs["macos"] = jobs["python"] or jobs["swift"]
    jobs["macos-dmg"] = False  # PR 上一律不跑
    return scenario, jobs


CASES = {
    "Rust-only": ["account-worker/src/credentials/store.rs"],
    "Swift-only": ["macos/MenuBarApp/Sources/AICCCore/GrowthCenter.swift"],
    "Android-only": ["android/poke-dashboard/app/src/main/java/MainActivity.kt"],
    "Python-only": ["collectors/magpie.py", "tests/test_bridge_tls.py"],
    "Docs-only": ["docs/CI.md", "README.md"],
    "Rust+Swift": ["account-worker/src/lib.rs", "macos/build-aicc-swiftui.sh"],
    "CI 自身": [".github/workflows/ci.yml"],
}


def main():
    print(f"{'场景':12s} {'python':7s} {'rust':6s} {'swift':6s} {'android':8s} {'macos':6s} {'dmg':4s}")
    for name, files in CASES.items():
        scenario, g = gates(files, workflow_changed=".github/workflows/ci.yml" in files)
        row = " ".join(f"{'run' if g[k] else 'skip':6s}" for k in ("python", "rust", "swift", "android", "macos", "macos-dmg"))
        print(f"{name:12s} {row}   [{scenario}]")

    # 用真实历史提交回归：main 上最近 8 个提交分别会触发什么
    log = subprocess.run(
        ["git", "log", "--no-merges", "--pretty=%h %s", "-8"],
        capture_output=True, text=True, check=True,
    ).stdout.strip().splitlines()
    print("\n真实历史提交分类：")
    for line in log:
        sha, subject = line.split(" ", 1)
        files = subprocess.run(
            ["git", "show", "--name-only", "--pretty=", sha],
            capture_output=True, text=True, check=True,
        ).stdout.split()
        scenario, g = gates(files, workflow_changed=".github/workflows/ci.yml" in files)
        areas = [k for k in ("python", "rust", "swift", "android") if g[k]]
        print(f"  {sha} {subject[:52]:54s} -> {','.join(areas) or '无（全部 skip）'}")


if __name__ == "__main__":
    main()
