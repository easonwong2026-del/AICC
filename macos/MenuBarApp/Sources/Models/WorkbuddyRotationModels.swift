import Foundation

/// WorkBuddy **夜间账号轮换**（`workbuddy.rotation.*`）的 Swift 侧模型。
///
/// 三条界线（对应 `docs/WORKBUDDY_ROTATION.md`）：
///
/// 1. **默认关闭**。没有任何自动操作会在用户没打开开关时发生。
/// 2. 计划结束时恢复的是**这一轮最初登录的账号**，不是"上一个账号"。
/// 3. 调度器活在 AICC 的账号助手进程里 —— AICC 退出就停，界面必须这么说，
///    不能暗示"退出后仍在后台跑"。
///
/// 这里只翻 JSON 与写文案，不做任何"能不能切"的判断：全部由 Rust 侧给。

// MARK: - 计划

struct WorkbuddyRotationPlan: Equatable {
    var enabled: Bool
    var timezone: String
    /// ISO 星期：1 = 周一 … 7 = 周日。
    var weekdays: [Int]
    var startMinutes: Int
    var endMinutes: Int
    var accountIds: [String]
    var dwellSeconds: Int
    var startText: String
    var endText: String

    init(json: [String: Any]) {
        enabled = json["enabled"] as? Bool ?? false
        timezone = json["timezone"] as? String ?? "Asia/Shanghai"
        weekdays = (json["weekdays"] as? [NSNumber] ?? []).map(\.intValue)
        startMinutes = (json["startMinutes"] as? NSNumber)?.intValue ?? (22 * 60 + 30)
        endMinutes = (json["endMinutes"] as? NSNumber)?.intValue ?? (7 * 60)
        accountIds = json["accountIds"] as? [String] ?? []
        dwellSeconds = (json["dwellSeconds"] as? NSNumber)?.intValue ?? 3600
        startText = json["startText"] as? String ?? WorkbuddyRotationPlan.clockText(startMinutes)
        endText = json["endText"] as? String ?? WorkbuddyRotationPlan.clockText(endMinutes)
    }

    static func clockText(_ minutes: Int) -> String {
        String(format: "%02d:%02d", minutes / 60, minutes % 60)
    }

    /// 跨午夜的计划要在界面上说清楚，否则 22:30 → 07:00 看起来像负时间。
    var crossesMidnight: Bool { endMinutes <= startMinutes }

    var windowText: String {
        crossesMidnight
            ? "\(startText) → 次日 \(endText)"
            : "\(startText) → \(endText)"
    }

    var dwellText: String {
        if dwellSeconds % 3600 == 0 { return "\(dwellSeconds / 3600) 小时" }
        return "\(dwellSeconds / 60) 分钟"
    }

    var weekdayText: String {
        if weekdays.count == 7 { return "每天" }
        let names = ["一", "二", "三", "四", "五", "六", "日"]
        let sorted = weekdays.sorted()
        if sorted == [1, 2, 3, 4, 5] { return "工作日" }
        if sorted == [6, 7] { return "周末" }
        return sorted.compactMap { day in
            (1...7).contains(day) ? "周" + names[day - 1] : nil
        }.joined(separator: "、")
    }
}

// MARK: - 调度视图

struct WorkbuddyRotationSchedule: Equatable {
    let inWindow: Bool
    let windowStart: Date?
    let windowEnd: Date?
    let nextRunAt: Date?
    let nextRunEndAt: Date?

    init(json: [String: Any]) {
        inWindow = json["inWindow"] as? Bool ?? false
        windowStart = WorkbuddyTime.parse(json["windowStart"] as? String)
        windowEnd = WorkbuddyTime.parse(json["windowEnd"] as? String)
        nextRunAt = WorkbuddyTime.parse(json["nextRunAt"] as? String)
        nextRunEndAt = WorkbuddyTime.parse(json["nextRunEndAt"] as? String)
    }

    /// 一句话说清"下一次什么时候开始"。没有下一次时如实说。
    var nextRunText: String {
        guard let nextRunAt else { return inWindow ? "正在执行窗口内" : "没有下一次" }
        return "下次 \(WorkbuddyTime.dayTime(nextRunAt))"
    }
}

// MARK: - 步骤

struct WorkbuddyRotationStep: Equatable, Identifiable {
    let id: String
    let seq: Int
    let accountId: String
    let uid: String
    let mode: String?
    /// `planned` / `switching` / `dwelling` / `done` / `skipped` / `failed` / `already_live`
    let status: String
    let skipReason: String?
    let detail: String?
    let authPrecheck: String?
    let activityBefore: WorkbuddyActivityEvidence?
    let activityAfter: WorkbuddyActivityEvidence?

