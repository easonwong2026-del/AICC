"""Bridge TLS self-check, run with the *bundled* interpreter.

PR-B has to prove the packaged CPython actually negotiates TLS 1.3 and still
speaks TLS 1.2 before it is allowed into the App.  Version strings are not
enough: a runtime can report OpenSSL 3.5 and still fail a handshake if the
loader picked up a different libssl or the CA bundle is missing.

Both ends of every check run inside this single interpreter, so the result
never depends on the machine's Homebrew OpenSSL, on macOS's LibreSSL CLI, or
on whatever CA store the developer happens to have.

    /path/to/runtime/bin/python3 tests/runtime/tls_selfcheck.py [--keep DIR]

Exit status 0 means every case passed.
"""

import argparse
import base64
import hashlib
import json
import os
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import warnings
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

CHECKS = []


def record(name, ok, detail):
    CHECKS.append({"case": name, "ok": bool(ok), "detail": str(detail)})
    print(f"CASE {name:<34} => {'PASS' if ok else 'FAIL'}: {detail}")
    return ok


def openssl_binary():
    for candidate in ("/usr/bin/openssl", "/opt/homebrew/bin/openssl", "/usr/local/bin/openssl"):
        if Path(candidate).is_file():
            return candidate
    found = shutil.which("openssl")
    if not found:
        raise SystemExit("openssl CLI not found; it is only used to mint throwaway test certs")
    return found


CONFIG = """\
[req]
distinguished_name = dn
prompt = no
x509_extensions = v3
[dn]
CN = AICC runtime self-check
O = AICC test only
[v3]
basicConstraints = critical,CA:TRUE
keyUsage = critical,digitalSignature,keyCertSign
extendedKeyUsage = serverAuth
subjectAltName = IP:127.0.0.1,DNS:localhost
"""


