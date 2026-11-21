import Foundation
import SwiftUI

/// 主控制台的数据中枢：账号、额度、助手状态都从这里出。
///
/// 为什么集中在一处：
/// - 切换分页不能重新起助手进程，也不能重新打一遍 Magpie；
/// - 总览页和额度页必须看到同一份快照，否则两个页面数字会打架。
///
/// 这里只做"取数"，不做任何写入；扫码、签到等未接入能力不在这里假装。
@MainActor
final class ConsoleStore: ObservableObject {
    @Published private(set) var accounts: [ConsoleAccount] = []
    @Published private(set) var snapshot: QuotaSnapshot?
    @Published private(set) var accountsError: String?
    @Published private(set) var quotaError: String?
    @Published private(set) var loadingAccounts = false
    @Published private(set) var loadingQuota = false
    @Published private(set) var lastAccountsRefresh: Date?
    @Published private(set) var lastQuotaRefresh: Date?

    /// 助手侧的统一额度看板（四平台）。与上面的 `snapshot`（Magpie 额度卡）是两条
    /// 互不替代的数据链路：Magpie 给窗口，助手给账号级余额与用量。
    @Published private(set) var workerQuotaRows: [WorkerQuotaRow] = []
    @Published private(set) var workerQuotaError: String?
    @Published private(set) var loadingWorkerQuota = false
    /// 用量窗口（7 / 30 天）。只影响用量统计，不改余额口径。
    @Published private(set) var workerQuotaWindowDays = 7

    /// Buddy 旅行的本地快照 + 运行信息。
    @Published private(set) var travelRows: [TravelAccountRow] = []
    @Published private(set) var travelAuto: TravelAutoStatus?
    @Published private(set) var travelError: String?
    @Published private(set) var travelLoading = false
    /// 正在执行动作的账号 id，用来禁用该行按钮并显示进度。
    @Published private(set) var travelActing: String?
    /// 最近一次动作的一句话结果（成功/失败都给一句），显示在区域底部。
    @Published private(set) var travelNotice: String?

    /// 成长中心（PR #67：只读）。没有扫描过就是空，不预填假数据。
    @Published private(set) var growthResult: GrowthScanResult = .empty
    @Published private(set) var growthError: String?
    @Published private(set) var growthLoading = false
    @Published private(set) var lastGrowthScan: Date?
    /// 展开某个账号的任务列表（默认只展开第一个，避免一屏全是任务）。
    @Published var expandedGrowthAccount: String?

    /// 自动管理（PR #68）。开关默认关闭，状态从 worker 读。
    @Published private(set) var growthAuto: GrowthAutoStatus = GrowthAutoStatus(json: nil)
    @Published private(set) var growthRuns: [String: GrowthRuns] = [:]
    @Published private(set) var growthActing = false
    /// 最近一次手动检查 / 开关动作的一句话结果。
    @Published private(set) var growthNotice: String?

    /// 奖励状态（PR #98：只读）。按账号缓存，只有用户点「刷新」才会联网。
    ///
    /// 分成三份而不是一个 Optional 状态：加载中、失败原因、上一次结果要能同时成立，
    /// 否则刷新失败时界面会把旧数据当成"当前状态"。
    @Published private(set) var rewardStates: [String: RewardAccountState] = [:]
    @Published private(set) var rewardFailures: [String: String] = [:]
    @Published private(set) var rewardLoading: Set<String> = []

    // MARK: - 成长活跃（PR #103）

    /// 四个账号"今天亮了没有"。**只读**，且只在用户点「刷新」时才联网。
    @Published private(set) var activity: ActivityFleetState = .empty
    @Published private(set) var activityError: String?
    @Published private(set) var activityLoading = false
    @Published private(set) var lastActivityRefresh: Date?

    /// 每日签到（PR #69）。首屏先用本地缓存出图，联网查询由用户点按钮触发。
    @Published private(set) var checkin: CheckinFleetStatus = .empty
    @Published private(set) var checkinAuto: CheckinAutoStatus = .unknown
    @Published private(set) var checkinError: String?
    @Published private(set) var checkinLoading = false
    /// 正在执行动作的账号，用来禁用该行按钮。
    @Published private(set) var checkinActing: String?
    /// 批量操作进行中。
    @Published private(set) var checkinBatching = false
    /// 最近一次动作的一句话结果。
    @Published private(set) var checkinNotice: String?

    /// Qoder 双版本多账号（PR #70）。Profile 是 Qoder 的唯一数据源，
    /// 老的 `accounts` 表只留给 WorkBuddy。
    @Published private(set) var qoderProfiles: [QoderProfile] = []
    /// Qoder 的**平台服务账号**（PR #71，v13）：Magpie 导入 / 网页授权都只写这里，
    /// 与上面的本机快照是两回事，界面上分组显示。
    @Published private(set) var qoderServiceAccounts: [QoderServiceAccount] = []
    @Published private(set) var qoderEditions: [QoderEditionInfo] = []
    /// 未完成的切换日志：只用于提示，恢复必须由用户显式确认。
    @Published private(set) var qoderPendingSwitches: [QoderPendingSwitch] = []
    @Published private(set) var qoderError: String?
    @Published private(set) var qoderNotice: String?
    @Published private(set) var qoderLoading = false
    /// 最近一次 Qoder 数据同步成功的时间（总览页"最近同步"用）。
    @Published private(set) var qoderSyncedAt: Date?
    /// 正在执行动作的 profileId，用来禁用该卡片按钮。
    @Published private(set) var qoderActing: String?

    /// 正在删除的 Qoder 服务账号 id：删除期间禁用同一账号的重复删除（PRD §4.5）。
    @Published private(set) var qoderDeleting: String?
    /// 删除失败的原因：账号仍在，界面提供重试入口。
    @Published private(set) var qoderDeleteError: String?
    /// 删除成功的一次性提示。
    @Published private(set) var qoderDeleteNotice: String?

    // MARK: WorkBuddy 客户端切换（WB-SWITCH-1）

    /// 客户端状态 + 当前登录账号 + 每个账号能不能切。`nil` = 还没查过。
    @Published private(set) var wbClientStatus: WorkbuddyClientStatus?
    @Published private(set) var wbClientError: String?
    /// 正在切换的账号 id；`restore` 用固定标记。
    @Published private(set) var wbSwitching: String?
    /// 一次切换/恢复的结论。只保留最近一条，切号时覆盖。
    @Published private(set) var wbSwitchNotice: String?
    /// 结论是失败还是成功（决定提示的色调）。
    @Published private(set) var wbSwitchFailed = false
    /// 恢复被"当前登录已变"挡住：非 nil 时界面弹确认，用户同意后走 force。
    @Published private(set) var wbRestoreNeedsConfirm: String?

    // MARK: 夜间账号轮换（WB-ROTATE-1）

