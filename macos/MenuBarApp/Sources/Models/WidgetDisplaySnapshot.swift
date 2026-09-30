import Foundation

struct WidgetStatusPayload: Decodable {
    let codex_account_display_mode: CodexAccountDisplayMode?
    let display_revision: String?
    let display_snapshot: WidgetDisplaySnapshot?
    let fetched_at: Double?
    let collection: WidgetCollection?
    let google: GoogleDisplayQuota?
    let codex: WidgetCodexData?
    let workbuddy: WidgetWorkBuddyData?
    let deepseek: WidgetDeepSeekData?

    enum CodingKeys: String, CodingKey {
        case fetched_at, collection, display_revision, display_snapshot, codex_account_display_mode
        case codex, google
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
    let source: String?
    let selectionMode: String?
    let activeAccountID: String?
    let accounts: [WidgetCodexAccount]?

    let fiveHour: WidgetRateWindow?
    let weekly: WidgetRateWindow?

    enum CodingKeys: String, CodingKey {
        case stale, source
        case selectionMode = "selection_mode"
        case activeAccountID = "active_account_id"
        case accounts
        case fiveHour = "five_hour"
        case weekly
    }
}

struct WidgetCodexAccount: Codable, Equatable {
    let id: String?
    let label: String?
    let plan: String?
    let active: Bool?
    let needsReauth: Bool?
    let stale: Bool?
    let fiveHour: WidgetRateWindow?
    let weekly: WidgetRateWindow?

    enum CodingKeys: String, CodingKey {
        case id, label, plan, active
        case needsReauth = "needs_reauth"
        case stale
        case fiveHour = "five_hour"
        case weekly
    }

    var safeDisplayName: String {
        if let label = label?.trimmingCharacters(in: .whitespacesAndNewlines), !label.isEmpty {
            return label
        }
        if let id = id?.trimmingCharacters(in: .whitespacesAndNewlines), !id.isEmpty, id != "__main__" {
            return id.count > 12 ? String(id.prefix(8)) + "…" : id
        }
        return "Account"
    }
}

enum CodexAccountDisplayMode: String, CaseIterable, Identifiable, Codable {
    case automatic = "automatic"
    case one = "one"
    case two = "two"
    case all = "all"

    var id: String { rawValue }

    func displayName(localize: (String) -> String) -> String {
        switch self {
        case .automatic: return localize("Automatic")
        case .one: return localize("1 Account")
        case .two: return localize("2 Accounts")
        case .all: return localize("All Accounts")
        }
    }

    var limit: Int? {
        switch self {
        case .automatic: return 2
        case .one: return 1
        case .two: return 2
        case .all: return nil
        }
    }
}

enum CodexAccountResolver {
    static func resolve<T>(
        accounts: [T],
        selectionMode: String?,
        activeAccountID: String?,
        displayMode: CodexAccountDisplayMode,
        idProvider: (T) -> String?,
        activeProvider: (T) -> Bool?
    ) -> [T] {
        guard !accounts.isEmpty else { return [] }

        var ordered: [T] = accounts
        let isPinned = selectionMode == "pinned" || (selectionMode == nil && activeAccountID != nil)

        if isPinned, let activeID = activeAccountID {
            if let idx = ordered.firstIndex(where: { idProvider($0) == activeID }) {
                let activeItem = ordered.remove(at: idx)
                ordered.insert(activeItem, at: 0)
            }
        } else if isPinned {
            if let idx = ordered.firstIndex(where: { activeProvider($0) == true }) {
                let activeItem = ordered.remove(at: idx)
                ordered.insert(activeItem, at: 0)
            }
        }
        // OpenCodex auto: selectionMode == "auto" && activeAccountID == nil -> strict original order preserved.

        if let limit = displayMode.limit {
            return Array(ordered.prefix(limit))
        }
        return ordered
    }
}

struct WidgetRateWindow: Codable, Equatable {
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

// The same optional quota object survives payload decoding and widget caching.
struct GoogleDisplayQuota: Codable, Equatable {
    let weekly: WidgetRateWindow?
    let five_hour: WidgetRateWindow?
    let updated_epoch: Double?
    var stale: Bool?
    let error: String?

