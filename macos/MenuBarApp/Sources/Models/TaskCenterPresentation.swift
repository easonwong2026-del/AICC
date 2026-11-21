import Foundation

/// 签到任务中心的纯展示模型（UI V5 Phase 2A-2）。
///
/// 这一层只做三件事：
/// 1. 把四个任务域（签到 / 成长 / 旅行 / Credits）的业务状态翻译成同一套短状态词；
/// 2. 把行文案压到「一行标题 + 一行说明」，机制说明留给 help；
/// 3. 为分区标题与页面摘要提供**与表格同源**的计数。
///
/// 它不持有 `ConsoleStore`、不发请求、不触发任何写操作，所以能进 `swift test`。
///
/// 两条硬规则：
/// - 服务端没有明确结论的状态**永远不是**「已完成」；
/// - 「已完成 / 领取成功」只在服务端确认后出现，`unknown` 只能是「待确认 / 待查询」。

// MARK: - 语气

enum TaskCenterTone: Equatable {
    case ok
    case attention
    case danger
    case neutral
}

// MARK: - 状态词汇

/// 页面唯一的状态词表。视图不得再自行拼接状态文案。
enum TaskCenterStatus: Equatable {
    case normal
    case done
    case pending
    case accepted
    case inProgress
    case running
    case claimable
    case claimed
    case needsConfirm
    case needsLogin
    case unsupported
    case failed
    case locked
    case unknown
    case idle
    case partial

    var title: String {
        switch self {
        case .normal: return ""
        case .done: return "已完成"
        case .pending: return "待处理"
        case .accepted: return "已接受"
        case .inProgress: return "进行中"
        case .running: return "执行中"
        case .claimable: return "可领取"
        case .claimed: return "领取成功"
        case .needsConfirm: return "待确认"
        case .needsLogin: return "需要登录"
        case .unsupported: return "暂不支持"
        case .failed: return "执行失败"
        case .locked: return "已锁定"
        case .unknown: return "待查询"
        case .idle: return "暂无活动"
        case .partial: return "部分失败"
        }
    }

    var tone: TaskCenterTone {
        switch self {
        case .normal, .done, .claimed: return .ok
        case .pending, .claimable, .needsConfirm, .partial: return .attention
        case .needsLogin, .failed: return .danger
        case .accepted, .inProgress, .running, .unsupported, .locked, .unknown, .idle: return .neutral
        }
    }

    /// 正常状态只画一个点，不写字（V5：正常状态弱化）。
    var isSilent: Bool { self == .normal }

    var isDone: Bool { self == .done || self == .claimed }

    /// 需要用户处理或等待：待处理、可领取、进行中。
    var isPending: Bool {
        switch self {
        case .pending, .accepted, .inProgress, .claimable: return true
        default: return false
        }
    }

    /// 要计入「执行异常」的状态。**`待确认` 也算**：结果没定就不能当正常。
    var isProblem: Bool {
        switch self {
        case .failed, .needsLogin, .needsConfirm, .partial: return true
        default: return false
        }
    }
}

// MARK: - 业务状态 → 展示状态

extension CheckinState {
    var taskCenterStatus: TaskCenterStatus {
        switch self {
        case .claimed, .already: return .done
        case .unchecked, .waiting, .stopped: return .pending
        case .unknown: return .needsConfirm
        case .unauthorized, .noCredential: return .needsLogin
        case .failed: return .failed
        case .unsupported: return .unsupported
        case .busy: return .running
        case .disabled: return .unknown
        }
    }
}

extension GrowthTaskStatus {
    var taskCenterStatus: TaskCenterStatus {
        switch self {
        case .notAccepted: return .pending
        case .accepted: return .accepted
        case .inProgress: return .inProgress
        case .completed: return .claimable
        case .claimed: return .claimed
        case .locked: return .locked
        case .unknown: return .unknown
        }
    }
}

extension GrowthAccountScan {
    /// 账号行的状态：`ok` 只留一个绿点，异常才写字。
    var taskCenterStatus: TaskCenterStatus {
        switch state {
        case "ok": return .normal
        case "partial": return .partial
        case "failed": return .failed
        case "unauthorized", "missing_credential": return .needsLogin
        default: return .unknown
        }
    }
}