    /// 计划 + 下一轮时间 + 当前这一轮 + 最近记录。`nil` = 还没查到。
    @Published private(set) var wbRotationStatus: WorkbuddyRotationStatus?
    @Published private(set) var wbRotationError: String?
    /// 正在读取轮换状态。冷启动时总览据此显示"状态待确认"，而不是"一切正常"。
    @Published private(set) var wbRotationLoading = false
    /// 一次保存 / 执行 / 恢复的结论。
    @Published private(set) var wbRotationNotice: String?
    @Published private(set) var wbRotationFailed = false
    /// 正在执行会动客户端的动作（保存配置不算）。
    @Published private(set) var wbRotationBusy = false
    /// 预演结果：只在用户点「预演」之后出现，不自动刷新。
    @Published private(set) var wbRotationDryRun: WorkbuddyRotationDryRun?

    let worker: AccountWorkerManager
    private let settings: AppSettings

    init(worker: AccountWorkerManager, settings: AppSettings) {
        self.worker = worker
        self.settings = settings
    }

    var stats: ConsoleOverviewStats { ConsoleOverviewStats(accounts: accounts) }
    var quotaSummary: ConsoleQuotaSummary { ConsoleQuotaSummary(snapshot: snapshot) }
    /// 总览页的平台统计：服务账号总数不含本机 Profile。
    var fleet: ConsoleFleetSummary {
        ConsoleFleetSummary(
            accounts: accounts,
            qoderAccounts: qoderServiceAccounts,
            qoderProfiles: qoderProfiles
        )
    }

    func clearQoderDeleteFeedback() {
        qoderDeleteError = nil
        qoderDeleteNotice = nil
    }

    /// 账号详情里的一次性结论（PAT 检测、移除结果等）。只写提示，不改数据。
    func showQoderNotice(_ text: String) {
        qoderNotice = text
    }

    /// 首次进入窗口时拉一遍。重复调用是安全的。
    ///
    /// 顺序很重要（PRD §11.4）：先把助手拉起来一次，再**并行**取四份数据。
    /// 早先串行时，Magpie 的 HTTP 超时会把账号列表一起拖到十几秒之后。
    func loadIfNeeded() async {
        #if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
        // 离屏渲染只消费固定样本，绝不启动助手或发真实 RPC。
        return
        #else
        if case .running = worker.state {} else {
            await worker.refresh()
        }
        // 助手是单线程处理请求的：先要那份最便宜的本地汇总，
        // 否则 Qoder 的目录扫描会把"今日完成"一起拖到十几秒之后。
        if tasksOverview == nil && !tasksOverviewLoading {
            await refreshTasksOverview()
        }

        let needQuota = snapshot == nil && !loadingQuota
        let needAccounts = lastAccountsRefresh == nil && !loadingAccounts
        let needQoder = qoderEditions.isEmpty && !qoderLoading
        let needWorkerQuota = workerQuotaRows.isEmpty && !loadingWorkerQuota
        let needWbClient = wbClientStatus == nil
        // 冷启动必须读一次轮换状态：总览的恢复异常与摘要计数依赖它。
        // 只读 RPC，不会签发、切换或恢复任何登录态。
        let needRotation = ConsoleColdStartLoadPolicy.needsRotation(
            status: wbRotationStatus,
            loading: wbRotationLoading
        )

        async let quota: Void = needQuota ? refreshQuota(force: false) : ()
        async let accounts: Void = needAccounts ? refreshAccounts() : ()
        async let qoder: Void = needQoder ? refreshQoder(silent: true) : ()
        async let workerQuota: Void = needWorkerQuota ? refreshWorkerQuota(force: false) : ()
        async let wbClient: Void = needWbClient ? refreshWorkbuddyClient(silent: true) : ()
        async let rotation: Void = needRotation ? refreshWorkbuddyRotation(silent: false) : ()
        _ = await (quota, accounts, qoder, workerQuota, wbClient, rotation)
        #endif
    }

    func refreshAll() async {
        async let accountsTask: Void = refreshAccounts()
        async let quotaTask: Void = refreshQuota(force: true)
        async let qoderTask: Void = refreshQoder(silent: true)
        async let wbTask: Void = refreshWorkbuddyClient(silent: true)
        _ = await (accountsTask, quotaTask, qoderTask, wbTask)
    }

    // MARK: - Qoder 双版本多账号（PR #70）

    /// 版本探测 + Profile 列表。`silent` 用于自动刷新，不覆盖已有提示。
    func refreshQoder(silent: Bool = false) async {
        guard !qoderLoading else { return }
        qoderLoading = true
        guard await ensureWorkerReady() else {
            if !silent { qoderError = workerFailureText }
            qoderLoading = false
            return
        }
        defer { qoderLoading = false }
        do {
            async let editionsRaw = worker.qoderEditionsDetect()
            async let profilesRaw = worker.qoderProfilesList()
            async let accountsRaw = worker.qoderAccountsList()
            let (editionsJSON, profilesJSON, accountsJSON) = try await (editionsRaw, profilesRaw, accountsRaw)
            qoderEditions = (editionsJSON["editions"] as? [[String: Any]] ?? [])
                .compactMap(QoderEditionInfo.init(json:))
            qoderProfiles = (profilesJSON["profiles"] as? [[String: Any]] ?? [])
                .compactMap(QoderProfile.init(json:))
            qoderServiceAccounts = (accountsJSON["accounts"] as? [[String: Any]] ?? [])
                .compactMap(QoderServiceAccount.init(json:))
            let pending = profilesJSON["pendingSwitches"] as? [String: Any] ?? [:]
            qoderPendingSwitches = (pending["items"] as? [[String: Any]] ?? [])
                .compactMap(QoderPendingSwitch.init(json:))
            qoderError = nil
            qoderSyncedAt = Date()
        } catch {
            qoderError = message(for: error)
            if !silent { qoderNotice = nil }
        }
    }

    /// 删除一个 Qoder **服务账号**（PRD §4.5，走 PR #77 的 `qoder.accounts.delete`）。
    ///
    /// 只删 AICC 自己保存的凭据、授权元数据、领取记录与额度快照：
    /// 不碰 Magpie 配置，不删客户端本机 Profile，也不代表服务端 PAT 已撤销。
    /// 失败时账号仍在，返回 false 并留下可重试的错误文案。
    @discardableResult
    func deleteQoderServiceAccount(_ account: QoderServiceAccount) async -> Bool {
        guard qoderDeleting == nil else { return false }
        qoderDeleting = account.id
        qoderDeleteError = nil
        qoderDeleteNotice = nil
        defer { qoderDeleting = nil }
        do {
            let result = try await worker.deleteQoderAccount(accountId: account.id)
            if result.removed {
                qoderDeleteNotice = "已从 AICC 移除 \(account.displayName)"
                    + "（清掉 \(result.secretsDeleted) 份凭据）"
                await refreshQoder(silent: true)
                return true
            }
            qoderDeleteError = "账号仍保留：这次没有完成删除，可以直接重试"
            return false
        } catch {
            qoderDeleteError = message(for: error)
            return false
        }
    }

