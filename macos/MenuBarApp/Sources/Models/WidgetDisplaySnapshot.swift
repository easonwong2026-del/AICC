import Foundation

struct WidgetStatusPayload: Decodable {
    let display_revision: String?
    let display_snapshot: WidgetDisplaySnapshot?
    let fetched_at: Double?
    let collection: WidgetCollection?
    let codex: WidgetCodexData?
    let workbuddy: WidgetWorkBuddyData?
    let deepseek: WidgetDeepSeekData?

    enum CodingKeys: String, CodingKey {
        case fetched_at, collection, display_revision, display_snapshot
        case codex
        case workbuddy
        case deepseek
    }
}

struct WidgetCollection: Decodable {
    let codex: WidgetCollector?
    let workbuddy: WidgetCollector?
    let deepseek: WidgetCollector?
}

struct WidgetCollector: Decodable {
    let state: String?
}

struct WidgetCodexData: Decodable {
    let stale: Bool?

    let fiveHour: WidgetRateWindow?
    let weekly: WidgetRateWindow?

    enum CodingKeys: String, CodingKey {
        case stale
        case fiveHour = "five_hour"
        case weekly
    }
}

struct WidgetRateWindow: Decodable {
    let remaining: Double?
    let reset: String?
    let label: String?
    let durationMinutes: Int?

    enum CodingKeys: String, CodingKey {
        case remaining
        case reset
        case label
        case durationMinutes = "duration_minutes"
    }
}

struct WidgetWorkBuddyData: Decodable {
    let balance_stale: Bool?
    let balance_state: String?

    let points: Double?

    enum CodingKeys: String, CodingKey {
        case points, balance_stale, balance_state
    }
}

struct WidgetDeepSeekData: Decodable {
    let stale: Bool?
    let error_code: String?

    let status: String?
    let balances: [WidgetDeepSeekBalance]?

    enum CodingKeys: String, CodingKey {
        case status, stale, error_code
        case balances
    }
}

struct WidgetDeepSeekBalance: Decodable {
    let currency: String?
    let totalBalance: String?

    enum CodingKeys: String, CodingKey {
        case currency
        case totalBalance = "total_balance"
    }
}

struct WidgetDisplaySnapshot: Codable, Equatable {
    // Codex properties
    let codexWeeklyNumber: String
    let codexWeeklyRemaining: Double?
    let codexFiveHourRemaining: Double?
    let codexWeeklyReset: String?
    let codexResetText: String?
    let codexWeeklyProgress: Double

    // WorkBuddy properties
    let workbuddyPoints: Double?
    let workbuddyPointsText: String
    var workbuddyIsOnline: Bool

    // DeepSeek properties
    let deepseekBalanceText: String
    let deepseekCurrency: String
    var deepseekIsOnline: Bool

    var codexStale: Bool = false
    var workbuddyStale: Bool = false
    var deepseekStale: Bool = false
    var deepseekError: String? = nil
    static let liveLifetime: TimeInterval = 5 * 60
    static let cacheLifetime: TimeInterval = 24 * 60 * 60
    var age: TimeInterval { age(at: .now) }
    func age(at now: Date) -> TimeInterval { max(0, now.timeIntervalSince(fetchedAt)) }
    func evaluated(at now: Date = .now, offline: Bool = false) -> Self {
        if age(at: now) >= Self.cacheLifetime { return .placeholder }
        return offline || age(at: now) >= Self.liveLifetime ? staleCopy : self
    }
    var codexState: String { codexWeeklyNumber == "—" ? "unavailable" : (stale || codexStale ? "stale" : "live") }
    var workbuddyState: String { workbuddyPoints == nil ? "unavailable" : (stale || workbuddyStale ? "stale" : "live") }
    var deepseekState: String { deepseekBalanceText == "—" ? "unavailable" : (stale || deepseekStale || !deepseekIsOnline ? "stale" : "live") }
    var deepseekStatusText: String {
        if deepseekState == "live" { return "Online" }
        if deepseekState == "unavailable" { return deepseekError ?? "Unavailable" }
        return "缓存 / \(deepseekError ?? "stale")"
    }

    // Metadata
    let fetchedAt: Date
    var stale: Bool

    // Helper presentation accessors
    var codexTitle: String {
        if codexWeeklyRemaining != nil {
            return "Codex 每周额度"
        } else if codexFiveHourRemaining != nil {
            return "Codex 5小时额度"
        } else {
            return "Codex 额度"
        }
    }

    var codexSecondaryFiveHourRemaining: Double? {
        // Only show secondary 5h when weekly is present as the primary metric
        if codexWeeklyRemaining != nil {
            return codexFiveHourRemaining
        }
        return nil
    }

