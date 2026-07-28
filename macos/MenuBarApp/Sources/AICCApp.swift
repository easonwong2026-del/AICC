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
        if root.path.contains(".app/Contents/Resources/Server") {
            let dataDirectory = URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support/AICC-Dashboard/data", isDirectory: true)
            try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
            env["EINK_DATA_DIR"] = dataDirectory.path
        }
        process.environment = env

        // Set up log files
        let logsDir = NSHomeDirectory() + "/Library/Logs/AICC-Dashboard"
        try? FileManager.default.createDirectory(atPath: logsDir,
                                                  withIntermediateDirectories: true)

        let outLog = logsDir + "/aicc-server.log"
        FileManager.default.createFile(atPath: outLog, contents: nil)
        if let handle = FileHandle(forWritingAtPath: outLog) {
            handle.seekToEndOfFile()
            process.standardOutput = handle
        }

        let errLog = logsDir + "/aicc-server-error.log"
        FileManager.default.createFile(atPath: errLog, contents: nil)
        if let handle = FileHandle(forWritingAtPath: errLog) {
            handle.seekToEndOfFile()
            process.standardError = handle
        }

        do {
            try process.run()
            serverProcess = process
            isServerRunning = true
            return true
        } catch {
            print("Failed to start server: \(error)")
            return false
        }
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
    private var settingsWindow: NSWindow?
    private let serverManager = ServerManager.shared

    func applicationDidFinishLaunching(_ notification: Notification) {
        NotificationCenter.default.addObserver(forName: .showAICCSettings, object: nil, queue: .main) { [weak self] _ in Task { @MainActor in self?.showSettingsWindow() } }
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

    @MainActor @objc func showSettingsWindow() {
        if let existing = settingsWindow, existing.isVisible {
            existing.makeKeyAndOrderFront(nil)
            return
        }

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 500, height: 420),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "AICC Settings"
        window.titlebarAppearsTransparent = false
        window.center()
        window.isReleasedWhenClosed = false

        let settingsView = SettingsView()
            .environmentObject(AppSettings.shared)
            .environmentObject(OpenCodexController.shared)
            .environmentObject(APIService.shared)

        let hostingView = NSHostingView(rootView: settingsView)
        window.contentView = hostingView
        settingsWindow = window
        window.makeKeyAndOrderFront(nil)
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

    var body: some Scene {
        MenuBarExtra {
            DashboardView()
                .environmentObject(api)
                .environmentObject(ocx)
                .environmentObject(settings)
                .environmentObject(monitor)
        } label: {
            MenuBarStatusLabel(status: api.status)
        }
        .menuBarExtraStyle(.window)

    }
}

extension Notification.Name {
    static let showAICCSettings = Notification.Name("showAICCSettings")
}
