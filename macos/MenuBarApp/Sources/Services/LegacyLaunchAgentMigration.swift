import Foundation
import Darwin

/// Removes the LaunchAgents installed by pre-DMG AICC releases.
///
/// This is intentionally a one-purpose migration, not a general service
/// manager. It only touches the three exact labels and plist paths that the
/// retired installer owned. A failed unload is left in place so the app never
/// guesses at another process or service; ServerManager will then keep its
/// normal port-ownership safety check.
enum LegacyLaunchAgentMigration {
    typealias Launchctl = ([String]) -> Bool

    private struct LegacyService {
        let label: String
        let plistName: String
    }

    private static let services = [
        LegacyService(label: "com.aieink.dashboard", plistName: "com.aieink.dashboard.plist"),
        LegacyService(label: "com.aieink.workbuddy-monitor", plistName: "com.aieink.workbuddy-monitor.plist"),
        LegacyService(label: "com.aieink.log-maintenance", plistName: "com.aieink.log-maintenance.plist"),
    ]

    static func run(
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
        launchctl: @escaping Launchctl = LegacyLaunchAgentMigration.runLaunchctl
    ) {
        let fileManager = FileManager.default
        let launchAgents = homeDirectory.appendingPathComponent("Library/LaunchAgents", isDirectory: true)
        let guiDomain = "gui/\(getuid())"

        for service in services {
            let plist = launchAgents.appendingPathComponent(service.plistName)
            let target = "\(guiDomain)/\(service.label)"
            let plistExists = fileManager.fileExists(atPath: plist.path)
            let isLoaded = launchctl(["print", target])

            guard plistExists || isLoaded else { continue }

            // Try the exact loaded service first, then the exact plist path
            // used by the old installer. Both commands are scoped to the
            // current user's GUI domain and cannot match an unrelated job.
            _ = launchctl(["bootout", target]) || launchctl(["bootout", guiDomain, plist.path])

            // Do not remove a registration while launchd still reports the
            // exact old job as loaded. This keeps a failed migration safe and
            // makes a later App launch able to retry it.
            if launchctl(["print", target]) {
                print("AICC: could not unload legacy LaunchAgent \(service.label); leaving it for a later migration attempt.")
                continue
            }

            do {
                if fileManager.fileExists(atPath: plist.path) {
                    try fileManager.removeItem(at: plist)
                }
            } catch {
                print("AICC: could not remove legacy LaunchAgent plist \(plist.path): \(error.localizedDescription)")
            }
        }
    }

    private static func runLaunchctl(_ arguments: [String]) -> Bool {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
            process.waitUntilExit()
            return process.terminationStatus == 0
        } catch {
            return false
        }
    }
}