    private func valid(_ window: WidgetRateWindow?) -> WidgetRateWindow? {
        guard let value = window?.remaining, value.isFinite, (0...100).contains(value) else { return nil }
        return window
    }
    var primary: WidgetRateWindow? { valid(weekly) ?? valid(five_hour) }
    var isWeekly: Bool { valid(weekly) != nil }
    var secondary: WidgetRateWindow? { isWeekly ? valid(five_hour) : nil }
    var title: String { isWeekly ? "Google Weekly" : (primary != nil ? "Google 5h" : "Google") }
    var number: String { primary?.remaining.map { String(format: "%.0f", $0) } ?? "—" }
    var reset: String? {
        guard let text = primary?.reset?.trimmingCharacters(in: .whitespacesAndNewlines),
              !text.isEmpty, text != "--" else { return nil }
        return text
    }
    func evaluated(at now: Date) -> Self? {
        guard let updated_epoch, updated_epoch.isFinite else {
            var copy = self
            copy.stale = true
            return copy
        }
        let age = max(0, now.timeIntervalSince1970 - updated_epoch)
        if age >= WidgetDisplaySnapshot.cacheLifetime { return nil }
        var copy = self
        copy.stale = stale == true || age >= WidgetDisplaySnapshot.liveLifetime
        return copy
    }
}

struct WidgetDisplaySnapshot: Codable, Equatable {
    var google: GoogleDisplayQuota? = nil
    var googleState: String { google?.primary == nil ? "unavailable" : (stale || google?.stale == true ? "stale" : "live") }

    // Codex properties
    let codexWeeklyNumber: String
    let codexWeeklyRemaining: Double?
    let codexFiveHourRemaining: Double?
    let codexWeeklyReset: String?
    let codexResetText: String?
    let codexWeeklyProgress: Double
    var codexAccounts: [WidgetCodexAccount]? = nil
    var codexActiveAccountID: String? = nil
    var codexSelectionMode: String? = nil
    var codexSource: String? = nil
    var codexAccountDisplayMode: CodexAccountDisplayMode = .automatic

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
    var deepseekAccountStatus: String? = nil
    static let liveLifetime: TimeInterval = 5 * 60
    static let cacheLifetime: TimeInterval = 24 * 60 * 60
    var age: TimeInterval { age(at: .now) }
    func age(at now: Date) -> TimeInterval { max(0, now.timeIntervalSince(fetchedAt)) }
    func evaluated(at now: Date = .now, offline: Bool = false) -> Self {
        if age(at: now) >= Self.cacheLifetime { return .placeholder }
        var copy = offline || age(at: now) >= Self.liveLifetime ? staleCopy : self
        copy.google = google?.evaluated(at: now)
        return copy
    }
    var codexState: String { codexWeeklyNumber == "—" ? "unavailable" : (stale || codexStale ? "stale" : "live") }
    var workbuddyState: String { workbuddyPoints == nil ? "unavailable" : (stale || workbuddyStale ? "stale" : "live") }
    var deepseekState: String { deepseekBalanceText == "—" ? "unavailable" : (stale || deepseekStale ? "stale" : "live") }
    var deepseekStatusText: String {
        if deepseekState == "live" { return deepseekAccountStatus ?? "Online" }
        if deepseekState == "unavailable" { return deepseekError ?? deepseekAccountStatus ?? "Unavailable" }
        return "缓存 / \(deepseekError ?? "stale")"
    }

    var resolvedCodexAccounts: [WidgetCodexAccount]? {
        codexAccounts.map {
            CodexAccountResolver.resolve(accounts: $0, selectionMode: codexSelectionMode,
                activeAccountID: codexActiveAccountID, displayMode: codexAccountDisplayMode,
                idProvider: { $0.id }, activeProvider: { $0.active })
        }
    }

    var workbuddyCompactStatusText: String {
        workbuddyState == "live" ? "" : (workbuddyState == "stale" ? "Cached" : "Unavailable")
    }

    var deepseekCompactStatusText: String {
        if deepseekState == "stale" { return deepseekError ?? "Cached" }
        if deepseekState == "unavailable" { return deepseekStatusText }
        return deepseekAccountStatus == "Online" ? "" : (deepseekAccountStatus ?? "")
    }

    // Metadata
    let fetchedAt: Date
    var stale: Bool