    var codexResetShortText: String? {
        guard let text = codexResetText else { return nil }
        let prefix = text.hasPrefix("重置于 ") ? "重置于 " : (text.hasPrefix("重置于") ? "重置于" : "")
        let datePart = prefix.isEmpty ? text : String(text.dropFirst(prefix.count))

        if let match = datePart.range(of: #"^\d{4}-(\d{2}-\d{2} \d{2}:\d{2})"#, options: .regularExpression) {
            let sub = String(datePart[match])
            let shortDate = String(sub.dropFirst(5)) // drops "YYYY-"
            return "重置于 \(shortDate)"
        }
        return text
    }

    static let placeholder = WidgetDisplaySnapshot(
        codexWeeklyNumber: "—",
        codexWeeklyRemaining: nil,
        codexFiveHourRemaining: nil,
        codexWeeklyReset: nil,
        codexResetText: nil,
        codexWeeklyProgress: 0.0,
        workbuddyPoints: nil,
        workbuddyPointsText: "—",
        workbuddyIsOnline: false,
        deepseekBalanceText: "—",
        deepseekCurrency: "CNY",
        deepseekIsOnline: false,
        fetchedAt: .now,
        stale: true
    )

    private enum CodingKeys: String, CodingKey {
        case codexWeeklyNumber
        case codexWeeklyRemaining
        case codexFiveHourRemaining
        case codexWeeklyReset
        case codexResetText
        case codexWeeklyProgress
        case workbuddyPoints
        case workbuddyPointsText
        case workbuddyIsOnline
        case deepseekBalanceText
        case deepseekCurrency
        case deepseekIsOnline
        case fetchedAt
        case stale, age, codexStale, workbuddyStale, deepseekStale, deepseekError

        // Legacy 2.7.0 keys
        case codex
        case workbuddy
        case deepseek
        case system
    }

