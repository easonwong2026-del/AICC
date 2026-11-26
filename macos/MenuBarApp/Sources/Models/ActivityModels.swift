import Foundation

/// 成长活跃状态（PR #103）：`growth.activity.status` 的 Swift 侧模型。
///
/// 这一层只做一件事：把 JSON 翻成界面能画的东西，并且**守住四条结论**
/// （`docs/activity-rules-2026-11-17.md`）：
///
/// 1. 签到与活跃是两个字段。签到成功不代表热力墙亮了，界面上不许合并成"今日已完成"。
/// 2. 签到结论分五种来源：服务端确认、AICC 本地确认、确认未签、尝试失败、未确认。
///    **"本地没记录"不等于"没签到"**——可能是在官方客户端签的。
/// 3. `nextTierRemaining` 是服务端算好的，**不做 `tier - streak` 的减法**。
/// 4. `todayActive == nil` 是"没问到"，不是"未活跃"。

// MARK: - 汇总

struct ActivitySummary: Equatable {
    let total: Int
    let active: Int
    let pending: Int
    let unknown: Int
    let needsAuth: Int

    static let zero = ActivitySummary(total: 0, active: 0, pending: 0, unknown: 0, needsAuth: 0)

    init(total: Int, active: Int, pending: Int, unknown: Int, needsAuth: Int) {
        self.total = total
        self.active = active
        self.pending = pending
        self.unknown = unknown
        self.needsAuth = needsAuth
    }

    init(json: [String: Any]?) {
        let json = json ?? [:]
        total = (json["total"] as? NSNumber)?.intValue ?? 0
        active = (json["active"] as? NSNumber)?.intValue ?? 0
        pending = (json["pending"] as? NSNumber)?.intValue ?? 0
        unknown = (json["unknown"] as? NSNumber)?.intValue ?? 0
        needsAuth = (json["needsAuth"] as? NSNumber)?.intValue ?? 0
    }
}

// MARK: - 诊断

struct ActivityDiagnosis: Equatable {
    /// `active` / `checkin_only` / `checkin_unverified` / `inactive` / `stale_stats` /
    /// `unknown` / `needs_auth` / `unsupported`
    let code: String
    /// 后端给的原话。完整句子只进 tooltip，不占表格宽度。
    let label: String
    /// 该做什么。**由后端给**，界面不自己编。
    let action: String
    let needsAction: Bool

    init(json: [String: Any]?) {
        let json = json ?? [:]
        code = json["code"] as? String ?? "unknown"
        label = json["label"] as? String ?? "活跃状态未知"
        action = json["action"] as? String ?? ""
        needsAction = json["needsAction"] as? Bool ?? false
    }

    init(code: String, label: String, action: String, needsAction: Bool) {
        self.code = code
        self.label = label
        self.action = action
        self.needsAction = needsAction
    }

    var isActive: Bool { code == "active" }
    var isUnknown: Bool { code == "unknown" }

    /// 表格里用的短标签（四字以内）。完整说法在 `label` / `action`。
    var shortLabel: String {
        switch code {
        case "active": return "已活跃"
        case "checkin_only": return "未活跃"
        case "checkin_unverified": return "签到待查"
        case "inactive": return "今日未用"
        case "stale_stats": return "待落库"
        case "needs_auth": return "需授权"
        case "unsupported": return "不支持"
        default: return "未知"
        }
    }

    /// 这一类需要用户打开客户端做一次真实任务。
    var needsClient: Bool {
        code == "checkin_only" || code == "inactive"
    }

    var tooltip: String {
        action.isEmpty ? label : "\(label) · \(action)"
    }

    /// 表格右列只放得下四五个字；完整说法在 `action` / `tooltip`。
    var shortAction: String {
        switch code {
        case "checkin_unverified": return "看签到"
        case "needs_auth": return "去授权"
        case "unknown": return "刷新"
        case "stale_stats": return "稍后刷新"
        default: return ""
        }
    }
}

