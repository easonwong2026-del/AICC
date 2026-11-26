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
/// launch to run from the installed copy, and a self-relaunch mid-launch leaves
/// the menu bar extra half-built.
///
/// **A repair only counts when it worked.**  The build key is written after the
/// two steps that decide whether the extension is loadable — clearing quarantine
/// and re-registering the appex.  If either fails, nothing is recorded and the
/// next launch tries again; marking a half-done repair as done would leave a
/// black tile that never heals.
enum WidgetHealthService {
    private static let lastRepairedBuildKey = "aicc.widget.health.last-build"

    /// Idempotent. Cheap when nothing changed: one defaults read.
    @MainActor
    static func repairIfNeeded(key: String = currentRepairKey()) {
        switch WidgetRepairPolicy.plan(
            runningPath: Bundle.main.bundleURL.path,
            recordedKey: UserDefaults.standard.string(forKey: lastRepairedBuildKey),
            currentKey: key
        ) {
        case .alreadyRepaired:
            return
        case .nothingToRepair(let reason):
            record("nothing to repair: \(reason)")
            return
        case .repair(let path):
            performRepair(installedPath: path,
                          runningPath: Bundle.main.bundleURL.path,
                          key: key)
        }
    }

    @MainActor
    private static func performRepair(installedPath: String, runningPath: String, key: String) {
        let installed = URL(fileURLWithPath: installedPath, isDirectory: true)
        let widget = installed.appendingPathComponent("Contents/PlugIns/AICCWidget.appex", isDirectory: true)
        guard FileManager.default.fileExists(atPath: widget.path) else {
            record("no widget extension inside \(installed.path); will retry if it appears")
            return
        }

        record("repairing \(installed.path) (running from \(runningPath), build \(key))")
        // Recursive: quarantine lands on the App *and* every nested file.
        let cleared = run("/usr/bin/xattr", ["-dr", "com.apple.quarantine", installed.path])
        record("xattr rc=\(cleared)")
        let registered = run("/usr/bin/pluginkit", ["-a", widget.path])
        record("pluginkit rc=\(registered)")
        // chronod caches descriptors per bundle identity, so a re-registration
        // only takes effect after it reloads them.  Failure here is not a
        // reason to give up: the registration itself already succeeded.
        record("chronod rc=\(run("/usr/bin/killall", ["chronod"]))")

        guard WidgetRepairPolicy.marksRepairComplete(clearedQuarantine: cleared,
                                                     registeredExtension: registered) else {
            // Leave the key unwritten on purpose: the next launch retries
            // instead of pretending a black tile is healthy.
            record("repair incomplete (xattr=\(cleared), pluginkit=\(registered)); will retry next launch")
            return
        }
        UserDefaults.standard.set(key, forKey: lastRepairedBuildKey)
        record("repaired build \(key)")
    }

    static func currentRepairKey() -> String {
        let info = Bundle.main.infoDictionary ?? [:]
        let version = info["CFBundleShortVersionString"] as? String ?? ""
        let build = info["CFBundleVersion"] as? String ?? ""
        // Bundle.main may be a translocation copy; its cdhash is identical to the
        // installed one, which is exactly the identity this key is about.
        return WidgetRepairPolicy.key(version: version,
                                      build: build,
                                      cdhash: InstalledAppIdentity.cdhash(Bundle.main.bundleURL.path))
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
            _ = try? handle.seekToEnd()
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
