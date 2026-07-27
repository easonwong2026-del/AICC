import AppKit

@MainActor
class CodexLaunchMonitor: ObservableObject {
    static let shared = CodexLaunchMonitor()

    @Published var isCodexRunning = false

    private var runningAppsObserver: NSKeyValueObservation?

    func startMonitoring() {
        // Monitor Codex Desktop app launch/terminate
        runningAppsObserver = NSWorkspace.shared.observe(\.runningApplications, options: [.initial, .new]) { [weak self] _, _ in
            Task { @MainActor in
                self?.checkCodexRunning()
            }
        }

        // Also listen for app launch notifications
        NSWorkspace.shared.notificationCenter.addObserver(
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
        runningAppsObserver?.invalidate()
        runningAppsObserver = nil
        NSWorkspace.shared.notificationCenter.removeObserver(self)
    }

    func openCodex() {
        Task {
            let settings = AppSettings.shared
            if settings.ocxAutoStart {
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
            }
            // Launch Codex Desktop
            if let codexURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.openai.codex") ??
                               NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.openai.chatgpt") {
                
                _ = try? await NSWorkspace.shared.openApplication(at: codexURL, configuration: NSWorkspace.OpenConfiguration())
            }
        }
    }

    private func checkCodexRunning() {
        let apps = NSWorkspace.shared.runningApplications
        isCodexRunning = apps.contains { app in
            app.bundleIdentifier == "com.openai.codex" || app.bundleIdentifier == "com.openai.chatgpt"
        }
    }

    private func handleCodexLaunched() {
        isCodexRunning = true
        guard AppSettings.shared.ocxAutoStart else { return }

        Task {
            let ocx = OpenCodexController.shared
            if !ocx.status.isRunning {
                await ocx.ensure()
            }
        }
    }
}
