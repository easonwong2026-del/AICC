import Foundation

struct WidgetStatusPayload: Decodable {
    let codex: WidgetCodexData?
    let workbuddy: WidgetWorkBuddyData?
    let deepseek: WidgetDeepSeekData?

    enum CodingKeys: String, CodingKey {
        case codex
        case workbuddy
        case deepseek
    }
}

struct WidgetCodexData: Decodable {
    let fiveHour: WidgetRateWindow?
    let weekly: WidgetRateWindow?

    enum CodingKeys: String, CodingKey {
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
    let points: Double?

    enum CodingKeys: String, CodingKey {
        case points
    }
}

struct WidgetDeepSeekData: Decodable {
    let status: String?
    let balances: [WidgetDeepSeekBalance]?

    enum CodingKeys: String, CodingKey {
        case status
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
    let workbuddyIsOnline: Bool

    // DeepSeek properties
    let deepseekBalanceText: String
    let deepseekCurrency: String
    let deepseekIsOnline: Bool

    // Metadata
    let fetchedAt: Date
    let stale: Bool

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
        case stale

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
            self.fetchedAt = (try? container.decodeIfPresent(Date.self, forKey: .fetchedAt)) ?? .now
            self.stale = (try? container.decodeIfPresent(Bool.self, forKey: .stale)) ?? true
            return
        }

        // Fallback for legacy 2.7.0 cache format
        let legacyCodex = (try? container.decodeIfPresent(String.self, forKey: .codex)) ?? "—"
        let legacyWorkbuddy = (try? container.decodeIfPresent(String.self, forKey: .workbuddy)) ?? "—"
        let legacyDeepseek = (try? container.decodeIfPresent(String.self, forKey: .deepseek)) ?? "—"
        self.fetchedAt = (try? container.decodeIfPresent(Date.self, forKey: .fetchedAt)) ?? .now
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
    }

    init(payload: WidgetStatusPayload, fetchedAt: Date) {
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
            fetchedAt: fetchedAt,
            stale: false
        )
    }

    var staleCopy: WidgetDisplaySnapshot {
        WidgetDisplaySnapshot(
            codexWeeklyNumber: codexWeeklyNumber,
            codexWeeklyRemaining: codexWeeklyRemaining,
            codexFiveHourRemaining: codexFiveHourRemaining,
            codexWeeklyReset: codexWeeklyReset,
            codexResetText: codexResetText,
            codexWeeklyProgress: codexWeeklyProgress,
            workbuddyPoints: workbuddyPoints,
            workbuddyPointsText: workbuddyPointsText,
            workbuddyIsOnline: workbuddyIsOnline,
            deepseekBalanceText: deepseekBalanceText,
            deepseekCurrency: deepseekCurrency,
            deepseekIsOnline: deepseekIsOnline,
            fetchedAt: fetchedAt,
            stale: true
        )
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

enum WidgetStatusStore {
    private static let key = "aicc.widget.last-display-snapshot"

    static func load() -> WidgetDisplaySnapshot? {
        guard
            let data = UserDefaults.standard.data(forKey: key),
            let snapshot = try? JSONDecoder().decode(WidgetDisplaySnapshot.self, from: data)
        else { return nil }
        return snapshot
    }

    static func save(_ snapshot: WidgetDisplaySnapshot) {
        guard !snapshot.stale, let data = try? JSONEncoder().encode(snapshot) else { return }
        UserDefaults.standard.set(data, forKey: key)
    }

    static func cachedOrPlaceholder() -> WidgetDisplaySnapshot {
        load()?.staleCopy ?? .placeholder
    }

    static func remove() {
        UserDefaults.standard.removeObject(forKey: key)
    }
}

enum WidgetStatusLoader {
    private static let endpoint = URL(string: "http://127.0.0.1:8765/api/status")!

    static func snapshot() async -> WidgetDisplaySnapshot {
        if let fresh = await load() {
            return fresh
        }
        return WidgetStatusStore.cachedOrPlaceholder()
    }

    static func load() async -> WidgetDisplaySnapshot? {
        var request = URLRequest(url: endpoint)
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 3

        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard
                let http = response as? HTTPURLResponse,
                (200..<300).contains(http.statusCode)
            else { return nil }

            let payload = try JSONDecoder().decode(WidgetStatusPayload.self, from: data)
            let snapshot = WidgetDisplaySnapshot(payload: payload, fetchedAt: .now)
            WidgetStatusStore.save(snapshot)
            return snapshot
        } catch {
            return nil
        }
    }
}