    init(json: [String: Any]) {
        id = json["id"] as? String ?? ""
        seq = (json["seq"] as? NSNumber)?.intValue ?? 0
        accountId = json["accountId"] as? String ?? ""
        uid = json["uid"] as? String ?? ""
        mode = json["mode"] as? String
        status = json["status"] as? String ?? "planned"
        skipReason = json["skipReason"] as? String
        detail = json["detail"] as? String
        authPrecheck = json["authPrecheck"] as? String
        activityBefore = (json["activityBefore"] as? [String: Any])
            .map(WorkbuddyActivityEvidence.init)
        activityAfter = (json["activityAfter"] as? [String: Any])
            .map(WorkbuddyActivityEvidence.init)
    }

    var temporary: Bool { mode == "magpie_access_temporary" }

    var statusText: String {
        switch status {
        case "planned": return "等待"
        case "switching": return "正在切换"
        case "dwelling": return "停留中"
        case "done": return "已切换"
        case "skipped": return "已跳过"
        case "failed": return "失败"
        case "already_live": return "本来就是它"
        default: return status
        }
    }

    /// 跳过原因翻成人话。Rust 给稳定码，界面不解析中文。
    var skipText: String? {
        guard let skipReason else { return nil }
        switch skipReason {
        case "own_grant_rejected": return "自有授权被服务端拒绝，需要重新授权"
        case "own_grant_unreadable": return "读不到自有授权的令牌"
        case "magpie_too_short_for_dwell": return "Magpie 令牌剩余时间不够覆盖停留时长"
        case "magpie_source_missing", "magpie_uid_mismatch", "magpie_platform_mismatch":
            return "Magpie 关联与这个账号对不上，需要重新导入"
        case "magpie_access_expired", "magpie_access_empty":
            return "Magpie 访问令牌已失效，请在 Magpie 重新登录"
        case "magpie_unavailable": return "读不到 Magpie 登录态"
        case "missing_refresh": return "自有授权缺刷新令牌"
        case "no_credential": return "没有可用授权"
        case "no_account": return "账号已不存在"
        case "live": return "客户端当前登录的就是它"
        case "switch_failed": return "切换失败，已按快照回滚"
        case "interrupted": return "切换中断，客户端仍是原账号"
        default: return skipReason
        }
    }

    /// 「今日活跃」只作证据：查不到就说查不到，不说"未活跃"。
    var activityText: String {
        guard let after = activityAfter ?? activityBefore else { return "活跃未查" }
        return after.summary
    }
}

// MARK: - 一轮

struct WorkbuddyRotationRun: Equatable, Identifiable {
    let id: String
    let planDate: String
    let state: String
    let cursor: Int
    let originUid: String?
    let originAccountId: String?
    let originSnapshotTracked: Bool
    let lastTargetUid: String?
    let lastTargetAccountId: String?
    let restoreResult: String?
    let detail: String?
    let currentAccountId: String?
    let currentUid: String?
    /// 停留中时的下一次切换时刻；其它状态为 nil。
    let nextSwitchAt: Date?
    let steps: [WorkbuddyRotationStep]
    let createdAt: Date?
    let updatedAt: Date?

    init(json: [String: Any]) {
        id = json["id"] as? String ?? ""
        planDate = json["planDate"] as? String ?? ""
        state = json["state"] as? String ?? "scheduled"
        cursor = (json["cursor"] as? NSNumber)?.intValue ?? 0
        originUid = json["originUid"] as? String
        originAccountId = json["originAccountId"] as? String
        originSnapshotTracked = json["originSnapshotTracked"] as? Bool ?? false
        lastTargetUid = json["lastTargetUid"] as? String
        lastTargetAccountId = json["lastTargetAccountId"] as? String
        restoreResult = json["restoreResult"] as? String
        detail = json["detail"] as? String
        currentAccountId = json["currentAccountId"] as? String
        currentUid = json["currentUid"] as? String
        nextSwitchAt = WorkbuddyTime.parse(json["nextSwitchAt"] as? String)
        steps = (json["steps"] as? [[String: Any]] ?? []).map(WorkbuddyRotationStep.init)
        createdAt = WorkbuddyTime.parse(json["createdAt"] as? String)
        updatedAt = WorkbuddyTime.parse(json["updatedAt"] as? String)
    }

    var isTerminal: Bool {
        ["completed", "skipped", "failed", "recovery_required"].contains(state)
    }

    var stateText: String {
        switch state {
        case "scheduled": return "已排定"
        case "prechecking": return "检查凭据中"
        case "switching": return "正在切换"
        case "dwelling": return "停留中"
        case "restoring": return "正在恢复原账号"
        case "completed": return "已完成"
        case "skipped": return "已跳过"
        case "failed": return "失败"
        case "recovery_required": return "需要你确认"
        default: return state
        }
    }

    /// 是否需要人来处理。只有两种情况：恢复没成功，或不敢覆盖用户的选择。
    var needsAttention: Bool { state == "recovery_required" }

    var originUidShort: String? {
        originUid.map { String($0.prefix(8)) }
    }

