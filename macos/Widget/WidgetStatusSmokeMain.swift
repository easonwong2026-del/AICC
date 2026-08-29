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
                "points": 5760,
                "balance_state": "Connected",
                "balance_age_seconds": 60
              },
              "deepseek": {
                "status": "Online",
                "balances": [{ "currency": "CNY", "total_balance": "58.70" }]
              },
              "system": { "status": "Online" },
              "updated_at": "2026-08-29 12:00"
            }
            """
        )
        let fetchedAt = Date(timeIntervalSince1970: 1000)
        let snapshot = WidgetDisplaySnapshot(payload: fullPayload, fetchedAt: fetchedAt)

        // MARK: - 1. Codex Tests
        try require(snapshot.codexWeeklyNumber == "83", "Codex weekly number: \(snapshot.codexWeeklyNumber)")
        try require(snapshot.codex == "83%", "Codex weekly string formatted with %: \(snapshot.codex)")
        try require(abs(snapshot.codexWeeklyProgress - 0.83) < 0.001, "Codex weekly progress is 0.83: \(snapshot.codexWeeklyProgress)")
        try require(snapshot.codexFiveHourRemaining == 87, "Codex five-hour remaining: \(String(describing: snapshot.codexFiveHourRemaining))")
        try require(snapshot.codexResetText == "重置于 2026-09-04 08:01", "Codex reset text: \(String(describing: snapshot.codexResetText))")

        // Reset parsing variants
        let alreadyPrefixed = WidgetDisplaySnapshot(
            payload: try decode(#"{"codex": {"weekly": {"remaining": 50, "reset": "重置于 2026-09-04 08:01"}}}"#),
            fetchedAt: fetchedAt
        )
        try require(alreadyPrefixed.codexResetText == "重置于 2026-09-04 08:01", "Does not duplicate 重置于 prefix")

        let invalidReset = WidgetDisplaySnapshot(
            payload: try decode(#"{"codex": {"weekly": {"remaining": 50, "reset": "--"}}}"#),
            fetchedAt: fetchedAt
        )
        try require(invalidReset.codexResetText == nil, "Invalid reset string '--' returns nil")

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
        try require(fiveHourOnly.codexWeeklyNumber == "88", "Codex falls back to five_hour number")
        try require(fiveHourOnly.codex == "88%", "Codex falls back to five_hour string")
        try require(fiveHourOnly.codexResetText == "重置于 14:00", "Codex falls back to five_hour reset")

        // Nil codex
        let missingCodex = WidgetDisplaySnapshot(
            payload: try decode(#"{"workbuddy": {"points": 10}}"#),
            fetchedAt: fetchedAt
        )
        try require(missingCodex.codexWeeklyNumber == "—", "Missing codex number is —")
        try require(missingCodex.codex == "—", "Missing codex is —")
        try require(missingCodex.codexWeeklyProgress == 0.0, "Missing codex progress is 0")
        try require(missingCodex.codexResetText == nil, "Missing codex reset is nil")

        // MARK: - 2. WorkBuddy Tests
        try require(snapshot.workbuddyPointsText == "5,760", "WorkBuddy points with grouping separator: \(snapshot.workbuddyPointsText)")
        try require(snapshot.workbuddy == "5,760", "WorkBuddy legacy accessor")
        try require(snapshot.workbuddySubtitle == "已连接 · 1 分钟前", "WorkBuddy subtitle with age: \(snapshot.workbuddySubtitle)")
        try require(snapshot.workbuddyIsOnline, "WorkBuddy is online")

        let wbJustNow = WidgetDisplaySnapshot(
            payload: try decode(#"{"workbuddy": {"points": 100, "balance_age_seconds": 20}}"#),
            fetchedAt: fetchedAt
        )
        try require(wbJustNow.workbuddySubtitle == "已连接 · 刚刚", "WorkBuddy age < 60s is 刚刚: \(wbJustNow.workbuddySubtitle)")

        let wbHoursAgo = WidgetDisplaySnapshot(
            payload: try decode(#"{"workbuddy": {"points": 100, "balance_age_seconds": 7200}}"#),
            fetchedAt: fetchedAt
        )
        try require(wbHoursAgo.workbuddySubtitle == "已连接 · 2 小时前", "WorkBuddy age in hours: \(wbHoursAgo.workbuddySubtitle)")

        let wbUpdatedAt = WidgetDisplaySnapshot(
            payload: try decode(#"{"workbuddy": {"points": 100, "balance_updated_at": "12:34"}}"#),
            fetchedAt: fetchedAt
        )
        try require(wbUpdatedAt.workbuddySubtitle == "已连接 · 12:34", "WorkBuddy balance_updated_at fallback")

        let wbCached = WidgetDisplaySnapshot(
            payload: try decode(#"{"workbuddy": {"points": 100, "balance_state": "Cached"}}"#),
            fetchedAt: fetchedAt
        )
        try require(wbCached.workbuddySubtitle == "已缓存", "WorkBuddy cached state")

        let missingWb = WidgetDisplaySnapshot(
            payload: try decode(#"{}"#),
            fetchedAt: fetchedAt
        )
        try require(missingWb.workbuddyPointsText == "—", "Missing WorkBuddy points is —")
        try require(missingWb.workbuddySubtitle == "未连接", "Missing WorkBuddy subtitle is 未连接")
        try require(!missingWb.workbuddyIsOnline, "Missing WorkBuddy is not online")

        // MARK: - 3. DeepSeek Tests
        try require(snapshot.deepseekBalanceText == "58.70", "DeepSeek balance: \(snapshot.deepseekBalanceText)")
        try require(snapshot.deepseekCurrency == "CNY", "DeepSeek currency: \(snapshot.deepseekCurrency)")
        try require(snapshot.deepseek == "58.70 CNY", "DeepSeek legacy string: \(snapshot.deepseek)")
        try require(snapshot.deepseekStatusText == "在线", "DeepSeek online status: \(snapshot.deepseekStatusText)")
        try require(snapshot.deepseekIsOnline, "DeepSeek is online")

        let dsUnconfigured = WidgetDisplaySnapshot(
            payload: try decode(#"{"deepseek": {"status": "Not configured"}}"#),
            fetchedAt: fetchedAt
        )
        try require(dsUnconfigured.deepseekStatusText == "未配置", "DeepSeek Not configured status")
        try require(!dsUnconfigured.deepseekIsOnline, "DeepSeek Not configured is not online")

        let dsOffline = WidgetDisplaySnapshot(
            payload: try decode(#"{"deepseek": {"status": "Offline"}}"#),
            fetchedAt: fetchedAt
        )
        try require(dsOffline.deepseekStatusText == "离线", "DeepSeek Offline status")

        let missingDs = WidgetDisplaySnapshot(
            payload: try decode(#"{}"#),
            fetchedAt: fetchedAt
        )
        try require(missingDs.deepseekBalanceText == "—", "Missing DeepSeek balance is —")
        try require(missingDs.deepseekStatusText == "—", "Missing DeepSeek status is —")
        try require(!missingDs.deepseekIsOnline, "Missing DeepSeek is not online")

        // MARK: - 4. Footer Time Formatting Tests
        let refNow = fetchedAt.addingTimeInterval(30)
        try require(snapshot.formattedFooterTime(at: refNow) == "30 秒前更新", "Footer seconds ago: \(snapshot.formattedFooterTime(at: refNow))")

        let refJustNow = fetchedAt.addingTimeInterval(3)
        try require(snapshot.formattedFooterTime(at: refJustNow) == "刚刚更新", "Footer just now: \(snapshot.formattedFooterTime(at: refJustNow))")

        let refMinutes = fetchedAt.addingTimeInterval(120)
        try require(snapshot.formattedFooterTime(at: refMinutes) == "2 分钟前更新", "Footer minutes ago: \(snapshot.formattedFooterTime(at: refMinutes))")

        let refStale = snapshot.staleCopy
        try require(refStale.stale, "Cached snapshot is stale")
        try require(refStale.formattedFooterTime(at: refMinutes) == "2 分钟前更新 · 已缓存", "Footer stale label appended")

        // MARK: - 5. Store & Cache Tests
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
        try require(WidgetStatusStore.cachedOrPlaceholder() == refStale, "last successful snapshot fallback")

        print("AICC Widget status smoke tests passed.")
    }

    private static func decode(_ json: String) throws -> WidgetStatusPayload {
        try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(json.utf8))
    }
}
