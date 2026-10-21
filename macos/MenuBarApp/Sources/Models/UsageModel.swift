import Foundation

/// What Magpie's usage API says, in the shape the board widget draws.
///
/// The field selection and every derivation here mirror `metrics.py` in
/// <https://github.com/peterroe/magpie-usage-display> (MIT), so the same
/// numbers appear on the e-ink board and on this widget:
///
///   success  = (today.calls - today.errors) / today.calls
///   ttft     = today.ttft_ms / today.timed
///   tokens   = input + output + reasoning + cache_read + cache_write
///   cache    = cache_read / (input + cache_read)
///
/// `cost` has no currency in the payload; Magpie prices in USD, so the widget
/// prints `$`.
struct RecentRequest: Codable, Equatable, Sendable {
    var model: String = ""
    var status: Int = 0
    var millis: Int = 0
}

struct UsageSnapshot: Codable, Equatable, Sendable {
    var generatedAt: Double = 0
    var port: Int = 3425
    var available: Bool = false
    var stale: Bool = false

    var remainingPercent: Int = 100
    var remainingWindow: String = ""
    var resetSeconds: Int = 0

    var callsToday: Int = 0
    var errorsToday: Int = 0
    var successPermille: Int = 1000
    var tokensToday: Int = 0
    var tokens7d: Int = 0
    var averageTTFTMillis: Int = 0

    var latestModel: String = ""
    var latestStatus: Int = 0
    var latestLatencyMillis: Int = 0
    var latestFailed: Bool = false
    var recent: [RecentRequest] = []

    var sessionCount: Int = 0
    var totalCalls: Int = 0
    var totalInput: Int = 0
    var totalOutput: Int = 0
    var totalCacheRead: Int = 0
    var totalReasoning: Int = 0
    var costMicros: Int = 0
    var cacheHitPermille: Int = 0
    var topModel: String = ""
    var topModelCalls: Int = 0

    static func placeholder(port: Int = 3425) -> UsageSnapshot {
        var snapshot = UsageSnapshot()
        snapshot.port = port
        snapshot.generatedAt = Date.now.timeIntervalSince1970
        return snapshot
    }

    /// A reading older than this is shown as cached rather than live.
    static let liveLifetime: TimeInterval = 90

    func evaluated(at date: Date = .now, offline: Bool = false) -> UsageSnapshot {
        var copy = self
        copy.stale = offline || date.timeIntervalSince1970 - generatedAt > Self.liveLifetime
        return copy
    }
}

// MARK: - Formatting

enum UsageFormat {
    /// 965 / 12.6K / 1.24M / 944M — the same rounding the device uses.
    static func count(_ value: Int) -> String {
        let number = max(0, value)
        if number >= 1_000_000 {
            let millions = Double(number) / 1_000_000
            return millions >= 100 ? String(format: "%.0fM", millions) : String(format: "%.2fM", millions)
        }
        if number >= 1_000 {
            let thousands = Double(number) / 1_000
            return thousands >= 100 ? String(format: "%.0fK", thousands) : String(format: "%.1fK", thousands)
        }
        return String(number)
    }

    static func grouped(_ value: Int) -> String {
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.groupingSeparator = " "
        return formatter.string(from: NSNumber(value: max(0, value))) ?? "\(value)"
    }

    static func percent(permille: Int) -> String {
        let value = Double(max(0, min(1000, permille))) / 10
        return value == value.rounded() ? String(format: "%.0f%%", value) : String(format: "%.1f%%", value)
    }

    static func percent(_ value: Int) -> String { "\(max(0, min(100, value)))%" }

    /// 985 → "98.5", 1000 → "100" — a permille without the sign.
    static func tenths(_ permille: Int) -> String {
        let value = Double(max(0, min(1000, permille))) / 10
        return value == value.rounded() ? String(format: "%.0f", value) : String(format: "%.1f", value)
    }

    static func money(micros: Int) -> String {
        let value = Double(max(0, micros)) / 1_000_000
        return value >= 1000 ? String(format: "$%.0f", value) : String(format: "$%.2f", value)
    }

