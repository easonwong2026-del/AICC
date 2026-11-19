#!/usr/bin/env python3
"""Reject `$VAR` immediately followed by a non-ASCII character in shell scripts.

Why this exists: bash (3.2 on this machine, still common) treats the bytes of a
multi-byte character as part of the variable name, so

    echo "省了 $SIZE 字节（约 $SIZE（…"     # fine
    echo "写入 $PATH（已存在）"             # $PATH（ is one name -> unbound

fails at runtime with `unbound variable` and no line-level clue. It has broken
this repository's build scripts more than once, always in Chinese user-facing
strings, always only on the path that prints them.

Rule: brace any variable that is directly followed by a non-ASCII byte.

    $name（  ->  ${name}（

Usage: python3 scripts/check-shell-var-braces.py [paths...]
Exit:  0 clean, 1 violations, 2 usage error.
"""

import glob
import re
import sys

PATTERN = re.compile(r"\$([A-Za-z_][A-Za-z0-9_]*)([^\x00-\x7f])")


def default_paths():
    # Shell only: Python source legitimately contains `$name（` in documentation
    # and in regex patterns.
    return sorted(glob.glob("scripts/*.sh") + glob.glob("macos/*.sh"))


def main(argv):
    paths = argv[1:] or default_paths()
    if not paths:
        print("no files to check", file=sys.stderr)
        return 2

    violations = 0
    for path in paths:
        try:
            with open(path, encoding="utf-8") as handle:
                lines = handle.readlines()
        except (OSError, UnicodeDecodeError) as error:
            print("%s: cannot read (%s)" % (path, error), file=sys.stderr)
            continue
        for number, line in enumerate(lines, 1):
            for match in PATTERN.finditer(line):
                violations += 1
                print(
                    "%s:%d: $%s 后面直接跟了非 ASCII 字符，请写成 ${%s}"
                    % (path, number, match.group(1), match.group(1)),
                    file=sys.stderr,
                )

    if violations:
        print("\n%d 处需要加花括号（否则 bash 会把后面的中文当成变量名）" % violations, file=sys.stderr)
        return 1
    print("ok: 没有 $VAR 紧跟非 ASCII 字符的写法（检查了 %d 个文件）" % len(paths))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