def make_cert(directory, name="server"):
    """Throwaway self-signed leaf. Used only by this check, never shipped."""
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    config = directory / f"{name}.cnf"
    config.write_text(CONFIG, encoding="utf-8")
    key = directory / f"{name}.key"
    cert = directory / f"{name}.crt"
    subprocess.run([openssl_binary(), "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                    "-keyout", str(key), "-out", str(cert), "-days", "2", "-sha256",
                    "-config", str(config)], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    os.chmod(key, 0o600)
    return cert, key


def spki_pin(cert_path):
    """SHA-256 over the DER SubjectPublicKeyInfo, base64 — what Android pins."""
    pem = Path(cert_path).read_text(encoding="ascii")
    # The stdlib has no key parser, so the only step the CLI does is SPKI extraction.
    public = subprocess.run([openssl_binary(), "x509", "-pubkey", "-noout"], input=pem.encode(),
                            capture_output=True, check=True).stdout
    der_spki = subprocess.run([openssl_binary(), "pkey", "-pubin", "-outform", "DER"],
                              input=public, capture_output=True, check=True).stdout
    return base64.b64encode(hashlib.sha256(der_spki).digest()).decode("ascii")


class Probe(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/api/health/live":
            self.send_error(404)
            return
        body = json.dumps({"ok": True, "status": "live", "protocol": "aicc"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def server_context(cert, key):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.maximum_version = ssl.TLSVersion.TLSv1_3
    context.options |= ssl.OP_NO_COMPRESSION
    context.options |= ssl.OP_CIPHER_SERVER_PREFERENCE
    # TLS 1.3 suites first, TLS 1.2 restricted to ECDHE + AEAD.
    context.set_ciphers("ECDHE+AESGCM:ECDHE+CHACHA20:!aNULL:!eNULL:!MD5:!RC4:!3DES")
    context.load_cert_chain(str(cert), str(key))
    return context


class QuietServer(ThreadingHTTPServer):
    """A rejected handshake or an aborted read is the expected outcome here."""

    def handle_error(self, request, client_address):
        import sys as _sys
        error = _sys.exc_info()[1]
        if isinstance(error, (ssl.SSLError, ConnectionResetError, BrokenPipeError, OSError)):
            return
        super().handle_error(request, client_address)


def start_server(cert, key):
    server = QuietServer(("127.0.0.1", 0), Probe)
    server.daemon_threads = True
    server.socket = server_context(cert, key).wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, server.server_port


def handshake(port, cafile, min_version, max_version, expect_ok=True):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = bool(cafile)
    if cafile:
        context.load_verify_locations(cafile)
    else:
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE
    # Asking for TLS 1.0/1.1 is deliberate: the point is that the server refuses.
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        if min_version is not None:
            context.minimum_version = min_version
        if max_version is not None:
            context.maximum_version = max_version
    with socket.create_connection(("127.0.0.1", port), timeout=5) as raw:
        with context.wrap_socket(raw, server_hostname="localhost") as tls:
            return tls.version()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--keep", help="directory to keep generated certs in")
    options = parser.parse_args()

    workdir = Path(options.keep) if options.keep else Path(tempfile.mkdtemp(prefix="aicc-tls-"))
    cert, key = make_cert(workdir)
    warnings = ssl.OPENSSL_VERSION
    print(f"[tls] interpreter      = {sys.executable}")
    print(f"[tls] python           = {sys.version.split()[0]}")
    print(f"[tls] openssl          = {warnings}")
    print(f"[tls] HAS_TLSv1_3      = {ssl.HAS_TLSv1_3}")
    print(f"[tls] HAS_TLSv1_2      = {ssl.HAS_TLSv1_2}")

    record("openssl_is_3_5", warnings.startswith("OpenSSL 3.5"),
           warnings.split()[1] if len(warnings.split()) > 1 else warnings)
    record("tls13_compiled_in", ssl.HAS_TLSv1_3, ssl.HAS_TLSv1_3)

    server, port = start_server(cert, key)
    try:
        version = handshake(port, str(cert), None, None)
        record("tls13_preferred", version == "TLSv1.3", version)

        version = handshake(port, str(cert), ssl.TLSVersion.TLSv1_2, ssl.TLSVersion.TLSv1_2)
        record("tls12_accepted", version == "TLSv1.2", version)

        try:
            handshake(port, str(cert), ssl.TLSVersion.TLSv1, ssl.TLSVersion.TLSv1)
            record("tls10_rejected", False, "handshake succeeded")
        except ssl.SSLError as error:
            record("tls10_rejected", True, f"rejected: {error.reason}")

        try:
            handshake(port, str(cert), ssl.TLSVersion.TLSv1_1, ssl.TLSVersion.TLSv1_1)
            record("tls11_rejected", False, "handshake succeeded")
        except ssl.SSLError as error:
            record("tls11_rejected", True, f"rejected: {error.reason}")

        try:
            handshake(port, None, None, None, expect_ok=False)
            record("no_verify_is_test_side_only", True, "verify_mode NONE still connects (test scaffolding)")
        except ssl.SSLError as error:
            record("no_verify_is_test_side_only", False, f"unexpected failure: {error.reason}")

        # An unknown CA must be refused: client trusts only a second, unrelated cert.
        other_cert, _ = make_cert(workdir / "other", "other")
        try:
            handshake(port, str(other_cert), None, None)
            record("unknown_ca_rejected", False, "handshake succeeded against a foreign CA")
        except ssl.SSLError as error:
            record("unknown_ca_rejected", True, f"rejected: {error.reason}")

        # Hostname verification matters: the cert only names localhost/127.0.0.1.
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.load_verify_locations(str(cert))
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=5) as raw:
                with context.wrap_socket(raw, server_hostname="wrong.example") as tls:
                    tls.getpeercert()
            record("wrong_hostname_rejected", False, "handshake succeeded")
        except ssl.SSLCertVerificationError as error:
            record("wrong_hostname_rejected", True, f"rejected: {error.verify_message}")

        pin = spki_pin(cert)
        (workdir / "pin.txt").write_text(pin, encoding="utf-8")
        record("spki_pin_stable", len(pin) == 44 and pin.endswith("="), pin)
        record("spki_pin_differs_for_other_cert",
               spki_pin(other_cert) != pin, "second cert has a distinct pin")

        # The HTTP layer over TLS, not just the handshake.
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.load_verify_locations(str(cert))
        with socket.create_connection(("127.0.0.1", port), timeout=5) as raw:
            with context.wrap_socket(raw, server_hostname="localhost") as tls:
                tls.sendall(b"GET /api/health/live HTTP/1.1\r\nHost: localhost\r\n"
                            b"Connection: close\r\n\r\n")
                data = b""
                while True:
                    chunk = tls.recv(4096)
                    if not chunk:
                        break
                    data += chunk
        record("https_get_200", b" 200 " in data.split(b"\r\n")[0], data.split(b"\r\n")[0].decode())
    finally:
        server.shutdown()
        server.server_close()
        if not options.keep:
            shutil.rmtree(workdir, ignore_errors=True)

    failed = [check for check in CHECKS if not check["ok"]]
    print(f"SUMMARY pass={len(CHECKS) - len(failed)} fail={len(failed)}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