    static func seconds(millis: Int) -> String {
        let value = max(0, millis)
        if value >= 10_000 { return String(format: "%.0fs", Double(value) / 1000) }
        if value >= 1_000 { return String(format: "%.1fs", Double(value) / 1000) }
        return "\(value)ms"
    }

    /// "3 天 4 时", "4 时 12 分", "58 分" — a reset is read as a countdown.
    static func countdown(seconds: Int) -> String {
        let value = max(0, seconds)
        if value == 0 { return "—" }
        let minutes = value / 60
        if minutes < 60 { return "\(max(1, minutes)) 分" }
        let hours = minutes / 60
        if hours < 24 { return "\(hours) 时 \(minutes % 60) 分" }
        return "\(hours / 24) 天 \(hours % 24) 时"
    }
}

// MARK: - Parsing

enum UsageParser {
    /// Builds a snapshot from the three payloads Magpie answers.  Any payload
    /// may be nil: the board then keeps whatever the others can tell.
    static func snapshot(usage: Data?, requests: Data?, quotas: Data?,
                         now: Date = .now, port: Int = 3425) -> UsageSnapshot {
        var snapshot = UsageSnapshot()
        snapshot.port = port
        snapshot.generatedAt = now.timeIntervalSince1970

        if let usage, let root = object(usage) {
            snapshot.available = true
            let series = (root["series"] as? [[String: Any]]) ?? []
            let today = series.last ?? [:]
            snapshot.callsToday = int(today["calls"])
            snapshot.errorsToday = min(snapshot.callsToday, int(today["errors"]))
            snapshot.successPermille = snapshot.callsToday > 0
                ? Int((Double(snapshot.callsToday - snapshot.errorsToday) * 1000 / Double(snapshot.callsToday)).rounded())
                : 1000
            snapshot.tokensToday = tokens(today)
            snapshot.tokens7d = series.suffix(7).reduce(0) { $0 + tokens($1) }
            let timed = int(today["timed"])
            snapshot.averageTTFTMillis = timed > 0 ? int(today["ttft_ms"]) / timed : 0

            snapshot.sessionCount = int(root["sessionCount"])
            snapshot.totalCalls = int(root["calls"])
            snapshot.totalInput = int(root["input"])
            snapshot.totalOutput = int(root["output"])
            snapshot.totalCacheRead = int(root["cache_read"])
            snapshot.totalReasoning = int(root["reasoning"])
            snapshot.costMicros = Int((double(root["cost"]) * 1_000_000).rounded())
            let cacheBase = snapshot.totalInput + snapshot.totalCacheRead
            snapshot.cacheHitPermille = cacheBase > 0
                ? Int((Double(snapshot.totalCacheRead) * 1000 / Double(cacheBase)).rounded())
                : 0

            let models = (root["models"] as? [[String: Any]]) ?? []
            if let top = models.max(by: { int($0["calls"]) < int($1["calls"]) }) {
                snapshot.topModel = short(string(top["name"]) ?? string(top["model"]) ?? "")
                snapshot.topModelCalls = int(top["calls"])
            }
        }

        if let requests, let root = object(requests) {
            snapshot.available = true
            let rows = (root["rows"] as? [[String: Any]]) ?? []
            if let latest = rows.first {
                snapshot.latestModel = short(string(latest["served"]) ?? string(latest["model"]) ?? "")
                snapshot.latestStatus = max(0, int(latest["status"]))
                snapshot.latestLatencyMillis = int(latest["ms"])
                snapshot.latestFailed = latestStatusFails(snapshot.latestStatus)
            }
            snapshot.recent = rows.prefix(3).map { row in
                RecentRequest(model: short(string(row["served"]) ?? string(row["model"]) ?? ""),
                              status: max(0, int(row["status"])),
                              millis: int(row["ms"]))
            }
        }

        if let quotas, let root = object(quotas) {
            if let tightest = tightestWindow(root["data"] as? [[String: Any]] ?? []) {
                snapshot.remainingPercent = max(0, min(100, Int((tightest.remaining).rounded())))
                snapshot.remainingWindow = tightest.label
                snapshot.resetSeconds = tightest.resetSeconds
            }
        }
        return snapshot
    }

