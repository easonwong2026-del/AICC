import Foundation

/// 成长活跃状态（PR #103）：`growth.activity.status` 的 Swift 侧模型。
///
/// 这一层只做一件事：把 JSON 翻成界面能画的东西，并且**守住三条实测结论**
/// （`docs/activity-rules-2026-11-17.md`）：
///
/// 1. 签到与活跃是两个字段。签到成功不代表热力墙亮了，界面上也不许合并成一句"今日已完成"。
/// 2. `nextTierRemaining` 是服务端算好的，**不做 `tier - streak` 的减法**：
///    实测有 `连登 0 天 / 7 天档还差 6 天` 的账号。
/// 3. `todayActive == nil` 是"没问到"，不是"未活跃"。把未知当 false 会催用户
///    重复去做一件可能已经做完的事。

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
    /// `active` / `checkin_only` / `checkin_missing` / `stale_stats` / `unknown` / `needs_auth` / `unsupported`
    let code: String
    /// 后端给的原话。**界面直接展示，不自己拼**：两种"没活跃"的原因不同，动作也不同。
    let label: String
    let needsAction: Bool

    init(json: [String: Any]?) {
        let json = json ?? [:]
        code = json["code"] as? String ?? "unknown"
        label = json["label"] as? String ?? "活跃状态未知"
        needsAction = json["needsAction"] as? Bool ?? false
    }

    init(code: String, label: String, needsAction: Bool) {
        self.code = code
        self.label = label
        self.needsAction = needsAction
    }

    var isActive: Bool { code == "active" }
    var isUnknown: Bool { code == "unknown" }

    /// 表格里用的四字以内标签。后端 `label` 是完整句子（也带动作指引），
    /// 放进表格会把列撑爆；完整句子走 tooltip，不丢信息。
    var shortLabel: String {
        switch code {
        case "active": return "已活跃"
        case "checkin_only": return "未活跃"
        case "checkin_missing": return "签到未跑"
        case "stale_stats": return "待落库"
        case "needs_auth": return "需授权"
        case "unsupported": return "不支持"
        default: return "未知"
        }
    }
}

// MARK: - 当天签到

struct ActivityCheckinState: Equatable {
    /// 服务端原话：`success` / `already` / `failed` …
    let status: String
    let checkedAt: String
    /// 签到记录的本地日期。只有等于服务端今天时才出现在这里。
    let localDate: String
    /// `auto` / `manual`。
    let source: String?

