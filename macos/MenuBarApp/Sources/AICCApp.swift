import SwiftUI
import AppKit
import WidgetKit

// MARK: - Server Manager

enum ServerHealthState: String {
    case stopped
    case starting
    case healthy
    case degraded
    case recovering
    case failed
}

enum ServerOwnership {
    case none
    case managed
    case external
}

enum ServerStopReason {
    case none
    case recovery
    case user
    case appExit
}

@MainActor
final class ServerManager: ObservableObject {
    static let shared = ServerManager()

    @Published var isServerRunning = false
    @Published var healthState: ServerHealthState = .stopped
    private var serverProcess: Process?
    private var ownership: ServerOwnership = .none
    private var supervisorTask: Task<Void, Never>?
    private var stopReason: ServerStopReason = .none
    private var restartFailures = 0
    private var failureWindowStarted = Date.distantPast
    private var nextRetryDelay: UInt64 = 1_000_000_000

    func resolveServerRoot() -> URL? {
        if let marker = Bundle.main.url(forResource: "ServerRoot", withExtension: "txt") {
            if let path = try? String(contentsOf: marker, encoding: .utf8)
                .trimmingCharacters(in: .whitespacesAndNewlines) {
                if path.hasPrefix("@resources/") {
                    let relative = String(path.dropFirst("@resources/".count))
                    return Bundle.main.resourceURL?.appendingPathComponent(relative, isDirectory: true)
                }
                if !path.isEmpty {
                    return URL(fileURLWithPath: path, isDirectory: true)
                }
            }
        }
        // Fallback: assume running from project root
        return Bundle.main.bundleURL
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    func startServer() async -> Bool {
        if serverProcess?.isRunning == true {
            let identity = await checkServerIdentity()
            return identity.alive && identity.compatible
        }

        stopReason = .none
        healthState = .starting
        guard let root = resolveServerRoot() else {
            healthState = .degraded
            return false
        }
        let serverScript = root.appendingPathComponent("server.py").path

        guard FileManager.default.fileExists(atPath: serverScript) else {
            healthState = .degraded
            return false
        }

        let identity = await checkServerIdentity()
        if identity.alive {
            if identity.compatible {
                isServerRunning = true
                ownership = .external
                healthState = .healthy
                return true
            } else {
                isServerRunning = false
                ownership = .external
                healthState = .degraded
                return false
            }
        }

        guard let python = resolvePython() else {
            healthState = .degraded
            return false
        }

        let process = Process()
        process.launchPath = python
        process.arguments = ["-B", serverScript]
        process.currentDirectoryPath = root.path

        var env = ProcessInfo.processInfo.environment
        var path = env["PATH"] ?? ""
        path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:\(path)"
        env["PATH"] = path
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        env["EINK_ACCESS_LOG"] = AppSettings.shared.debugMode ? "1" : "0"
        let hostBuild = (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !hostBuild.isEmpty {
            env["AICC_BUILD"] = hostBuild
        }
        if root.path.contains(".app/Contents/Resources/Server") {
            let dataDirectory = URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support/AICC-Dashboard/data", isDirectory: true)
            try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
            env["EINK_DATA_DIR"] = dataDirectory.path
        }
        process.environment = env

        // Set up log files
        let logsDir = AICCPaths.logsDirectory
        try? FileManager.default.createDirectory(at: logsDir, withIntermediateDirectories: true)

        let outLog = logsDir.appendingPathComponent("aicc-server.log")
        FileManager.default.createFile(atPath: outLog.path, contents: nil)
        if let handle = FileHandle(forWritingAtPath: outLog.path) {
            handle.seekToEndOfFile()
            process.standardOutput = handle
        }

        let errLog = logsDir.appendingPathComponent("aicc-server-error.log")
        FileManager.default.createFile(atPath: errLog.path, contents: nil)
        if let handle = FileHandle(forWritingAtPath: errLog.path) {
            handle.seekToEndOfFile()
            process.standardError = handle
        }

        do {
            stopReason = .none
            process.terminationHandler = { [weak self] _ in
                Task { @MainActor [weak self] in
                    self?.handleProcessTermination()
                }
            }
            try process.run()
            serverProcess = process
            ownership = .managed
            isServerRunning = false
            let ready = await waitForServerAlive()
            if ready {
                isServerRunning = true
                healthState = .healthy
                return true
            }
            stopOwnedServer(reason: .recovery)
            healthState = .degraded
            return false
        } catch {
            print("Failed to start server: \(error)")
            healthState = .degraded
            return false
        }
    }

    func startMonitoring() {
        guard supervisorTask == nil else { return }
        supervisorTask = Task { @MainActor [weak self] in
            guard let self else { return }
            _ = await self.startServer()
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 60_000_000_000)
                if Task.isCancelled { break }
                await self.superviseOnce()
            }
        }
    }

