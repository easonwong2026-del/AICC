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
        try require(AppThemeMode.allCases.count == 3, "theme modes")
        var settingsLifecycle = SettingsWindowLifecycle()
        try require(settingsLifecycle.beginPresentation(), "settings lifecycle begin")
        try require(settingsLifecycle.state == .presented, "settings lifecycle present")
        settingsLifecycle.close()
        try require(settingsLifecycle.state == .closed, "settings lifecycle close")
        try require(settingsLifecycle.beginPresentation(), "settings lifecycle re-present")
        try require(settingsLifecycle.state == .presented, "settings lifecycle re-presented")

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

        // Provider manifest models: unknown fields must not break decoding and
        // long numbers must stay readable without scientific notation.
        let manifestJSON = """
        {
          "schema_version": 1,
          "unknown_root": {"x": 1},
          "providers": [{
            "id": "workbuddy",
            "display_name": "WorkBuddy",
            "category": "credits",
            "state": "connected",
            "available": true,
            "stale": false,
            "sort_order": 20,
            "capabilities": ["refresh", "reconnect"],
            "unknown_provider_field": "ignored",
            "metrics": [
              {"key": "points", "label": "剩余积分", "value": 5343.37, "value_type": "number", "format": "decimal", "unit": "积分", "primary": true, "unknown_metric_field": true},
              {"key": "used_today", "label": "今日使用", "value": "126", "value_type": "number", "format": "decimal", "unit": "积分", "primary": false},
              {"key": "flag", "label": "Flag", "value": true, "value_type": "status", "format": "plain", "primary": false},
              {"key": "missing", "label": "Missing", "value": null, "value_type": "number", "format": "decimal", "primary": false}
            ],
            "actions": [{"id": "reconnect", "label": "重连", "kind": "reconnect", "local_only": true}]
          }]
        }
        """
        let decoded = try JSONDecoder().decode(ProvidersResponse.self, from: Data(manifestJSON.utf8))
        try require(decoded.providers.count == 1, "manifest provider count")
        let provider = decoded.providers[0]
        try require(provider.primaryMetrics.count == 1, "primary metric")
        try require(provider.metrics[0].value == .number(5343.37), "numeric metric value")
        try require(
            ProviderAPI.actionPath(providerID: "workbuddy", kind: "reconnect")
                == "/api/providers/workbuddy/actions/reconnect",
            "action routes use kind"
        )
        try require(
            ProviderAPI.refreshPath(providerID: "codex") == "/api/providers/codex/refresh",
            "provider refresh route"
        )
        let display = MetricFormatter.format(
            value: provider.metrics[0].value,
            valueType: provider.metrics[0].safeValueType,
            format: provider.metrics[0].safeFormat,
            unit: provider.metrics[0].unit
        )
        try require(display.number == "5,343.37", "long number formatting")
        try require(display.unit == "积分", "metric unit")
        let long = MetricFormatter.format(value: .number(999_999.99), valueType: "number", format: "decimal", unit: nil)
        try require(long.number == "999,999.99", "max-length number formatting")
        try require(MetricFormatter.format(value: .null, valueType: "number", format: "decimal", unit: nil).placeholder, "null placeholder")

        // Explicit JSON null and malformed values must decode to .null, while
        // a missing value key stays nil — both render as placeholders.
        let nullJSON = """
        {"schema_version":1,"providers":[{"id":"x","display_name":"X","state":"connected","available":true,"metrics":[{"key":"m","label":"M","value":null,"value_type":"number","format":"decimal","primary":true}],"actions":[]}]}
        """
        let nullDecoded = try JSONDecoder().decode(ProvidersResponse.self, from: Data(nullJSON.utf8))
        try require(nullDecoded.providers[0].metrics[0].value == .null, "json null decodes to .null")
        let malformedJSON = """
        {"schema_version":1,"providers":[{"id":"x","display_name":"X","state":"connected","available":true,"metrics":[{"key":"m","label":"M","value":{"nested":true},"value_type":"number","format":"decimal","primary":true}],"actions":[]}]}
        """
        let malformedDecoded = try JSONDecoder().decode(ProvidersResponse.self, from: Data(malformedJSON.utf8))
        try require(malformedDecoded.providers[0].metrics[0].value == .null, "malformed value degrades to .null")
        let missingJSON = """
        {"schema_version":1,"providers":[{"id":"x","display_name":"X","state":"connected","available":true,"metrics":[{"key":"m","label":"M","value_type":"number","format":"decimal","primary":true}],"actions":[]}]}
        """
        let missingDecoded = try JSONDecoder().decode(ProvidersResponse.self, from: Data(missingJSON.utf8))
        try require(missingDecoded.providers[0].metrics[0].value == nil, "missing value key stays nil")

        print("AICC Swift core smoke tests passed.")
    }
}