    init(json: [String: Any]) {
        status = json["status"] as? String ?? ""
        checkedAt = json["checkedAt"] as? String ?? ""
        localDate = json["localDate"] as? String ?? ""
        source = (json["source"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    init(status: String, checkedAt: String, localDate: String, source: String?) {
        self.status = status
        self.checkedAt = checkedAt
        self.localDate = localDate
        self.source = source
    }

    /// `already` 是"服务端说今天已经签过了"，不是失败。
    var statusLabel: String {
        switch status {
        case "success": return "签到成功"
        case "already": return "今日已签"
        case "failed": return "签到失败"
        default: return status.isEmpty ? "已签到" : status
        }
    }

    var sourceLabel: String {
        switch source {
        case "auto": return "自动"
        case "manual": return "手动"
        default: return "—"
        }
    }
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
    /// 服务端给用户看的那句话。
    let todayNote: String?
    let streakDays: Int?
    let nextTier: String?
    let nextTierRemaining: Int?
    let monthActiveDays: Int?
    let tiers: [RewardTierState]
    let lastActiveDate: String?
    let activeDays7: Int?
    let updatedAt: String?
    let checkin: ActivityCheckinState?
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
        updatedAt = (json["updatedAt"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        checkin = (json["checkin"] as? [String: Any]).map { ActivityCheckinState(json: $0) }
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
        updatedAt: String?,
        checkin: ActivityCheckinState?,
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
        self.updatedAt = updatedAt
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

    /// 今日签到。**它只说明签到跑没跑，不说明活跃。**
    var checkinText: String {
        guard let checkin else { return "今天没有签到记录" }
        return "\(checkin.statusLabel)（\(checkin.sourceLabel)）"
    }

    var streakText: String {
        guard let streakDays else { return "—" }
        return "连登 \(streakDays) 天"
    }

    /// 下一档进度：数字来自服务端，不做减法。
    var nextTierText: String {
        guard let nextTier else { return "—" }
        // 服务端给的是 `7d` / `14d` / `28d`；认不出来就原样显示，不猜。
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
    ///
    /// 表格里只有一行的位置，所以日期压成 `MM-DD`；完整日期在 tooltip 里。
    var recentText: String? {
        var parts: [String] = []
        if let activeDays7 { parts.append("近 7 天活跃 \(activeDays7) 天") }
        if let lastActiveDate {
            let label = lastActiveDate == serverToday ? "今天" : String(lastActiveDate.suffix(5))
            parts.append("上次 \(label)")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// 统计延迟时给一句解释：别让用户看着"未活跃"去重复做事。
    ///
    /// 只留一行字，完整说法（"过一会儿点刷新回查"）在 tooltip 里。
    var staleNote: String? {
        diagnosis.code == "stale_stats" ? "今天有行为未落库，稍后回查" : nil
    }

    /// tooltip 用的完整说法。
    var staleHelp: String? {
        diagnosis.code == "stale_stats"
            ? "今天已经有过行为，但服务端热力墙还没落库；过一会儿点「刷新」再看，不用重复做事"
            : nil
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
        // `7d` / `14d` / `28d`
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

extension ActivityFleetState {
    /// 视觉回归与单测样本：**不是真实账号数据**。
    ///
    /// 覆盖四种现状：今天已活跃 / 签到在但未活跃 / 统计延迟 / 没问到。
    static func mock() -> ActivityFleetState {
        ActivityFleetState(
            accounts: [
                ActivityAccountState(
                    accountId: "acc-wb-1",
                    nickname: "主账号",
                    state: "ok",
                    serverToday: "2026-11-17",
                    todayActive: true,
                    todayScore: 105,
                    todayNote: "今日已活跃，继续点亮成长热力墙",
                    streakDays: 17,
                    nextTier: "28d",
                    nextTierRemaining: 11,
                    monthActiveDays: 17,
                    tiers: [
                        RewardTierState(tier: "7d", days: 7, status: "claimed", state: "claimed", claimable: false),
                        RewardTierState(tier: "14d", days: 14, status: "claimed", state: "claimed", claimable: false),
                        RewardTierState(tier: "28d", days: 28, status: "locked", state: "locked", claimable: false),
                    ],
                    lastActiveDate: "2026-11-17",
                    activeDays7: 7,
                    updatedAt: "2026-11-17T22:38:04+08:00",
                    checkin: ActivityCheckinState(
                        status: "already",
                        checkedAt: "2026-11-17T01:03:01Z",
                        localDate: "2026-11-17",
                        source: "auto"
                    ),
                    diagnosis: ActivityDiagnosis(code: "active", label: "今日已活跃", needsAction: false),
                    fetchedAt: "2026-11-17T14:40:00Z"
                ),
                ActivityAccountState(
                    accountId: "acc-wb-2",
                    nickname: "运营小号",
                    state: "ok",
                    serverToday: "2026-11-17",
                    todayActive: false,
                    todayScore: 0,
                    todayNote: "开始使用以点亮今日热力墙",
                    streakDays: 0,
                    nextTier: "7d",
                    nextTierRemaining: 6,
                    monthActiveDays: 1,
                    tiers: [
                        RewardTierState(tier: "7d", days: 7, status: "locked", state: "locked", claimable: false),
                        RewardTierState(tier: "14d", days: 14, status: "locked", state: "locked", claimable: false),
                        RewardTierState(tier: "28d", days: 28, status: "locked", state: "locked", claimable: false),
                    ],
                    lastActiveDate: "2026-11-13",
                    activeDays7: 1,
                    updatedAt: "2026-11-13T17:11:53+08:00",
                    checkin: ActivityCheckinState(
                        status: "already",
                        checkedAt: "2026-11-17T01:03:00Z",
                        localDate: "2026-11-17",
                        source: "auto"
                    ),
                    diagnosis: ActivityDiagnosis(code: "checkin_only", label: "签到已完成，今日未活跃", needsAction: true),
                    fetchedAt: "2026-11-17T14:40:00Z"
                ),
                ActivityAccountState(
                    accountId: "acc-wb-3",
                    nickname: "设计外包",
                    state: "ok",
                    serverToday: "2026-11-17",
                    todayActive: false,
                    todayScore: 0,
                    todayNote: "开始使用以点亮今日热力墙",
                    streakDays: 3,
                    nextTier: "7d",
                    nextTierRemaining: 4,
                    monthActiveDays: 3,
                    tiers: [
                        RewardTierState(tier: "7d", days: 7, status: "locked", state: "locked", claimable: false),
                        RewardTierState(tier: "14d", days: 14, status: "locked", state: "locked", claimable: false),
                        RewardTierState(tier: "28d", days: 28, status: "locked", state: "locked", claimable: false),
                    ],
                    lastActiveDate: "2026-11-16",
                    activeDays7: 3,
                    updatedAt: "2026-11-17T21:00:00+08:00",
                    checkin: ActivityCheckinState(
                        status: "already",
                        checkedAt: "2026-11-17T01:01:01Z",
                        localDate: "2026-11-17",
                        source: "auto"
                    ),
                    diagnosis: ActivityDiagnosis(code: "stale_stats", label: "今天有行为，活跃尚未落库（可能延迟）", needsAction: false),
                    fetchedAt: "2026-11-17T14:40:00Z"
                ),
                ActivityAccountState(
                    accountId: "acc-wb-4",
                    nickname: "备用账号",
                    state: "partial",
                    serverToday: nil,
                    todayActive: nil,
                    todayScore: nil,
                    todayNote: nil,
                    streakDays: 0,
                    nextTier: "7d",
                    nextTierRemaining: 7,
                    monthActiveDays: 0,
                    tiers: [],
                    lastActiveDate: nil,
                    activeDays7: nil,
                    updatedAt: nil,
                    checkin: nil,
                    diagnosis: ActivityDiagnosis(code: "unknown", label: "活跃状态未知", needsAction: false),
                    fetchedAt: "2026-11-17T14:40:00Z"
                ),
            ],
            summary: ActivitySummary(total: 4, active: 1, pending: 2, unknown: 1, needsAuth: 0),
            fetchedAt: "2026-11-17T14:40:00Z",
            readOnly: true
        )
    }
}