extension TravelStatus.Stage {
    /// 旅行阶段 → 状态词。`arrived` 只有在未领取时才是「可领取」。
    func taskCenterStatus(claimed: Bool) -> TaskCenterStatus {
        switch self {
        case .dispatchable: return .pending
        case .traveling: return .inProgress
        case .arrived: return claimed ? .claimed : .claimable
        case .doneToday: return claimed ? .claimed : .done
        case .noBuddy, .unsupported: return .unsupported
        case .failed: return .failed
        case .unknown: return .unknown
        }
    }
}

extension QoderCreditsReport {
    /// 只认服务端回执。没有 `claimable` 就不能显示「可领取」。
    var taskCenterStatus: TaskCenterStatus {
        if claimed == true { return .claimed }
        switch state {
        case "claimable": return .claimable
        case "already", "done": return .claimed
        case "nothing": return .idle
        case "no_credential", "unauthorized": return .needsLogin
        case "unsupported": return .unsupported
        case "busy": return .running
        default: return .failed
        }
    }
}

// MARK: - 计数

/// 分区标题的计数：与表格行同源，避免「标题 8/12、表格 5 行」这种自相矛盾。
struct TaskCenterTally: Equatable {
    private(set) var rows = 0
    private(set) var done = 0
    private(set) var pending = 0
    private(set) var problems = 0
    private(set) var claimable = 0

    init(_ statuses: [TaskCenterStatus] = []) {
        for status in statuses { add(status) }
    }

    mutating func add(_ status: TaskCenterStatus) {
        rows += 1
        if status.isDone { done += 1 }
        if status.isPending { pending += 1 }
        if status.isProblem { problems += 1 }
        if status == .claimable { claimable += 1 }
    }
}

// MARK: - 每日签到

struct CheckinRowDisplay: Identifiable, Equatable {
    let accountId: String
    let name: String
    /// 行内一行说明。完整原文同时进 help，不在这里截断。
    let detail: String
    let status: TaskCenterStatus
    let reward: String
    let hasReward: Bool
    /// 最近一次签到/检查的时间（北京时间口径）；没有记录就是 `—`。
    let lastRunText: String
    let autoEnabled: Bool
    let isBusy: Bool
    /// 这个账号当前是否允许发起签到：授权失效与不支持的平台不发无意义的写请求。
    let isActionable: Bool
    /// 不可执行时的原因（直接进 help）。
    let blockedReason: String?
    /// 单账号「立即签到」是否可点：本行没在跑、也没有别的账号在跑/批量在跑。
    let canRun: Bool
    /// 这个账号是否有资格**开启**自动签到：不支持签到、授权失效、缺少凭据都不行。
    /// 已经开启的账号不受这条限制 —— 必须允许用户把它关掉。
    let canEnableAuto: Bool
    /// 自动签到开关当前不可用的原因（直接进 help）；`nil` 表示可以切换。
    let autoLockReason: String?
    let canToggleAuto: Bool

    var id: String { accountId }

    init(row: CheckinAccountRow, acting: String?, batching: Bool) {
        accountId = row.accountId
        name = row.nickname.isEmpty ? "未命名账号" : row.nickname
        let busy = row.running || acting == row.accountId
        isBusy = busy
        status = busy ? .running : row.state.taskCenterStatus
        detail = CheckinRowDisplay.detail(row, busy: busy)
        hasReward = row.credits != nil
        reward = row.credits.map { "+\($0)" } ?? "—"
        lastRunText = row.checkedAt.map { CheckinClock.describe($0) } ?? "—"
        autoEnabled = row.enabled
        switch row.state {
        case .unsupported:
            isActionable = false
            blockedReason = "该平台不支持签到"
        case .unauthorized, .noCredential:
            isActionable = false
            blockedReason = "授权失效：先到「AI 账号」重新登录，再回来签到"
        default:
            // 失败但原因是授权失效：再点一次也只会拿到同一个 401。
            if row.errorKind == "unauthorized" {
                isActionable = false
                blockedReason = "授权失效：先到「AI 账号」重新登录，再回来签到"
            } else {
                isActionable = true
                blockedReason = nil
            }
        }
        canRun = !busy && acting == nil && !batching && isActionable
        canEnableAuto = isActionable
        // 已开启的账号永远允许关闭：异常账号不能被卡在「停不下来」的状态。
        if busy || acting != nil || batching {
            autoLockReason = "正在执行，结束后再切换"
        } else if !row.enabled && !isActionable {
            autoLockReason = row.state == .unsupported
                ? "该平台不支持签到，无法开启自动签到"
                : "授权失效：先重新登录，再开启自动签到"
        } else {
            autoLockReason = nil
        }
        canToggleAuto = autoLockReason == nil
    }

