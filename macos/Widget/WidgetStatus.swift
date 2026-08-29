import Foundation

struct WidgetStatusPayload: Decodable {
    let codex: WidgetCodexData?
    let workbuddy: WidgetWorkBuddyData?
    let deepseek: WidgetDeepSeekData?
    let system: WidgetSystemData?

    enum CodingKeys: String, CodingKey {
        case codex
        case workbuddy
        case deepseek
        case system
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
    let balanceState: String?
    let balanceAgeSeconds: Int?
    let balanceUpdatedAt: String?
    let balanceStale: Bool?

    enum CodingKeys: String, CodingKey {
        case points
        case balanceState = "balance_state"
        case balanceAgeSeconds = "balance_age_seconds"
        case balanceUpdatedAt = "balance_updated_at"
        case balanceStale = "balance_stale"
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

struct WidgetSystemData: Decodable {
    let status: String?
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
    let workbuddySubtitle: String
    let workbuddyIsOnline: Bool

    // DeepSeek properties
    let deepseekBalanceText: String
    let deepseekCurrency: String
    let deepseekStatusText: String
    let deepseekIsOnline: Bool

    // Metadata
    let fetchedAt: Date
    let stale: Bool

    // Legacy accessors for backward compatibility
    var codex: String {
        guard codexWeeklyNumber != "—" else { return "—" }
        return "\(codexWeeklyNumber)%"
    }

    var workbuddy: String {
        workbuddyPointsText
    }

    var deepseek: String {
        guard deepseekBalanceText != "—" else { return "—" }
        return deepseekCurrency.isEmpty ? deepseekBalanceText : "\(deepseekBalanceText) \(deepseekCurrency)"
    }

    var system: String {
        deepseekIsOnline ? "Online" : "—"
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
        workbuddySubtitle: "未连接",
        workbuddyIsOnline: false,
        deepseekBalanceText: "—",
        deepseekCurrency: "CNY",
        deepseekStatusText: "—",
        deepseekIsOnline: false,
        fetchedAt: .now,
        stale: true
    )

    init(
        codexWeeklyNumber: String,
        codexWeeklyRemaining: Double?,
        codexFiveHourRemaining: Double?,
        codexWeeklyReset: String?,
        codexResetText: String?,
        codexWeeklyProgress: Double,
        workbuddyPoints: Double?,
        workbuddyPointsText: String,
        workbuddySubtitle: String,
        workbuddyIsOnline: Bool,
        deepseekBalanceText: String,
        deepseekCurrency: String,
        deepseekStatusText: String,
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
        self.workbuddySubtitle = workbuddySubtitle
        self.workbuddyIsOnline = workbuddyIsOnline
        self.deepseekBalanceText = deepseekBalanceText
        self.deepseekCurrency = deepseekCurrency
        self.deepseekStatusText = deepseekStatusText
        self.deepseekIsOnline = deepseekIsOnline
        self.fetchedAt = fetchedAt
        self.stale = stale
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
        let (wbSubtitle, wbOnline) = Self.formatWorkBuddyStatus(payload.workbuddy)

        // 3. DeepSeek
        let (dsBalance, dsCurrency) = Self.formatDeepSeekBalance(payload.deepseek)
        let (dsStatus, dsOnline) = Self.formatDeepSeekStatus(payload.deepseek)

        self.init(
            codexWeeklyNumber: weeklyNum,
            codexWeeklyRemaining: weeklyRem ?? (payload.codex?.weekly == nil ? fiveHourRem : nil),
            codexFiveHourRemaining: fiveHourRem,
            codexWeeklyReset: rawReset,
            codexResetText: resetText,
            codexWeeklyProgress: progress,
            workbuddyPoints: wbPoints,
            workbuddyPointsText: wbPointsFormatted,
            workbuddySubtitle: wbSubtitle,
            workbuddyIsOnline: wbOnline,
            deepseekBalanceText: dsBalance,
            deepseekCurrency: dsCurrency,
            deepseekStatusText: dsStatus,
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
            workbuddySubtitle: workbuddySubtitle,
            workbuddyIsOnline: workbuddyIsOnline,
            deepseekBalanceText: deepseekBalanceText,
            deepseekCurrency: deepseekCurrency,
            deepseekStatusText: deepseekStatusText,
            deepseekIsOnline: deepseekIsOnline,
            fetchedAt: fetchedAt,
            stale: true
        )
    }

    func formattedFooterTime(at referenceDate: Date = .now) -> String {
        let diff = max(0, Int(referenceDate.timeIntervalSince(fetchedAt)))
        let timeStr: String
        if diff < 60 {
            timeStr = diff <= 5 ? "刚刚更新" : "\(diff) 秒前更新"
        } else if diff < 3600 {
            timeStr = "\(diff / 60) 分钟前更新"
        } else {
            let formatter = DateFormatter()
            formatter.dateFormat = "HH:mm"
            timeStr = "\(formatter.string(from: fetchedAt)) 更新"
        }
        return stale ? "\(timeStr) · 已缓存" : timeStr
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

    private static func formatWorkBuddyStatus(_ data: WidgetWorkBuddyData?) -> (String, Bool) {
        guard let data, let points = data.points, points.isFinite else {
            return ("未连接", false)
        }

        let isCached = data.balanceStale == true || data.balanceState == "Cached"
        let baseState = isCached ? "已缓存" : "已连接"

        if let age = data.balanceAgeSeconds, age >= 0 {
            let ageText: String
            if age < 60 {
                ageText = "刚刚"
            } else if age < 3600 {
                ageText = "\(age / 60) 分钟前"
            } else if age < 86400 {
                ageText = "\(age / 3600) 小时前"
            } else {
                ageText = "\(age / 86400) 天前"
            }
            return ("\(baseState) · \(ageText)", true)
        }

        if let updated = data.balanceUpdatedAt?.trimmingCharacters(in: .whitespacesAndNewlines), !updated.isEmpty {
            return ("\(baseState) · \(updated)", true)
        }

        return (baseState, true)
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

    private static func formatDeepSeekStatus(_ data: WidgetDeepSeekData?) -> (String, Bool) {
        guard let rawStatus = data?.status?.trimmingCharacters(in: .whitespacesAndNewlines), !rawStatus.isEmpty else {
            return ("—", false)
        }

        switch rawStatus {
        case "Online":
            return ("在线", true)
        case "Not configured":
            return ("未配置", false)
        case "Offline":
            return ("离线", false)
        case "Loading":
            return ("加载中", false)
        default:
            return (rawStatus, false)
        }
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
