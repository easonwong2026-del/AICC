import Foundation

enum CoreSmokeError: Error {
    case assertion(String)
}

@main
struct CoreSmokeMain {
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw CoreSmokeError.assertion(message) }
    }

    static func main() async throws {
        let fixtureRoot = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)

        try require(
            StatusItemMenuCommand.allCases.map(\.rawValue) == [
                "Open AICC Dashboard",
                "Refresh All Now",
                "Settings…",
                "Quit AICC"
            ],
            "status item menu contract"
        )

        let displayState = StatusItemDisplayState(
            remaining: 77,
            showStatus: true,
            showBalance: true,
            languageCode: "en",
            themeMode: "system"
        )
        try require(
            displayState == StatusItemDisplayState(
                remaining: 77,
                showStatus: true,
                showBalance: true,
                languageCode: "en",
                themeMode: "system"
            ),
            "status display state duplicate"
        )
        try require(AppLanguage.english.localizationIdentifier == "en", "english locale")
        try require(AppLanguage.simplifiedChinese.localizationIdentifier == "zh-Hans", "Chinese locale")
        try require(
            AppLanguage.english.pickerDisplayName(localize: { _ in "跟随系统" }) == "English",
            "English native language name"
        )
        try require(
            AppLanguage.simplifiedChinese.pickerDisplayName(localize: { _ in "System Default" }) == "简体中文",
            "Chinese native language name"
        )
        try require(SemanticVersion("2.5.9")! < SemanticVersion("2.5.10")!, "semantic version ordering")
        try require(SemanticVersion("2.5.1-beta")! < SemanticVersion("2.5.1")!, "prerelease ordering")
        try require(AppThemeMode.allCases.count == 3, "theme modes")

        let defaults = UserDefaults.standard
        let systemSettingKey = "menuBarShowSystem"
        let previousSystemSetting = defaults.object(forKey: systemSettingKey)
        defer {
            if let previousSystemSetting {
                defaults.set(previousSystemSetting, forKey: systemSettingKey)
            } else {
                defaults.removeObject(forKey: systemSettingKey)
            }
        }
        defaults.removeObject(forKey: systemSettingKey)
        let settings = AppSettings.shared
        try require(settings.menuBarShowSystem, "system menu bar setting defaults on")
        settings.menuBarShowSystem = false
        try require(
            defaults.object(forKey: systemSettingKey) as? Bool == false,
            "system menu bar setting persists off"
        )
        settings.menuBarShowSystem = true
        try require(defaults.bool(forKey: systemSettingKey), "system menu bar setting reads back on")

        var terminationGate = AppTerminationGate()
        try require(!terminationGate.allowsTermination, "implicit termination is blocked")
        terminationGate.requestTermination()
        try require(terminationGate.allowsTermination, "explicit termination is allowed")

        func fixture(_ name: String) throws -> Data {
            try Data(contentsOf: fixtureRoot.appendingPathComponent(name))
        }

        let running = try OCXSnapshot(jsonData: fixture("ocx-running.json"))
        try require(running.resolvedStatus == .running, "running fixture")
        try require(running.port == 12_001, "dynamic port")
        try require(running.dashboardURL?.port == 12_001, "dashboard URL")

        let unhealthy = try OCXSnapshot(jsonData: fixture("ocx-running-missing-health.json"))
        try require(unhealthy.resolvedStatus == .unhealthy, "missing health")
        let stopped = try OCXSnapshot(jsonData: fixture("ocx-stopped.json"))
        try require(stopped.resolvedStatus == .stopped, "stopped fixture")
        do {
            _ = try OCXSnapshot(jsonData: fixture("ocx-bad-json.json"))
            throw CoreSmokeError.assertion("bad JSON accepted")
        } catch is OCXSnapshotError {
            // Expected.
        }

        let runner = ProcessRunner()
        let output = try await runner.run(
            executable: "/bin/sh",
            arguments: ["-c", "printf stdout; printf stderr 1>&2"],
            timeout: 2
        )
        try require(output.stdout == "stdout", "stdout capture")
        try require(output.stderr == "stderr", "stderr capture")

        do {
            _ = try await runner.run(
                executable: "/bin/sleep",
                arguments: ["5"],
                timeout: 0.1
            )
            throw CoreSmokeError.assertion("timeout did not fire")
        } catch let error as ProcessRunnerError {
            guard case .timedOut = error else { throw error }
        }

        print("AICC Swift core smoke tests passed.")
    }
}
