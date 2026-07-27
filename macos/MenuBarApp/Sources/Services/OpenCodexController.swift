import AppKit
import Foundation

@MainActor
class OpenCodexController: ObservableObject {
    static let shared = OpenCodexController()

    @Published var status: OCXStatus = .unknown
    @Published var detectedPath: String?
    @Published var isHealthChecking = false

    private let healthURL = "http://127.0.0.1:10100/healthz"
    private let candidates = [
        "/opt/homebrew/bin/ocx",
        "/usr/local/bin/ocx",
        "\(NSHomeDirectory())/.npm-global/bin/ocx"
    ]
    private var healthCheckTask: Task<Void, Never>?
    private let session: URLSession

    init() {
        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 3
        config.timeoutIntervalForResource = 5
        session = URLSession(configuration: config)
        let saved = AppSettings.shared.ocxCustomPath
        if !saved.isEmpty && FileManager.default.isExecutableFile(atPath: saved) {
            detectedPath = saved
        }
    }

    // MARK: - Path Detection

    func detectExecutable() async {
        status = .detecting

        // 1. Try saved custom path
        let custom = AppSettings.shared.ocxCustomPath
        if !custom.isEmpty && FileManager.default.isExecutableFile(atPath: custom) {
            detectedPath = custom
            await refreshStatus()
            return
        }

        // 2. Try shell command detection (mirrors terminal PATH)
        if let path = await detectViaShell() {
            detectedPath = path
            AppSettings.shared.ocxCustomPath = path
            await refreshStatus()
            return
        }

        // 3. Try known paths
        for candidate in candidates {
            if FileManager.default.isExecutableFile(atPath: candidate) {
                detectedPath = candidate
                AppSettings.shared.ocxCustomPath = candidate
                await refreshStatus()
                return
            }
        }

        detectedPath = nil
        status = .notFound
    }

    private func detectViaShell() async -> String? {
        await withCheckedContinuation { continuation in
            let process = Process()
            process.launchPath = "/bin/zsh"
            process.arguments = ["-lc", "command -v ocx"]
            process.standardOutput = Pipe()
            process.standardError = Pipe()

            process.terminationHandler = { proc in
                guard proc.terminationStatus == 0,
                      let data = try? (proc.standardOutput as? Pipe)?.fileHandleForReading.readToEnd(),
                      let path = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
                      !path.isEmpty else {
                    continuation.resume(returning: nil)
                    return
                }
                continuation.resume(returning: path)
            }

            do {
                try process.run()
            } catch {
                continuation.resume(returning: nil)
            }
        }
    }

    // MARK: - Status

    func refreshStatus() async {
        isHealthChecking = true
        defer { isHealthChecking = false }

        guard let url = URL(string: healthURL) else {
            status = .error("Invalid health URL")
            return
        }

        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData

        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                status = .stopped
                return
            }
            if http.statusCode == 200 {
                _ = String(data: data, encoding: .utf8) ?? ""
                status = .running
            } else {
                status = .error("Health returned \(http.statusCode)")
            }
        } catch {
            let code = (error as NSError).code
            if code == NSURLErrorCannotConnectToHost || code == NSURLErrorTimedOut {
                status = .stopped
            } else {
                status = .error(error.localizedDescription)
            }
        }
    }

    // MARK: - Lifecycle

    func ensure() async {
        if detectedPath == nil {
            await detectExecutable()
        }
        guard let path = detectedPath else {
            status = .notFound
            return
        }

        // Already running?
        await refreshStatus()
        if case .running = status { return }

        status = .starting
        await runOCX(command: "ensure", path: path)
        // Wait briefly then check
        try? await Task.sleep(nanoseconds: 2_000_000_000)
        await refreshStatus()
    }

    func stop() async {
        guard let path = detectedPath else { return }
        status = .stopping
        await runOCX(command: "stop", path: path)
        try? await Task.sleep(nanoseconds: 500_000_000)
        await refreshStatus()
    }

    func restart() async {
        await stop()
        try? await Task.sleep(nanoseconds: 500_000_000)
        await ensure()
    }

    func openDashboard() {
        guard let url = URL(string: AppSettings.shared.ocxServiceAddress) else { return }
        NSWorkspace.shared.open(url)
    }

    // MARK: - Private

    private func runOCX(command: String, path: String) async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let process = Process()
            process.launchPath = path
            process.arguments = [command]
            process.standardOutput = Pipe()
            process.standardError = Pipe()

            var env = ProcessInfo.processInfo.environment
            var procPath = env["PATH"] ?? ""
            procPath = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:\(procPath)"
            env["PATH"] = procPath
            process.environment = env

            process.terminationHandler = { _ in
                continuation.resume()
            }

            do {
                try process.run()
            } catch {
                continuation.resume()
            }
        }
    }
}
