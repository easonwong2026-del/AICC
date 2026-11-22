import AppKit
import Foundation

/// Keeps the shipped Widget extension loadable after an upgrade.
///
/// Three things break it, all outside the extension's own code:
///
/// 1. **Quarantine.** A downloaded DMG marks the App and every nested bundle
///    with `com.apple.quarantine`.  Gatekeeper evaluates an XPC extension on its
///    own, so the host App opens while `AICCWidget.appex` is still refused.
/// 2. **App Translocation.** While quarantine is set, macOS runs the App from a
///    random, read-only copy under `…/T/AppTranslocation/<UUID>/d/`.  WidgetKit
///    never registers an extension that lives there.
/// 3. **A stale registration.** `pluginkit` caches the extension by cdhash, and
///    an ad-hoc signature changes cdhash on every build.  Until it is registered
///    again and chronod reloads, Desktop keeps drawing the previous build's
///    snapshot — or nothing at all once that build is gone.
///
/// Whichever applies, the symptom is the same: a rounded rectangle with **no
/// text at all**.  (With no *data* the widget still draws "未连接 Magpie", so a
/// featureless black tile always means nothing ran.)
///
/// The host App repairs all three on the first launch of each build, because no
/// amount of reinstalling makes a user run `xattr` and `killall` by hand.  It
/// does not restart itself: clearing the quarantine flag is enough for the next
/// launch to run from `/Applications`, and a self-relaunch mid-launch leaves the
/// menu bar extra half-built.
enum WidgetHealthService {
    private static let lastRepairedBuildKey = "aicc.widget.health.last-build"

    /// Idempotent. Cheap when nothing changed: one defaults read.
    @MainActor
    static func repairIfNeeded(build: String = currentBuild()) {
        guard !build.isEmpty else { return }
        guard UserDefaults.standard.string(forKey: lastRepairedBuildKey) != build else { return }

        let running = Bundle.main.bundleURL.path
        // Always heal the installed copy, never the translocated one: that copy
        // is read-only, so registering from there can never work.
        let installed: URL
        if let found = WidgetInstallation.installedPath(forBundlePath: running) {
            installed = URL(fileURLWithPath: found, isDirectory: true)
        } else if WidgetInstallation.isTranslocated(bundlePath: running) {
            // Translocated with no installed copy under any known root: the App
            // was launched straight out of the DMG / a random folder. Nothing
            // here can be repaired, and running `xattr` on the read-only copy
            // would only fail. Say so instead of leaving a silent black tile.
            record("translocated with no installed copy; looked in "
                   + WidgetInstallation.installRoots().joined(separator: ", ")
                   + " (running from \(running))")
            return
        } else {
            installed = Bundle.main.bundleURL
        }
        let widget = installed.appendingPathComponent("Contents/PlugIns/AICCWidget.appex", isDirectory: true)
        guard FileManager.default.fileExists(atPath: widget.path) else { return }

        record("repairing \(installed.path) (running from \(running))")
        // Recursive: quarantine lands on the App *and* every nested file.
        let cleared = run("/usr/bin/xattr", ["-dr", "com.apple.quarantine", installed.path])
        record("xattr rc=\(cleared)")
        if cleared != 0 {
            // Almost always "Operation not permitted" on a read-only location.
            // The widget will stay a black tile, and the fix is about the copy
            // the user actually installed, not this process.
            record("could not clear quarantine on \(installed.path); widget may stay blank")
        }
        record("pluginkit rc=\(run("/usr/bin/pluginkit", ["-a", widget.path]))")
        // chronod caches descriptors per bundle identity, so a re-registration
        // only takes effect after it reloads them.
        record("chronod rc=\(run("/usr/bin/killall", ["chronod"]))")
        UserDefaults.standard.set(build, forKey: lastRepairedBuildKey)
        record("repaired build \(build)")
    }

    /// Version *and* build: an ad-hoc rebuild can reuse a build number.
    static func currentBuild() -> String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? ""
        let build = info["CFBundleVersion"] as? String ?? ""
        return version.isEmpty ? "" : "\(version)/\(build)"
    }

    /// A one-line trail beside the server logs.
    ///
    /// `NSLog` is invisible in a shipped App and this runs before any window
    /// exists, so a repair that silently failed would otherwise leave no
    /// evidence at all.
    private static func record(_ message: String) {
        NSLog("AICC widget health: %@", message)
        let url = AICCPaths.logsDirectory.appendingPathComponent("widget-health.log")
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        let line = "\(ISO8601DateFormatter().string(from: .now)) \(message)\n"
        guard let data = line.data(using: .utf8) else { return }
        if let handle = try? FileHandle(forWritingTo: url) {
            defer { try? handle.close() }
            try? handle.seekToEnd()
            try? handle.write(contentsOf: data)
        } else {
            try? data.write(to: url)
        }
    }

    @discardableResult
    private static func run(_ launchPath: String, _ arguments: [String]) -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: launchPath)
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            NSLog("AICC could not run %@: %@", launchPath, error.localizedDescription)
            return -1
        }
        process.waitUntilExit()
        return process.terminationStatus
    }
}
