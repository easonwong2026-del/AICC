import Foundation

@main
struct WidgetStatusSmokeMain {
    enum Failure: Error {
        case assertion(String)
    }

    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw Failure.assertion(message) }
    }

    static func stamp() -> String {
        ISO8601DateFormatter().string(from: .now)
    }

    static var mixedReport: String {
        """
        {
          "object": "list",
          "data": [
            {"provider": "codex", "name": "Codex", "kind": "subscription", "plan": "team",
             "user": "one@example.invalid · Team", "readAt": "\(stamp())", "last": true,
             "windows": [{"name": "5 hours", "used": 9, "remaining": 91, "resetsAt": "2026-10-20T16:58:46+08:00"},
                         {"name": "7 days", "used": 12, "remaining": 88}]},
            {"provider": "codex", "name": "Codex", "kind": "subscription", "plan": "plus",
             "user": "two@example.invalid", "readAt": "\(stamp())",
             "windows": [{"name": "5 hours", "used": 100, "remaining": 0}]},
            {"provider": "workbuddy", "kind": "subscription", "plan": "Free", "user": "wb@example.invalid",
             "readAt": "\(stamp())",
             "windows": [{"name": "Credits", "used": 0.048, "remaining": 99.95,
                          "display": "2.27 / 4723", "amount": 2.27, "limit": 4723, "unit": "credits"}]},
            {"provider": "zcode", "kind": "plan", "plan": "GLM Coding Plan", "user": "glm@example.invalid",
             "readAt": "\(stamp())", "renew": "auto", "until": "2026-11-20T00:00:00Z",
             "windows": [{"name": "prompts", "used": 12.5, "remaining": 87.5}]},
            {"provider": "deepseek", "kind": "balance", "user": "key@example.invalid",
             "readAt": "\(stamp())", "balance": "12.34 USD",
             "balanceSplit": [{"label": "paid", "text": "10.00 USD"}]},
            {"provider": "brandnew-vendor", "kind": "subscription", "user": "x@example.invalid",
             "readAt": "\(stamp())", "windows": [{"name": "Monthly", "used": 10, "remaining": 90}]},
            {"provider": "codex", "kind": "subscription", "user": "broken@example.invalid",
             "readAt": "\(stamp())", "error": "401 invalid_grant for broken@example.invalid", "windows": []},
            {"provider": "workbuddy", "kind": "subscription", "user": "empty@example.invalid",
             "readAt": "\(stamp())", "windows": []}
          ]
        }
        """
    }

    static func parse(_ json: String, now: Date = .now) throws -> QuotaSnapshot {
        try MagpieQuotaParser.snapshot(data: Data(json.utf8), now: now)
    }

    static func main() throws {
        let snapshot = try parse(mixedReport)
        try require(snapshot.providers == ["codex", "workbuddy", "zcode", "deepseek", "brandnew-vendor"],
                    "Every provider keeps its gateway order, unknown ones included")
        try require(snapshot.accounts.count == 8, "Every readable row becomes a card")
        try require(snapshot.state == "Connected", "Fresh readings are connected")

        let codex = snapshot.accounts[0]
        try require(codex.providerName == "Codex" && codex.accountLabel == "o***", "Identity is masked to one character")
        try require(codex.primaryItem?.primaryText == "91%", "A percentage window keeps its share")
        try require(codex.secondaryItem?.name == "7 days", "A second window is kept, not dropped")
        try require(codex.items.count == 2, "Both windows survive")

        let credits = snapshot.accounts[2]
        try require(credits.primaryItem?.primaryText == "4721", "A credits window reports what is left, not used / total")
        try require(credits.primaryItem?.unit == "credits" && credits.plan == "Free", "Plan and unit survive")
        try require(credits.primaryItem?.isPercentage == false, "A credit window is not shown as a percentage")

        let plan = snapshot.accounts[3]
        try require(plan.kind == .plan && plan.primaryItem?.name == "prompts", "Plan cards accept any window name")
        try require(plan.renew == "auto", "A plan term is kept")

        let balance = snapshot.accounts[4]
        try require(balance.isBalance && balance.balanceText == "12.34 USD", "A balance is money, never a share")
        try require(balance.items.isEmpty, "A balance card carries no windows")

        let unknown = snapshot.accounts[5]
        try require(unknown.providerName == "Brandnew Vendor", "An unknown provider is named, not dropped")
        try require(MagpieQuotaParser.providerName("my_own-vendor") == "My Own Vendor", "Fallback name")

        try require(snapshot.accounts[6].failed && snapshot.accounts[6].stale, "A failed read is marked, not hidden")
        try require(snapshot.accounts[7].items.isEmpty && !snapshot.accounts[7].failed, "An empty card stays")
        let encoded = String(data: try JSONEncoder().encode(snapshot), encoding: .utf8) ?? ""
        try require(!encoded.contains("@"), "No email reaches the cache")
        try require(!encoded.contains("invalid_grant"), "No provider error text reaches the cache")

        let small = snapshot.slots(selection: [], limit: 2)
        try require(small.shown.count == 2 && small.shown[0].card.last, "The last served card leads")
        let medium = snapshot.slots(selection: [], limit: 4)
        try require(medium.shown.count == 4 && medium.hidden == 2, "Overflow is counted, not truncated silently")
        let onlyWorkBuddy = snapshot.slots(selection: [], provider: "workbuddy", limit: 4)
        try require(onlyWorkBuddy.shown.count == 1
                    && onlyWorkBuddy.shown.allSatisfy { $0.card.provider == "workbuddy" },
                    "A provider filter keeps only that provider")
        try require(snapshot.slots(selection: [], provider: "missing", limit: 4).shown.isEmpty,
                    "An unknown filter shows nothing")

        let balanceSlot = snapshot.slots(selection: [], limit: 8).shown.first { $0.card.kind == .balance }
        try require(balanceSlot?.primary.primaryText == "12.34 USD", "A balance card survives without any window")

        // A credits window reports what is left, never "used / total".
        let creditWindow = snapshot.accounts.first { $0.provider == "workbuddy" }?.primaryItem
        try require(creditWindow?.primaryText == "4721",
                    "Credits show the remaining count: \(creditWindow?.primaryText ?? "-")")
        try require(creditWindow?.totalText == "共 4723", "The total moves to a secondary label")

        // Selecting one window of one account is enough to make it its own card.
        let pools = """
        {"object": "list", "data": [{"provider": "antigravity", "kind": "subscription", "user": "a@example.invalid",
          "readAt": "\(stamp())", "windows": [
            {"name": "7 天", "used": 19, "remaining": 81, "pool": "Gemini"},
            {"name": "7 天", "used": 2, "remaining": 98, "pool": "Claude & GPT"},
            {"name": "5 小时", "used": 9, "remaining": 91, "pool": "Gemini"}]}]}
        """
        let poolSnapshot = try parse(pools)
        let card = poolSnapshot.accounts[0]
        let geminiWeek = card.key(for: card.items[0])
        let picked = poolSnapshot.slots(selection: [geminiWeek], limit: 4)
        try require(picked.shown.count == 1 && picked.shown[0].primary.name == "7 天"
                    && picked.shown[0].primary.pool == "Gemini",
                    "A window-level selection picks exactly that window")
        try require(picked.shown[0].secondary == nil, "An unselected window of the same account stays out")
        let bothGemini = poolSnapshot.slots(selection: [geminiWeek, card.key(for: card.items[2])], limit: 4)
        try require(bothGemini.shown[0].secondary?.pool == "Gemini", "Two selected windows share one card")
        let stale = poolSnapshot.slots(selection: ["gone|window|"], limit: 4)
        try require(stale.shown.count == 1, "A selection whose keys vanished falls back to automatic")
        try require(WidgetCardDisplayMode.one.capacity(for: 4) == 1, "One card means one")
        try require(WidgetCardDisplayMode.automatic.capacity(for: 2) == 2, "Small family holds two")
        try require(WidgetCardDisplayMode.all.capacity(for: 4) > 1000, "All means all")

        let later = snapshot.evaluated(at: .now.addingTimeInterval(QuotaSnapshot.liveLifetime + 1))
        try require(later.stale && later.accounts.allSatisfy(\.stale), "An old reading is cached")
        try require(later.accounts[2].primaryItem?.primaryText == "4721", "Ageing keeps the last value")
        let offline = snapshot.evaluated(at: .now, offline: true)
        try require(offline.stale && offline.state == "Cached", "Offline is cached, not live")
        let expired = snapshot.evaluated(at: .now.addingTimeInterval(QuotaSnapshot.retention + 1))
        try require(expired.accounts.isEmpty, "A day old cache is not shown as quota")

        let empty = try parse(#"{"object": "list", "data": []}"#)
        try require(empty.accounts.isEmpty && empty.state == "Empty" && !empty.available, "Empty is not zero")
        try require(empty.slots(selection: [], limit: 4).shown.isEmpty, "No cards without data")
        for broken in [#"{}"#, #"{"object": "list"}"#, #"{"object": "list", "data": {}}"#] {
            var rejected = false
            do { _ = try parse(broken) } catch { rejected = true }
            try require(rejected, "A broken report must be rejected")
        }

        let odd = """
        {"object": "list", "data": [{"provider": "codex", "kind": "subscription", "user": "a@example.invalid",
          "readAt": "not-a-time", "windows": [{"name": "5 hours", "used": true, "remaining": 250},
          {"name": "7 days", "unlimited": true}, {"used": 5}]}]}
        """
        let oddSnapshot = try parse(odd)
        let oddCard = oddSnapshot.accounts[0]
        try require(oddCard.items.count == 2, "A nameless window is skipped, the rest stay")
        try require(oddCard.items[0].remaining == nil && oddCard.items[0].used == nil, "A boolean is not a percentage")
        try require(oddCard.items[1].unlimited && oddCard.items[1].primaryText == "∞", "Unlimited stays unlimited")
        try require(oddCard.items[0].reset == nil && oddCard.stale, "No reset time and no read time means cached")

        let duplicate = """
        {"object": "list", "data": [{"provider": "antigravity", "kind": "subscription", "user": "a@example.invalid",
          "readAt": "\(stamp())", "windows": [{"name": "7 days", "used": 1, "remaining": 99, "pool": "Gemini"},
          {"name": "7 days", "used": 2, "remaining": 98, "pool": "Claude & GPT"}]}]}
        """
        let dupe = try parse(duplicate)
        try require(dupe.accounts[0].items.count == 2, "Duplicate window names are kept apart")
        try require(dupe.accounts[0].extraWindows == 0 && dupe.accounts[0].items[1].pool == "Claude & GPT",
                    "A pool keeps its own name")

        let legacy = try JSONDecoder().decode(AICCWidgetConfiguration.self,
                                              from: Data(#"{"magpiePort": 3426, "accounts": "two"}"#.utf8))
        try require(legacy.magpiePort == 3426 && legacy.cards == .two && legacy.provider == nil,
                    "A pre-3.0 configuration still decodes")
        let modern = try JSONDecoder().decode(AICCWidgetConfiguration.self,
                                              from: Data(#"{"magpiePort": 3427, "cards": "four", "provider": "workbuddy"}"#.utf8))
        try require(modern.cards == .four && modern.provider == "workbuddy", "Provider filter decodes")
        let roundTrip = try JSONDecoder().decode(AICCWidgetConfiguration.self, from: JSONEncoder().encode(modern))
        try require(roundTrip == modern, "Configuration round trips")

        WidgetStatusStore.remove(port: 13400)
        WidgetStatusStore.remove(port: 13401)
        WidgetStatusStore.save(snapshot, port: 13400)
        try require(WidgetStatusStore.load(port: 13400)?.accounts.count == 8, "A snapshot survives the widget cache")
        try require(WidgetStatusStore.load(port: 13401) == nil, "A second port starts empty")
        try require(WidgetStatusStore.cachedOrPlaceholder(port: 13401).accounts.isEmpty,
                    "No cache is a placeholder, not another port's data")
        try require(WidgetStatusStore.cachedOrPlaceholder(port: 13400).stale,
                    "A cached reading is never labelled live")
        WidgetStatusStore.remove(port: 13400)

        let bridge = """
        {"schema_version": 3, "source": "Magpie", "available": true, "state": "Connected", "stale": false,
         "providers": ["codex"], "refresh_seconds": 300, "fetched_at": \(Date.now.timeIntervalSince1970),
         "accounts": [{"id": "magpie-abc", "provider": "codex", "provider_name": "Codex",
           "account_label": "o***", "kind": "subscription", "plan": "team",
           "items": [{"name": "5 hours", "used": 9, "remaining": 91, "unlimited": false}],
           "balance": null, "balance_parts": [], "resets_count": 2, "renew": null,
           "last_served_at": null, "last": false, "failed": false, "deferred": false,
           "stale": false, "read_at": \(Date.now.timeIntervalSince1970)}]}
        """
        let decoded = try JSONDecoder().decode(QuotaSnapshot.self, from: Data(bridge.utf8))
        try require(decoded.accounts.first?.primaryItem?.remaining == 91, "A bridge snapshot decodes unchanged")
        try require(decoded.refreshSeconds == 300, "The bridge publishes the shared refresh policy")

        print("Widget contract: providers, plan/balance/subscription, masking, capacity, freshness, config, cache — passed")
    }
}