    static func rows(
        _ fleet: CheckinFleetStatus,
        acting: String?,
        batching: Bool
    ) -> [CheckinRowDisplay] {
        fleet.sortedRows.map { CheckinRowDisplay(row: $0, acting: acting, batching: batching) }
    }

    /// 一句话说明「谁签的 / 为什么没成」。技术机制不进这一行。
    static func detail(_ row: CheckinAccountRow, busy: Bool) -> String {
        if busy { return row.running ? "正在提交" : "正在处理" }
        switch row.state {
        case .claimed:
            return "AICC 已确认"
        case .already:
            return row.claimedByAiccToday ? "今天已由 AICC 签到" : "已由其他客户端签到"
        case .unchecked:
            return "服务端确认未签到"
        case .unknown:
            return "服务端没有给出明确结论"
        case .unauthorized:
            return "授权失效，重新登录后再试"
        case .noCredential:
            return "账号没有可用授权"
        case .failed:
            let text = row.errorDetail?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return text.isEmpty ? "服务端拒绝了请求" : text
        case .unsupported:
            return "该平台不支持签到"
        case .disabled:
            return "自动签到未开启"
        case .waiting:
            return "等待计划时间"
        case .stopped:
            return "开关已关闭，未提交"
        case .busy:
            return "已有一次执行在进行"
        }
    }
}

// MARK: - 成长中心

struct GrowthTaskDisplay: Identifiable, Equatable {
    let code: String
    let title: String
    let condition: String
    let progress: String
    let reward: String
    let status: TaskCenterStatus
    /// 只有「需人工 / 待识别」才显示类型短标签；自动类型不再占用列宽。
    let classLabel: String?
    let classReason: String
    /// 最近一次执行出了问题才显示的一行红字；正常不占高度。
    let problemText: String?

    var id: String { code }

    init(task: GrowthTaskRow, latestRun: GrowthRunRecord?) {
        code = task.code
        title = task.title
        condition = task.condition
        progress = task.progressText
        reward = task.rewardText
        status = task.status.taskCenterStatus
        switch task.taskClass {
        case .manual:
            classLabel = "需人工"
        case .unknown:
            classLabel = "待识别"
        case .automatic, .realAction:
            classLabel = nil
        }
        classReason = task.classReason
        problemText = GrowthTaskDisplay.problemText(latestRun)
    }

    static func problemText(_ run: GrowthRunRecord?) -> String? {
        guard let run, run.outcome.isProblem else { return nil }
        let head = "上次\(run.action.title)\(run.outcome.title)"
        if let detail = run.errorDetail, !detail.isEmpty {
            return "\(head)：\(detail)"
        }
        return head
    }
}

struct GrowthAccountDisplay: Identifiable, Equatable {
    let accountId: String
    let name: String
    /// `Lv.3 · 120 能量 · 连签 5 天`；没有数据时为空串（不显示破折号行）。
    let profileText: String
    /// `5 / 19 完成` 或 `12 个任务`。
    let taskText: String
    /// `2 待领奖` / `1 可接受` / `3 进行中`；无事可做时为 `—`。
    let pendingText: String
    let pendingIsAttention: Bool
    let lastRunText: String
    let status: TaskCenterStatus
    /// 接口错误、授权来源、接口前缀等技术信息，只进 help。
    let help: String
    let tasks: [GrowthTaskDisplay]
    let isExpanded: Bool

    var id: String { accountId }

