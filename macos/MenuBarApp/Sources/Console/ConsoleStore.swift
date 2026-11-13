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
    @Published private(set) var qoderEditions: [QoderEditionInfo] = []
    /// 未完成的切换日志：只用于提示，恢复必须由用户显式确认。
    @Published private(set) var qoderPendingSwitches: [QoderPendingSwitch] = []
    @Published private(set) var qoderError: String?
    @Published private(set) var qoderNotice: String?
    @Published private(set) var qoderLoading = false
    /// 正在执行动作的 profileId，用来禁用该卡片按钮。
    @Published private(set) var qoderActing: String?

    let worker: AccountWorkerManager
    private let settings: AppSettings

    init(worker: AccountWorkerManager, settings: AppSettings) {
        self.worker = worker
        self.settings = settings
    }

    var stats: ConsoleOverviewStats { ConsoleOverviewStats(accounts: accounts) }
    var quotaSummary: ConsoleQuotaSummary { ConsoleQuotaSummary(snapshot: snapshot) }

    /// 首次进入窗口时拉一遍。重复调用是安全的。
    func loadIfNeeded() async {
        if snapshot == nil && !loadingQuota {
            await refreshQuota(force: false)
        }
        if lastAccountsRefresh == nil && !loadingAccounts {
            await refreshAccounts()
        }
        if qoderEditions.isEmpty && !qoderLoading {
            await refreshQoder(silent: true)
        }
    }

    func refreshAll() async {
        async let accountsTask: Void = refreshAccounts()
        async let quotaTask: Void = refreshQuota(force: true)
        async let qoderTask: Void = refreshQoder(silent: true)
        _ = await (accountsTask, quotaTask, qoderTask)
    }

    // MARK: - Qoder 双版本多账号（PR #70）

    /// 版本探测 + Profile 列表。`silent` 用于自动刷新，不覆盖已有提示。
    func refreshQoder(silent: Bool = false) async {
        guard !qoderLoading else { return }
        qoderLoading = true
        defer { qoderLoading = false }
        do {
            async let editionsRaw = worker.qoderEditionsDetect()
            async let profilesRaw = worker.qoderProfilesList()
            let (editionsJSON, profilesJSON) = try await (editionsRaw, profilesRaw)
            qoderEditions = (editionsJSON["editions"] as? [[String: Any]] ?? [])
                .compactMap(QoderEditionInfo.init(json:))
            qoderProfiles = (profilesJSON["profiles"] as? [[String: Any]] ?? [])
                .compactMap(QoderProfile.init(json:))
            let pending = profilesJSON["pendingSwitches"] as? [String: Any] ?? [:]
            qoderPendingSwitches = (pending["items"] as? [[String: Any]] ?? [])
                .compactMap(QoderPendingSwitch.init(json:))
            qoderError = nil
        } catch {
            qoderError = message(for: error)
            if !silent { qoderNotice = nil }
        }
    }

    /// 扫描本机已有的 Qoder 配置并登记。只登记引用，不动任何目录。
    func scanQoderProfiles() async {
        guard qoderActing == nil else { return }
        qoderActing = "scan"
        defer { qoderActing = nil }
        do {
            let json = try await worker.qoderProfilesScan()
            let created = (json["created"] as? [[String: Any]] ?? []).count
            let skipped = (json["skipped"] as? [[String: Any]] ?? []).count
            qoderNotice = created == 0
                ? "没有发现新的 Qoder 配置（已有 \(skipped) 个已登记）"
                : "已登记 \(created) 个 Qoder 配置，原有登录状态未改动"
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

        // 助手没起就起一次；起不来时把原因原样带到界面上。
        if case .running = worker.state {} else {
            await worker.refresh()
        }
        // 另一个页面可能正在拉起助手，`refresh()` 会立刻返回 .starting。
        // 这种情况等它落定，别把"启动中"当成错误甩给用户。
        if case .starting = worker.state {
            for _ in 0..<20 {
                try? await Task.sleep(nanoseconds: 200_000_000)
                if case .starting = worker.state { continue }
                break
            }
        }
        guard case .running = worker.state else {
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
