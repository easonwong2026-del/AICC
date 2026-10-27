import Foundation
import Security

/// The bridge's shared secret, kept out of the login keychain.
///
/// It used to live in a `kSecClassGenericPassword` item, which is keyed to the
/// signing identity.  Every ad-hoc rebuild changes cdhash, so the item's ACL no
/// longer matches the binary and `SecItemCopyMatching` blocks forever on an
/// access prompt — a menu-bar-only App (`LSUIElement`) has no window to show
/// that prompt in.  The App then never finishes launching: the supervisor
/// parks inside `startServer()` and the bridge, the widgets and the settings
/// all stay dead until someone deletes the item by hand.
///
/// `kSecUseAuthenticationUIFail` does **not** help for these legacy file-based
/// items: it is honoured for data-protection keychain items only, and the call
/// still blocks.  So the keychain is read at most once, with a hard deadline,
/// and only to carry an existing token forward — never on the launch path after
/// that.  A file in Application Support has no ACL to mismatch and is exactly as
/// private as the data behind it (the server only exposes quota readings).
enum BridgeCredential {
    static var fileURL: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AICC-Dashboard/bridge-token", isDirectory: false)
    }

    static func loadOrCreate() throws -> String {
        if let token = readToken() { return token }
        // Adopt the token an older build stored in the keychain, so e-ink panels
        // already paired keep working.  Bounded: a mismatched ACL would park the
        // caller on a dialog this App cannot show.
        if let legacy = legacyKeychainToken(timeout: .seconds(2)) {
            try write(token: legacy)
            return legacy
        }
        let token = try generateToken()
        try write(token: token)
        return token
    }

    /// Deletes the file and any leftover keychain item from an older build.
    static func revoke() throws {
        try? FileManager.default.removeItem(at: fileURL)
        // Best effort and non-blocking: this only ever runs from the settings
        // window, after the App is already up.
        SecItemDelete([
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.aieink.dashboard.bridge",
            kSecAttrAccount as String: "read-only-display",
        ] as CFDictionary)
    }

    // MARK: - File storage

    private static func readToken() -> String? {
        guard let data = try? Data(contentsOf: fileURL),
              let token = String(data: data, encoding: .utf8) else { return nil }
        let trimmed = token.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func generateToken() throws -> String {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else {
            throw CocoaError(.fileWriteUnknown)
        }
        return Data(bytes).base64EncodedString()
    }

    private static func write(token: String) throws {
        let directory = fileURL.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        guard let data = token.data(using: .utf8) else { throw CocoaError(.fileWriteUnknown) }
        try data.write(to: fileURL, options: .atomic)
        // Owner-only: the token authorises every bridge read.
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: fileURL.path)
    }

    // MARK: - Legacy keychain migration

    /// The pre-2.12.2 keychain token, if the system answers inside `timeout`.
    ///
    /// The lookup runs on its own queue and is abandoned when the deadline
    /// passes.  Abandoning is safe: a blocked thread only waits for a dialog
    /// nobody can see, and it is never retried once the file exists.
    private static func legacyKeychainToken(timeout: DispatchTimeInterval) -> String? {
        let semaphore = DispatchSemaphore(value: 0)
        let box = TokenBox()
        DispatchQueue.global(qos: .utility).async {
            var lookup: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: "com.aieink.dashboard.bridge",
                kSecAttrAccount as String: "read-only-display",
                kSecReturnData as String: true,
            ]
            lookup[kSecUseAuthenticationUI as String] = kSecUseAuthenticationUIFail
            var result: CFTypeRef?
            if SecItemCopyMatching(lookup as CFDictionary, &result) == errSecSuccess,
               let data = result as? Data,
               let token = String(data: data, encoding: .utf8),
               !token.isEmpty {
                box.token = token
            }
            semaphore.signal()
        }
        guard semaphore.wait(timeout: .now() + timeout) == .success else { return nil }
        return box.token
    }

    /// Carries the token out of the abandoned lookup's closure.
    private final class TokenBox: @unchecked Sendable {
        var token: String?
    }
}