    init(
        codexWeeklyNumber: String,
        codexWeeklyRemaining: Double?,
        codexFiveHourRemaining: Double?,
        codexWeeklyReset: String?,
        codexResetText: String?,
        codexWeeklyProgress: Double,
        workbuddyPoints: Double?,
        workbuddyPointsText: String,
        workbuddyIsOnline: Bool,
        deepseekBalanceText: String,
        deepseekCurrency: String,
        deepseekIsOnline: Bool,
        fetchedAt: Date,
        stale: Bool
    ) {
        self.codexWeeklyNumber = codexWeeklyNumber
        self.codexWeeklyRemaining = codexWeeklyRemaining
        self.codexFiveHourRemaining = codexFiveHourRemaining
        self.codexWeeklyReset = codexWeeklyReset
        self.codexResetText = codexResetText
        self.codexWeeklyProgress = codexWeeklyProgress
        self.workbuddyPoints = workbuddyPoints
        self.workbuddyPointsText = workbuddyPointsText
        self.workbuddyIsOnline = workbuddyIsOnline
        self.deepseekBalanceText = deepseekBalanceText
        self.deepseekCurrency = deepseekCurrency
        self.deepseekIsOnline = deepseekIsOnline
        self.fetchedAt = fetchedAt
        self.stale = stale
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        codexStale = (try? container.decode(Bool.self, forKey: .codexStale)) ?? false
        workbuddyStale = (try? container.decode(Bool.self, forKey: .workbuddyStale)) ?? false
        deepseekStale = (try? container.decode(Bool.self, forKey: .deepseekStale)) ?? false
        deepseekError = try? container.decode(String.self, forKey: .deepseekError)

        if let num = try? container.decodeIfPresent(String.self, forKey: .codexWeeklyNumber) {
            self.codexWeeklyNumber = num
            self.codexWeeklyRemaining = try? container.decodeIfPresent(Double.self, forKey: .codexWeeklyRemaining)
            self.codexFiveHourRemaining = try? container.decodeIfPresent(Double.self, forKey: .codexFiveHourRemaining)
            self.codexWeeklyReset = try? container.decodeIfPresent(String.self, forKey: .codexWeeklyReset)
            self.codexResetText = try? container.decodeIfPresent(String.self, forKey: .codexResetText)
            self.codexWeeklyProgress = (try? container.decodeIfPresent(Double.self, forKey: .codexWeeklyProgress)) ?? 0.0
            self.workbuddyPoints = try? container.decodeIfPresent(Double.self, forKey: .workbuddyPoints)
            self.workbuddyPointsText = (try? container.decodeIfPresent(String.self, forKey: .workbuddyPointsText)) ?? "—"
            self.workbuddyIsOnline = (try? container.decodeIfPresent(Bool.self, forKey: .workbuddyIsOnline)) ?? false
            self.deepseekBalanceText = (try? container.decodeIfPresent(String.self, forKey: .deepseekBalanceText)) ?? "—"
            self.deepseekCurrency = (try? container.decodeIfPresent(String.self, forKey: .deepseekCurrency)) ?? "CNY"
            self.deepseekIsOnline = (try? container.decodeIfPresent(Bool.self, forKey: .deepseekIsOnline)) ?? false
            self.fetchedAt = (try? container.decodeIfPresent(Date.self, forKey: .fetchedAt)) ?? .distantPast
            self.stale = (try? container.decodeIfPresent(Bool.self, forKey: .stale)) ?? true
            return
        }

        // Fallback for legacy 2.7.0 cache format
        let legacyCodex = (try? container.decodeIfPresent(String.self, forKey: .codex)) ?? "—"
        let legacyWorkbuddy = (try? container.decodeIfPresent(String.self, forKey: .workbuddy)) ?? "—"
        let legacyDeepseek = (try? container.decodeIfPresent(String.self, forKey: .deepseek)) ?? "—"
        self.fetchedAt = (try? container.decodeIfPresent(Date.self, forKey: .fetchedAt)) ?? .distantPast
        self.stale = (try? container.decodeIfPresent(Bool.self, forKey: .stale)) ?? true

        let numStr = legacyCodex.replacingOccurrences(of: "%", with: "").trimmingCharacters(in: .whitespacesAndNewlines)
        if let val = Double(numStr), val.isFinite {
            self.codexWeeklyNumber = String(format: "%.0f", val)
            self.codexWeeklyRemaining = val
            self.codexWeeklyProgress = min(max(val / 100.0, 0.0), 1.0)
        } else {
            self.codexWeeklyNumber = "—"
            self.codexWeeklyRemaining = nil
            self.codexWeeklyProgress = 0.0
        }
        self.codexFiveHourRemaining = nil
        self.codexWeeklyReset = nil
        self.codexResetText = nil

        self.workbuddyPointsText = legacyWorkbuddy
        let cleanedPoints = legacyWorkbuddy.replacingOccurrences(of: ",", with: "")
        self.workbuddyPoints = Double(cleanedPoints)
        self.workbuddyIsOnline = (legacyWorkbuddy != "—")

        let dsParts = legacyDeepseek.split(separator: " ")
        if dsParts.count >= 2 {
            self.deepseekBalanceText = String(dsParts[0])
            self.deepseekCurrency = String(dsParts[1])
        } else if dsParts.count == 1 {
            self.deepseekBalanceText = String(dsParts[0])
            self.deepseekCurrency = "CNY"
        } else {
            self.deepseekBalanceText = "—"
            self.deepseekCurrency = "CNY"
        }
        self.deepseekIsOnline = (legacyDeepseek != "—")
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(codexWeeklyNumber, forKey: .codexWeeklyNumber)
        try container.encodeIfPresent(codexWeeklyRemaining, forKey: .codexWeeklyRemaining)
        try container.encodeIfPresent(codexFiveHourRemaining, forKey: .codexFiveHourRemaining)
        try container.encodeIfPresent(codexWeeklyReset, forKey: .codexWeeklyReset)
        try container.encodeIfPresent(codexResetText, forKey: .codexResetText)
        try container.encode(codexWeeklyProgress, forKey: .codexWeeklyProgress)
        try container.encodeIfPresent(workbuddyPoints, forKey: .workbuddyPoints)
        try container.encode(workbuddyPointsText, forKey: .workbuddyPointsText)
        try container.encode(workbuddyIsOnline, forKey: .workbuddyIsOnline)
        try container.encode(deepseekBalanceText, forKey: .deepseekBalanceText)
        try container.encode(deepseekCurrency, forKey: .deepseekCurrency)
        try container.encode(deepseekIsOnline, forKey: .deepseekIsOnline)
        try container.encode(fetchedAt, forKey: .fetchedAt)
        try container.encode(stale, forKey: .stale)
        try container.encode(age, forKey: .age)
        try container.encode(codexStale, forKey: .codexStale)
        try container.encode(workbuddyStale, forKey: .workbuddyStale)
        try container.encode(deepseekStale, forKey: .deepseekStale)
        try container.encodeIfPresent(deepseekError, forKey: .deepseekError)
    }

