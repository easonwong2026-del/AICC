#!/usr/bin/env python3
"""Listen on a loopback port and publish the connection count, accurately.

Why this is a separate file instead of a heredoc: the count has to be *right*
even when the process is killed. A previous version wrote the count from an
`atexit` hook, and `kill <pid>` sends SIGTERM — Python does not run `atexit`
on SIGTERM, so the file kept its initial value and the whole "zero connections"
assertion silently degraded to "cannot tell".

Rules this file enforces:

1. The count file is written **immediately after** `bind() + listen()`, before
   any connection can arrive. Its presence means "counting has started".
2. Every `accept()` rewrites the count **atomically** (temp file + `os.replace`),
   so a reader never sees a half-written or stale-plus-new mix.
3. A port that cannot be bound publishes `unavailable`, and nothing else. It
   never fakes a zero: "port was taken" and "nobody connected" are different
   facts and the audit's verdict depends on telling them apart.
4. No signal handlers and no exit hooks: correctness must not depend on the
   process getting a chance to clean up.

Usage: canary_loopback_port.py <port> <count file>
"""

import os
import socket
import sys

READY = "0"
UNAVAILABLE = "unavailable"


def publish(path, text):
    """Atomically replace the count file. Readers see old or new, never torn."""
    tmp = "%s.tmp.%d" % (path, os.getpid())
    with open(tmp, "w") as handle:
        handle.write(text + "\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(tmp, path)


def main(argv):
    if len(argv) != 3:
        print("usage: canary_loopback_port.py <port> <count file>", file=sys.stderr)
        return 2

    port = int(argv[1])
    path = argv[2]
    count = 0

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    # SO_REUSEADDR would let us bind a port another process is listening on in
    # TIME_WAIT, which is exactly the confusion this canary must not have.
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 0)
    try:
        server.bind(("127.0.0.1", port))
    except OSError:
        publish(path, UNAVAILABLE)
        return 0

    server.listen(16)
    publish(path, READY)

    while True:
        try:
            connection, _ = server.accept()
        except OSError:
            break
        count += 1
        publish(path, str(count))
        try:
            connection.close()
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