    var dataDirectoryURL: URL? {
        guard let root = resolveServerRoot() else { return nil }
        if root.path.contains(".app/Contents/Resources/Server") {
            return URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support/AICC-Dashboard/data", isDirectory: true)
        }
        return root.appendingPathComponent("data", isDirectory: true)
    }

    func restartServer() async -> Bool {
        if ownership == .managed {
            stopOwnedServer(reason: .recovery)
            try? await Task.sleep(nanoseconds: 400_000_000)
        }
        restartFailures = 0
        nextRetryDelay = 1_000_000_000
        return await startServer()
    }

    func stopMonitoring() {
        supervisorTask?.cancel()
        supervisorTask = nil
    }

    private func resolvePython() -> String? {
        let environment = ProcessInfo.processInfo.environment
        let candidates = [
            environment["AICC_PYTHON_PATH"],
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
            "/usr/bin/python3"
        ].compactMap { $0 }
        return candidates.first(where: { FileManager.default.isExecutableFile(atPath: $0) })
    }

    private struct LiveHealthResponse: Decodable {
        let ok: Bool?
        let status: String?
        let version: String?
        let build: String?
    }

    private func checkServerIdentity() async -> (alive: Bool, compatible: Bool) {
        guard let port = ProcessInfo.processInfo.environment["EINK_PORT"] ?? Optional("8765"),
              let url = URL(string: "http://127.0.0.1:\(port)/api/health/live") else {
            return (false, false)
        }
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 1.0
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                return (false, false)
            }
            guard let health = try? JSONDecoder().decode(LiveHealthResponse.self, from: data),
                  health.ok == true else {
                return (true, false)
            }
            let currentVersion = (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let currentBuild = (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)

            let versionMatch = currentVersion == nil || currentVersion?.isEmpty == true || health.version == currentVersion
            let buildMatch = currentBuild != nil && !currentBuild!.isEmpty && health.build == currentBuild

            return (true, versionMatch && buildMatch)
        } catch {
            return (false, false)
        }
    }

    private func waitForServerAlive() async -> Bool {
        for _ in 0..<10 {
            let identity = await checkServerIdentity()
            if identity.alive && identity.compatible { return true }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        return false
    }

    private func superviseOnce() async {
        guard stopReason == .none || stopReason == .recovery else { return }
        let identity = await checkServerIdentity()
        if identity.alive && identity.compatible {
            isServerRunning = true
            healthState = .healthy
            restartFailures = 0
            nextRetryDelay = 1_000_000_000
            return
        }

        if identity.alive && !identity.compatible {
            isServerRunning = false
            healthState = .degraded
            return
        }

        isServerRunning = false
        healthState = .recovering
        if ownership == .managed {
            stopOwnedServer(reason: .recovery)
            try? await Task.sleep(nanoseconds: 400_000_000)
        }

        let now = Date()
        if now.timeIntervalSince(failureWindowStarted) > 300 {
            failureWindowStarted = now
            restartFailures = 0
            nextRetryDelay = 1_000_000_000
        }
        guard restartFailures < 5 else {
            healthState = .failed
            return
        }

        try? await Task.sleep(nanoseconds: nextRetryDelay)
        if await startServer() {
            restartFailures = 0
            nextRetryDelay = 1_000_000_000
        } else {
            restartFailures += 1
            nextRetryDelay = min(nextRetryDelay * 2, 30_000_000_000)
        }
    }

    private func handleProcessTermination() {
        serverProcess = nil
        isServerRunning = false
        ownership = .none
        healthState = stopReason == .appExit || stopReason == .user ? .stopped : .degraded
    }

    private func stopOwnedServer(reason: ServerStopReason, waitForExit: Bool = false) {
        guard ownership == .managed else { return }
        stopReason = reason
        let process = serverProcess
        process?.terminate()
        if waitForExit, let process, process.isRunning {
            let deadline = Date().addingTimeInterval(1.5)
            while process.isRunning && Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }
        }
        serverProcess = nil
        ownership = .none
        isServerRunning = false
        healthState = reason == .recovery ? .recovering : .stopped
    }

    func stopServer(reason: ServerStopReason = .user, waitForExit: Bool = false) {
        stopReason = reason
        stopOwnedServer(reason: reason, waitForExit: waitForExit)
        if ownership == .none {
            healthState = .stopped
        }
    }
}

