import AppKit

@MainActor
class CodexLaunchMonitor: ObservableObject {
    static let shared = CodexLaunchMonitor()

    private var launchObserver: NSObjectProtocol?

    func startMonitoring() {
        stopMonitoring()

        // Also listen for app launch notifications
        launchObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didLaunchApplicationNotification,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication,
                  app.bundleIdentifier == "com.openai.codex" || app.bundleIdentifier == "com.openai.chatgpt" else {
                return
            }
            Task { @MainActor in
                self?.handleCodexLaunched()
            }
        }
    }

    func stopMonitoring() {
        if let launchObserver {
            NSWorkspace.shared.notificationCenter.removeObserver(launchObserver)
            self.launchObserver = nil
        }
    }

    func openCodex() {
        Task {
            let settings = AppSettings.shared
            var proxyReady = true
            if settings.ocxAutoStart {
                proxyReady = false
                let ocx = OpenCodexController.shared
                if !ocx.status.isRunning {
                    await ocx.ensure()
                    // Wait for healthz to be ready
                    for _ in 0..<10 {
                        if case .running = ocx.status { break }
                        try? await Task.sleep(nanoseconds: 1_000_000_000)
                        await ocx.refreshStatus()
                    }
                }
                proxyReady = ocx.status.isRunning
            }
            guard proxyReady else { return }
            // Launch Codex Desktop
            if let codexURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.openai.codex") ??
                               NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.openai.chatgpt") {
                
                _ = try? await NSWorkspace.shared.openApplication(at: codexURL, configuration: NSWorkspace.OpenConfiguration())
            }
        }
    }

    private func handleCodexLaunched() {
        guard AppSettings.shared.ocxAutoStart else { return }

        Task {
            let ocx = OpenCodexController.shared
            if !ocx.status.isRunning {
                await ocx.ensure()
            }
        }
    }
}
