# Test-only TLS identity

`test-bridge.p12` / `test-bridge-ca.pem` are a throwaway self-signed identity
(CN=127.0.0.1, 10 years) used by `BridgeTlsTest` to run a real TLS handshake on
the JVM. The password is `aicc-test`.

They are **not** production material: nothing outside `src/test` reads them, the
key never ships in the APK, and the bridge's real identity is minted per Mac by
the account worker (see `docs/BRIDGE_TLS.md`).