    init(scan: GrowthAccountScan, runs: GrowthRuns?, expanded: Bool) {
        accountId = scan.accountId
        name = scan.nickname
        profileText = scan.profile.summaryText == "—" ? "" : scan.profile.summaryText
        if let completed = scan.profile.completedTasks,
           let total = scan.profile.totalTasks, total > 0 {
            taskText = "\(completed) / \(total) 完成"
        } else if scan.counts.total > 0 {
            taskText = "\(scan.counts.total) 个任务"
        } else {
            taskText = "—"
        }

        let counts = scan.counts
        if counts.claimable > 0 {
            pendingText = "\(counts.claimable) 待领奖"
            pendingIsAttention = true
        } else if counts.acceptable > 0 {
            pendingText = "\(counts.acceptable) 可接受"
            pendingIsAttention = true
        } else if counts.inProgress > 0 {
            pendingText = "\(counts.inProgress) 进行中"
            pendingIsAttention = false
        } else if counts.manual > 0 {
            pendingText = "\(counts.manual) 需人工"
            pendingIsAttention = false
        } else {
            pendingText = ""
            pendingIsAttention = false
        }

        let recent = runs?.recent.first
        lastRunText = recent.map { GrowthAccountDisplay.runText($0) } ?? "—"
        status = scan.taskCenterStatus
        isExpanded = expanded
        tasks = scan.tasks.map {
            GrowthTaskDisplay(task: $0, latestRun: runs?.latestByTask[$0.code])
        }
        help = GrowthAccountDisplay.helpText(scan)
    }

    static func rows(
        _ result: GrowthScanResult,
        runs: [String: GrowthRuns],
        expanded: String?
    ) -> [GrowthAccountDisplay] {
        result.scans.map {
            GrowthAccountDisplay(scan: $0, runs: runs[$0.accountId], expanded: expanded == $0.accountId)
        }
    }

    static func runText(_ run: GrowthRunRecord) -> String {
        var parts: [String] = []
        switch (run.action, run.outcome) {
        case (.accept, .ok): parts.append("接单成功")
        case (.claim, .ok): parts.append("领奖成功")
        case (.claim, .already): parts.append("已领过")
        case (.claim, .rejected): parts.append("领奖被拒")
        case (.claim, .unknown): parts.append("领奖待确认")
        case (.claim, .failed): parts.append("领奖失败")
        case (.accept, _): parts.append("接单\(run.outcome.title)")
        }
        if let credit = run.credit, credit > 0 { parts.append("+\(credit)") }
        if let date = run.executedAt {
            // 与签到同一口径（北京时间），「今天/昨天」比 MM-dd 更容易读。
            parts.append(CheckinClock.describe(date))
        }
        return parts.joined(separator: " · ")
    }

    static func helpText(_ scan: GrowthAccountScan) -> String {
        var lines: [String] = []
        if let source = scan.authSourceText {
            lines.append("授权来源：\(source)")
        }
        for error in scan.errors {
            lines.append("\(error.field)：\(error.message)")
        }
        if let prefix = scan.pathPrefix, !prefix.isEmpty {
            lines.append("接口前缀 \(prefix)")
        }
        return lines.joined(separator: "\n")
    }
}

// MARK: - Buddy 旅行

struct TravelRowDisplay: Identifiable, Equatable {
    let accountId: String
    let name: String
    /// 账号副行：`Magpie 授权` / `上次检查 11:20` / `还没检查`。
    let note: String
    /// 地点 + 阶段，一行内说清「现在在哪、到哪一步」。
    let statusText: String
    let status: TaskCenterStatus
    /// `+30` 或 `—`。
    let reward: String
    /// `已领取` / `待领取`；没有奖励时为空串。
    let rewardNote: String
    let warning: String?
    let help: String
    let autoEnabled: Bool
    /// 这个阶段允许领取 / 派出（不看是否正在执行），用来保持按钮位置稳定。
    let isClaimable: Bool
    let isDispatchable: Bool
    /// 现在能不能点：阶段允许且本行不在执行。
    let canDepart: Bool
    let canClaim: Bool
    let isBusy: Bool
    /// 这个账号是否有资格**开启**自动旅行。
    /// 只有「该档位没有成长中心」算真正不支持；`no_buddy` 与查询失败都不改资格。
    let canEnableAuto: Bool
    /// 自动旅行开关当前不可用的原因（直接进 help）；`nil` 表示可以切换。
    let autoLockReason: String?
    let canToggleAuto: Bool

