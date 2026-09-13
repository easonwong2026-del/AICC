import Foundation

@main
struct WidgetStatusSmokeMain {
    enum Failure: Error {
        case assertion(String)
    }

    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw Failure.assertion(message) }
    }

    static func main() async throws {
        let fullPayload = try decode(
            """
            {
              "codex": {
                "five_hour": { "remaining": 87, "reset": "14:27" },
                "weekly": { "remaining": 83, "reset": "2026-09-04 08:01" }
              },
              "workbuddy": {
                "points": 5760
              },
              "deepseek": {
                "status": "Online",
                "balances": [{ "currency": "CNY", "total_balance": "58.70" }]
              },
              "updated_at": "2026-08-29 12:00"
            }
            """
        )
        let fetchedAt = Date.now
        let snapshot = WidgetDisplaySnapshot(payload: fullPayload, fetchedAt: fetchedAt)

        // Google uses source time, independent windows and backward-compatible snapshots.
        let googleSource = Date(timeIntervalSince1970: 1788000000)
        let googlePayload = try decode(#"{"google":{"weekly":{"remaining":12,"reset":"2026-09-18 08:00"},"five_hour":{"remaining":0,"reset":"2026-09-12 13:00"},"updated_epoch":1788000000}}"#)
        let google = WidgetDisplaySnapshot(payload: googlePayload, fetchedAt: googleSource)
        try require(google.google?.number == "12", "Google primary is weekly")
        try require(google.google?.secondary?.remaining == 0, "5h zero remains visible")
        try require(google.google?.reset == "2026-09-18 08:00", "Weekly reset stays paired")
        try require(google.google?.five_hour?.reset == "2026-09-12 13:00", "5h keeps its own reset")
        try require(google.googleState == "live", "Fresh Google source")
        let roundTrip = try JSONDecoder().decode(WidgetDisplaySnapshot.self, from: JSONEncoder().encode(google))
        try require(roundTrip == google, "Google survives widget cache round trip")
        for weekly in ["null", #"{"remaining":-1,"reset":"wrong"}"#, #"{"reset":"wrong"}"#] {
            let payload = try decode("{\"google\":{\"weekly\":\(weekly),\"five_hour\":{\"remaining\":80,\"reset\":\"13:00\"},\"updated_epoch\":1788000000}}")
            let fallback = WidgetDisplaySnapshot(payload: payload, fetchedAt: googleSource)
            try require(fallback.google?.title == "Google 5h", "Explicit 5h fallback")
            try require(fallback.google?.reset == "13:00", "Fallback reset must be from 5h")
            try require(fallback.google?.secondary == nil, "No duplicated secondary 5h")
        }
        let missing = WidgetDisplaySnapshot(payload: try decode(#"{"google":{}}"#), fetchedAt: googleSource)
        try require(missing.google?.number == "—" && missing.googleState == "unavailable", "Missing is not zero")
        let weeklyOnly = WidgetDisplaySnapshot(payload: try decode(#"{"google":{"weekly":{"remaining":0}}}"#), fetchedAt: googleSource)
        try require(weeklyOnly.google?.number == "0" && weeklyOnly.google?.secondary == nil, "Weekly only zero")
        try require(weeklyOnly.googleState == "stale", "Unknown upstream timestamp is not fresh")
        try require(google.evaluated(at: googleSource.addingTimeInterval(300)).googleState == "stale", "Google expires to stale")
        try require(google.evaluated(at: googleSource, offline: true).googleState == "stale", "Offline Google cache")
        var old = try JSONSerialization.jsonObject(with: JSONEncoder().encode(google)) as! [String: Any]
        old["fetchedAt"] = googleSource.addingTimeInterval(86400).timeIntervalSinceReferenceDate
        let recentlyFetched = try JSONDecoder().decode(WidgetDisplaySnapshot.self, from: JSONSerialization.data(withJSONObject: old))
        try require(recentlyFetched.evaluated(at: googleSource.addingTimeInterval(86400)).google == nil, "Fresh transport cannot extend Google source lifetime")
        old.removeValue(forKey: "google")
        let oldDecoded = try JSONDecoder().decode(WidgetDisplaySnapshot.self, from: JSONSerialization.data(withJSONObject: old))
        try require(oldDecoded.google == nil, "Old caches decode without Google")
        let mixed: [String: Any] = ["display_snapshot": old, "google": ["weekly": ["remaining": 77], "updated_epoch": googleSource.timeIntervalSince1970]]
        let mixedPayload = try JSONDecoder().decode(WidgetStatusPayload.self, from: JSONSerialization.data(withJSONObject: mixed))
        try require(WidgetDisplaySnapshot(payload: mixedPayload, fetchedAt: googleSource).google?.number == "77", "Old shared snapshot cannot erase new Google payload")

        // MARK: - 1. Codex Tests
        try require(snapshot.codexTitle == "Codex 每周额度", "Codex title when weekly is present: \(snapshot.codexTitle)")
        try require(snapshot.codexWeeklyNumber == "83", "Codex weekly number: \(snapshot.codexWeeklyNumber)")
        try require(snapshot.codexWeeklyRemaining == 83, "Codex weekly remaining: \(String(describing: snapshot.codexWeeklyRemaining))")
        try require(abs(snapshot.codexWeeklyProgress - 0.83) < 0.001, "Codex weekly progress is 0.83: \(snapshot.codexWeeklyProgress)")
        try require(snapshot.codexFiveHourRemaining == 87, "Codex five-hour remaining: \(String(describing: snapshot.codexFiveHourRemaining))")
        try require(snapshot.codexSecondaryFiveHourRemaining == 87, "Secondary five-hour remaining when weekly is present")
        try require(snapshot.codexResetText == "重置于 2026-09-04 08:01", "Codex reset text: \(String(describing: snapshot.codexResetText))")
        try require(snapshot.codexResetShortText == "重置于 09-04 08:01", "Codex shortened reset text: \(String(describing: snapshot.codexResetShortText))")

        // Reset parsing variants
        let alreadyPrefixed = WidgetDisplaySnapshot(
            payload: try decode(#"{"codex": {"weekly": {"remaining": 50, "reset": "重置于 2026-09-04 08:01"}}}"#),
            fetchedAt: fetchedAt
        )
        try require(alreadyPrefixed.codexResetText == "重置于 2026-09-04 08:01", "Does not duplicate 重置于 prefix")
        try require(alreadyPrefixed.codexResetShortText == "重置于 09-04 08:01", "Shortens already-prefixed reset date")

        let invalidReset = WidgetDisplaySnapshot(
            payload: try decode(#"{"codex": {"weekly": {"remaining": 50, "reset": "--"}}}"#),
            fetchedAt: fetchedAt
        )
        try require(invalidReset.codexResetText == nil, "Invalid reset string '--' returns nil")
        try require(invalidReset.codexResetShortText == nil, "Invalid reset string short returns nil")

        let emptyReset = WidgetDisplaySnapshot(
            payload: try decode(#"{"codex": {"weekly": {"remaining": 50, "reset": "   "}}}"#),
            fetchedAt: fetchedAt
        )
        try require(emptyReset.codexResetText == nil, "Empty reset string returns nil")

        // Fallback to five_hour when weekly is missing
        let fiveHourOnly = WidgetDisplaySnapshot(
            payload: try decode(#"{"codex": {"five_hour": {"remaining": 88, "reset": "14:00"}}}"#),
            fetchedAt: fetchedAt
        )
        try require(fiveHourOnly.codexTitle == "Codex 5小时额度", "Codex title when only five_hour is present: \(fiveHourOnly.codexTitle)")
        try require(fiveHourOnly.codexWeeklyNumber == "88", "Codex falls back to five_hour number")
        try require(fiveHourOnly.codexFiveHourRemaining == 88, "Codex falls back to five_hour remaining")
        try require(fiveHourOnly.codexResetText == "重置于 14:00", "Codex falls back to five_hour reset")
        try require(fiveHourOnly.codexResetShortText == "重置于 14:00", "Preserves time-only reset string")
        try require(fiveHourOnly.codexSecondaryFiveHourRemaining == nil, "Does not duplicate five_hour as secondary when it is primary")

        // Nil codex
        let missingCodex = WidgetDisplaySnapshot(
            payload: try decode(#"{"workbuddy": {"points": 10}}"#),
            fetchedAt: fetchedAt
        )
        try require(missingCodex.codexTitle == "Codex 额度", "Codex title when no codex data: \(missingCodex.codexTitle)")
        try require(missingCodex.codexWeeklyNumber == "—", "Missing codex number is —")
        try require(missingCodex.codexWeeklyRemaining == nil, "Missing codex remaining is nil")
        try require(missingCodex.codexWeeklyProgress == 0.0, "Missing codex progress is 0")
        try require(missingCodex.codexResetText == nil, "Missing codex reset is nil")

        // MARK: - 2. WorkBuddy Tests
        try require(snapshot.workbuddyPointsText == "5,760", "WorkBuddy points with grouping separator: \(snapshot.workbuddyPointsText)")
        try require(snapshot.workbuddyPoints == 5760, "WorkBuddy points: \(String(describing: snapshot.workbuddyPoints))")
        try require(snapshot.workbuddyIsOnline, "WorkBuddy is online")

        let missingWb = WidgetDisplaySnapshot(
            payload: try decode(#"{}"#),
            fetchedAt: fetchedAt
        )
        try require(missingWb.workbuddyPointsText == "—", "Missing WorkBuddy points is —")
        try require(missingWb.workbuddyPoints == nil, "Missing WorkBuddy points are nil")
        try require(!missingWb.workbuddyIsOnline, "Missing WorkBuddy is not online")

        // MARK: - 3. DeepSeek Tests
        try require(snapshot.deepseekBalanceText == "58.70", "DeepSeek balance: \(snapshot.deepseekBalanceText)")
        try require(snapshot.deepseekCurrency == "CNY", "DeepSeek currency: \(snapshot.deepseekCurrency)")
        try require(snapshot.deepseekIsOnline, "DeepSeek is online")

        let dsUnconfigured = WidgetDisplaySnapshot(
            payload: try decode(#"{"deepseek": {"status": "Not configured"}}"#),
            fetchedAt: fetchedAt
        )
        try require(!dsUnconfigured.deepseekIsOnline, "DeepSeek Not configured is not online")

        let dsOffline = WidgetDisplaySnapshot(
            payload: try decode(#"{"deepseek": {"status": "Offline"}}"#),
            fetchedAt: fetchedAt
        )
        try require(!dsOffline.deepseekIsOnline, "DeepSeek Offline is not online")

        let missingDs = WidgetDisplaySnapshot(
            payload: try decode(#"{}"#),
            fetchedAt: fetchedAt
        )
        try require(missingDs.deepseekBalanceText == "—", "Missing DeepSeek balance is —")
        try require(!missingDs.deepseekIsOnline, "Missing DeepSeek is not online")

        // MARK: - 5. Backward Compatibility (Legacy 2.7.0 Cache Decode)
        let legacyCacheJSON = """
        {
          "codex": "83%",
          "workbuddy": "5,760",
          "deepseek": "58.70 CNY",
          "system": "Online",
          "fetchedAt": 1000,
          "stale": false
        }
        """
        let legacyDecoded = try JSONDecoder().decode(WidgetDisplaySnapshot.self, from: Data(legacyCacheJSON.utf8))
        try require(legacyDecoded.codexWeeklyNumber == "83", "Legacy 2.7.0 codex decoded number: \(legacyDecoded.codexWeeklyNumber)")
        try require(legacyDecoded.codexWeeklyRemaining == 83, "Legacy 2.7.0 codex remaining")
        try require(abs(legacyDecoded.codexWeeklyProgress - 0.83) < 0.001, "Legacy 2.7.0 codex progress")
        try require(legacyDecoded.workbuddyPointsText == "5,760", "Legacy 2.7.0 workbuddy points text")
        try require(legacyDecoded.workbuddyPoints == 5760, "Legacy 2.7.0 workbuddy points")
        try require(legacyDecoded.deepseekBalanceText == "58.70", "Legacy 2.7.0 deepseek balance")
        try require(legacyDecoded.deepseekCurrency == "CNY", "Legacy 2.7.0 deepseek currency")

        // MARK: - 6. Store & Cache Tests
        let refStale = snapshot.staleCopy
        let previous = WidgetStatusStore.load()
        defer {
            if let previous {
                WidgetStatusStore.save(previous)
            } else {
                WidgetStatusStore.remove()
            }
        }
        WidgetStatusStore.remove()
        try require(WidgetStatusStore.cachedOrPlaceholder() == .placeholder, "empty cache placeholder")
        WidgetStatusStore.save(snapshot)
        try require(refStale.stale, "Cached snapshot is stale")
        try require(WidgetStatusStore.cachedOrPlaceholder() == refStale, "last successful snapshot fallback")

        try require(refStale.deepseekState == "stale" && !refStale.deepseekIsOnline, "offline balance cannot be online")
        try require(snapshot.evaluated(at: fetchedAt.addingTimeInterval(300)).stale, "five minute expiry")
        try require(snapshot.evaluated(at: fetchedAt.addingTimeInterval(86400)) == .placeholder, "24h expiry hides old values")
        try require(snapshot.age(at: fetchedAt.addingTimeInterval(60)) == 60, "snapshot age")
        let failedBalance = WidgetDisplaySnapshot(payload: try decode(#"{"deepseek":{"status":"Connection error","error_code":"connection_error","stale":true,"balances":[{"currency":"CNY","total_balance":"58.39"}]}}"#), fetchedAt: fetchedAt)
        try require(failedBalance.deepseekBalanceText == "58.39", "last known good remains visible")
        try require(failedBalance.deepseekState == "stale", "failed balance is stale")
        try require(failedBalance.deepseekStatusText.contains("connection_error"), "safe error shown")
        let neverSucceeded = WidgetDisplaySnapshot(payload: try decode(#"{"deepseek":{"status":"Connection error","stale":true,"balances":[]}}"#), fetchedAt: fetchedAt)
        try require(neverSucceeded.deepseekState == "unavailable", "no success means unavailable")
        for (status, balance, expectedState, expectedText) in [
            ("Online", "58.39", "live", "Online"),
            ("No balance", "0.00", "live", "No balance"),
            ("Not configured", "", "unavailable", "Not configured")
        ] {
            let balances = balance.isEmpty ? [] : [["currency": "CNY", "total_balance": balance]]
            let json = try JSONSerialization.data(withJSONObject: ["deepseek": ["status": status, "balances": balances, "stale": false]])
            let value = WidgetDisplaySnapshot(payload: try JSONDecoder().decode(WidgetStatusPayload.self, from: json), fetchedAt: .now)
            try require(value.deepseekState == expectedState, "freshness: \(status)")
            try require(value.deepseekStatusText == expectedText, "account status: \(status)")
            try require(!value.deepseekStale, "fresh/not configured is not cache")
            let roundTrip = try JSONDecoder().decode(WidgetDisplaySnapshot.self, from: JSONEncoder().encode(value))
            try require(roundTrip == value, "shared cache preserves account status")
        }
        // MARK: - 7. Widget Configuration & Per-Widget Customization Tests
        // 7.1 Intent Defaults
        let defaultIntent = AICCWidgetConfigurationIntent()
        try require(defaultIntent.topLeft == .codex, "Default topLeft is Codex")
        try require(defaultIntent.topRight == .google, "Default topRight is Google")
        try require(defaultIntent.bottomLeft == .workbuddy, "Default bottomLeft is WorkBuddy")
        try require(defaultIntent.bottomRight == .deepseek, "Default bottomRight is DeepSeek")
        try require(defaultIntent.primaryMetric == nil, "Default primaryMetric is nil (falls back to topLeft)")
        try require(defaultIntent.secondaryMetric == nil, "Default secondaryMetric is nil (falls back to topRight)")

        let defaultSmall = defaultIntent.resolvedMetrics(for: .systemSmall)
        try require(defaultSmall == [.codex, .google], "Default Small metrics must be Codex + Google: \(defaultSmall)")

        let defaultMedium = defaultIntent.resolvedMetrics(for: .systemMedium)
        try require(defaultMedium == [.codex, .google, .workbuddy, .deepseek], "Default Medium metrics must be Codex, Google, WorkBuddy, DeepSeek: \(defaultMedium)")

        // 7.2 Custom Intent for Small & Medium
        let customSmall = AICCWidgetConfigurationIntent(primary: .workbuddy, secondary: .deepseek)
        try require(customSmall.resolvedMetrics(for: .systemSmall) == [.workbuddy, .deepseek], "Custom Small metrics: \(customSmall.resolvedMetrics(for: .systemSmall))")

        let customMedium1 = AICCWidgetConfigurationIntent(topLeft: .workbuddy, topRight: .deepseek, bottomLeft: .codex, bottomRight: .google)
        try require(customMedium1.resolvedMetrics(for: .systemMedium) == [.workbuddy, .deepseek, .codex, .google], "Custom Medium 1 metrics: \(customMedium1.resolvedMetrics(for: .systemMedium))")

        let customMedium2 = AICCWidgetConfigurationIntent(topLeft: .codex, topRight: .workbuddy, bottomLeft: .google, bottomRight: .deepseek)
        try require(customMedium2.resolvedMetrics(for: .systemMedium) == [.codex, .workbuddy, .google, .deepseek], "Custom Medium 2 metrics: \(customMedium2.resolvedMetrics(for: .systemMedium))")

        // 7.3 Duplicate Selection Normalization (Crash-safe & Deterministic)
        let dupSmall1 = AICCWidgetConfigurationIntent(primary: .codex, secondary: .codex).resolvedMetrics(for: .systemSmall)
        try require(dupSmall1 == [.codex, .google], "Duplicate Codex+Codex normalized to Codex+Google: \(dupSmall1)")

        let dupSmall2 = AICCWidgetConfigurationIntent(primary: .google, secondary: .google).resolvedMetrics(for: .systemSmall)
        try require(dupSmall2 == [.google, .codex], "Duplicate Google+Google normalized to Google+Codex: \(dupSmall2)")

        let dupSmall3 = AICCWidgetConfigurationIntent(primary: .workbuddy, secondary: .workbuddy).resolvedMetrics(for: .systemSmall)
        try require(dupSmall3 == [.workbuddy, .codex], "Duplicate WorkBuddy+WorkBuddy normalized to WorkBuddy+Codex: \(dupSmall3)")

        let dupSmall4 = AICCWidgetConfigurationIntent(primary: .deepseek, secondary: .deepseek).resolvedMetrics(for: .systemSmall)
        try require(dupSmall4 == [.deepseek, .codex], "Duplicate DeepSeek+DeepSeek normalized to DeepSeek+Codex: \(dupSmall4)")

        let dupMed1 = AICCWidgetConfigurationIntent.normalize([.codex, .codex, .google, .google], targetCount: 4)
        try require(dupMed1 == [.codex, .google, .workbuddy, .deepseek], "Duplicate Codex/Codex/Google/Google normalized: \(dupMed1)")

        let dupMed2 = AICCWidgetConfigurationIntent.normalize([.google, .google, .google, .google], targetCount: 4)
        try require(dupMed2 == [.google, .codex, .workbuddy, .deepseek], "Duplicate 4x Google normalized: \(dupMed2)")

        let dupMed3 = AICCWidgetConfigurationIntent.normalize([.workbuddy, .workbuddy, .deepseek, .deepseek], targetCount: 4)
        try require(dupMed3 == [.workbuddy, .codex, .deepseek, .google], "Duplicate WorkBuddy/WorkBuddy/DeepSeek/DeepSeek normalized: \(dupMed3)")

        // 7.4 Config Persistence / Codable Round Trip
        let intentToEncode = AICCWidgetConfigurationIntent(primary: .workbuddy, secondary: .deepseek)
        let encodedData = try JSONEncoder().encode(intentToEncode)
        let decodedIntent = try JSONDecoder().decode(AICCWidgetConfigurationIntent.self, from: encodedData)
        try require(decodedIntent == intentToEncode, "Intent survives JSON roundtrip")
        try require(decodedIntent.resolvedMetrics(for: .systemSmall) == [.workbuddy, .deepseek], "Decoded intent resolves correctly")

        let fullIntent = AICCWidgetConfigurationIntent(topLeft: .deepseek, topRight: .google, bottomLeft: .workbuddy, bottomRight: .codex)
        let fullData = try JSONEncoder().encode(fullIntent)
        let fullDecoded = try JSONDecoder().decode(AICCWidgetConfigurationIntent.self, from: fullData)
        try require(fullDecoded == fullIntent, "Full 4-slot intent survives JSON roundtrip")

        // 7.5 Metric Enum Contract
        try require(WidgetMetricOption.codex.rawValue == "codex", "Stable identifier codex")
        try require(WidgetMetricOption.google.rawValue == "google", "Stable identifier google")
        try require(WidgetMetricOption.workbuddy.rawValue == "workbuddy", "Stable identifier workbuddy")
        try require(WidgetMetricOption.deepseek.rawValue == "deepseek", "Stable identifier deepseek")
        try require(WidgetMetricOption.allCases.count == 4, "Exactly 4 metrics in first version")
        try require(WidgetMetricOption.codex.isQuota, "Codex is quota")
        try require(WidgetMetricOption.google.isQuota, "Google is quota")
        try require(!WidgetMetricOption.workbuddy.isQuota, "WorkBuddy is balance")
        try require(!WidgetMetricOption.deepseek.isQuota, "DeepSeek is balance")
        print("AICC Widget status smoke tests passed.")
    }

    private static func decode(_ json: String) throws -> WidgetStatusPayload {
        try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(json.utf8))
    }
}