    // Helper presentation accessors
    var codexTitle: String {
        if let accounts = codexAccounts, !accounts.isEmpty {
            if codexSelectionMode == "auto" && codexActiveAccountID == nil {
                return "Codex Auto"
            }
            if let activeID = codexActiveAccountID,
               let activeAccount = accounts.first(where: { $0.id == activeID }) {
                return "Codex · \(activeAccount.safeDisplayName)"
            }
            if let activeAccount = accounts.first(where: { $0.active == true }) {
                return "Codex · \(activeAccount.safeDisplayName)"
            }
        }
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

    var isCodexAutoPool: Bool {
        guard let accounts = codexAccounts, !accounts.isEmpty else { return false }
        return codexSelectionMode == "auto" && codexActiveAccountID == nil
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
        case google
        case codexWeeklyNumber
        case codexWeeklyRemaining
        case codexFiveHourRemaining
        case codexWeeklyReset
        case codexResetText
        case codexWeeklyProgress
        case codexAccounts
        case codexActiveAccountID
        case codexSelectionMode
        case codexSource, codexAccountDisplayMode
        case workbuddyPoints
        case workbuddyPointsText
        case workbuddyIsOnline
        case deepseekBalanceText
        case deepseekCurrency
        case deepseekIsOnline
        case fetchedAt
        case stale, age, codexStale, workbuddyStale, deepseekStale, deepseekError, deepseekAccountStatus

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
        codexAccountDisplayMode = (try? container.decode(CodexAccountDisplayMode.self, forKey: .codexAccountDisplayMode)) ?? .automatic
        google = try? container.decode(GoogleDisplayQuota.self, forKey: .google)
        codexStale = (try? container.decode(Bool.self, forKey: .codexStale)) ?? false
        workbuddyStale = (try? container.decode(Bool.self, forKey: .workbuddyStale)) ?? false
        deepseekStale = (try? container.decode(Bool.self, forKey: .deepseekStale)) ?? false
        deepseekAccountStatus = try? container.decode(String.self, forKey: .deepseekAccountStatus)
        deepseekError = try? container.decode(String.self, forKey: .deepseekError)

        if let num = try? container.decodeIfPresent(String.self, forKey: .codexWeeklyNumber) {
            self.codexWeeklyNumber = num
            self.codexWeeklyRemaining = try? container.decodeIfPresent(Double.self, forKey: .codexWeeklyRemaining)
            self.codexFiveHourRemaining = try? container.decodeIfPresent(Double.self, forKey: .codexFiveHourRemaining)
            self.codexWeeklyReset = try? container.decodeIfPresent(String.self, forKey: .codexWeeklyReset)
            self.codexResetText = try? container.decodeIfPresent(String.self, forKey: .codexResetText)
            self.codexWeeklyProgress = (try? container.decodeIfPresent(Double.self, forKey: .codexWeeklyProgress)) ?? 0.0
            self.codexAccounts = try? container.decodeIfPresent([WidgetCodexAccount].self, forKey: .codexAccounts)
            self.codexActiveAccountID = try? container.decodeIfPresent(String.self, forKey: .codexActiveAccountID)
            self.codexSelectionMode = try? container.decodeIfPresent(String.self, forKey: .codexSelectionMode)
            self.codexSource = try? container.decodeIfPresent(String.self, forKey: .codexSource)
            self.workbuddyPoints = try? container.decodeIfPresent(Double.self, forKey: .workbuddyPoints)
            self.workbuddyPointsText = (try? container.decodeIfPresent(String.self, forKey: .workbuddyPointsText)) ?? "—"
            self.workbuddyIsOnline = (try? container.decodeIfPresent(Bool.self, forKey: .workbuddyIsOnline)) ?? false
            self.deepseekBalanceText = (try? container.decodeIfPresent(String.self, forKey: .deepseekBalanceText)) ?? "—"
            self.deepseekCurrency = (try? container.decodeIfPresent(String.self, forKey: .deepseekCurrency)) ?? "CNY"
            self.deepseekIsOnline = (try? container.decodeIfPresent(Bool.self, forKey: .deepseekIsOnline)) ?? false
            if deepseekAccountStatus == nil && !deepseekIsOnline { deepseekStale = true }
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
        try container.encodeIfPresent(google, forKey: .google)
        try container.encode(codexWeeklyNumber, forKey: .codexWeeklyNumber)
        try container.encodeIfPresent(codexWeeklyRemaining, forKey: .codexWeeklyRemaining)
        try container.encodeIfPresent(codexFiveHourRemaining, forKey: .codexFiveHourRemaining)
        try container.encodeIfPresent(codexWeeklyReset, forKey: .codexWeeklyReset)
        try container.encodeIfPresent(codexResetText, forKey: .codexResetText)
        try container.encode(codexWeeklyProgress, forKey: .codexWeeklyProgress)
        try container.encodeIfPresent(codexAccounts, forKey: .codexAccounts)
        try container.encodeIfPresent(codexActiveAccountID, forKey: .codexActiveAccountID)
        try container.encodeIfPresent(codexSelectionMode, forKey: .codexSelectionMode)
        try container.encodeIfPresent(codexSource, forKey: .codexSource)
        try container.encode(codexAccountDisplayMode, forKey: .codexAccountDisplayMode)
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
        try container.encodeIfPresent(deepseekAccountStatus, forKey: .deepseekAccountStatus)
    }

    init(payload: WidgetStatusPayload, fetchedAt: Date) {
        if let shared = payload.display_snapshot {
            self = shared
            codexAccountDisplayMode = payload.codex_account_display_mode ?? shared.codexAccountDisplayMode
            // Old clients may publish snapshots without the new optional field.
            google = (payload.google ?? shared.google)?.evaluated(at: fetchedAt)
            return
        }
        // 1. Codex Pool & Active Account
        let accounts = payload.codex?.accounts
        let activeID = payload.codex?.activeAccountID
        let selectionMode = payload.codex?.selectionMode
        let source = payload.codex?.source

        let activeAccount = accounts?.first(where: {
            if let activeID { return $0.id == activeID }
            return $0.active == true
        })

        // 1. Codex Weekly & 5-hour
        let weeklyRem = activeAccount?.weekly?.remaining ?? payload.codex?.weekly?.remaining
        let fiveHourRem = activeAccount?.fiveHour?.remaining ?? payload.codex?.fiveHour?.remaining
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

        let rawReset = activeAccount?.weekly?.reset ?? activeAccount?.fiveHour?.reset ?? payload.codex?.weekly?.reset ?? payload.codex?.fiveHour?.reset
        let resetText = Self.formatReset(rawReset)

        // 2. WorkBuddy
        let wbPoints = payload.workbuddy?.points
        let wbPointsFormatted = Self.formatWorkBuddyPoints(wbPoints)
        let wbOnline = wbPoints?.isFinite == true

        // 3. DeepSeek
        let (dsBalance, dsCurrency) = Self.formatDeepSeekBalance(payload.deepseek)
        let dsStatus = payload.deepseek?.status?.trimmingCharacters(in: .whitespacesAndNewlines)
        let dsOnline = dsStatus == "Online"
        let dsFresh = dsOnline || dsStatus == "No balance"

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
        codexAccountDisplayMode = payload.codex_account_display_mode ?? .automatic
        codexAccounts = accounts
        codexActiveAccountID = activeID
        codexSelectionMode = selectionMode
        codexSource = source
        google = payload.google?.evaluated(at: fetchedAt)
        let failureStates = ["error", "timeout", "stale", "pending", "refreshing"]
        let activeAccountStale = activeAccount?.stale == true
        let poolStale = payload.codex?.stale == true || failureStates.contains(payload.collection?.codex?.state ?? "")
        codexStale = poolStale || activeAccountStale
        workbuddyStale = payload.workbuddy?.balance_stale == true || payload.workbuddy?.balance_state == "Cached"
            || failureStates.contains(payload.collection?.workbuddy?.state ?? "")
        deepseekStale = payload.deepseek?.stale == true || (!dsFresh && dsStatus != "Not configured")
            || failureStates.contains(payload.collection?.deepseek?.state ?? "")
        deepseekError = payload.deepseek?.error_code
        deepseekAccountStatus = dsStatus
    }

    var staleCopy: WidgetDisplaySnapshot {
        var copy = self
        copy.stale = true
        copy.codexStale = true
        if let accounts = copy.codexAccounts {
            copy.codexAccounts = accounts.map {
                WidgetCodexAccount(id: $0.id, label: $0.label, plan: $0.plan, active: $0.active,
                                   needsReauth: $0.needsReauth, stale: true, fiveHour: $0.fiveHour, weekly: $0.weekly)
            }
        }
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
    // A normal probe followed by force can each take 65s; leave transport margin.
    static let refreshTimeout: TimeInterval = 140
    static func publish(_ snapshot: WidgetDisplaySnapshot, revision: String?, baseURL: String,
                        session: URLSession, codexAccountDisplayMode: CodexAccountDisplayMode? = nil) async {
        guard let revision, let url = URL(string: baseURL + "/api/display-snapshot") else { return }
        struct Publication: Encodable {
            let revision: String
            let snapshot: WidgetDisplaySnapshot
            let codexAccountDisplayMode: CodexAccountDisplayMode?
        }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = 3
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONEncoder().encode(Publication(revision: revision, snapshot: snapshot, codexAccountDisplayMode: codexAccountDisplayMode))
        _ = try? await session.data(for: request)
    }
}