    var id: String { accountId }

    init(row: TravelAccountRow, acting: String?) {
        accountId = row.accountId
        name = row.nickname
        let busy = acting == row.accountId
        isBusy = busy
        let status = row.status
        self.status = status?.stage.taskCenterStatus(claimed: status?.claimed ?? false) ?? .unknown
        if let status, status.stage == .dispatchable, !status.locationName.isEmpty {
            // `destinationText` 对「可派出」只给城市名，这里补上阶段，避免只看到「上海」。
            statusText = "\(status.locationName) · 等待派出"
        } else {
            statusText = status?.destinationText ?? "待检查"
        }

        if let credits = status?.rewardCredits, credits > 0, status?.claimed == true {
            reward = "+\(credits)"
            rewardNote = "已领取"
        } else if let credits = status?.rewardCredits, credits > 0, status?.canClaim == true {
            reward = "+\(credits)"
            rewardNote = "待领取"
        } else {
            reward = "—"
            rewardNote = ""
        }

        if row.isBorrowed {
            note = "Magpie 授权"
        } else if let raw = status?.lastCheckedAt,
                  let date = TravelStatus.date(fromRFC3339: raw) {
            note = "上次检查 \(TravelStatus.timeFormatter.string(from: date))"
        } else {
            note = "还没检查"
        }

        // 失败时 `destinationText` 已经就是那条错误，再挂一遍就成了同屏重复。
        warning = status?.stage == .failed ? status?.conflict : status?.warningText
        autoEnabled = row.travelEnabled
        isClaimable = status?.canClaim == true
        isDispatchable = status?.canDepart == true
        canDepart = isDispatchable && !busy
        canClaim = isClaimable && !busy
        // 只有 unsupported 是「这个账号/档位根本没有成长中心」；
        // no_buddy（还没生成 Buddy）与 failed（这次没查到）都不阻止开启，避免误伤。
        canEnableAuto = status?.stage != .unsupported
        if busy {
            autoLockReason = "正在执行，结束后再切换"
        } else if !row.travelEnabled && !canEnableAuto {
            autoLockReason = "该档位没有成长中心，无法开启自动旅行"
        } else {
            autoLockReason = nil
        }
        canToggleAuto = autoLockReason == nil
        help = TravelRowDisplay.helpText(status)
    }

    static func rows(_ rows: [TravelAccountRow], acting: String?) -> [TravelRowDisplay] {
        rows.map { TravelRowDisplay(row: $0, acting: acting) }
    }

    static func helpText(_ status: TravelStatus?) -> String {
        var lines: [String] = []
        if let status {
            if let raw = status.lastCheckedAt,
               let date = TravelStatus.date(fromRFC3339: raw) {
                lines.append("上次检查 \(TravelStatus.shortFormatter.string(from: date))")
            }
            if status.dailyLimitReached {
                lines.append("今天已经达到服务端的出行/领取上限")
            }
            if let error = status.lastError, !error.isEmpty {
                lines.append("上次错误：\(error)")
            }
            if let conflict = status.conflict, !conflict.isEmpty {
                lines.append("检测到其他客户端也在操作这个账号：\(conflict)")
            }
        }
        return lines.joined(separator: "\n")
    }
}

// MARK: - Qoder 每日 Credits

struct CreditRunDisplay: Identifiable, Equatable {
    let id: Int
    let trigger: String
    let outcome: String
    let detail: String
    let time: String
    let isProblem: Bool

    init(run: QoderCreditRun) {
        id = run.id
        trigger = run.triggerLabel
        outcome = run.outcomeLabel
        detail = run.errorDetail?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        time = QuotaPresentation.localTimestamp(run.executedAt)
            ?? String(run.executedAt.prefix(16)).replacingOccurrences(of: "T", with: " ")
        isProblem = !run.isSuccess
    }
}

