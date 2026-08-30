import Foundation

@main
struct WidgetStatusSmokeMain {
    enum Failure: Error {
        case assertion(String)
    }

    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw Failure.assertion(message) }
    }

    static func main() throws {
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
        let fetchedAt = Date(timeIntervalSince1970: 1000)
        let snapshot = WidgetDisplaySnapshot(payload: fullPayload, fetchedAt: fetchedAt)

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

        print("AICC Widget status smoke tests passed.")
    }

    private static func decode(_ json: String) throws -> WidgetStatusPayload {
        try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(json.utf8))
    }
}
