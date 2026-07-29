import SwiftUI
import AppKit

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
            return await serverIsAlive()
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

        if await serverIsAlive() {
            isServerRunning = true
            ownership = .external
            healthState = .healthy
            return true
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

    private func serverIsAlive() async -> Bool {
        guard let port = ProcessInfo.processInfo.environment["EINK_PORT"] ?? Optional("8765"),
              let url = URL(string: "http://127.0.0.1:\(port)/api/health/live") else {
            return false
        }
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 1.0
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            return (response as? HTTPURLResponse)?.statusCode == 200
        } catch {
            return false
        }
    }

    private func waitForServerAlive() async -> Bool {
        for _ in 0..<10 {
            if await serverIsAlive() { return true }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        return false
    }

    private func superviseOnce() async {
        guard stopReason == .none || stopReason == .recovery else { return }
        if await serverIsAlive() {
            isServerRunning = true
            healthState = .healthy
            restartFailures = 0
            nextRetryDelay = 1_000_000_000
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

    private func stopOwnedServer(reason: ServerStopReason) {
        guard ownership == .managed else { return }
        stopReason = reason
        serverProcess?.terminate()
        serverProcess = nil
        ownership = .none
        isServerRunning = false
        healthState = reason == .recovery ? .recovering : .stopped
    }

    func stopServer(reason: ServerStopReason = .user) {
        stopReason = reason
        stopOwnedServer(reason: reason)
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

    func applicationWillFinishLaunching(_ notification: Notification) {
        guard singleInstance.acquire() else {
            NSApp.terminate(nil)
            return
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        // Start and supervise the Python server.
        serverManager.startMonitoring()
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            await APIService.shared.fetchStatus()
            await APIService.shared.fetchHealth()
            APIService.shared.startAutoRefresh(interval: AppSettings.shared.autoRefreshInterval)
            APIService.shared.startHealthRefresh()
        }

        // Start Codex monitoring
        CodexLaunchMonitor.shared.startMonitoring()

        // Detect OpenCodex
        Task {
            await OpenCodexController.shared.detectExecutable()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        APIService.shared.stopAutoRefresh()
        CodexLaunchMonitor.shared.stopMonitoring()
        serverManager.stopMonitoring()
        serverManager.stopServer(reason: .appExit)
    }
}

// MARK: - App

@main
struct AICCApp: App {
    @NSApplicationDelegateAdaptor(AICCAppDelegate.self) var appDelegate
    @StateObject private var api = APIService.shared
    @StateObject private var ocx = OpenCodexController.shared
    @StateObject private var settings = AppSettings.shared
    @StateObject private var monitor = CodexLaunchMonitor.shared
    @StateObject private var server = ServerManager.shared
    @StateObject private var loginAtLaunch = LaunchAtLoginService.shared

    var body: some Scene {
        MenuBarExtra {
            DashboardView()
                .environmentObject(api)
                .environmentObject(ocx)
                .environmentObject(settings)
                .environmentObject(monitor)
                .environment(\.locale, settings.locale)
                .preferredColorScheme(settings.preferredColorScheme)
        } label: {
            MenuBarStatusLabel(
                status: api.status,
                showCodexStatus: settings.menuBarShowCodexStatus,
                showBalance: settings.menuBarShowCodexBalance
            )
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environmentObject(api)
                .environmentObject(ocx)
                .environmentObject(settings)
                .environmentObject(monitor)
                .environmentObject(server)
                .environmentObject(loginAtLaunch)
                .environment(\.locale, settings.locale)
                .preferredColorScheme(settings.preferredColorScheme)
        }

    }
}