    /// "Codex · 7 天" — the window closest to running out across every provider.
    private static func tightestWindow(_ providers: [[String: Any]]) -> (remaining: Double, label: String, resetSeconds: Int)? {
        var best: (Double, String, Int)?
        let now = Date.now.timeIntervalSince1970
        for provider in providers {
            let providerName = displayProvider(string(provider["provider_name"]) ?? string(provider["provider"]) ?? "")
            let windows = (provider["windows"] as? [[String: Any]]) ?? []
            for window in windows {
                if window["unlimited"] as? Bool == true { continue }
                guard let remaining = doubleOrNil(window["remaining"]) else { continue }
                if best == nil || remaining < best!.0 {
                    let name = windowName(string(window["name"]) ?? "")
                    let reset = iso(string(window["resetsAt"])).map { max(0, Int($0 - now)) } ?? 0
                    best = (remaining, providerName.isEmpty ? name : "\(providerName) · \(name)", reset)
                }
            }
        }
        guard let best else { return nil }
        return (best.0, best.1, best.2)
    }

    /// "Codex", "WorkBuddy", "Antigravity" — Magpie sends ids, not names.
    static func displayProvider(_ id: String) -> String {
        switch id.lowercased() {
        case "codex": return "Codex"
        case "workbuddy": return "WorkBuddy"
        case "antigravity": return "Antigravity"
        case "zcode", "zhipu", "glm": return "ZCode"
        case "deepseek": return "DeepSeek"
        case "kiro": return "Kiro"
        case "minimax": return "MiniMax"
        default:
            return id.split(separator: "-")
                .map { $0.prefix(1).uppercased() + $0.dropFirst() }
                .joined(separator: " ")
        }
    }

    /// "5 hours" → "5 小时", "7 days" → "7 天", "Credits" → "积分".
    static func windowName(_ name: String) -> String {
        let lower = name.lowercased()
        let parts = lower.split(separator: " ")
        if parts.count >= 2, let amount = Int(parts[0]) {
            switch parts[1] {
            case "minute", "minutes", "min", "mins": return "\(amount) 分"
            case "hour", "hours": return "\(amount) 小时"
            case "day", "days": return "\(amount) 天"
            case "week", "weeks": return "\(amount) 周"
            case "month", "months": return "\(amount) 月"
            default: break
            }
        }
        if lower.hasPrefix("credit") { return "积分" }
        if lower.hasPrefix("prompt") { return "次" }
        return name
    }

    private static func iso(_ value: String?) -> Double? {
        guard let value, !value.isEmpty else { return nil }
        let plain = ISO8601DateFormatter()
        if let date = plain.date(from: value) { return date.timeIntervalSince1970 }
        plain.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return plain.date(from: value)?.timeIntervalSince1970
    }

    private static func latestStatusFails(_ status: Int) -> Bool {
        status < 200 || status >= 400
    }

    private static func tokens(_ bucket: [String: Any]) -> Int {
        ["input", "output", "reasoning", "cache_read", "cache_write"].reduce(0) { $0 + int(bucket[$1]) }
    }

    private static func short(_ value: String) -> String { String(value.prefix(23)) }

    private static func object(_ data: Data) -> [String: Any]? {
        guard data.count <= 4 * 1024 * 1024 else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    private static func int(_ value: Any?) -> Int {
        if let number = value as? NSNumber { return max(0, number.intValue) }
        if let text = value as? String, let parsed = Int(text) { return max(0, parsed) }
        return 0
    }

    private static func double(_ value: Any?) -> Double { doubleOrNil(value) ?? 0 }

    private static func doubleOrNil(_ value: Any?) -> Double? {
        if let number = value as? NSNumber, !(value is Bool) {
            let parsed = number.doubleValue
            return parsed.isFinite ? parsed : nil
        }
        if let text = value as? String, let parsed = Double(text), parsed.isFinite { return parsed }
        return nil
    }

    private static func string(_ value: Any?) -> String? {
        guard let text = value as? String else { return nil }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