struct CreditsRowDisplay: Identifiable, Equatable {
    let accountId: String
    let name: String
    let edition: String
    /// `今日` 列的完整结论，只进 help；列内用状态词。
    let detail: String
    let status: TaskCenterStatus
    /// `+100` 或 `—`。
    let reward: String
    /// `待领取` / `已领取`；无奖励为空串。
    let rewardNote: String
    let lastRunOutcome: String
    let lastRunTime: String
    let lastRunIsProblem: Bool
    let autoEnabled: Bool
    let supportsAuto: Bool
    let canCheck: Bool
    let canClaim: Bool
    let isBusy: Bool
    let runs: [CreditRunDisplay]

    var id: String { accountId }

    init(
        item: QoderCreditsAutoItem,
        report: QoderCreditsReport?,
        runs: [QoderCreditRun],
        busy: String?
    ) {
        accountId = item.accountId
        name = item.displayName
        edition = item.editionName
        isBusy = busy == item.accountId
        supportsAuto = item.supported
        autoEnabled = item.autoEnabled

        if !item.supported {
            status = .unsupported
            detail = "该版本没有已证实的 Credits 接口"
        } else if let report {
            status = report.taskCenterStatus
            detail = report.statusText
        } else {
            status = .unknown
            detail = "还没查询。点「检查今日」向服务端确认。"
        }

        // 只认这次查询/领取返回的今日奖励。历史记录里的数字不是今天的，
        // 不能拿来填「奖励」列（否则「暂无活动 +100」会自相矛盾）。
        let credits = report?.after?.todayCredits ?? report?.info?.todayCredits
        if let credits {
            reward = "+\(WorkerQuotaRow.format(credits))"
        } else {
            reward = "—"
        }
        switch status {
        case .claimed: rewardNote = "已领取"
        case .claimable: rewardNote = "待领取"
        default: rewardNote = ""
        }

        let latest: QoderCreditRun? = runs.first ?? item.lastRun
        let creditRun = latest.map { CreditRunDisplay(run: $0) }
        lastRunOutcome = creditRun?.outcome ?? "—"
        lastRunTime = creditRun?.time ?? ""
        lastRunIsProblem = creditRun?.isProblem ?? false
        self.runs = runs.prefix(5).map { CreditRunDisplay(run: $0) }

        canCheck = !isBusy && item.supported
        // 只有本次会话确认「可领取」才允许点；领过之后立刻收回，防重复点击。
        canClaim = !isBusy
            && item.supported
            && report?.claimed != true
            && report?.taskCenterStatus == .claimable
    }

    static func rows(
        items: [QoderCreditsAutoItem],
        reports: [String: QoderCreditsReport],
        runs: [String: [QoderCreditRun]],
        busy: String?
    ) -> [CreditsRowDisplay] {
        items.map {
            CreditsRowDisplay(
                item: $0,
                report: reports[$0.accountId],
                runs: runs[$0.accountId] ?? [],
                busy: busy
            )
        }
    }
}

// MARK: - 分区标题

/// 成长域的异常拆分：账号级（授权/扫描）与任务级（执行记录）分开数，
/// 但页面摘要与分区标题用的是同一个 `total`，两处不会打架。
struct GrowthProblemBreakdown: Equatable {
    let needsLogin: Int
    let accounts: Int
    let tasks: Int

    var total: Int { needsLogin + accounts + tasks }

    init(rows: [GrowthAccountDisplay]) {
        needsLogin = rows.filter { $0.status == .needsLogin }.count
        accounts = rows.filter { $0.status == .failed || $0.status == .partial }.count
        tasks = rows.reduce(0) { total, row in
            total + row.tasks.filter { $0.problemText != nil }.count
        }
    }
}

/// 分区标题的补充信息。
///
/// 只放页面摘要里**没有**的事实（自动状态、账号数、不支持数）：
/// 完成/待处理/异常三个数字已经在摘要里，标题再写一遍就是同屏重复。
enum TaskCenterSummary {
    static func checkin(_ rows: [CheckinRowDisplay]) -> String {
        let auto = rows.filter(\.autoEnabled).count
        return auto > 0 ? "自动 \(auto) 个" : "自动未开启"
    }

    static func growth(auto: GrowthAutoStatus, scanned: Bool) -> String {
        guard scanned else { return "尚未扫描" }
        return auto.enabled ? "自动管理已开启" : "仅手动"
    }

    static func travel(_ rows: [TravelRowDisplay]) -> String? {
        let auto = rows.filter(\.autoEnabled).count
        return auto > 0 ? "自动 \(auto) 个" : nil
    }

