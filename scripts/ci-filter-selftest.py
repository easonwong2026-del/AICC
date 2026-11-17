#!/usr/bin/env python3
"""ci.yml 的路径过滤自测。

三件事，全部基于 **ci.yml 里真实的 filters 块**（不再手抄一份规则）：

1. 场景分类：给一组变更文件，算出每个 job 该不该跑；
2. 历史回归：拿仓库里真实的历史提交跑一遍分类，便于人工核对；
3. 覆盖断言：每个 job 实际会执行的文件（脚本 / 模块，含传递调用），
   必须落在能触发该 job 的某一类路径里。漏一个就是"改了却永远不会被测试"。

用法：
    python3 scripts/ci-filter-selftest.py            # 全部三项
    python3 scripts/ci-filter-selftest.py --check    # 只做覆盖断言（CI 里跑这个）
"""

from __future__ import annotations

import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WORKFLOW = os.path.join(ROOT, ".github", "workflows", "ci.yml")

# 每个 job 被哪些"路径分类"驱动，与 ci.yml 里的 `if:` 一一对应。
JOB_AREAS = {
    "test": ["python"],
    "lint": ["python"],
    "rust": ["rust"],
    "swift-test": ["swift"],
    "macos-build": ["swift", "python"],
    "macos-dmg": ["swift", "python"],
    "android": ["android"],
    "android-visual": ["android"],
}

# 会被 job 执行的仓库内文件
EXECUTABLE = re.compile(
    r"(scripts/[A-Za-z0-9._-]+\.(?:sh|py)"
    r"|macos/[A-Za-z0-9._-]+\.sh"
    r"|(?:server|bridge_tls)\.py"
    r"|collectors/[A-Za-z0-9_.-]+\.py"
    r"|tests/[A-Za-z0-9_./-]+\.py)"
)


def parse_filters(text: str) -> dict[str, list[str]]:
    """从 ci.yml 的 `filters: |` 块取出 {分类: [glob...]}，不依赖 PyYAML。"""
    lines = text.splitlines()
    for idx, line in enumerate(lines):
        if line.strip() == "filters: |":
            base = len(line) - len(line.lstrip())
            body = []
            for line2 in lines[idx + 1:]:
                if line2.strip() and (len(line2) - len(line2.lstrip())) <= base:
                    break
                body.append(line2)
            return _parse_filter_body(body)
    raise SystemExit("ci.yml 里找不到 `filters: |` 块")


def _parse_filter_body(body: list[str]) -> dict[str, list[str]]:
    filters: dict[str, list[str]] = {}
    current = None
    for raw in body:
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.endswith(":") and not stripped.startswith("-"):
            current = stripped[:-1]
            filters[current] = []
            continue
        if stripped.startswith("- ") and current:
            filters[current].append(stripped[2:].strip().strip("'\""))
    return filters


def yaml_jobs(text: str) -> dict[str, str]:
    """粗粒度切分 job 名 → job 文本块（只为找出它引用了哪些文件）。"""
    lines = text.splitlines()
    start = next(i for i, line in enumerate(lines) if line.strip() == "jobs:")
    jobs: dict[str, str] = {}
    current = None
    buf: list[str] = []
    for line in lines[start + 1:]:
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if m:
            if current:
                jobs[current] = "\n".join(buf)
            current, buf = m.group(1), []
        elif current:
            buf.append(line)
    if current:
        jobs[current] = "\n".join(buf)
    return jobs


def glob_regex(pattern: str) -> re.Pattern:
    out, i = [], 0
    while i < len(pattern):
        c = pattern[i]
        if c == "*":
            if pattern.startswith("**", i):
                i += 2
                if pattern.startswith("/", i):
                    i += 1
                    out.append("(?:.*/)?")
                else:
                    out.append(".*")
                continue
            out.append("[^/]*")
        elif c in ".+()[]{}|^$\\":
            out.append("\\" + c)
        else:
            out.append(c)
        i += 1
    return re.compile("^" + "".join(out) + "$")


def matches(path: str, patterns: list[str]) -> bool:
    return any(glob_regex(p).match(path) for p in patterns)


def direct_refs(blob: str) -> set[str]:
    return {
        m.group(1) for m in EXECUTABLE.finditer(blob)
        if os.path.exists(os.path.join(ROOT, m.group(1)))
    }


def closure(seeds: set[str]) -> set[str]:
    seen: set[str] = set()
    queue = list(seeds)
    while queue:
        cur = queue.pop()
        if cur in seen:
            continue
        seen.add(cur)
        try:
            text = open(os.path.join(ROOT, cur), encoding="utf-8", errors="ignore").read()
        except OSError:
            continue
        for nxt in direct_refs(text):
            if nxt not in seen:
                queue.append(nxt)
    return seen


