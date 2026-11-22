import Foundation
import Security

/// The identity of an installed bundle: what it *is*, not where it lives.
///
/// Two copies of AICC can sit in `/Applications` and `~/Applications` with
/// different builds.  Deciding by path order alone would let the repair run
/// `xattr` / `pluginkit` against the wrong (older) copy, so every candidate has
/// to prove it is the same build as the copy that is actually running.
struct InstalledAppIdentity: Equatable {
    let bundleIdentifier: String
    /// The signed cdhash of the main executable.  Changes with every rebuild,
    /// even when the version and build numbers are reused.
    let executableCDHash: String
    /// Kept for log lines only: two different builds never compare equal.
    let version: String
    let build: String

    var summary: String { "\(version)/\(build)@\(executableCDHash.prefix(8))" }

    /// Read the identity straight off the bundle on disk.
    ///
    /// `nil` when the bundle has no readable Info.plist or no valid signature —
    /// in that case it cannot be verified, so nothing may be repaired on it.
    static func read(at bundlePath: String,
                     cdhash: (String) -> String? = InstalledAppIdentity.cdhash) -> InstalledAppIdentity? {
        let plist = (bundlePath as NSString).appendingPathComponent("Contents/Info.plist")
        guard let info = NSDictionary(contentsOfFile: plist) else { return nil }
        let identifier = info["CFBundleIdentifier"] as? String ?? ""
        let version = info["CFBundleShortVersionString"] as? String ?? ""
        let build = info["CFBundleVersion"] as? String ?? ""
        guard !identifier.isEmpty else { return nil }
        guard let hash = cdhash(bundlePath), !hash.isEmpty else { return nil }
        return InstalledAppIdentity(bundleIdentifier: identifier,
                                    executableCDHash: hash,
                                    version: version,
                                    build: build)
    }

    /// cdhash of a signed bundle (the Security framework resolves the main
    /// executable itself when handed the `.app`).
    static func cdhash(_ bundlePath: String) -> String? {
        var code: SecStaticCode?
        let url = URL(fileURLWithPath: bundlePath) as CFURL
        guard SecStaticCodeCreateWithPath(url, [], &code) == errSecSuccess, let code else { return nil }
        var information: CFDictionary?
        guard SecCodeCopySigningInformation(code, SecCSFlags(rawValue: kSecCSSigningInformation), &information) == errSecSuccess,
              let dictionary = information as? [String: Any],
              let unique = dictionary[kSecCodeInfoUnique as String] as? Data,
              !unique.isEmpty else { return nil }
        return unique.map { String(format: "%02x", $0) }.joined()
    }
}

/// Pure path rules for locating the real installation of a translocated App.
///
/// Kept apart from `WidgetHealthService` so the rule is testable without
/// AppKit, a real translocation or a running WidgetKit daemon.
enum WidgetInstallation {
    static let translocationMarker = "/AppTranslocation/"

    /// True when macOS is running this bundle from its random, read-only copy
    /// under `…/T/AppTranslocation/<UUID>/d/`.
    ///
    /// That only happens while the bundle still carries `com.apple.quarantine`,
    /// and an extension inside such a copy is never registered by `chronod`, so
    /// every Desktop widget stays an empty black tile until the App repairs the
    /// real copy and restarts from it.
    static func isTranslocated(bundlePath: String) -> Bool {
        bundlePath.contains(translocationMarker)
    }

    /// Every directory the App may legitimately be installed in, most canonical
    /// first.
    ///
    /// `/Applications` is the DMG's drag target, but `~/Applications` is just as
    /// supported (no admin prompt, and the only workable location on machines
    /// whose `/Applications` is managed).  Looking in one hard-coded directory
    /// meant a translocated launch from `~/Applications` found no installed copy
    /// and quietly gave up, so the widget stayed black until the App was moved.
    static func installRoots(home: String = NSHomeDirectory()) -> [String] {
        ["/Applications", (home as NSString).appendingPathComponent("Applications")]
    }

    /// Every root that actually holds a bundle with the running bundle's name.
    static func candidates(forBundlePath bundlePath: String,
                           roots: [String] = installRoots(),
                           exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> [String] {
        let name = (bundlePath as NSString).lastPathComponent
        guard !name.isEmpty else { return [] }
        return roots
            .map { ($0 as NSString).appendingPathComponent(name) }
            .filter(exists)
    }

    /// The installed copy behind a translocated bundle, **verified by identity**.
    ///
    /// `nil` when the running copy is not translocated, when no root holds a
    /// bundle of that name, or when every candidate is a *different* build —
    /// in the last case repairing would touch the wrong App, so the caller must
    /// do nothing.  Path order only decides which verified candidate wins when
    /// several are byte-identical.
    static func installedPath(forBundlePath bundlePath: String,
                              roots: [String] = installRoots(),
                              identityAt: (String) -> InstalledAppIdentity? = { InstalledAppIdentity.read(at: $0) },
                              exists: (String) -> Bool = { FileManager.default.fileExists(atPath: $0) }) -> String? {
        guard isTranslocated(bundlePath: bundlePath) else { return nil }
        guard let running = identityAt(bundlePath) else { return nil }
        for candidate in candidates(forBundlePath: bundlePath, roots: roots, exists: exists) {
            guard let identity = identityAt(candidate) else { continue }
            if identity.bundleIdentifier == running.bundleIdentifier,
               identity.executableCDHash == running.executableCDHash {
                return candidate
            }
        }
        return nil
    }
}