// MARK: - App Delegate

@MainActor
class AICCAppDelegate: NSObject, NSApplicationDelegate {
    private let singleInstance = SingleInstanceService.shared
    private let serverManager = ServerManager.shared
    private var statusItemController: StatusItemController?
    private var settingsWindowCoordinator: SettingsWindowCoordinator?
    private var hasStartedShutdown = false
    private var terminationGate = AppTerminationGate()

    func applicationWillFinishLaunching(_ notification: Notification) {
        guard singleInstance.acquire() else {
            terminationGate.requestTermination()
            NSApp.terminate(nil)
            return
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        WidgetCenter.shared.reloadAllTimelines()

        settingsWindowCoordinator = SettingsWindowCoordinator(
            api: APIService.shared,
            ocx: OpenCodexController.shared,
            settings: AppSettings.shared,
            server: ServerManager.shared,
            loginAtLaunch: LaunchAtLoginService.shared
        )

        statusItemController = StatusItemController(
            api: APIService.shared,
            settings: AppSettings.shared,
            ocx: OpenCodexController.shared,
            openSettings: { [weak self] in self?.settingsWindowCoordinator?.present() },
            quitApplication: { [weak self] in self?.requestQuit() }
        )

        // Start and supervise the Python server.
        serverManager.startMonitoring()
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            await APIService.shared.fetchStatus()
            APIService.shared.startAutoRefresh(interval: AppSettings.shared.autoRefreshInterval)
        }

        // One low-frequency OpenCodex check at app launch. The controller
        // owns the visible-panel refresh loop and does not poll in the
        // background while the menu is closed.
        Task { @MainActor in
            await OpenCodexController.shared.checkOnce()
        }
    }

    /// AICC is a menu-bar application, so closing its only regular window
    /// (currently the Settings window) must not terminate the process.
    /// Quitting remains an explicit command handled by the status-item menu.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard terminationGate.allowsTermination else {
            return .terminateCancel
        }
        shutDownForTermination()
        return .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        shutDownForTermination()
    }

    private func requestQuit() {
        terminationGate.requestTermination()
        NSApp.terminate(nil)
    }

    private func shutDownForTermination() {
        guard !hasStartedShutdown else { return }
        hasStartedShutdown = true
        APIService.shared.stopAutoRefresh()
        statusItemController?.tearDown()
        statusItemController = nil
        settingsWindowCoordinator?.tearDown()
        settingsWindowCoordinator = nil
        serverManager.stopMonitoring()
        serverManager.stopServer(reason: .appExit, waitForExit: true)
    }
}

// MARK: - AppKit app shell

@main
@MainActor
struct AICCApp {
    static func main() {
        let application = NSApplication.shared
        let delegate = AICCAppDelegate()
        application.delegate = delegate
        application.run()
    }
}
