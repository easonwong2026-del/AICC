"""LAN-facing TLS for the bridge (PR-B).

The loopback management API stays plain HTTP on 127.0.0.1:8765; network devices
get HTTPS on 8767 or nothing at all.  Everything here fails closed: missing or
unreadable certificate material means no LAN listener, never a plaintext
fallback and never "temporarily disable verification".

Certificate material is produced by the account worker (Rust, `rcgen`), not
here: the bundled runtime has OpenSSL as a library but no `openssl` CLI, and
signing keys must never be handled by the process that also serves untrusted
network traffic.  The layout this module reads is:

    <dir>/ca.pem        self-signed root, long lived, public half of the pin
    <dir>/server.pem    leaf certificate followed by the root (chain)
    <dir>/server.key    leaf private key, mode 0600
    <dir>/identity.json {"spki": base64 SHA-256 over the root SPKI,
                         "not_after": ISO-8601, "sans": [...], "generation": n}
"""

import json
import os
import ssl
import stat
from pathlib import Path

LAN_PORT = 8767
DISCOVERY_PORT = 8766
PROTOCOL_VERSION = 2

REQUIRED_FILES = ("ca.pem", "server.pem", "server.key", "identity.json")

# TLS 1.2 is allowed for old devices; the suites below keep it to ECDHE + AEAD
# and leave TLS 1.3 on its own (correct) defaults.
ALLOWED_CIPHERS = "ECDHE+AESGCM:ECDHE+CHACHA20:!aNULL:!eNULL:!MD5:!RC4:!3DES:!DES"


class TlsUnavailable(Exception):
    """No usable certificate material: the LAN listener must not start."""


class TlsMaterial:
    def __init__(self, directory, identity):
        self.directory = directory
        self.spki = identity["spki"]
        self.not_after = identity.get("not_after")
        self.generation = identity.get("generation")
        self.sans = identity.get("sans") or []


def cert_directory():
    override = (os.environ.get("EINK_CERT_DIR") or "").strip()
    if override:
        return Path(override)
    return Path.home() / "Library/Application Support/AICC-Dashboard/certs"


def _tighten(path):
    """A private key that lost 0600 (copied, restored from backup) is repaired
    rather than trusted as is; it never gets read while it is world-readable."""
    mode = stat.S_IMODE(path.stat().st_mode)
    if mode & 0o077:
        try:
            os.chmod(path, 0o600)
        except OSError as error:
            raise TlsUnavailable(f"{path.name} is {oct(mode)} and cannot be tightened: {error}")


def load_identity(directory=None):
    root = cert_directory() if directory is None else Path(directory)
    missing = [name for name in REQUIRED_FILES if not (root / name).is_file()]
    if missing:
        raise TlsUnavailable(f"no certificate material in {root}: missing {', '.join(missing)}")
    try:
        identity = json.loads((root / "identity.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise TlsUnavailable(f"{root}/identity.json is unreadable: {error}")
    spki = identity.get("spki")
    # base64 of a SHA-256 is always 44 characters; anything else is a truncated
    # or hand-edited file, and a wrong pin silently locks every device out.
    if not isinstance(spki, str) or len(spki) != 44 or not spki.endswith("="):
        raise TlsUnavailable(f"{root}/identity.json has no usable spki pin")
    _tighten(root / "server.key")
    return TlsMaterial(root, identity)


def build_context(material=None):
    """Server context: TLS 1.3 preferred, TLS 1.2 kept, nothing older."""
    material = load_identity() if material is None else material
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.maximum_version = ssl.TLSVersion.TLSv1_3
    context.options |= ssl.OP_NO_COMPRESSION
    context.options |= ssl.OP_CIPHER_SERVER_PREFERENCE
    context.set_ciphers(ALLOWED_CIPHERS)
    context.load_cert_chain(str(material.directory / "server.pem"),
                            str(material.directory / "server.key"))
    return context


def diagnostics(context=None):
    """What the runtime can actually do, without any secret in it.

    Printed at startup so a support log answers "is this 1.3?" without asking
    the user to run a command.
    """
    context = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH) if context is None else context
    suites = context.get_ciphers()
    tls13 = [item["name"] for item in suites if item.get("protocol") == "TLSv1.3"]
    return {"openssl": ssl.OPENSSL_VERSION, "tls13_compiled": ssl.HAS_TLSv1_3,
            "minimum": "TLSv1.2", "maximum": "TLSv1.3",
            "tls13_suites": len(tls13), "cipher_count": len(suites)}


def summary(material, context):
    """Published to the LAN and to the settings UI; carries no secret."""
    payload = {"port": LAN_PORT, "protocol_version": PROTOCOL_VERSION,
               "spki": material.spki, "not_after": material.not_after,
               "generation": material.generation, "sans": material.sans}
    payload.update({key: value for key, value in diagnostics(context).items() if key != "cipher_count"})
    return payload