    init(payload: WidgetStatusPayload, fetchedAt: Date) {
        if let shared = payload.display_snapshot {
            self = shared
            return
        }
        // 1. Codex Weekly & 5-hour
        let weeklyRem = payload.codex?.weekly?.remaining
        let fiveHourRem = payload.codex?.fiveHour?.remaining
        let chosenRem = weeklyRem ?? fiveHourRem

        let weeklyNum: String
        let progress: Double
        if let rem = chosenRem, rem.isFinite {
            weeklyNum = String(format: "%.0f", rem)
            progress = min(max(rem / 100.0, 0.0), 1.0)
        } else {
            weeklyNum = "—"
            progress = 0.0
        }

        let rawReset = payload.codex?.weekly?.reset ?? payload.codex?.fiveHour?.reset
        let resetText = Self.formatReset(rawReset)

        // 2. WorkBuddy
        let wbPoints = payload.workbuddy?.points
        let wbPointsFormatted = Self.formatWorkBuddyPoints(wbPoints)
        let wbOnline = wbPoints?.isFinite == true

        // 3. DeepSeek
        let (dsBalance, dsCurrency) = Self.formatDeepSeekBalance(payload.deepseek)
        let dsOnline = payload.deepseek?.status?.trimmingCharacters(in: .whitespacesAndNewlines) == "Online"

        self.init(
            codexWeeklyNumber: weeklyNum,
            codexWeeklyRemaining: weeklyRem,
            codexFiveHourRemaining: fiveHourRem,
            codexWeeklyReset: rawReset,
            codexResetText: resetText,
            codexWeeklyProgress: progress,
            workbuddyPoints: wbPoints,
            workbuddyPointsText: wbPointsFormatted,
            workbuddyIsOnline: wbOnline,
            deepseekBalanceText: dsBalance,
            deepseekCurrency: dsCurrency,
            deepseekIsOnline: dsOnline,
            fetchedAt: payload.fetched_at.map(Date.init(timeIntervalSince1970:)) ?? fetchedAt,
            stale: false
        )
        let failureStates = ["error", "timeout", "stale", "pending", "refreshing"]
        codexStale = payload.codex?.stale == true || failureStates.contains(payload.collection?.codex?.state ?? "")
        workbuddyStale = payload.workbuddy?.balance_stale == true || payload.workbuddy?.balance_state == "Cached"
            || failureStates.contains(payload.collection?.workbuddy?.state ?? "")
        deepseekStale = payload.deepseek?.stale == true || !dsOnline
            || failureStates.contains(payload.collection?.deepseek?.state ?? "")
        deepseekError = payload.deepseek?.error_code
    }

    var staleCopy: WidgetDisplaySnapshot {
        var copy = self
        copy.stale = true
        copy.workbuddyIsOnline = false
        copy.deepseekIsOnline = false
        return copy
    }

    private static func formatReset(_ reset: String?) -> String? {
        guard let reset = reset?.trimmingCharacters(in: .whitespacesAndNewlines), !reset.isEmpty, reset != "--" else {
            return nil
        }
        if reset.hasPrefix("重置于") {
            return reset
        }
        return "重置于 \(reset)"
    }

    private static func formatWorkBuddyPoints(_ points: Double?) -> String {
        guard let points, points.isFinite else { return "—" }
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = true
        formatter.maximumFractionDigits = 2
        formatter.minimumFractionDigits = 0
        return formatter.string(from: NSNumber(value: points)) ?? "—"
    }

    private static func formatDeepSeekBalance(_ data: WidgetDeepSeekData?) -> (String, String) {
        guard
            let balance = data?.balances?.first(where: { $0.currency == "CNY" }) ?? data?.balances?.first,
            let total = balance.totalBalance?.trimmingCharacters(in: .whitespacesAndNewlines),
            !total.isEmpty
        else {
            return ("—", "CNY")
        }

        let currency = balance.currency?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "CNY"
        if let numeric = Double(total) {
            return (String(format: "%.2f", numeric), currency.isEmpty ? "CNY" : currency)
        }
        return (total, currency.isEmpty ? "CNY" : currency)
    }

}


// Both clients publish the same normalized presentation to the loopback backend.
// No App Group entitlement or second business-data source is required.
enum DisplaySnapshotBridge {
    static func publish(_ snapshot: WidgetDisplaySnapshot, revision: String?, baseURL: String,
                        session: URLSession) async {
        guard let revision, let url = URL(string: baseURL + "/api/display-snapshot") else { return }
        struct Publication: Encodable {
            let revision: String
            let snapshot: WidgetDisplaySnapshot
        }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = 3
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONEncoder().encode(Publication(revision: revision, snapshot: snapshot))
        _ = try? await session.data(for: request)
    }
}
