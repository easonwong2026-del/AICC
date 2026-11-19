#!/usr/bin/env python3
"""Fail if CI uploads the symbol archive to an Actions artifact.

The repository is public: an Actions artifact is readable by anyone who can
read the repo. "It is not on the Release page" is not access control, so the
dSYMs and unstripped binaries must stay on the machine that built them while CI
keeps *verifying* them (scripts/verify-symbols.sh) and logs only the manifest.

This check exists because the honest failure mode is silent: someone adds an
upload step back for convenience, the run stays green, and the symbols quietly
become public again.

Usage: python3 scripts/check-ci-artifacts.py [workflow...]
Exit:  0 clean, 1 violation.
"""

import glob
import sys

FORBIDDEN_PATHS = ("dist/symbols", ".dSYM", ".unstripped")
WINDOW = 8  # how many lines after an upload-artifact step to inspect


def main(argv):
    paths = argv[1:] or sorted(glob.glob(".github/workflows/*.yml"))
    if not paths:
        print("no workflow files found", file=sys.stderr)
        return 1

    problems = 0
    for path in paths:
        with open(path, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
        for index, line in enumerate(lines):
            if "upload-artifact" not in line:
                continue
            block = "\n".join(lines[index:index + WINDOW])
            for token in FORBIDDEN_PATHS:
                if token in block:
                    problems += 1
                    print(
                        "%s:%d: 这个 upload-artifact 步骤包含了 %s\n"
                        "  公开仓库的 artifact 不是私有存储；符号只允许留在构建机，"
                        "CI 里请用 scripts/verify-symbols.sh 校验并在日志打印 manifest。"
                        % (path, index + 1, token),
                        file=sys.stderr,
                    )

    if problems:
        return 1
    print("ok: CI 没有上传符号归档（检查了 %d 个 workflow）" % len(paths))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