// MARK: - 今日签到

struct ActivityCheckinState: Equatable {
    /// `confirmed` / `absent` / `failed` / `unconfirmed` / `auth_unavailable`
    let state: String
    /// `server`（服务端签到状态接口）或 `local`（AICC 本地记录）。
    let source: String?
    /// 本地记录原始状态：`success` / `already` / `failed` / `unknown` …
    let status: String?
    let checkedAt: String?
    let localDate: String?
    /// 服务端签到记录里最近的一天，用于排查"服务端认可、AICC 没记录"。
    let serverLastDate: String?
    let note: String?

    init(json: [String: Any]?) {
        let json = json ?? [:]
        state = json["state"] as? String ?? "unconfirmed"
        source = (json["source"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        status = (json["status"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        checkedAt = (json["checkedAt"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        localDate = (json["localDate"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        serverLastDate = (json["serverLastDate"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        note = (json["note"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    init(
        state: String,
        source: String?,
        status: String? = nil,
        checkedAt: String? = nil,
        localDate: String? = nil,
        serverLastDate: String? = nil,
        note: String? = nil
    ) {
        self.state = state
        self.source = source
        self.status = status
        self.checkedAt = checkedAt
        self.localDate = localDate
        self.serverLastDate = serverLastDate
        self.note = note
    }

    /// 今日签到一句话。**只说签到，不说活跃。**
    var text: String {
        switch state {
        case "confirmed":
            return source == "server" ? "已签（服务端）" : "已签（本地）"
        case "absent": return "未签到"
        case "failed": return "签到失败"
        case "auth_unavailable": return "—"
        default: return "未确认"
        }
    }

    /// tooltip：把证据链说清楚。
    var detail: String {
        var parts: [String] = [text]
        if let note { parts.append(note) }
        if state == "confirmed", source == "server", localDate == nil {
            parts.append("AICC 本地没有今天的签到记录，服务端签到接口确认已签")
        }
        if let serverLastDate { parts.append("服务端最近签到 \(serverLastDate)") }
        if let status { parts.append("本地状态 \(status)") }
        return parts.joined(separator: " · ")
    }

    var isConfirmed: Bool { state == "confirmed" }
}

// MARK: - 单账号

struct ActivityAccountState: Identifiable, Equatable {
    let accountId: String
    let nickname: String
    /// `ok` / `partial` / `unauthorized` / `missing_credential` / `unsupported` / `failed`
    let state: String
    /// 服务端自己的"今天"（`/heatmap` 的 `today.date`）。
    let serverToday: String?
    /// `nil` = 没问到，**不是**未活跃。
    let todayActive: Bool?
    let todayScore: Int?
    let todayNote: String?
    let streakDays: Int?
    let nextTier: String?
    let nextTierRemaining: Int?
    let monthActiveDays: Int?
    let tiers: [RewardTierState]
    let lastActiveDate: String?
    let activeDays7: Int?
    /// 服务端最后更新时间：只作排查线索，不参与判定。
    let serverUpdatedAt: String?
    let checkin: ActivityCheckinState
    let diagnosis: ActivityDiagnosis
    let errors: [GrowthFieldError]
    let fetchedAt: String

    var id: String { accountId }

    init(json: [String: Any]) {
        accountId = json["accountId"] as? String ?? ""
        nickname = json["nickname"] as? String ?? ""
        state = json["state"] as? String ?? "failed"
        serverToday = (json["serverToday"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        todayActive = json["todayActive"] as? Bool
        todayScore = (json["todayScore"] as? NSNumber)?.intValue
        todayNote = (json["todayNote"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        streakDays = (json["streakDays"] as? NSNumber)?.intValue
        nextTier = (json["nextTier"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        nextTierRemaining = (json["nextTierRemaining"] as? NSNumber)?.intValue
        monthActiveDays = (json["monthActiveDays"] as? NSNumber)?.intValue
        tiers = (json["tiers"] as? [[String: Any]] ?? []).map { RewardTierState(json: $0) }
        lastActiveDate = (json["lastActiveDate"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        activeDays7 = (json["activeDays7"] as? NSNumber)?.intValue
        serverUpdatedAt = (json["serverUpdatedAt"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        checkin = ActivityCheckinState(json: json["checkin"] as? [String: Any])
        diagnosis = ActivityDiagnosis(json: json["diagnosis"] as? [String: Any])
        errors = (json["errors"] as? [[String: Any]] ?? []).map { GrowthFieldError(json: $0) }
        fetchedAt = json["fetchedAt"] as? String ?? ""
    }

    init(
        accountId: String,
        nickname: String,
        state: String,
        serverToday: String?,
        todayActive: Bool?,
        todayScore: Int?,
        todayNote: String?,
        streakDays: Int?,
        nextTier: String?,
        nextTierRemaining: Int?,
        monthActiveDays: Int?,
        tiers: [RewardTierState],
        lastActiveDate: String?,
        activeDays7: Int?,
        serverUpdatedAt: String?,
        checkin: ActivityCheckinState,
        diagnosis: ActivityDiagnosis,
        errors: [GrowthFieldError] = [],
        fetchedAt: String
    ) {
        self.accountId = accountId
        self.nickname = nickname
        self.state = state
        self.serverToday = serverToday
        self.todayActive = todayActive
        self.todayScore = todayScore
        self.todayNote = todayNote
        self.streakDays = streakDays
        self.nextTier = nextTier
        self.nextTierRemaining = nextTierRemaining
        self.monthActiveDays = monthActiveDays
        self.tiers = tiers
        self.lastActiveDate = lastActiveDate
        self.activeDays7 = activeDays7
        self.serverUpdatedAt = serverUpdatedAt
        self.checkin = checkin
        self.diagnosis = diagnosis
        self.errors = errors
        self.fetchedAt = fetchedAt
    }

    /// 今日活跃。三态分开说。
    var activeText: String {
        switch todayActive {
        case .some(true):
            if let score = todayScore { return "已活跃 · \(score) 分" }
            return "已活跃"
        case .some(false):
            return "未活跃"
        case .none:
            return "—"
        }
    }

    var checkinText: String { checkin.text }

    var streakText: String {
        guard let streakDays else { return "—" }
        return "连登 \(streakDays) 天"
    }

    /// 下一档进度：数字来自服务端，不做减法。
    var nextTierText: String {
        guard let nextTier else { return "—" }
        let days = Self.tierDays(nextTier).map(String.init) ?? nextTier
        guard let remaining = nextTierRemaining else { return "\(days) 天档" }
        return "\(days) 天档 · 还差 \(remaining) 天"
    }

    /// 三档一句话：`7 天 已领 · 14 天 已领 · 28 天 未达成`。
    var tierLine: String {
        guard !tiers.isEmpty else { return "—" }
        return tiers
            .sorted { $0.days < $1.days }
            .map { "\($0.days) 天 \($0.stateLabel)" }
            .joined(separator: " · ")
    }

    /// 最近 7 天的活跃天数 + 上一次活跃日期。查不到就不出现。
    var recentText: String? {
        var parts: [String] = []
        if let activeDays7 { parts.append("近 7 天活跃 \(activeDays7) 天") }
        if let lastActiveDate {
            let label = lastActiveDate == serverToday ? "今天" : String(lastActiveDate.suffix(5))
            parts.append("上次 \(label)")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// 统计延迟时的一行提示。只有服务端"有分数但没标记活跃"才会出现。
    var staleNote: String? {
        diagnosis.code == "stale_stats" ? "今天有分数未落库，稍后回查" : nil
    }

    var statusLabel: String {
        switch state {
        case "ok": return "已读取"
        case "partial": return "部分字段缺失"
        case "unauthorized": return "需要重新授权"
        case "missing_credential": return "没有可用凭据"
        case "unsupported": return "国际版不支持"
        default: return "读取失败"
        }
    }

    static func tierDays(_ raw: String) -> Int? {
        guard raw.hasSuffix("d") else { return nil }
        return Int(raw.dropLast())
    }
}

// MARK: - 全量

struct ActivityFleetState: Equatable {
    let accounts: [ActivityAccountState]
    let summary: ActivitySummary
    let fetchedAt: String
    let readOnly: Bool

    static let empty = ActivityFleetState(
        accounts: [],
        summary: .zero,
        fetchedAt: "",
        readOnly: true
    )

    init(accounts: [ActivityAccountState], summary: ActivitySummary, fetchedAt: String, readOnly: Bool) {
        self.accounts = accounts
        self.summary = summary
        self.fetchedAt = fetchedAt
        self.readOnly = readOnly
    }

    init(json: [String: Any]) {
        accounts = (json["accounts"] as? [[String: Any]] ?? []).map { ActivityAccountState(json: $0) }
        summary = ActivitySummary(json: json["summary"] as? [String: Any])
        fetchedAt = json["fetchedAt"] as? String ?? ""
        readOnly = json["readOnly"] as? Bool ?? true
    }

    var needsAction: [ActivityAccountState] {
        accounts.filter { $0.diagnosis.needsAction }
    }

    var delayed: [ActivityAccountState] {
        accounts.filter { $0.diagnosis.code == "stale_stats" }
    }

    /// 顶栏一句话。**只写真实数字**：0 待处理时不写"待处理 0"。
    var summaryText: String {
        guard !accounts.isEmpty else { return "还没有可检查的国内版账号" }
        var parts = ["\(summary.total) 个账号"]
        if summary.active > 0 { parts.append("已活跃 \(summary.active)") }
        if summary.pending > 0 { parts.append("待处理 \(summary.pending)") }
        if summary.unknown > 0 { parts.append("未知 \(summary.unknown)") }
        if summary.needsAuth > 0 { parts.append("需授权 \(summary.needsAuth)") }
        return parts.joined(separator: " · ")
    }

    func account(_ accountId: String) -> ActivityAccountState? {
        accounts.first { $0.accountId == accountId }
    }
}

#if DEBUG || AICC_VISUAL_TEST
extension ActivityFleetState {
    /// 视觉回归与单测样本：**不是真实账号数据**。
    ///
    /// 覆盖五种现状：已活跃 / 签到确认但未活跃 / 签到未确认 / 疑似延迟 / 没问到。
    static func mock() -> ActivityFleetState {
        let locked = [
            RewardTierState(tier: "7d", days: 7, status: "locked", state: "locked", claimable: false),
            RewardTierState(tier: "14d", days: 14, status: "locked", state: "locked", claimable: false),
            RewardTierState(tier: "28d", days: 28, status: "locked", state: "locked", claimable: false),
        ]
        let claimed = [
            RewardTierState(tier: "7d", days: 7, status: "claimed", state: "claimed", claimable: false),
            RewardTierState(tier: "14d", days: 14, status: "claimed", state: "claimed", claimable: false),
            RewardTierState(tier: "28d", days: 28, status: "locked", state: "locked", claimable: false),
        ]

        return ActivityFleetState(
            accounts: [
                ActivityAccountState(
                    accountId: "acc-wb-1",
                    nickname: "主账号",
                    state: "ok",
                    serverToday: "2026-11-18",
                    todayActive: true,
                    todayScore: 4,
                    todayNote: "今日已活跃，继续点亮成长热力墙",
                    streakDays: 18,
                    nextTier: "28d",
                    nextTierRemaining: 10,
                    monthActiveDays: 18,
                    tiers: claimed,
                    lastActiveDate: "2026-11-18",
                    activeDays7: 7,
                    serverUpdatedAt: "2026-11-18T02:30:00+08:00",
                    checkin: ActivityCheckinState(
                        state: "confirmed",
                        source: "server",
                        serverLastDate: "2026-11-18"
                    ),
                    diagnosis: ActivityDiagnosis(
                        code: "active",
                        label: "今日已活跃",
                        action: "",
                        needsAction: false
                    ),
                    fetchedAt: "2026-11-18T02:43:00Z"
                ),
                ActivityAccountState(
                    accountId: "acc-wb-2",
                    nickname: "运营小号",
                    state: "ok",
                    serverToday: "2026-11-18",
                    todayActive: false,
                    todayScore: 0,
                    todayNote: "开始使用以点亮今日热力墙",
                    streakDays: 0,
                    nextTier: "7d",
                    nextTierRemaining: 6,
                    monthActiveDays: 1,
                    tiers: locked,
                    lastActiveDate: "2026-11-13",
                    activeDays7: 1,
                    serverUpdatedAt: "2026-11-13T17:11:53+08:00",
                    checkin: ActivityCheckinState(
                        state: "confirmed",
                        source: "server",
                        serverLastDate: "2026-11-18"
                    ),
                    diagnosis: ActivityDiagnosis(
                        code: "checkin_only",
                        label: "签到已完成，今日未活跃",
                        action: "去 WorkBuddy 客户端做一个真实任务，回来刷新回查",
                        needsAction: true
                    ),
                    fetchedAt: "2026-11-18T02:43:00Z"
                ),
                ActivityAccountState(
                    accountId: "acc-wb-3",
                    nickname: "设计外包",
                    state: "ok",
                    serverToday: "2026-11-18",
                    todayActive: false,
                    todayScore: 0,
                    todayNote: "开始使用以点亮今日热力墙",
                    streakDays: 3,
                    nextTier: "7d",
                    nextTierRemaining: 4,
                    monthActiveDays: 3,
                    tiers: locked,
                    lastActiveDate: "2026-11-16",
                    activeDays7: 3,
                    serverUpdatedAt: "2026-11-18T02:00:00+08:00",
                    checkin: ActivityCheckinState(
                        state: "unconfirmed",
                        source: "server",
                        note: "本地没有当天记录，服务端签到状态未取到"
                    ),
                    diagnosis: ActivityDiagnosis(
                        code: "checkin_unverified",
                        label: "签到状态未确认",
                        action: "去签到页确认，或等下一次自动签到",
                        needsAction: true
                    ),
                    fetchedAt: "2026-11-18T02:43:00Z"
                ),
                ActivityAccountState(
                    accountId: "acc-wb-4",
                    nickname: "备用账号",
                    state: "ok",
                    serverToday: "2026-11-18",
                    todayActive: false,
                    todayScore: 30,
                    todayNote: "开始使用以点亮今日热力墙",
                    streakDays: 2,
                    nextTier: "7d",
                    nextTierRemaining: 5,
                    monthActiveDays: 2,
                    tiers: locked,
                    lastActiveDate: "2026-11-17",
                    activeDays7: 2,
                    serverUpdatedAt: "2026-11-18T01:50:00+08:00",
                    checkin: ActivityCheckinState(
                        state: "confirmed",
                        source: "local",
                        status: "already",
                        localDate: "2026-11-18"
                    ),
                    diagnosis: ActivityDiagnosis(
                        code: "stale_stats",
                        label: "服务端有分数但未标记活跃（可能延迟）",
                        action: "稍后点刷新回查，不用重复做事",
                        needsAction: false
                    ),
                    fetchedAt: "2026-11-18T02:43:00Z"
                ),
            ],
            summary: ActivitySummary(total: 4, active: 1, pending: 3, unknown: 0, needsAuth: 0),
            fetchedAt: "2026-11-18T02:43:00Z",
            readOnly: true
        )
    }
}
#endif