    static func credits(_ rows: [CreditsRowDisplay]) -> String? {
        guard !rows.isEmpty else { return nil }
        var parts = ["\(rows.count) 个账号"]
        let unsupported = rows.filter { $0.status == .unsupported }.count
        if unsupported > 0 { parts.append("\(unsupported) 个不支持") }
        return parts.joined(separator: " · ")
    }
}

// MARK: - 自动 / 手动区分

/// 按钮权重。规则：只有「真的会写服务端」的动作才配主按钮。
enum TaskCenterEmphasis: Equatable {
    case primary
    case secondary
}

extension GrowthAutoStatus {
    /// 自动管理关着时 tick 是只读检查，因此不能给主按钮。
    var tickEmphasis: TaskCenterEmphasis { enabled ? .primary : .secondary }
    var modeLabel: String { enabled ? "自动管理" : "仅手动" }

    /// 按钮文案必须说出它**可能做什么**：
    /// 自动管理开启时这一轮可能接单/领奖，不能叫「检查」。
    var tickTitle: String { enabled ? "接单 / 领奖" : "只读检查" }

    var tickHelp: String {
        enabled
            ? "执行一轮：可能按服务端下发的结果接单并领取奖励；不会补签、兑换或抽奖。"
            : "只读检查：不会接受任务，也不会领取奖励。"
    }

    var modeHelp: String {
        enabled
            ? "自动管理已开启：只接单与领奖，不代替你完成任何任务。"
            : "当前仅手动：「只读检查」不会接受任务或领取奖励。"
    }
}

// MARK: - 页面摘要

/// 页面顶部三个数字。**按当前任务域统计**，与表格同一批状态派生，
/// 避免旧页面「摘要 11 项完成、表格 8 行」这种自相矛盾。
///
/// 加载中且没有任何数据时三个值都是 `—`：不伪造 0。
struct TaskCenterHeadline: Equatable {
    let doneLabel: String
    let done: String
    let pendingLabel: String
    let pending: String
    let problemLabel: String
    let problems: String

    var hasProblems: Bool { problems != "0" && problems != "—" }

    private static func value(_ number: Int, loading: Bool) -> String {
        loading ? "—" : "\(number)"
    }

    static func checkin(_ rows: [CheckinRowDisplay], loading: Bool) -> TaskCenterHeadline {
        let tally = TaskCenterTally(rows.map(\.status))
        return TaskCenterHeadline(
            doneLabel: "今日完成",
            done: value(tally.done, loading: loading),
            pendingLabel: "待处理",
            pending: value(tally.pending, loading: loading),
            problemLabel: "执行异常",
            problems: value(tally.problems, loading: loading)
        )
    }

    static func growth(
        _ rows: [GrowthAccountDisplay],
        claimed: Int,
        pending: Int,
        loading: Bool
    ) -> TaskCenterHeadline {
        let problems = GrowthProblemBreakdown(rows: rows).total
        return TaskCenterHeadline(
            doneLabel: "已领取",
            done: value(claimed, loading: loading),
            pendingLabel: "待处理",
            pending: value(pending, loading: loading),
            problemLabel: "执行异常",
            problems: value(problems, loading: loading)
        )
    }

    static func travel(_ rows: [TravelRowDisplay], loading: Bool) -> TaskCenterHeadline {
        let tally = TaskCenterTally(rows.map(\.status))
        return TaskCenterHeadline(
            doneLabel: "今日完成",
            done: value(tally.done, loading: loading),
            pendingLabel: "待领取",
            pending: value(tally.claimable, loading: loading),
            problemLabel: "执行异常",
            problems: value(tally.problems, loading: loading)
        )
    }

    static func credits(_ rows: [CreditsRowDisplay], loading: Bool) -> TaskCenterHeadline {
        let tally = TaskCenterTally(rows.map(\.status))
        return TaskCenterHeadline(
            doneLabel: "今日完成",
            done: value(tally.done, loading: loading),
            pendingLabel: "可领取",
            pending: value(tally.claimable, loading: loading),
            problemLabel: "执行异常",
            problems: value(tally.problems, loading: loading)
        )
    }
}