def coverage_problems(workflow_text: str, filters: dict[str, list[str]]) -> list[tuple[str, str]]:
    """不变量：被某个 job 执行的每个文件，至少要能触发"其中一个真正执行它的 job"。

    注意是"至少一个"而不是"每一个"：同一个文件可能被多个 job 顺带用到
    （例如 server.py 既被 bundled-server smoke 用到、也被 python 单测用到），
    改它只需要跑其中一个真正会执行它的 job 即可，不必把所有 job 全拉起来。
    """
    jobs = yaml_jobs(workflow_text)
    executed_by: dict[str, list[str]] = {}
    for job in JOB_AREAS:
        if job not in jobs:
            executed_by.setdefault("<job 不存在>", []).append(job)
            continue
        for path in closure(direct_refs(jobs[job])):
            executed_by.setdefault(path, []).append(job)

    problems = [(job, "<job 不存在>") for job in JOB_AREAS if job not in jobs]
    for path, executors in sorted(executed_by.items()):
        if path == "<job 不存在>":
            continue
        covered = any(
            matches(path, filters[area])
            for job in executors for area in JOB_AREAS[job]
        )
        if not covered:
            problems.append((executors[0], path))
    return problems


SCENARIOS = {
    "Rust-only": ["account-worker/src/credentials/store.rs"],
    "Swift-only": ["macos/MenuBarApp/Sources/AICCCore/GrowthCenter.swift"],
    "Android-only": ["android/poke-dashboard/app/src/main/java/MainActivity.kt"],
    "Python-only": ["collectors/magpie.py", "tests/test_bridge_tls.py"],
    "Python runtime 校验脚本": ["scripts/validate-python-runtime.sh"],
    "Docs-only": ["docs/CI.md", "README.md"],
    "Rust+Swift": ["account-worker/src/lib.rs", "macos/build-aicc-swiftui.sh"],
    "CI 自身": [".github/workflows/ci.yml"],
}


def classify(files: list[str], filters: dict[str, list[str]]) -> dict[str, bool]:
    hit = {area: any(matches(f, pats) for f in files) for area, pats in filters.items()}
    jobs = {
        "python": hit.get("python", False),
        "rust": hit.get("rust", False),
        "swift": hit.get("swift", False),
        "android": hit.get("android", False),
    }
    jobs["macos"] = jobs["python"] or jobs["swift"]
    if hit.get("workflow"):
        # 与 ci.yml 一致：改了 CI 自身就全量
        jobs = {k: True for k in ("python", "rust", "swift", "android", "macos")}
    return jobs


def history_check(filters: dict[str, list[str]]) -> None:
    try:
        log = subprocess.run(
            ["git", "log", "--no-merges", "--pretty=%h %s", "-6"],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout.strip().splitlines()
    except (OSError, subprocess.CalledProcessError):
        print("\n（跳过历史回归：不在 git 仓库里）")
        return
    print("\n真实历史提交分类：")
    for line in log:
        if not line.strip():
            continue
        sha, subject = line.split(" ", 1)
        files = subprocess.run(
            ["git", "show", "--name-only", "--pretty=", sha],
            cwd=ROOT, capture_output=True, text=True, check=True,
        ).stdout.split()
        jobs = classify(files, filters)
        areas = [k for k in ("python", "rust", "swift", "android") if jobs[k]]
        print(f"  {sha} {subject[:50]:52s} -> {','.join(areas) or '无（全部 skip）'}")


def main() -> int:
    text = open(WORKFLOW, encoding="utf-8").read()
    filters = parse_filters(text)

    if "--check" not in sys.argv:
        print("从 ci.yml 解析到的过滤分类：")
        for area, pats in filters.items():
            print(f"  {area:9s} {len(pats)} 条")
        print()
        print(f"{'场景':22s} python rust  swift android macos")
        for name, files in SCENARIOS.items():
            jobs = classify(files, filters)
            row = " ".join(f"{'run' if jobs[k] else 'skip':6s}" for k in ("python", "rust", "swift", "android", "macos"))
            print(f"{name:22s} {row}")

    problems = coverage_problems(text, filters)
    if problems:
        print("\n覆盖漏洞（这些文件被某个 job 执行，却不在能触发它的路径里）：")
        for job, path in problems:
            print(f"  {job:12s} {path}")
    else:
        print("\n覆盖断言通过：每个 job 执行的文件都在对应的触发路径里。")

    if "--check" not in sys.argv:
        history_check(filters)

    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
