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
              "codex": { "five_hour": { "remaining": 92 }, "weekly": { "remaining": 78 } },
              "workbuddy": { "points": 1245 },
              "deepseek": {
                "status": "Online",
                "balances": [{ "currency": "CNY", "total_balance": "86.42" }]
              },
              "system": { "status": "Online" },
              "updated_at": "2026-08-27 22:00"
            }
            """
        )
        let fetchedAt = Date(timeIntervalSince1970: 123)
        let snapshot = WidgetDisplaySnapshot(payload: fullPayload, fetchedAt: fetchedAt)
        try require(snapshot.codex == "78%", "Codex weekly takes precedence over five_hour: \(snapshot.codex)")
        try require(snapshot.workbuddy != "—", "full WorkBuddy snapshot")
        try require(snapshot.deepseek == "86.42 CNY", "full DeepSeek snapshot: \(snapshot.deepseek)")
        try require(snapshot.system == "Online", "full System snapshot")
        try require(!snapshot.stale, "fresh snapshot state")

        let fiveHourOnly = WidgetDisplaySnapshot(
            payload: try decode("{\"codex\": {\"five_hour\": {\"remaining\": 88}}}"),
            fetchedAt: fetchedAt
        )
        try require(fiveHourOnly.codex == "88%", "Codex falls back to five_hour when weekly is missing")

        let missingCodex = WidgetDisplaySnapshot(
            payload: try decode("{\"workbuddy\": {\"points\": 10}}"),
            fetchedAt: fetchedAt
        )
        try require(missingCodex.codex == "—", "missing Codex is unavailable")

        let missingWorkBuddy = WidgetDisplaySnapshot(
            payload: try decode("{\"codex\": {\"weekly\": {\"remaining\": 78}}}"),
            fetchedAt: fetchedAt
        )
        try require(missingWorkBuddy.workbuddy == "—", "missing WorkBuddy is unavailable")

        let stale = snapshot.staleCopy
        try require(stale.stale, "cached snapshot is stale")
        try require(stale.fetchedAt == fetchedAt, "cached timestamp is preserved")

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
        try require(WidgetStatusStore.cachedOrPlaceholder() == stale, "last successful snapshot fallback")

        print("AICC Widget status smoke tests passed.")
    }

    private static func decode(_ json: String) throws -> WidgetStatusPayload {
        try JSONDecoder().decode(WidgetStatusPayload.self, from: Data(json.utf8))
    }
}