    /// 用真实接口核一次 WorkBuddy 账号的登录态（只读）。
    ///
    /// 结论只代表"身份还能用"，不代表签到等写能力已被验证。
    @discardableResult
    func verifyWorkbuddyAccount(_ account: ConsoleAccount) async -> String {
        do {
            let json = try await worker.workbuddyStatus(accountId: account.id)
            let state = (json["state"] as? String) ?? (json["authStatus"] as? String) ?? "unknown"
            await refreshAccounts()
            switch state {
            case "valid", "ok", "authorized":
                return "授权有效：服务端确认身份仍可用"
            case "expired":
                return "授权已过期，请重新登录"
            case "denied":
                return "钥匙串被拒，请在系统弹窗里允许 AICC 访问"
            default:
                return "服务端返回状态：\(state)"
            }
        } catch {
            return message(for: error)
        }
    }

    /// 扫描本机已有的 Qoder 配置并登记。只登记引用，不动任何目录。
    ///
    /// `announce: false` 用于页面自动扫描：同一次打开不需要给用户一条"什么也没发生"的提示。
    func scanQoderProfiles(announce: Bool = true) async {
        guard qoderActing == nil else { return }
        qoderActing = "scan"
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfilesScan()
            let created = (json["created"] as? [[String: Any]] ?? []).count
            let skipped = (json["skipped"] as? [[String: Any]] ?? []).count
            if announce {
                qoderNotice = created == 0
                    ? "没有发现新的 Qoder 配置（已有 \(skipped) 个已登记）"
                    : "已登记 \(created) 个 Qoder 配置，原有登录状态未改动"
            }
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func addQoderProfile(edition: QoderEdition, displayName: String) async {
        guard qoderActing == nil else { return }
        qoderActing = "create"
        defer { qoderActing = nil }
        do {
            _ = try await worker.qoderProfileCreate(
                edition: edition.rawValue,
                displayName: displayName
            )
            qoderNotice = "已创建 \(edition.displayName) Profile：\(displayName)"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func importQoderProfile(edition: QoderEdition, snapshotDir: String, displayName: String) async {
        guard qoderActing == nil else { return }
        qoderActing = "import"
        defer { qoderActing = nil }
        do {
            _ = try await worker.qoderProfileImport(
                edition: edition.rawValue,
                snapshotDir: snapshotDir,
                displayName: displayName
            )
            qoderNotice = "已导入 \(edition.displayName) 账号快照（原目录未改动）"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func qoderInspect(_ profile: QoderProfile) async {
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfileInspect(profileId: profile.id)
            let summary = json["summary"] as? String ?? "检查完成"
            if let conflict = json["identityConflict"] as? String {
                qoderNotice = "\(profile.displayName)：\(summary)，但与另一个 Profile 身份重复（\(conflict)），未自动合并"
            } else {
                qoderNotice = "\(profile.displayName)：\(summary)"
            }
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func qoderOpen(_ profile: QoderProfile) async {
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfileOpen(profileId: profile.id)
            let state = json["state"] as? String ?? "unknown"
            if state != "running" {
                let reason = json["reason"] as? String ?? "进程未能核实"
                qoderNotice = "\(profile.displayName) 启动后未能核实：\(reason)"
            } else if json["requiresLogin"] as? Bool == true {
                qoderNotice = "已切到 \(profile.displayName)：客户端已启动，请在官方客户端里登录，"
                    + "登录后点「保存当前登录」。"
            } else {
                qoderNotice = "已切换到 \(profile.displayName)（PID \(json["pid"] ?? "-")，"
                    + "快照已写回并被客户端接受）"
            }
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func qoderClose(_ profile: QoderProfile, force: Bool = false) async {
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            _ = try await worker.qoderProfileClose(profileId: profile.id, force: force)
            qoderNotice = "\(profile.displayName) 已关闭"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func qoderActivate(_ profile: QoderProfile) async {
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfileActivate(profileId: profile.id)
            let closed = (json["opened"] as? [String: Any])?["closedProfiles"] as? [String] ?? []
            qoderNotice = closed.isEmpty
                ? "已切换到 \(profile.displayName)"
                : "已切换到 \(profile.displayName)，并关闭了同版本其它 \(closed.count) 个实例"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    /// 把客户端当前登录态存进该 Profile（登录完成、或令牌刷新后手动存一次）。
    func qoderCapture(_ profile: QoderProfile) async {
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfileCapture(profileId: profile.id)
            if json["captured"] as? Bool == true {
                qoderNotice = "已保存 \(profile.displayName) 的当前登录态"
            } else {
                let reason = json["reason"] as? String ?? "没有可保存的登录态"
                qoderNotice = "\(profile.displayName)：\(reason)"
            }
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    /// 显式恢复未完成的切换。后端会先确认客户端已完全退出，状态未知时不写盘。
    func qoderRecover() async {
        guard qoderActing == nil else { return }
        qoderActing = "recover"
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfileRecover()
            let count = json["count"] as? Int ?? 0
            qoderNotice = count == 0
                ? "没有需要恢复的切换记录"
                : "已恢复 \(count) 条未完成的切换，live 认证回到原账号"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    func qoderRename(_ profile: QoderProfile, to name: String) async {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != profile.displayName else { return }
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            _ = try await worker.qoderProfileRename(profileId: profile.id, displayName: trimmed)
            qoderNotice = "已重命名为 \(trimmed)"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    /// `purge` 只有在用户二次确认"删除磁盘数据"时才为 true。
    func qoderRemove(_ profile: QoderProfile, purge: Bool) async {
        guard qoderActing == nil else { return }
        qoderActing = profile.id
        defer { qoderActing = nil }
        do {
            _ = try await worker.qoderProfileRemove(profileId: profile.id, purge: purge)
            qoderNotice = purge
                ? "已删除 \(profile.displayName) 及其用户数据目录"
                : "已从 AICC 移除 \(profile.displayName)（磁盘数据保留）"
            await refreshQoder(silent: true)
        } catch {
            qoderNotice = message(for: error)
        }
    }

    // MARK: - WorkBuddy 客户端切换（WB-SWITCH-1）

    /// 只读查询客户端状态与"当前登录是谁"。
    ///
    /// `silent` 用于自动刷新：失败时保留上一次结果，不用一条过期错误盖掉界面。
    func refreshWorkbuddyClient(silent: Bool = false) async {
        guard await ensureWorkerReady() else {
            if !silent { wbClientError = workerFailureText }
            return
        }
        do {
            wbClientStatus = try await worker.workbuddyClientStatus()
            wbClientError = nil
        } catch {
            // 老助手没有这条 RPC 时如实说，不显示成"客户端状态未知"。
            wbClientError = message(for: error)
            if !silent { wbSwitchNotice = nil }
        }
    }

    /// 把客户端切到指定账号。**会关闭并重启 WorkBuddy**，所以只由用户点按钮触发。
    func switchWorkbuddy(to account: ConsoleAccount) async {
        guard wbSwitching == nil else { return }
        wbSwitching = account.id
        wbSwitchNotice = nil
        wbSwitchFailed = false
        defer { wbSwitching = nil }
        do {
            let outcome = try await worker.workbuddyClientSwitch(accountId: account.id)
            wbSwitchFailed = outcome.changed && !outcome.verified
            wbSwitchNotice = outcome.summary
            await refreshWorkbuddyClient(silent: true)
            // 用户要看的正是"切过去之后算不算活跃"，所以顺手刷一遍活跃状态。
            await loadActivity()
        } catch {
            let text = message(for: error)
            // 失败分两种：已回滚 / 回滚也失败。含混成一句会让人以为账号已经切过去了。
            switch (error as? AccountWorkerRPCError)?.restored {
            case .some(true):
                wbSwitchFailed = true
                wbSwitchNotice = "切换失败，已恢复到切换前的账号：\(text)"
            case .some(false):
                wbSwitchFailed = true
                wbSwitchNotice = "切换失败，恢复也没成功：\(text)。请在客户端里手动登录。"
            default:
                wbSwitchFailed = true
                wbSwitchNotice = text
            }
            await refreshWorkbuddyClient(silent: true)
        }
    }

    /// 把登录态放回某一轮切换之前。`journalId` 为空时由助手挑（未完成优先）。
    ///
    /// 当前登录已经不是"切过去的那个账号"时，助手会拒绝（`live_changed`）并带上
    /// 当前的 UID。界面据此弹出确认，用户同意后再以 `force = true` 调一次 —— 
    /// 绝不静默覆盖用户手动换上的登录态。
    func restoreWorkbuddyPrevious(journalId: String? = nil, force: Bool = false) async {
        guard wbSwitching == nil else { return }
        wbSwitching = "restore"
        wbSwitchFailed = false
        defer { wbSwitching = nil }
        do {
            let outcome = try await worker.workbuddyClientRestore(
                journalId: journalId,
                force: force
            )
            wbSwitchFailed = !outcome.restored
            wbSwitchNotice = outcome.summary
            await refreshWorkbuddyClient(silent: true)
        } catch {
            if let rpc = error as? AccountWorkerRPCError, rpc.kind == "live_changed" {
                // 交给界面确认；这里只留一句提示，不改任何文件。
                wbRestoreNeedsConfirm = wbClientStatus?.restoreOffer?.id ?? journalId
                wbSwitchFailed = false
                wbSwitchNotice = rpc.message
                return
            }
            wbSwitchFailed = true
            wbSwitchNotice = message(for: error)
            await refreshWorkbuddyClient(silent: true)
        }
    }

    /// 用户在确认弹层里点了"仍然恢复"。
    func confirmWorkbuddyRestore() async {
        let target = wbRestoreNeedsConfirm
        wbRestoreNeedsConfirm = nil
        await restoreWorkbuddyPrevious(journalId: target, force: true)
    }

    func cancelWorkbuddyRestore() {
        wbRestoreNeedsConfirm = nil
    }

    func clearWorkbuddySwitchNotice() {
        wbSwitchNotice = nil
    }

    // MARK: - 夜间账号轮换（WB-ROTATE-1）

    /// 只读刷新。轮换状态在计划执行期间会自己变（助手每分钟推进一次），
    /// 所以界面每 30 秒静默刷一次就够。
    func refreshWorkbuddyRotation(silent: Bool = false) async {
        guard !wbRotationLoading else { return }
        wbRotationLoading = true
        defer { wbRotationLoading = false }
        guard await ensureWorkerReady() else {
            if !silent { wbRotationError = workerFailureText }
            return
        }
        do {
            wbRotationStatus = try await worker.workbuddyRotationStatus()
            wbRotationError = nil
        } catch {
            // 老助手没有这条 RPC 时如实说，不把"不支持"显示成"计划不存在"。
            wbRotationError = message(for: error)
        }
    }

    /// 保存配置。只传变化过的字段，后端合并——改一个时间不该把账号顺序冲掉。
    ///
    /// 返回 `false` 表示**没有保存成功**：调用方（例如"保存并打开"）必须停下，
    /// 不能拿着旧配置继续往下走。
    @discardableResult
    func saveWorkbuddyRotation(_ params: [String: Any], note: String? = nil) async -> Bool {
        wbRotationFailed = false
        do {
            wbRotationStatus = try await worker.workbuddyRotationSave(params: params)
            wbRotationError = nil
            if let note { wbRotationNotice = note }
            return true
        } catch {
            wbRotationFailed = true
            wbRotationError = nil
            wbRotationNotice = "保存失败：\(message(for: error))。设置没有改动。"
            return false
        }
    }

    /// 打开 / 关闭。**打开前**后端会校验计划；缺账号会直接拒绝并说原因。
    func setWorkbuddyRotationEnabled(_ enabled: Bool) async {
        wbRotationFailed = false
        wbRotationNotice = nil
        do {
            let result = try await worker.workbuddyRotationSetEnabled(enabled)
            wbRotationStatus = result.status
            wbRotationError = nil
            // 关闭 = 停止后续计划 + 把账号恢复回来。两件事分别说，不合并成一句。
            let feedback = RotationFeedbackText.toggle(
                enabled: enabled,
                stopStatus: result.stopResult?["status"] as? String,
                restored: result.stopResult?["restored"] as? Bool
            )
            wbRotationNotice = feedback.text
            wbRotationFailed = feedback.isFailure
        } catch {
            wbRotationFailed = true
            wbRotationNotice = message(for: error)
            await refreshWorkbuddyRotation(silent: true)
        }
    }

    /// 预演：只演算，不关客户端。
    func previewWorkbuddyRotation() async {
        wbRotationFailed = false
        do {
            wbRotationDryRun = try await worker.workbuddyRotationDryRun()
            wbRotationNotice = "预演完成：没有改动任何登录态。"
        } catch {
            wbRotationFailed = true
            wbRotationNotice = message(for: error)
        }
    }

    /// 立即执行一轮。会关客户端，只给用户在场的短时验证用；占用当天名额。
    func runWorkbuddyRotationNow() async {
        guard !wbRotationBusy else { return }
        wbRotationBusy = true
        wbRotationFailed = false
        defer { wbRotationBusy = false }
        do {
            let result = try await worker.workbuddyRotationRunNow()
            let feedback = RotationFeedbackText.runNow(
                status: result["status"] as? String ?? "unknown",
                detail: result["detail"] as? String
            )
            wbRotationNotice = feedback.text
            wbRotationFailed = feedback.isFailure
            await refreshWorkbuddyRotation(silent: true)
            await refreshWorkbuddyClient(silent: true)
        } catch {
            wbRotationFailed = true
            wbRotationNotice = message(for: error)
            await refreshWorkbuddyRotation(silent: true)
        }
    }

    /// 暂停并恢复原账号。
    func stopWorkbuddyRotation() async {
        guard !wbRotationBusy else { return }
        wbRotationBusy = true
        wbRotationFailed = false
        defer { wbRotationBusy = false }
        do {
            let result = try await worker.workbuddyRotationStop()
            let feedback = RotationFeedbackText.stop(
                status: result["status"] as? String ?? "unknown",
                restored: result["restored"] as? Bool,
                detail: result["detail"] as? String
            )
            wbRotationNotice = feedback.text
            wbRotationFailed = feedback.isFailure
            await refreshWorkbuddyRotation(silent: true)
            await refreshWorkbuddyClient(silent: true)
        } catch {
            wbRotationFailed = true
            wbRotationNotice = message(for: error)
        }
    }

    /// 手动恢复最初账号。`force` 只在用户明确确认"覆盖我现在的登录"时用。
    func restoreWorkbuddyRotationOrigin(force: Bool = false) async {
        guard !wbRotationBusy else { return }
        wbRotationBusy = true
        wbRotationFailed = false
        defer { wbRotationBusy = false }
        do {
            let result = try await worker.workbuddyRotationRestoreOrigin(force: force)
            let restored = result["restored"] as? Bool ?? false
            wbRotationFailed = !restored
            wbRotationNotice = result["detail"] as? String
                ?? (restored ? "已恢复最初账号。" : "恢复未完成。")
            await refreshWorkbuddyRotation(silent: true)
            await refreshWorkbuddyClient(silent: true)
        } catch {
            wbRotationFailed = true
            wbRotationNotice = message(for: error)
        }
    }

    func clearWorkbuddyRotationNotice() {
        wbRotationNotice = nil
        wbRotationDryRun = nil
    }

    private func message(for error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }

    // MARK: - Magpie 导入

    func magpiePreview() async throws -> MagpiePreview {
        try await worker.magpiePreview()
    }

    func importMagpieAccounts(keys: [String]) async throws -> MagpieImportResult {
        try await worker.importMagpieAccounts(keys: keys)
    }

    /// 用真实接口核一次登录态（只读）。
    func workbuddyStatus(accountId: String) async throws -> [String: Any] {
        try await worker.workbuddyStatus(accountId: accountId)
    }

    func refreshAccounts() async {
        guard !loadingAccounts else { return }
        loadingAccounts = true
        defer { loadingAccounts = false }

        guard await ensureWorkerReady() else {
            accountsError = workerFailureText
            accounts = []
            lastAccountsRefresh = Date()
            return
        }

        do {
            let rows = try await worker.listAccounts()
            accounts = rows.compactMap(ConsoleAccount.init(json:))
            accountsError = nil
        } catch {
            accounts = []
            accountsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
        lastAccountsRefresh = Date()
        // 账号增删会改变"哪些账号可以切"，顺手刷新客户端状态（只读，本地文件 + pgrep）。
        await refreshWorkbuddyClient(silent: true)
    }

    /// 助手侧四平台额度。`refresh=false` 时助手优先用快照缓存（不打网络）。
    ///
    /// `days` 只影响**用量窗口**（7 / 30 天），不改余额口径。
    func refreshWorkerQuota(force: Bool = true, days: Int? = nil) async {
        guard !loadingWorkerQuota else { return }
        let window = days ?? workerQuotaWindowDays
        loadingWorkerQuota = true
        defer { loadingWorkerQuota = false }
        guard await ensureWorkerReady() else {
            workerQuotaError = workerFailureText
            return
        }
        do {
            workerQuotaRows = try await worker.workerQuota(days: window, refresh: force)
            workerQuotaWindowDays = window
            workerQuotaError = workerQuotaRows.isEmpty ? "还没有可显示的账号" : nil
        } catch {
            workerQuotaRows = []
            workerQuotaError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    // MARK: - 统一任务汇总（PR #76）

    /// 顶部汇总条的数字。只来自本地记录，不发网络请求。
    @Published private(set) var tasksOverview: TasksOverview?
    @Published private(set) var tasksOverviewError: String?
    @Published private(set) var tasksOverviewLoading = false

    func refreshTasksOverview() async {
        guard !tasksOverviewLoading else { return }
        tasksOverviewLoading = true
        defer { tasksOverviewLoading = false }
        guard await ensureWorkerReady() else {
            tasksOverviewError = workerFailureText
            return
        }
        do {
            let value = try await worker.tasksOverview()
            tasksOverview = value
            tasksOverviewError = nil
        } catch {
            // 汇总拿不到不该让整页报错：各分区还能各自工作。
            tasksOverviewError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    // MARK: - Qoder 每日 Credits（PR #75）

    /// 自动领取总览。每个账号独立开关，默认关闭。
    @Published private(set) var qoderCreditsAuto: [QoderCreditsAutoItem] = []
    @Published private(set) var qoderCreditsError: String?
    @Published private(set) var qoderCreditsLoading = false
    /// 正在执行动作的账号 id（查询 / 领取 / 切换开关），用来禁用该行按钮。
    @Published private(set) var qoderCreditsBusy: String?
    /// 每个账号最近一条领取结果，按需加载。
    @Published private(set) var qoderCreditsRuns: [String: [QoderCreditRun]] = [:]
    /// 本次会话里每个账号最近一次只读检查的结论。
    /// 用途只有一个：让「立即领取」只在服务端确实说可领时才可点（PRD §6.3）。
    @Published private(set) var qoderCreditsReports: [String: QoderCreditsReport] = [:]
    /// 最近一次动作的一句话结论（成功/失败都给一句）。
    @Published private(set) var qoderCreditsNotice: String?

    func refreshQoderCredits(silent: Bool = false) async {
        if qoderCreditsLoading { return }
        qoderCreditsLoading = true
        defer { qoderCreditsLoading = false }
        do {
            qoderCreditsAuto = try await worker.qoderCreditsAutoStatus()
            qoderCreditsError = nil
            if !silent {
                qoderCreditsNotice = nil
            }
        } catch {
            qoderCreditsAuto = []
            qoderCreditsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 只读检查今天有没有可领的活动。**不发写请求**。
    @discardableResult
    func checkQoderCredits(accountId: String) async -> QoderCreditsReport? {
        qoderCreditsBusy = accountId
        qoderCreditsNotice = nil
        defer { qoderCreditsBusy = nil }
        do {
            let report = try await worker.qoderCreditsStatus(accountId: accountId)
            qoderCreditsReports[accountId] = report
            qoderCreditsNotice = report.statusText
            await loadQoderCreditRuns(accountId: accountId)
            return report
        } catch {
            qoderCreditsNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return nil
        }
    }

    /// 手动领取。用户点了才发，不在任何自动路径上。
    @discardableResult
    func claimQoderCredits(accountId: String) async -> QoderCreditsReport? {
        qoderCreditsBusy = accountId
        qoderCreditsNotice = nil
        defer { qoderCreditsBusy = nil }
        do {
            let report = try await worker.qoderCreditsClaim(accountId: accountId)
            qoderCreditsReports[accountId] = report
            qoderCreditsNotice = report.claimText
            await loadQoderCreditRuns(accountId: accountId)
            // 领完之后顺手刷新额度缓存与顶部汇总：那两处的"今日已领"要跟着变。
            await refreshWorkerQuota(force: true)
            await refreshTasksOverview()
            return report
        } catch {
            qoderCreditsNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return nil
        }
    }

    func setQoderCreditsAuto(accountId: String, enabled: Bool) async {
        qoderCreditsBusy = accountId
        qoderCreditsNotice = nil
        defer { qoderCreditsBusy = nil }
        do {
            try await worker.setQoderCreditsAuto(accountId: accountId, enabled: enabled)
            qoderCreditsNotice = enabled ? "已开启自动领取（默认每天检查一次）" : "已关闭自动领取"
        } catch {
            qoderCreditsNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
        await refreshQoderCredits(silent: true)
    }

    func loadQoderCreditRuns(accountId: String) async {
        do {
            qoderCreditsRuns[accountId] = try await worker.qoderCreditsRuns(accountId: accountId, limit: 5)
        } catch {
            // 记录拿不到不影响领取本身，安静失败。
        }
    }

    func refreshQuota(force: Bool = true) async {
        guard !loadingQuota else { return }
        loadingQuota = true
        defer { loadingQuota = false }

        let result = await WidgetStatusLoader.snapshot(force: force, port: settings.magpiePort)
        snapshot = result
        quotaError = result.accounts.isEmpty
            ? "没有读到额度。确认 Magpie 正在运行（端口 \(settings.magpiePort)）。"
            : nil
        lastQuotaRefresh = Date()
    }

    // MARK: - Buddy 旅行

    /// 拉一次本地快照。不发旅行接口，便宜到可以 5 秒轮询一次。
    func loadTravel() async {
        guard !travelLoading else { return }
        travelLoading = true
        defer { travelLoading = false }
        guard await ensureWorkerReady() else {
            travelError = workerFailureText
            return
        }
        do {
            let summary = try await worker.travelSummary()
            travelRows = summary.rows
            travelAuto = summary.auto
            travelError = nil
            // 记一个「有没有开自动旅行」的标记：App 下次启动要靠它决定
            // 要不要提前把助手拉起来（否则用户设了自动、重启后没人跑）。
            TravelAutomationService.remember(summary.automationRunning)
            if !summary.automationRunning {
                TravelAutomationService.shared.stopIfIdle()
            }
        } catch {
            travelRows = []
            travelAuto = nil
            travelError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    func setTravelEnabled(_ row: TravelAccountRow, enabled: Bool) async {
        travelActing = row.accountId
        defer { travelActing = nil }
        do {
            try await worker.travelSetEnabled(accountId: row.accountId, enabled: enabled)
            travelNotice = enabled ? "已开启 \(row.nickname) 的自动旅行" : "已关闭 \(row.nickname) 的自动旅行"
            await loadTravel()
            TravelAutomationService.shared.startIfNeeded(worker: worker)
        } catch {
            travelNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 手动派出 / 领取 / 只查状态。
    func runTravel(_ method: TravelMethod, row: TravelAccountRow) async {
        travelActing = row.accountId
        defer { travelActing = nil }
        do {
            let result = try await worker.travelSync(method, accountId: row.accountId)
            travelNotice = result.summaryText ?? "\(row.nickname) 已更新"
            await loadTravel()
        } catch {
            travelNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            await loadTravel()
        }
    }

    // MARK: - 成长中心（PR #67：只读）

    /// 扫一遍成长中心。**由用户点按钮触发**，不自动轮询 ——
    /// 一次扫描要打 9 个接口 × N 个账号，静默定时跑会让用户以为"我没让它联网"。
    ///
    /// 也只做 GET：接受任务、领奖在 #68，各自会有独立开关。
    func loadGrowth(accountIds: [String] = []) async {
        guard !growthLoading else { return }
        growthLoading = true
        defer { growthLoading = false }
        do {
            let json = try await worker.growthScan(accountIds: accountIds)
            let result = GrowthScanResult(json: json)
            growthResult = result
            growthError = nil
            lastGrowthScan = Date()
            // 默认展开第一个"有事要做"的账号：全是空的时候展开哪一行都没意义。
            if expandedGrowthAccount == nil {
                expandedGrowthAccount = result.scans
                    .first(where: { $0.pendingCount > 0 || $0.needsAuth })?.accountId
                    ?? result.scans.first?.accountId
            }
            // 开关状态与执行记录一起刷新：任务行要显示"上次执行结果"。
            await loadGrowthAuto()
            await loadGrowthRuns()
        } catch {
            growthResult = .empty
            growthError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    func toggleGrowthAccount(_ accountId: String) {
        expandedGrowthAccount = expandedGrowthAccount == accountId ? nil : accountId
    }

    // MARK: - 成长中心自动管理（PR #68）

    /// 读开关与下次检查时间。便宜，界面可以随扫描一起调。
    func loadGrowthAuto() async {
        do {
            growthAuto = GrowthAutoStatus(json: try await worker.growthAutoStatus())
        } catch {
            // 读不到就保持原样：这是次要信息，不该把整页打成错误态。
        }
    }

    /// 打开 / 关闭自动管理。**打开之后才可能有写操作**。
    func setGrowthAutoEnabled(_ enabled: Bool) async {
        growthActing = true
        defer { growthActing = false }
        do {
            growthAuto = GrowthAutoStatus(json: try await worker.setGrowthAutoEnabled(enabled))
            growthNotice = enabled
                ? "已开启自动管理：会替你接单并领取已完成任务的奖励"
                : "已关闭自动管理：只保留手动扫描"
        } catch {
            growthNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 手动跑一轮。`force` 跳过间隔判断，但仍然受总开关约束。
    func runGrowthTick(force: Bool = true) async {
        growthActing = true
        defer { growthActing = false }
        do {
            let json = try await worker.growthTick(force: force)
            let result = GrowthTickResult(json: json)
            if let skipped = result.skippedText {
                growthNotice = skipped
            } else {
                growthNotice = result.summary ?? "检查完成"
            }
            await loadGrowthAuto()
            // 跑完把最新状态与记录一起刷新，界面不用等下一轮。
            await loadGrowth()
        } catch {
            growthNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 写操作的结果（按账号）。四种状态都留在这里，界面按 state 上色。
    @Published private(set) var rewardActions: [String: RewardActionResult] = [:]
    @Published private(set) var rewardActing: Set<String> = []
    /// 点按钮时现拉的预览（不是缓存）：确认框只显示这里的数字。
    @Published private(set) var rewardPreviews: [String: RewardPreview] = [:]
    @Published private(set) var rewardChecking: Set<String> = []

    /// 执行一次奖励写操作。**调用方必须先让用户确认**（界面里有 confirmationDialog）。
    func performReward(accountId: String, action: RewardAction, target: String? = nil) async {
        guard !rewardActing.contains(accountId) else { return }
        rewardActing.insert(accountId)
        defer { rewardActing.remove(accountId) }
        guard await ensureWorkerReady() else {
            rewardActions[accountId] = RewardActionResult(json: ["state": "failed", "message": workerFailureText])
            return
        }
        do {
            let json: [String: Any]
            switch action {
            case .draw:
                json = try await worker.rewardDraw(accountId: accountId, confirm: true)
            case .redeem:
                json = try await worker.rewardRedeem(accountId: accountId, tier: target ?? "", confirm: true)
            case .makeup:
                json = try await worker.rewardMakeup(accountId: accountId, date: target ?? "", confirm: true)
            }
            rewardActions[accountId] = RewardActionResult(json: json)
            // 无论结果如何都重读一次状态：成功要看新余额，未知要看账本提示。
            await loadReward(accountId: accountId)
        } catch {
            rewardActions[accountId] = RewardActionResult(json: [
                "state": "failed",
                "message": (error as? LocalizedError)?.errorDescription ?? error.localizedDescription,
            ])
        }
    }

    func clearRewardAction(accountId: String) { rewardActions[accountId] = nil }

    /// 打开确认框之前先拉一次实时预览。失败或资格不足 → 不弹框，直接报原因。
    func previewReward(accountId: String, action: RewardAction, target: String? = nil) async {
        guard !rewardActing.contains(accountId) else { return }
        rewardActing.insert(accountId)
        defer { rewardActing.remove(accountId) }
        do {
            let json = try await worker.rewardPreview(accountId: accountId, action: action, target: target)
            guard let preview = RewardPreview(json: json) else {
                rewardActions[accountId] = RewardActionResult(json: [
                    "state": "failed", "message": "预览返回无法解析，已取消操作",
                ])
                return
            }
            guard preview.ready else {
                // 资格不足：不弹确认框，直接说明原因。
                rewardActions[accountId] = RewardActionResult(json: [
                    "state": "rejected",
                    "message": preview.reason ?? "当前不满足执行条件，未发送",
                ])
                return
            }
            rewardPreviews[accountId] = preview
        } catch {
            rewardActions[accountId] = RewardActionResult(json: [
                "state": "failed",
                "message": (error as? LocalizedError)?.errorDescription ?? error.localizedDescription,
            ])
        }
    }

    func clearRewardPreview(accountId: String) { rewardPreviews[accountId] = nil }

    /// 回查：只读，不重发。收敛后刷新余额与历史。
    func recheckRewards(accountId: String) async {
        guard !rewardChecking.contains(accountId) else { return }
        rewardChecking.insert(accountId)
        defer { rewardChecking.remove(accountId) }
        do {
            let json = try await worker.rewardRecheck(accountId: accountId)
            let checked = (json["checked"] as? NSNumber)?.intValue ?? 0
            let results = json["results"] as? [[String: Any]] ?? []
            let moved = results.contains { ($0["moved"] as? Bool) == true }
            let message: String
            if checked == 0 {
                message = "没有待核验的操作"
            } else if moved {
                message = "回查完成：服务端状态有变化，但无法归属到本次请求，仍标记为待核验"
            } else {
                message = "回查完成：服务端状态没有变化，仍无法确认结果，继续待核验"
            }
            rewardActions[accountId] = RewardActionResult(json: [
                "state": moved ? "pending" : "pending",
                "message": message,
            ])
            await loadReward(accountId: accountId)
        } catch {
            rewardActions[accountId] = RewardActionResult(json: [
                "state": "failed",
                "message": (error as? LocalizedError)?.errorDescription ?? error.localizedDescription,
            ])
        }
    }

    /// 读一个账号的奖励状态。**只读**：worker 侧没有抽奖、兑换、补签的写入口。
    ///
    /// 失败不清空上一次结果：界面上的「上次读到」和「这次失败」是两件事，
    /// 把数据擦掉反而让人以为奖励没了。
    func loadReward(accountId: String) async {
        guard !rewardLoading.contains(accountId) else { return }
        rewardLoading.insert(accountId)
        defer { rewardLoading.remove(accountId) }
        guard await ensureWorkerReady() else {
            rewardFailures[accountId] = workerFailureText
            return
        }
        do {
            let json = try await worker.rewardStatus(accountIds: [accountId])
            let result = RewardStatusResult(json: json)
            if let state = result.account(accountId) {
                rewardStates[accountId] = state
                rewardFailures[accountId] = nil
            } else {
                rewardFailures[accountId] = "返回里没有这个账号的奖励状态（可能不是 WorkBuddy 国内版账号）"
            }
        } catch {
            rewardFailures[accountId] =
                (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 读所有账号的活跃状态。**只读**：worker 侧每账号 2 个成长 GET，
    /// 必要时 1 次只读语义 POST（签到状态），没有任何写入口。
    ///
    /// 失败时**保留上一次结果**并留一条错误：界面上「上次读到」和「这次没读到」
    /// 是两件事，把数据擦掉会让人以为活跃记录丢了。
    func loadActivity() async {
        guard !activityLoading else { return }
        activityLoading = true
        defer { activityLoading = false }
        guard await ensureWorkerReady() else {
            activityError = workerFailureText
            return
        }
        do {
            let json = try await worker.activityStatus()
            activity = ActivityFleetState(json: json)
            activityError = nil
            lastActivityRefresh = Date()
        } catch {
            activityError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 读所有账号的执行记录。失败保持原样，不影响任务列表。
    func loadGrowthRuns() async {
        for scan in growthResult.scans {
            guard let runs = try? await worker.growthRuns(accountId: scan.accountId) else { continue }
            growthRuns[scan.accountId] = GrowthRuns(json: runs)
        }
    }

    // MARK: - 每日签到（PR #69）

    /// 读签到状态。`cached: true` 只读本地库（不联网），用于首屏与快速刷新。
    ///
    /// 默认不联网：一次 refresh 要给每个账号发一个只读状态请求，
    /// 静默轮询会把接口打成筛子，也违背"我没让它联网"的预期。
    func loadCheckin(cached: Bool = true, silent: Bool = false) async {
        guard !checkinLoading else { return }
        checkinLoading = true
        defer { checkinLoading = false }
        guard await ensureWorkerReady() else {
            if !silent { checkinError = workerFailureText }
            return
        }
        do {
            let json = try await worker.checkinStatus(cached: cached)
            checkin = CheckinFleetStatus(json: json)
            checkinError = nil
            if !silent { checkinNotice = nil }
            await loadCheckinAuto()
        } catch {
            if !silent {
                checkinError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }

    /// 联网刷新全部账号的今日状态（只读，不提交任何签到）。
    func refreshCheckinStatus() async {
        await loadCheckin(cached: false)
        if checkinError == nil {
            let checked = checkin.counts.checked
            checkinNotice = "已刷新 \(checkin.counts.accounts) 个账号，今日 \(checked) 个已签到"
        }
    }

    func loadCheckinAuto() async {
        do {
            checkinAuto = CheckinAutoStatus(json: try await worker.checkinAutoStatus())
        } catch {
            // 次要信息，读不到就保持原样，不把整页打成错误态。
        }
    }

    func setCheckinEnabled(_ row: CheckinAccountRow, enabled: Bool) async {
        checkinActing = row.accountId
        defer { checkinActing = nil }
        do {
            _ = try await worker.checkinSetEnabled(accountId: row.accountId, enabled: enabled)
            checkinNotice = enabled
                ? "已开启 \(row.nickname) 的自动签到：每天 09:00 后检查一次"
                : "已关闭 \(row.nickname) 的自动签到"
            // 开关是本地状态，读缓存就够，不必再打一次接口。
            await loadCheckin(cached: true, silent: true)
            await refreshAccounts()
        } catch {
            checkinNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 单账号「立即签到」：用户显式操作，不受自动开关限制。
    func runCheckin(_ row: CheckinAccountRow) async {
        checkinActing = row.accountId
        defer { checkinActing = nil }
        do {
            let json = try await worker.checkinRun(accountId: row.accountId)
            let state = CheckinState(raw: json["state"] as? String)
            let credits = CheckinJSON.int(json["credits"])
            switch state {
            case .claimed:
                checkinNotice = credits.map { "\(row.nickname) 签到成功 +\($0)" }
                    ?? "\(row.nickname) 签到成功（服务端未返回奖励数额）"
            case .already:
                checkinNotice = "\(row.nickname) 今天已经签过了，本次没有重复提交"
            case .unknown:
                checkinNotice = "\(row.nickname) 已提交但服务端没确认，等下一轮重查"
            default:
                let detail = json["errorDetail"] as? String
                checkinNotice = detail.map { "\(row.nickname) 未完成：\($0)" }
                    ?? "\(row.nickname) 未完成（\(state.title)）"
            }
            await loadCheckin(cached: true, silent: true)
        } catch {
            checkinNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 批量「立即签到未完成账号」。只签服务端确认未签到的那些。
    func runPendingCheckins() async {
        checkinBatching = true
        defer { checkinBatching = false }
        do {
            let json = try await worker.checkinRunPending()
            checkinNotice = json["summary"] as? String ?? "已完成"
            await loadCheckin(cached: true, silent: true)
        } catch {
            checkinNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 确保账号助手已经起好再发 RPC。
    ///
    /// 控制台各页会在首屏并行取数，页面自己的 `.task` 可能早于
    /// `loadIfNeeded()` 里的进程启动，此时 RPC 直接抛 "未运行"，
    /// 界面上就会留下一条再也不消失的错误。所有入口先过这道门。
    @discardableResult
    func ensureWorkerReady() async -> Bool {
        if case .running = worker.state { return true }
        await worker.refresh()
        // `refresh()` 在别的页面已经开始启动时会立刻返回 .starting：等它落定。
        if case .starting = worker.state {
            for _ in 0..<25 {
                try? await Task.sleep(nanoseconds: 200_000_000)
                if case .starting = worker.state { continue }
                break
            }
        }
        if case .running = worker.state { return true }
        return false
    }

    private var workerFailureText: String {        switch worker.state {
        case .failed(let message):
            return message.components(separatedBy: "\n").first ?? "账号助手异常"
        case .starting:
            return "账号助手启动中，请稍后重试"
        case .stopped:
            return "账号助手未启动"
        case .running:
            return ""
        }
    }
}

#if AICC_VISUAL_TEST && AICC_CONSOLE_VISUAL
// MARK: - 视觉回归样本（只在控制台离屏渲染时编译）

/// 离屏渲染用：把一组固定样本灌进 store。
///
/// 生产构建里这段代码不存在：两个 flag 只在 `scripts/smoke-test-console-visual.sh`
/// 里同时定义，所以界面不会凭空多出假数据。只判断 `AICC_VISUAL_TEST` 会把这段
/// 编进设置页 / 看板的离屏渲染（它们引用不到 `ConsoleVisualFixture`）。
extension ConsoleStore {
    func applyVisualFixture(_ fixture: ConsoleVisualFixture) {
        accounts = fixture.accounts
        accountsError = fixture.accountsError
        lastAccountsRefresh = fixture.lastAccountsRefresh
        loadingAccounts = fixture.isLoading
        qoderLoading = fixture.isLoading

        snapshot = fixture.snapshot
        quotaError = fixture.quotaError
        lastQuotaRefresh = fixture.lastAccountsRefresh

        workerQuotaRows = fixture.workerQuotaRows
        workerQuotaError = fixture.workerQuotaError
        workerQuotaWindowDays = fixture.workerQuotaWindowDays

        tasksOverview = fixture.tasksOverview
        tasksOverviewError = fixture.tasksOverviewError
        tasksOverviewLoading = fixture.isLoading

        checkin = fixture.checkin
        checkinAuto = fixture.checkinAuto
        checkinError = fixture.checkinError

        growthResult = fixture.growthResult
        growthError = fixture.growthError
        lastGrowthScan = fixture.lastGrowthScan

        travelRows = fixture.travelRows
        travelAuto = fixture.travelAuto
        travelError = fixture.travelError

        qoderProfiles = fixture.qoderProfiles
        qoderServiceAccounts = fixture.qoderServiceAccounts
        qoderEditions = fixture.qoderEditions
        qoderPendingSwitches = fixture.qoderPendingSwitches
        qoderError = fixture.qoderError
        qoderSyncedAt = fixture.lastAccountsRefresh
        qoderCreditsAuto = fixture.qoderCreditsAuto
        qoderCreditsError = fixture.qoderCreditsError

        rewardStates = fixture.rewardStates
        rewardFailures = fixture.rewardFailures
        activity = fixture.activity
        activityError = fixture.activityError
        activityLoading = fixture.isLoading
        lastActivityRefresh = fixture.lastAccountsRefresh

        wbClientStatus = fixture.wbClientStatus
        wbSwitchNotice = fixture.wbSwitchNotice
        wbSwitchFailed = fixture.wbSwitchFailed

        wbRotationStatus = fixture.wbRotationStatus
    }
}
#endif