    var restoreText: String {
        switch restoreResult {
        case "restored": return "已恢复最初账号"
        case "already_origin": return "本来就在最初账号"
        case "no_origin": return "没有改动过登录态"
        case "snapshot_corrupt": return "原账号快照已损坏，未覆盖"
        case "snapshot_missing": return "原账号快照丢失，未覆盖"
        case "snapshot_untracked": return "缺少快照校验，未覆盖"
        case "live_changed": return "登录取被手动改过，未覆盖"
        case "restore_failed": return "写回失败，需要手动登录"
        default: return isTerminal ? "未恢复" : "等待恢复"
        }
    }

    /// 最近一条能解释"现在在干什么"的话。
    var summary: String {
        if let detail, !detail.isEmpty { return detail }
        if let nextSwitchAt {
            return "\(stateText) · 下次 \(WorkbuddyTime.clock(nextSwitchAt))"
        }
        if let currentUid, !isTerminal {
            return "\(stateText) · \(String(currentUid.prefix(8)))"
        }
        return stateText
    }
}

// MARK: - 整体状态

struct WorkbuddyRotationStatus: Equatable {
    let plan: WorkbuddyRotationPlan
    let schedule: WorkbuddyRotationSchedule
    let liveUid: String?
    let liveLoggedIn: Bool
    let clientInstalled: Bool
    let current: WorkbuddyRotationRun?
    let lastRun: WorkbuddyRotationRun?
    let history: [WorkbuddyRotationRun]
    let runsWhileAppClosed: Bool
    /// 支持 / 不支持的边界，界面上只说必要的那几条。
    let supportedTimezones: [String]

    init(json: [String: Any]) {
        plan = WorkbuddyRotationPlan(json: json["plan"] as? [String: Any] ?? [:])
        schedule = WorkbuddyRotationSchedule(json: json["schedule"] as? [String: Any] ?? [:])
        let live = json["live"] as? [String: Any] ?? [:]
        liveUid = live["uid"] as? String
        liveLoggedIn = live["loggedIn"] as? Bool ?? false
        clientInstalled = (json["client"] as? [String: Any])?["installed"] as? Bool ?? false
        current = (json["current"] as? [String: Any]).map(WorkbuddyRotationRun.init)
        lastRun = (json["lastRun"] as? [String: Any]).map(WorkbuddyRotationRun.init)
        history = (json["history"] as? [[String: Any]] ?? []).map(WorkbuddyRotationRun.init)
        runsWhileAppClosed = (json["scheduler"] as? [String: Any])?["runsWhileAppClosed"] as? Bool ?? false
        supportedTimezones = (json["limits"] as? [String: Any])?["supportedTimezones"] as? [String]
            ?? ["Asia/Shanghai"]
    }

    /// 主界面那一行状态：一句话说清"会不会动、什么时候动"。
    var headline: String {
        if !clientInstalled { return "没找到 WorkBuddy 客户端" }
        if plan.accountIds.isEmpty { return "还没有选择参与轮换的账号" }
        if !plan.enabled { return "已关闭" }
        if let current { return current.summary }
        if !liveLoggedIn { return "客户端当前没有登录，计划不会执行" }
        return schedule.nextRunText
    }
}

// MARK: - 预演

struct WorkbuddyRotationDryRunStep: Equatable, Identifiable {
    let accountId: String
    let nickname: String?
    let uid: String?
    /// `switch` / `skip` / `already_live`
    let outcome: String
    let reason: String?
    let detail: String?
    let plannedAt: Date?
    let temporary: Bool
    let remainingSeconds: Int?

    var id: String { accountId }

    init(json: [String: Any]) {
        accountId = json["accountId"] as? String ?? ""
        nickname = json["nickname"] as? String
        uid = json["uid"] as? String
        outcome = json["outcome"] as? String ?? "skip"
        reason = json["reason"] as? String
        detail = json["detail"] as? String
        plannedAt = WorkbuddyTime.parse(json["plannedAt"] as? String)
        temporary = json["temporary"] as? Bool ?? false
        remainingSeconds = (json["remainingSeconds"] as? NSNumber)?.intValue
    }

    var summary: String {
        switch outcome {
        case "switch":
            let at = plannedAt.map { WorkbuddyTime.clock($0) } ?? "—"
            let mode = temporary ? "临时" : "长期"
            return "\(at) 切到 \(nickname ?? accountId)（\(mode)）"
        case "already_live":
            return "\(nickname ?? accountId)：客户端已经登录它"
        default:
            let why = detail ?? reason ?? "不可用"
            return "跳过 \(nickname ?? accountId)：\(why)"
        }
    }
}

struct WorkbuddyRotationDryRun: Equatable {
    let steps: [WorkbuddyRotationDryRunStep]
    let warnings: [String]
    let startAt: Date?
    let endAt: Date?
    let originLoggedIn: Bool

    init(json: [String: Any]) {
        steps = (json["steps"] as? [[String: Any]] ?? []).map(WorkbuddyRotationDryRunStep.init)
        warnings = json["warnings"] as? [String] ?? []
        startAt = WorkbuddyTime.parse(json["startAt"] as? String)
        endAt = WorkbuddyTime.parse(json["endAt"] as? String)
        originLoggedIn = json["originLoggedIn"] as? Bool ?? false
    }
}
