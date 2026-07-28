import SwiftUI
import AppKit

// MARK: - Server Manager

@MainActor
final class ServerManager: ObservableObject {
    static let shared = ServerManager()

    @Published var isServerRunning = false
    private var serverProcess: Process?

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
        if isServerRunning {
            return true
        }

        guard let root = resolveServerRoot() else { return false }
        let serverScript = root.appendingPathComponent("server.py").path

        guard FileManager.default.fileExists(atPath: serverScript) else { return false }

        if await serverIsHealthy() {
            isServerRunning = true
            return true
        }

        guard let python = resolvePython() else { return false }

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
            process.terminationHandler = { [weak self] _ in
                Task { @MainActor [weak self, process] in
                    guard let self, self.serverProcess === process else { return }
                    self.serverProcess = nil
                    self.isServerRunning = false
                }
            }
            try process.run()
            serverProcess = process
            isServerRunning = true
            return true
        } catch {
            print("Failed to start server: \(error)")
            return false
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
        if serverProcess != nil {
            stopServer()
            try? await Task.sleep(nanoseconds: 400_000_000)
        }
        return await startServer()
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

    private func serverIsHealthy() async -> Bool {
        guard let port = ProcessInfo.processInfo.environment["EINK_PORT"] ?? Optional("8765"),
              let url = URL(string: "http://127.0.0.1:\(port)/api/health") else {
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

    func stopServer() {
        serverProcess?.terminate()
        serverProcess = nil
        isServerRunning = false
    }
}

// MARK: - App Delegate

@MainActor
class AICCAppDelegate: NSObject, NSApplicationDelegate {
    private let serverManager = ServerManager.shared

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)

        // Start the Python server
        Task { @MainActor in
            _ = await self.serverManager.startServer()
            // Give the server a moment to start, then fetch status
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            await APIService.shared.fetchStatus()
            APIService.shared.startAutoRefresh(interval: AppSettings.shared.autoRefreshInterval)
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
        serverManager.stopServer()
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
